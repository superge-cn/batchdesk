//! batchdesk 命令行入口。
//!
//! 当前提供 C09 只读检测与计划预览切片，以及 C01 编码转换的计划/执行切片：
//! 扫描 + 大小/快速/全量哈希 + 纯函数分组；`--plan` 只生成 `PlanDocument`，
//! 不删除、不执行 apply。快速哈希窗口必须由调用方显式给出，本程序不设产品
//! 默认值（见 PDR-001 与 OQ-28）；C01 采样窗口/块数同样必须显式给出（方案
//! §5.2 未给数值）。

use std::env;
use std::path::PathBuf;

use kernel::apply::{apply_plan, undo_plan, ApplyContext, ApplyOptions, ApplyOutcome, UndoOutcome};
use kernel::duplicate::{
    group_duplicates, DuplicateCandidate, DuplicateReport, GroupingMode, GroupingOptions,
};
use kernel::encoding::{
    detect, transcode, InvalidSequencePolicy, PlatformPrior, SourceEncoding, TargetEncoding,
};
use kernel::fs::FsAdapter;
use kernel::generic_apply::{apply_generic_plan, ContentEditor, GenericApplyContext};
use kernel::journal::JournalStore;
use kernel::license::AlwaysAllowLicense;
use kernel::plan::{
    plan_c01, plan_c09, KeepPolicy, PlanC01File, PlanC09Input, PlanDocument, PlanFile,
};
use kernel::split::{
    plan_f01, plan_f02, plan_f03, PlanF01File, PlanF01Input, PlanF02Input, PlanF03Input,
};
use platform::fs::{now_rfc3339, rfc3339_days_ago, CliFsAdapter};
use platform::ids::{is_ulid, new_plan_id, new_ulid};
use platform::journal::FileJournal;
use platform::protected::{make_absolute, protected_dirs_from_paths};
use platform::retention::{
    cleanup, validate_retention_days, CleanupOutcome, DEFAULT_RETENTION_DAYS,
};
use platform::scan::{scan_path, ScanMode, ScanOptions, ScanOutcome};
use platform::text::{
    sample_bytes, split_layout, split_layout_by_lines, split_layout_by_parts, SampleSpec,
};

const USAGE: &str = "usage:
  batchdesk c09 <path> [--mode full|fast] [--quick-window-bytes N]
                     [--plan --keep earliest|shortest-path|newest [--plan-id ULID]]
                     [--json]
  batchdesk c01 --file <path>... --target <encoding>
                --sample-window <bytes> --sample-blocks <n>
                [--source <encoding>] [--invalid replace-and-count|abort]
                [--platform-prior chinese-windows|other]
                [--plan-id ULID] [--protected-dir <dir>]... [--json]
  batchdesk f01 --file <path>... --target-bytes N --output-dir <dir>
                [--plan-id ULID] [--protected-dir <dir>]... [--json]
  batchdesk f02 --file <path>... --lines N --output-dir <dir>
                [--plan-id ULID] [--protected-dir <dir>]... [--json]
  batchdesk f03 --file <path>... --parts N --output-dir <dir>
                [--plan-id ULID] [--protected-dir <dir>]... [--json]
  batchdesk apply --plan <plan.json> --quarantine-dir <dir> --journal <file>
                  [--protected-dir <dir>]... [--json]
  batchdesk undo --plan <planId> --journal <file> [--retention-days N] [--json]
  batchdesk cleanup --journal <file> --quarantine-dir <dir>
                    [--retention-days N] [--apply] [--json]

c09 options:
  --mode full|fast        grouping mode (default: full)
  --quick-window-bytes N  head/tail quick-hash window; required for fast mode
  --plan                  build a C09 PlanDocument (preview only; no writes)
  --keep POLICY           keep suggestion (default: earliest): earliest | shortest-path | newest
  --plan-id ULID          optional caller-supplied ULID; generated when omitted
  --protected-dir DIR     repeatable S02 protected directory (default none)
  --json                  machine-readable output

c01 options:
  --file PATH             repeatable source file (regular files only)
  --target ENC            utf-8 | utf-8-bom | gb18030 | utf-16le[-bom] | utf-16be[-bom]
  --sample-window BYTES   sampling window per block; explicit because §5.2 sets no number
  --sample-blocks N       number of evenly spaced sample blocks after the head (0 = head only)
  --source ENC            optional global source override: utf-8 | gb18030 | big5 |
                          shift-jis | latin1 | utf-16le | utf-16be
  --invalid POLICY        invalid-sequence policy (default: replace-and-count, §5.2)
  --platform-prior PRIOR  chinese-windows | other (default: other; informational only)
  --plan-id ULID          optional caller-supplied ULID; generated when omitted
  --json                  machine-readable plan output

f01 options:
  --file PATH             repeatable source text file
  --target-bytes N        target part size in bytes (no fixed minimum; §5.1)
  --output-dir DIR        directory for generated parts
  --plan-id ULID          optional caller-supplied ULID; generated when omitted
  --json                  machine-readable plan output

f03 options:
  --file PATH             repeatable source text file
  --parts N               number of parts (pre-scan line count, remainder to
                          first parts, at least one line per part; §5.15)
  --output-dir DIR        directory for generated parts
  --plan-id ULID          optional caller-supplied ULID; generated when omitted
  --json                  machine-readable plan output

apply/undo/cleanup:
  apply consumes a previously generated plan: c09 entries move to quarantine; c01
  entries transcode in place (backup in quarantine, undo restores from the Journal);
  f01/f02/f03 entries write line-aligned parts (undo moves the parts into quarantine);
  undo replays the Journal reverse operations for that planId;
  cleanup defaults to dry-run; --apply permanently removes expired quarantine
  files and their Journal entries. Retention defaults to 7 days (1–30).";

#[derive(Debug)]
struct CliOptions {
    path: PathBuf,
    scan: ScanOptions,
    json: bool,
    plan: bool,
    keep: Option<KeepPolicy>,
    plan_id: Option<String>,
    protected_dirs: Vec<PathBuf>,
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() == 1 {
        println!("batchdesk kernel {}", kernel::KERNEL_VERSION);
        println!("{USAGE}");
        return;
    }
    if args[1] == "--help" || args[1] == "-h" {
        println!("{USAGE}");
        return;
    }
    if args[1] == "apply" {
        std::process::exit(run_apply(&args));
    }
    if args[1] == "c01" {
        std::process::exit(run_c01(&args));
    }
    if args[1] == "f01" {
        std::process::exit(run_f01(&args));
    }
    if args[1] == "f02" {
        std::process::exit(run_f02(&args));
    }
    if args[1] == "f03" {
        std::process::exit(run_f03(&args));
    }
    if args[1] == "undo" {
        std::process::exit(run_undo(&args));
    }
    if args[1] == "cleanup" {
        std::process::exit(run_cleanup(&args));
    }

    let options = match parse_args(&args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };

    let scan_root = if options.path.is_absolute() {
        options.path.clone()
    } else {
        match env::current_dir() {
            Ok(current) => current.join(&options.path),
            Err(error) => {
                eprintln!("E9001: cannot resolve the current directory: {error}");
                std::process::exit(1);
            }
        }
    };
    let outcome = match scan_path(&scan_root, options.scan) {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("scan failed: {error}");
            std::process::exit(1);
        }
    };

    let candidates = build_candidates(&outcome, options.scan.mode);
    let grouping_mode = match options.scan.mode {
        ScanMode::Full => GroupingMode::Full,
        ScanMode::Fast => GroupingMode::Fast,
    };
    let report = group_duplicates(
        &candidates,
        GroupingOptions {
            mode: grouping_mode,
            include_zero_byte: false,
        },
    );

    if options.plan {
        let keep = options.keep.unwrap_or(KeepPolicy::Earliest);
        let plan_id = options
            .plan_id
            .clone()
            .unwrap_or_else(|| new_plan_id(&scan_root));
        let protected_dirs = match protected_dirs_from_paths(&options.protected_dirs) {
            Ok(protected_dirs) => protected_dirs,
            Err(error) => {
                eprintln!("{}: {}", error.code, error.message);
                std::process::exit(2);
            }
        };
        let case_sense = CliFsAdapter::new(None).case_sensitivity();
        let files = outcome
            .files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let canonical_path = if protected_dirs.is_empty() {
                    None
                } else {
                    platform::fs::canonicalize_display(&file.path)
                };
                PlanFile {
                    id: index as u64,
                    path: file.path.to_string_lossy().into_owned(),
                    canonical_path,
                    mtime_nanos: file.mtime_nanos,
                    full_hash: file.full_hash.clone(),
                }
            })
            .collect();
        let plan_input = PlanC09Input {
            plan_id,
            keep,
            mode: grouping_mode,
            hash_algorithm: "blake3".to_string(),
            quick_window_bytes: options.scan.quick_window,
            include_zero_byte: false,
            protected_dirs,
            case_sense,
            files,
            report,
            skipped_paths: outcome
                .skipped
                .iter()
                .map(|(path, _)| path.to_string_lossy().into_owned())
                .collect(),
        };

        match plan_c09(plan_input) {
            Ok(plan) => {
                if options.json {
                    print_plan_json(&plan);
                } else {
                    print_plan_human(&plan);
                }
            }
            Err(error) => {
                eprintln!("{}: {}", error.code, error.message);
                if let Some(detail) = error.detail {
                    eprintln!("detail: {detail}");
                }
                std::process::exit(if error.code.starts_with("E1") { 2 } else { 1 });
            }
        }
        return;
    }

    if options.json {
        print_json(&options, &outcome, &report);
    } else {
        print_human(&options, &outcome, &report);
    }
}

fn run_apply(args: &[String]) -> i32 {
    let mut plan_path = None;
    let mut quarantine_dir = None;
    let mut journal_path = None;
    let mut protected_dirs = Vec::new();
    let mut json = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--plan" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: apply requires --plan <plan.json>");
                };
                plan_path = Some(PathBuf::from(value));
            }
            "--quarantine-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: apply requires --quarantine-dir <dir>");
                };
                quarantine_dir = Some(PathBuf::from(value));
            }
            "--journal" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: apply requires --journal <file>");
                };
                journal_path = Some(PathBuf::from(value));
            }
            "--protected-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --protected-dir requires a path");
                };
                protected_dirs.push(PathBuf::from(value));
            }
            "--json" => json = true,
            other => return input_error(format!("E1002: unknown apply argument: {other}")),
        }
        index += 1;
    }
    let Some(plan_path) = plan_path else {
        return input_error("E1001: apply requires --plan <plan.json>");
    };
    let Some(quarantine_dir) = quarantine_dir else {
        return input_error("E1001: apply requires --quarantine-dir <dir>");
    };
    let Some(journal_path) = journal_path else {
        return input_error("E1001: apply requires --journal <file>");
    };

    let contents = match std::fs::read_to_string(&plan_path) {
        Ok(contents) => contents,
        Err(error) => {
            eprintln!(
                "E1004: plan file is not readable ({}): {error}",
                plan_path.display()
            );
            return 2;
        }
    };
    let plan: PlanDocument = match serde_json::from_str(&contents) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!(
                "E1004: plan file is not valid PlanDocument JSON ({}): {error}",
                plan_path.display()
            );
            return 2;
        }
    };

    let adapter = CliFsAdapter::new(Some(make_absolute(&quarantine_dir)));
    let mut journal = match FileJournal::open(journal_path) {
        Ok(journal) => journal,
        Err(error) => return print_kernel_error(&error),
    };
    let protected_dirs = match protected_dirs_from_paths(&protected_dirs) {
        Ok(protected_dirs) => protected_dirs,
        Err(error) => {
            eprintln!("{}: {}", error.code, error.message);
            return 2;
        }
    };
    let license = AlwaysAllowLicense;
    if plan.kind == kernel::plan::C09_KIND {
        let mut context = ApplyContext {
            fs: &adapter,
            license: &license,
            journal: &mut journal,
            task_id: new_ulid(&format!("apply:{}", plan.plan_id)),
            now_rfc3339: now_rfc3339(),
            protected_dirs,
            case_sense: adapter.case_sensitivity(),
            progress: None,
            cancel: None,
        };
        run_generic_or_specific(
            apply_plan(&plan, &ApplyOptions::default(), &mut context),
            json,
        )
    } else {
        // 通用执行器：c01 等 rename/move/copy/delete/edit-content 计划。
        let has_edit_content = plan
            .entries
            .iter()
            .any(|entry| entry.action == kernel::plan::PlanAction::EditContent);
        let encoding = match has_edit_content {
            true => match generic_plan_encoding(&plan) {
                Ok(encoding) => Some(encoding),
                Err(error) => return print_kernel_error(&error),
            },
            false => None,
        };
        let editor = |data: &[u8]| -> Result<Vec<u8>, kernel::error::KernelError> {
            let Some((target, invalid, prior, source)) = encoding else {
                return Err(kernel::error::KernelError::invalid_params(
                    "content editor is only available for transcode plans",
                ));
            };
            let source = match source {
                Some(source) => source,
                None => detect(data, prior).require_encoding()?,
            };
            transcode(data, source, target, invalid).map(|result| result.bytes)
        };
        let content_editor = has_edit_content.then_some(&editor as ContentEditor<'_>);
        let mut generic = GenericApplyContext {
            fs: &adapter,
            license: &license,
            journal: &mut journal,
            task_id: new_ulid(&format!("apply:{}", plan.plan_id)),
            now_rfc3339: now_rfc3339(),
            protected_dirs,
            case_sense: adapter.case_sensitivity(),
            progress: None,
            cancel: None,
            content_editor,
        };
        run_generic_or_specific(
            apply_generic_plan(&plan, &ApplyOptions::default(), &mut generic),
            json,
        )
    }
}

fn run_generic_or_specific(
    outcome: Result<ApplyOutcome, kernel::error::KernelError>,
    json: bool,
) -> i32 {
    match outcome {
        Ok(outcome) => {
            if json {
                match serde_json::to_string(&outcome) {
                    Ok(json) => println!("{json}"),
                    Err(error) => {
                        eprintln!("E9001: failed to serialize apply report: {error}");
                        return 1;
                    }
                }
            } else {
                print_apply_human(&outcome);
            }
            if outcome.state == kernel::apply::ApplyState::Completed {
                0
            } else {
                1
            }
        }
        Err(error) => print_kernel_error(&error),
    }
}

/// c01 计划的 apply 侧参数（与 plan 期写入的 params 快照保持一致）。
fn generic_plan_encoding(
    plan: &PlanDocument,
) -> Result<
    (
        TargetEncoding,
        InvalidSequencePolicy,
        PlatformPrior,
        Option<SourceEncoding>,
    ),
    kernel::error::KernelError,
> {
    let string_param = |name: &str| -> Result<&str, kernel::error::KernelError> {
        match plan.params.get(name) {
            Some(kernel::plan::PlanParamValue::String(value)) => Ok(value.as_str()),
            _ => Err(kernel::error::KernelError::missing_param(format!(
                "plan param {name} is required"
            ))),
        }
    };
    let target = TargetEncoding::parse(string_param("target")?).ok_or_else(|| {
        kernel::error::KernelError::invalid_params(
            "plan param target is not a supported target encoding",
        )
    })?;
    let invalid = InvalidSequencePolicy::parse(string_param("invalid")?).ok_or_else(|| {
        kernel::error::KernelError::invalid_params(
            "plan param invalid is not a supported invalid-sequence policy",
        )
    })?;
    let prior = PlatformPrior::parse(string_param("platformPrior")?).ok_or_else(|| {
        kernel::error::KernelError::invalid_params(
            "plan param platformPrior is not a supported platform prior",
        )
    })?;
    let source = match plan.params.get("source") {
        Some(kernel::plan::PlanParamValue::String(value)) => {
            Some(SourceEncoding::parse(value).ok_or_else(|| {
                kernel::error::KernelError::invalid_params(
                    "plan param source is not a supported source encoding",
                )
            })?)
        }
        _ => None,
    };
    Ok((target, invalid, prior, source))
}

fn run_c01(args: &[String]) -> i32 {
    let mut files = Vec::new();
    let mut target = None;
    let mut source = None;
    let mut invalid = InvalidSequencePolicy::ReplaceAndCount;
    let mut prior = PlatformPrior::Other;
    let mut sample_window = None;
    let mut sample_blocks = None;
    let mut plan_id = None;
    let mut protected_dirs = Vec::new();
    let mut json = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--file" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --file requires a path");
                };
                files.push(PathBuf::from(value));
            }
            "--target" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --target requires an encoding");
                };
                let Some(parsed) = TargetEncoding::parse(value) else {
                    return input_error(format!("E1002: unknown target encoding: {value}"));
                };
                target = Some(parsed);
            }
            "--source" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --source requires an encoding");
                };
                let Some(parsed) = SourceEncoding::parse(value) else {
                    return input_error(format!("E1002: unknown source encoding: {value}"));
                };
                source = Some(parsed);
            }
            "--invalid" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --invalid requires a policy");
                };
                let Some(parsed) = InvalidSequencePolicy::parse(value) else {
                    return input_error(format!("E1002: unknown invalid-sequence policy: {value}"));
                };
                invalid = parsed;
            }
            "--platform-prior" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --platform-prior requires a value");
                };
                let Some(parsed) = PlatformPrior::parse(value) else {
                    return input_error(format!("E1002: unknown platform prior: {value}"));
                };
                prior = parsed;
            }
            "--sample-window" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --sample-window requires bytes");
                };
                match value.parse::<u64>() {
                    Ok(window) if window > 0 => sample_window = Some(window),
                    _ => {
                        return input_error(format!(
                            "E1002: --sample-window must be a positive byte count: {value}"
                        ))
                    }
                }
            }
            "--sample-blocks" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --sample-blocks requires a count");
                };
                match value.parse::<usize>() {
                    Ok(blocks) => sample_blocks = Some(blocks),
                    Err(_) => {
                        return input_error(format!(
                            "E1002: --sample-blocks must be a non-negative integer: {value}"
                        ))
                    }
                }
            }
            "--plan-id" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --plan-id requires a ULID");
                };
                plan_id = Some(value.clone());
            }
            "--protected-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --protected-dir requires a path");
                };
                protected_dirs.push(PathBuf::from(value));
            }
            "--json" => json = true,
            other => return input_error(format!("E1002: unknown c01 argument: {other}")),
        }
        index += 1;
    }

    if files.is_empty() {
        return input_error("E1001: c01 requires at least one --file <path>");
    }
    let Some(target) = target else {
        return input_error("E1001: c01 requires --target <encoding>");
    };
    let Some(sample_window) = sample_window else {
        return input_error(
            "E1001: c01 requires --sample-window <bytes> (方案 §5.2 sets no numeric window)",
        );
    };
    let Some(sample_blocks) = sample_blocks else {
        return input_error("E1001: c01 requires --sample-blocks <n> (0 = head only)");
    };

    let protected_dirs = match protected_dirs_from_paths(&protected_dirs) {
        Ok(protected_dirs) => protected_dirs,
        Err(error) => {
            eprintln!("{}: {}", error.code, error.message);
            return 2;
        }
    };
    let adapter = CliFsAdapter::new(None);
    let case_sense = adapter.case_sensitivity();
    let spec = SampleSpec {
        window_bytes: sample_window,
        blocks: sample_blocks,
    };
    let mut plan_files = Vec::with_capacity(files.len());
    for path in &files {
        let absolute = make_absolute(path);
        let metadata = match std::fs::metadata(&absolute) {
            Ok(metadata) => metadata,
            Err(error) => {
                eprintln!(
                    "E1004: cannot read file metadata ({}): {error}",
                    absolute.display()
                );
                return 2;
            }
        };
        if !metadata.is_file() {
            eprintln!("E1002: c01 requires regular files: {}", absolute.display());
            return 2;
        }
        let sampled = match sample_bytes(&absolute, spec) {
            Ok(sampled) => sampled,
            Err(error) => {
                eprintln!(
                    "E1004: cannot sample file ({}): {error}",
                    absolute.display()
                );
                return 2;
            }
        };
        let canonical_path = if protected_dirs.is_empty() {
            None
        } else {
            platform::fs::canonicalize_display(&absolute)
        };
        plan_files.push(PlanC01File {
            path: absolute.to_string_lossy().into_owned(),
            canonical_path,
            file_size: sampled.file_size,
            sample: sampled.bytes,
        });
    }

    let plan_id = plan_id.unwrap_or_else(|| new_ulid("c01"));
    let input = kernel::plan::PlanC01Input {
        plan_id,
        target,
        invalid,
        prior,
        confirmed_source: source,
        protected_dirs,
        case_sense,
        files: plan_files,
    };
    match plan_c01(input) {
        Ok(plan) => {
            if json {
                print_plan_json(&plan);
            } else {
                print_plan_human(&plan);
            }
            0
        }
        Err(error) => print_kernel_error(&error),
    }
}

/// F01/F02/F03 共用 CLI 入口，仅计量基准与数量参数不同。
#[derive(Clone, Copy, PartialEq, Eq)]
enum SplitMode {
    Bytes,
    Lines,
    Parts,
}

impl SplitMode {
    fn kind(self) -> &'static str {
        match self {
            SplitMode::Bytes => "f01",
            SplitMode::Lines => "f02",
            SplitMode::Parts => "f03",
        }
    }

    fn quantity_flag(self) -> &'static str {
        match self {
            SplitMode::Bytes => "--target-bytes",
            SplitMode::Lines => "--lines",
            SplitMode::Parts => "--parts",
        }
    }
}

fn run_f01(args: &[String]) -> i32 {
    run_split(args, SplitMode::Bytes)
}

fn run_f02(args: &[String]) -> i32 {
    run_split(args, SplitMode::Lines)
}

fn run_f03(args: &[String]) -> i32 {
    run_split(args, SplitMode::Parts)
}

fn run_split(args: &[String], mode: SplitMode) -> i32 {
    let kind = mode.kind();
    let mut files = Vec::new();
    let mut quantity = None;
    let mut output_dir = None;
    let mut plan_id = None;
    let mut protected_dirs = Vec::new();
    let mut json = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--file" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --file requires a path");
                };
                files.push(PathBuf::from(value));
            }
            "--target-bytes" if mode == SplitMode::Bytes => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --target-bytes requires a byte count");
                };
                match value.parse::<u64>() {
                    Ok(bytes) if bytes > 0 => quantity = Some(bytes),
                    _ => {
                        return input_error(format!(
                            "E1002: --target-bytes must be a positive byte count: {value}"
                        ))
                    }
                }
            }
            "--lines" if mode == SplitMode::Lines => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --lines requires a line count");
                };
                match value.parse::<u64>() {
                    Ok(lines) if lines > 0 => quantity = Some(lines),
                    _ => {
                        return input_error(format!(
                            "E1002: --lines must be a positive line count: {value}"
                        ))
                    }
                }
            }
            "--parts" if mode == SplitMode::Parts => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --parts requires a part count");
                };
                match value.parse::<u64>() {
                    Ok(parts) if parts > 0 => quantity = Some(parts),
                    _ => {
                        return input_error(format!(
                            "E1002: --parts must be a positive part count: {value}"
                        ))
                    }
                }
            }
            "--output-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --output-dir requires a path");
                };
                output_dir = Some(PathBuf::from(value));
            }
            "--plan-id" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --plan-id requires a ULID");
                };
                plan_id = Some(value.clone());
            }
            "--protected-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --protected-dir requires a path");
                };
                protected_dirs.push(PathBuf::from(value));
            }
            "--json" => json = true,
            other => return input_error(format!("E1002: unknown {kind} argument: {other}")),
        }
        index += 1;
    }

    if files.is_empty() {
        return input_error(format!("E1001: {kind} requires at least one --file <path>"));
    }
    let Some(quantity) = quantity else {
        return input_error(format!(
            "E1001: {kind} requires {} <n>",
            mode.quantity_flag()
        ));
    };
    let Some(output_dir) = output_dir else {
        return input_error(format!("E1001: {kind} requires --output-dir <dir>"));
    };
    let output_dir = make_absolute(&output_dir);
    let protected_dirs = match protected_dirs_from_paths(&protected_dirs) {
        Ok(protected_dirs) => protected_dirs,
        Err(error) => {
            eprintln!("{}: {}", error.code, error.message);
            return 2;
        }
    };
    let adapter = CliFsAdapter::new(None);
    let case_sense = adapter.case_sensitivity();
    let mut plan_files = Vec::with_capacity(files.len());
    let mut existing_targets = Vec::new();
    for path in &files {
        let absolute = make_absolute(path);
        let metadata = match std::fs::metadata(&absolute) {
            Ok(metadata) => metadata,
            Err(error) => {
                eprintln!(
                    "E1004: cannot read file metadata ({}): {error}",
                    absolute.display()
                );
                return 2;
            }
        };
        if !metadata.is_file() {
            eprintln!(
                "E1002: {kind} requires regular files: {}",
                absolute.display()
            );
            return 2;
        }
        let layout = match mode {
            SplitMode::Bytes => split_layout(&absolute, quantity),
            SplitMode::Lines => split_layout_by_lines(&absolute, quantity),
            SplitMode::Parts => split_layout_by_parts(&absolute, quantity),
        };
        let layout = match layout {
            Ok(layout) => layout,
            Err(error) => {
                eprintln!(
                    "E1004: cannot scan file for split layout ({}): {error}",
                    absolute.display()
                );
                return 2;
            }
        };
        let file_name = absolute
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("file");
        let total = layout.spans.len() as u32;
        for span in &layout.spans {
            let target = output_dir.join(kernel::split::shard_name(file_name, span.index, total));
            if target.exists() {
                existing_targets.push(target.to_string_lossy().into_owned());
            }
        }
        let canonical_path = if protected_dirs.is_empty() {
            None
        } else {
            platform::fs::canonicalize_display(&absolute)
        };
        plan_files.push(PlanF01File {
            path: absolute.to_string_lossy().into_owned(),
            canonical_path,
            group_id: new_ulid(kind),
            layout,
        });
    }

    let plan_id = plan_id.unwrap_or_else(|| new_ulid(kind));
    let output_dir = output_dir.to_string_lossy().into_owned();
    let planned = match mode {
        SplitMode::Bytes => plan_f01(PlanF01Input {
            plan_id,
            target_bytes: quantity,
            output_dir,
            protected_dirs,
            case_sense,
            existing_targets,
            files: plan_files,
        }),
        SplitMode::Lines => plan_f02(PlanF02Input {
            plan_id,
            lines_per_part: quantity,
            output_dir,
            protected_dirs,
            case_sense,
            existing_targets,
            files: plan_files,
        }),
        SplitMode::Parts => plan_f03(PlanF03Input {
            plan_id,
            parts: quantity,
            output_dir,
            protected_dirs,
            case_sense,
            existing_targets,
            files: plan_files,
        }),
    };
    match planned {
        Ok(plan) => {
            if json {
                print_plan_json(&plan);
            } else {
                print_plan_human(&plan);
            }
            0
        }
        Err(error) => print_kernel_error(&error),
    }
}

fn run_undo(args: &[String]) -> i32 {
    let mut plan_id = None;
    let mut journal_path = None;
    let mut retention_days = DEFAULT_RETENTION_DAYS;
    let mut json = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--plan" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: undo requires --plan <planId>");
                };
                plan_id = Some(value.clone());
            }
            "--journal" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: undo requires --journal <file>");
                };
                journal_path = Some(PathBuf::from(value));
            }
            "--retention-days" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --retention-days requires a value");
                };
                retention_days = match parse_retention_days(value) {
                    Ok(days) => days,
                    Err(message) => return input_error(message),
                };
            }
            "--json" => json = true,
            other => return input_error(format!("E1002: unknown undo argument: {other}")),
        }
        index += 1;
    }
    let Some(plan_id) = plan_id else {
        return input_error("E1001: undo requires --plan <planId>");
    };
    let Some(journal_path) = journal_path else {
        return input_error("E1001: undo requires --journal <file>");
    };
    if !journal_path.exists() {
        eprintln!(
            "E2001: journal file does not exist: {}",
            journal_path.display()
        );
        return 1;
    }

    let mut journal = match FileJournal::open(journal_path) {
        Ok(journal) => journal,
        Err(error) => return print_kernel_error(&error),
    };
    let cutoff = rfc3339_days_ago(retention_days);
    let entries = match journal.entries_for_plan(&plan_id) {
        Ok(entries) => entries,
        Err(error) => return print_kernel_error(&error),
    };
    if entries.iter().any(|entry| entry.ts <= cutoff) {
        eprintln!(
            "E9001: undo window expired (retention {retention_days} days); run cleanup to purge expired quarantine data"
        );
        return 1;
    }
    let adapter = CliFsAdapter::new(None);
    let license = AlwaysAllowLicense;
    match undo_plan(&plan_id, &adapter, &license, &mut journal) {
        Ok(outcome) => {
            if json {
                match serde_json::to_string(&outcome) {
                    Ok(json) => println!("{json}"),
                    Err(error) => {
                        eprintln!("E9001: failed to serialize undo report: {error}");
                        return 1;
                    }
                }
            } else {
                print_undo_human(&plan_id, &outcome);
            }
            if outcome.failed.is_empty() {
                0
            } else {
                1
            }
        }
        Err(error) => print_kernel_error(&error),
    }
}

fn run_cleanup(args: &[String]) -> i32 {
    let mut journal_path = None;
    let mut quarantine_dir = None;
    let mut retention_days = DEFAULT_RETENTION_DAYS;
    let mut apply = false;
    let mut json = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--journal" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: cleanup requires --journal <file>");
                };
                journal_path = Some(PathBuf::from(value));
            }
            "--quarantine-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: cleanup requires --quarantine-dir <dir>");
                };
                quarantine_dir = Some(PathBuf::from(value));
            }
            "--retention-days" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --retention-days requires a value");
                };
                retention_days = match parse_retention_days(value) {
                    Ok(days) => days,
                    Err(message) => return input_error(message),
                };
            }
            "--apply" => apply = true,
            "--json" => json = true,
            other => return input_error(format!("E1002: unknown cleanup argument: {other}")),
        }
        index += 1;
    }
    let Some(journal_path) = journal_path else {
        return input_error("E1001: cleanup requires --journal <file>");
    };
    let Some(quarantine_dir) = quarantine_dir else {
        return input_error("E1001: cleanup requires --quarantine-dir <dir>");
    };
    if !journal_path.exists() {
        eprintln!(
            "E2001: journal file does not exist: {}",
            journal_path.display()
        );
        return 1;
    }

    let mut journal = match FileJournal::open(journal_path) {
        Ok(journal) => journal,
        Err(error) => return print_kernel_error(&error),
    };
    let outcome = match cleanup(&mut journal, &quarantine_dir, retention_days, apply) {
        Ok(outcome) => outcome,
        Err(error) => return print_kernel_error(&error),
    };
    if json {
        match serde_json::to_string(&outcome) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("E9001: failed to serialize cleanup report: {error}");
                return 1;
            }
        }
    } else {
        print_cleanup_human(&outcome);
    }
    if outcome.failed.is_empty() {
        0
    } else {
        1
    }
}

fn print_cleanup_human(outcome: &CleanupOutcome) {
    println!(
        "cleanup cutoff={} retention_days={} dry_run={} expired_entries={} expired_files={}",
        outcome.cutoff,
        outcome.retention_days,
        outcome.dry_run,
        outcome.expired_entries,
        outcome.expired_quarantine_files
    );
    for failure in &outcome.failed {
        println!(
            "  {} {} — {}",
            failure.entry_id,
            failure.path.as_deref().unwrap_or("-"),
            failure.message
        );
    }
    println!(
        "summary: removed_entries={} removed_files={} failed={}",
        outcome.removed_entries,
        outcome.removed_files,
        outcome.failed.len()
    );
}

fn parse_retention_days(value: &str) -> Result<u64, String> {
    let days = value
        .parse::<u64>()
        .map_err(|_| format!("E1002: invalid retention days: {value}"))?;
    validate_retention_days(days).map_err(|error| format!("{}: {}", error.code, error.message))?;
    Ok(days)
}

fn print_apply_human(outcome: &ApplyOutcome) {
    println!(
        "apply plan={} task={} state={}",
        outcome.report.plan_id,
        outcome.report.task_id,
        outcome.state.as_str()
    );
    let mut ok = 0;
    let mut skipped = 0;
    let mut failed = 0;
    for entry in &outcome.report.entries {
        match entry.status.as_str() {
            "ok" | "overwritten" => ok += 1,
            "skipped" => skipped += 1,
            "failed" => failed += 1,
            _ => {}
        }
        match &entry.error {
            Some(error) => println!(
                "  {} {} — {}: {}",
                entry.entry_id,
                entry.status.as_str(),
                error.code,
                error.message
            ),
            None => println!("  {} {}", entry.entry_id, entry.status.as_str()),
        }
    }
    println!("summary: ok={ok} skipped={skipped} failed={failed}");
}

fn print_undo_human(plan_id: &str, outcome: &UndoOutcome) {
    println!(
        "undo plan={} reverted={} failed={}",
        plan_id,
        outcome.reverted,
        outcome.failed.len()
    );
    for failure in &outcome.failed {
        println!(
            "  {} failed — {}: {}",
            failure.entry_id, failure.error.code, failure.error.message
        );
    }
}

fn input_error(message: impl Into<String>) -> i32 {
    eprintln!("{}", message.into());
    eprintln!("{USAGE}");
    2
}

fn print_kernel_error(error: &kernel::error::KernelError) -> i32 {
    eprintln!("{}: {}", error.code, error.message);
    if let Some(detail) = &error.detail {
        eprintln!("detail: {detail}");
    }
    if error.code.starts_with("E1") {
        2
    } else {
        1
    }
}

fn parse_args(args: &[String]) -> Result<CliOptions, String> {
    if args.len() < 2 || args[1] != "c09" {
        return Err("expected the c09 subcommand".to_string());
    }
    let path = PathBuf::from(
        args.get(2)
            .ok_or_else(|| "c09 requires a path".to_string())?,
    );

    let mut mode = ScanMode::Full;
    let mut quick_window = None;
    let mut json = false;
    let mut plan = false;
    let mut keep = None;
    let mut plan_id = None;
    let mut protected_dirs = Vec::new();
    let mut index = 3;
    while index < args.len() {
        match args[index].as_str() {
            "--mode" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--mode requires full|fast".to_string())?;
                mode = match value.as_str() {
                    "full" => ScanMode::Full,
                    "fast" => ScanMode::Fast,
                    other => return Err(format!("unknown mode: {other}")),
                };
            }
            "--quick-window-bytes" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--quick-window-bytes requires a value".to_string())?;
                let window = value
                    .parse::<u64>()
                    .map_err(|_| format!("invalid quick window: {value}"))?;
                quick_window = Some(window);
            }
            "--json" => json = true,
            "--plan" => plan = true,
            "--keep" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    "E1001: --keep requires earliest|shortest-path|newest".to_string()
                })?;
                keep = Some(
                    KeepPolicy::parse(value)
                        .ok_or_else(|| format!("E1002: invalid keep policy: {value}"))?,
                );
            }
            "--plan-id" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "E1001: --plan-id requires a ULID".to_string())?;
                if !is_ulid(value) {
                    return Err(format!(
                        "E1002: invalid plan id (expected a 26-char ULID): {value}"
                    ));
                }
                plan_id = Some(value.clone());
            }
            "--protected-dir" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "E1001: --protected-dir requires a path".to_string())?;
                protected_dirs.push(PathBuf::from(value));
            }
            other => return Err(format!("unknown argument: {other}")),
        }
        index += 1;
    }

    if plan {
        if mode == ScanMode::Fast {
            return Err(
                "E1002: plan preview requires --mode full; fast mode produces candidate groups only"
                    .to_string(),
            );
        }
        if keep.is_none() {
            keep = Some(KeepPolicy::Earliest);
        }
    } else {
        if keep.is_some() {
            return Err("E1002: --keep is only valid with --plan".to_string());
        }
        if plan_id.is_some() {
            return Err("E1002: --plan-id is only valid with --plan".to_string());
        }
    }

    let scan = ScanOptions { mode, quick_window };
    scan.validate().map_err(|message| {
        let code = if message.contains("greater than zero") {
            "E1002"
        } else {
            "E1001"
        };
        format!("{code}: {message}")
    })?;
    Ok(CliOptions {
        path,
        scan,
        json,
        plan,
        keep,
        plan_id,
        protected_dirs,
    })
}

fn build_candidates(outcome: &ScanOutcome, mode: ScanMode) -> Vec<DuplicateCandidate> {
    outcome
        .files
        .iter()
        .enumerate()
        .filter_map(|(index, file)| {
            if file.size == 0 {
                return Some(DuplicateCandidate {
                    id: index as u64,
                    size: 0,
                    quick_hash: None,
                    full_hash: None,
                    identity: file.identity,
                });
            }
            let (quick_hash, full_hash) = match mode {
                ScanMode::Full => (file.quick_hash.clone(), file.full_hash.clone()),
                ScanMode::Fast => (file.quick_hash.clone(), None),
            };
            let has_required = match mode {
                ScanMode::Full => full_hash.is_some(),
                ScanMode::Fast => quick_hash.is_some(),
            };
            has_required.then_some(DuplicateCandidate {
                id: index as u64,
                size: file.size,
                quick_hash,
                full_hash,
                identity: file.identity,
            })
        })
        .collect()
}

fn print_human(options: &CliOptions, outcome: &ScanOutcome, report: &DuplicateReport) {
    for (path, reason) in &outcome.skipped {
        eprintln!("skip {}: {reason}", path.display());
    }

    println!(
        "mode={} quick_window={} files={} quick_hashed={} full_hashed={} size_pruned={} quick_pruned={}",
        mode_name(options.scan.mode),
        quick_window_name(options.scan.quick_window),
        outcome.files.len(),
        outcome.stats.quick_hashed,
        outcome.stats.full_hashed,
        outcome.stats.size_pruned,
        outcome.stats.quick_pruned
    );

    for (group_index, group) in report.groups.iter().enumerate() {
        println!(
            "group {}: size={} members={} confidence={} hash={} manual_review={}",
            group_index + 1,
            group.size,
            group.members.len(),
            confidence_name(group.confidence),
            group
                .hash
                .as_deref()
                .map(hex)
                .unwrap_or_else(|| "none".to_string()),
            group.needs_manual_review
        );
        for member in &group.members {
            println!("  {}", path_of(outcome, *member));
        }
    }

    for id in &report.zero_byte_ids {
        println!("zero-byte ignored: {} (id={id})", path_of(outcome, *id));
    }
    for id in &report.hardlink_skipped {
        println!("hardlink skipped: {} (id={id})", path_of(outcome, *id));
    }
    for id in &report.incomplete {
        println!("incomplete: {} (id={id})", path_of(outcome, *id));
    }

    println!(
        "summary: files={} groups={} zero_byte={} incomplete={} hardlink_skipped={} skipped={}",
        outcome.files.len(),
        report.groups.len(),
        report.zero_byte_ids.len(),
        report.incomplete.len(),
        report.hardlink_skipped.len(),
        outcome.skipped.len()
    );
}

fn print_plan_human(plan: &PlanDocument) {
    println!(
        "plan {} kind={} entries={} warnings={}",
        plan.plan_id, plan.kind, plan.stats.total, plan.stats.warning
    );
    for (index, entry) in plan.entries.iter().enumerate() {
        println!(
            "  {}. {} {} — {}",
            index + 1,
            entry.action.as_str(),
            entry.source,
            entry.reason
        );
    }
    for warning in &plan.warnings {
        println!(
            "  warning {} ({}): {}",
            warning.kind,
            warning.risk_level.as_str(),
            warning.suggestion.as_deref().unwrap_or("")
        );
    }
    println!(
        "stats: total={} conflict={} warning={} blocked={}",
        plan.stats.total, plan.stats.conflict, plan.stats.warning, plan.stats.blocked
    );
    println!("preview only: no files were written");
}

fn print_json(options: &CliOptions, outcome: &ScanOutcome, report: &DuplicateReport) {
    let mut out = String::new();
    out.push('{');
    out.push_str(&format!("\"mode\":\"{}\",", mode_name(options.scan.mode)));
    out.push_str(&format!(
        "\"quick_window_bytes\":{},",
        match options.scan.quick_window {
            Some(window) => window.to_string(),
            None => "null".to_string(),
        }
    ));
    out.push_str(&format!(
        "\"stats\":{{\"quick_hashed\":{},\"full_hashed\":{},\"size_pruned\":{},\"quick_pruned\":{}}},",
        outcome.stats.quick_hashed,
        outcome.stats.full_hashed,
        outcome.stats.size_pruned,
        outcome.stats.quick_pruned
    ));

    out.push_str("\"skipped\":[");
    for (index, (path, reason)) in outcome.skipped.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"path\":\"{}\",\"reason\":\"{}\"}}",
            json_escape(&path.to_string_lossy()),
            json_escape(reason)
        ));
    }
    out.push(']');

    out.push_str(",\"files\":[");
    for (index, file) in outcome.files.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"id\":{index},\"path\":\"{}\",\"size\":{},\"quick_hash\":{},\"full_hash\":{},\"identity\":{}}}",
            json_escape(&file.path.to_string_lossy()),
            file.size,
            file.quick_hash.is_some(),
            file.full_hash.is_some(),
            file.identity.is_some()
        ));
    }
    out.push(']');

    out.push_str(",\"groups\":[");
    for (index, group) in report.groups.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let hash = match &group.hash {
            Some(hash) => format!("\"{}\"", hex(hash)),
            None => "null".to_string(),
        };
        out.push_str(&format!(
            "{{\"size\":{},\"members\":[{}],\"confidence\":\"{}\",\"needs_manual_review\":{},\"hash\":{}}}",
            group.size,
            json_ids(&group.members),
            confidence_name(group.confidence),
            group.needs_manual_review,
            hash
        ));
    }
    out.push(']');

    out.push_str(&format!(
        ",\"zero_byte_ids\":[{}],\"hardlink_skipped\":[{}],\"incomplete\":[{}]}}",
        json_ids(&report.zero_byte_ids),
        json_ids(&report.hardlink_skipped),
        json_ids(&report.incomplete)
    ));
    println!("{out}");
}

fn print_plan_json(plan: &PlanDocument) {
    match serde_json::to_string(plan) {
        Ok(json) => println!("{json}"),
        Err(error) => {
            eprintln!("E9001: failed to serialize plan: {error}");
            std::process::exit(1);
        }
    }
}

fn path_of(outcome: &ScanOutcome, id: u64) -> String {
    outcome
        .files
        .get(id as usize)
        .map(|file| file.path.display().to_string())
        .unwrap_or_else(|| format!("<unknown id {id}>"))
}

fn mode_name(mode: ScanMode) -> &'static str {
    match mode {
        ScanMode::Full => "full",
        ScanMode::Fast => "fast",
    }
}

fn quick_window_name(window: Option<u64>) -> String {
    match window {
        Some(window) => window.to_string(),
        None => "none".to_string(),
    }
}

fn confidence_name(confidence: kernel::duplicate::Confidence) -> &'static str {
    match confidence {
        kernel::duplicate::Confidence::Confirmed => "confirmed",
        kernel::duplicate::Confidence::Candidate => "candidate",
    }
}

fn json_ids(ids: &[u64]) -> String {
    ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",")
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if (character as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => out.push(character),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(extra: &[&str]) -> Vec<String> {
        let mut args = vec![
            "batchdesk".to_string(),
            "c09".to_string(),
            "dir".to_string(),
        ];
        args.extend(extra.iter().map(|value| value.to_string()));
        args
    }

    #[test]
    fn plan_defaults_to_earliest() {
        let options = parse_args(&args(&["--plan"])).unwrap();
        assert_eq!(options.keep, Some(KeepPolicy::Earliest));
    }

    #[test]
    fn plan_rejects_fast_mode() {
        let error = parse_args(&args(&[
            "--mode",
            "fast",
            "--quick-window-bytes",
            "4096",
            "--plan",
            "--keep",
            "earliest",
        ]))
        .unwrap_err();
        assert!(error.starts_with("E1002"));
    }

    #[test]
    fn keep_is_only_valid_with_plan() {
        let error = parse_args(&args(&["--keep", "earliest"])).unwrap_err();
        assert!(error.starts_with("E1002"));
    }

    #[test]
    fn invalid_plan_id_is_rejected() {
        let error = parse_args(&args(&[
            "--plan",
            "--keep",
            "earliest",
            "--plan-id",
            "not-a-ulid",
        ]))
        .unwrap_err();
        assert!(error.starts_with("E1002"));
    }

    #[test]
    fn valid_plan_arguments_are_accepted() {
        let options = parse_args(&args(&[
            "--plan",
            "--keep",
            "shortest-path",
            "--plan-id",
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
        ]))
        .unwrap();
        assert!(options.plan);
        assert_eq!(options.keep, Some(KeepPolicy::ShortestPath));
        assert_eq!(
            options.plan_id.as_deref(),
            Some("01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8")
        );
    }

    #[test]
    fn plan_document_round_trips_through_json() {
        let mut params = std::collections::BTreeMap::new();
        params.insert(
            "quickWindowBytes".to_string(),
            kernel::plan::PlanParamValue::Null,
        );
        params.insert(
            "keep".to_string(),
            kernel::plan::PlanParamValue::String("earliest".to_string()),
        );
        let plan = PlanDocument {
            plan_id: "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8".to_string(),
            kind: "c09".to_string(),
            params,
            entries: Vec::new(),
            stats: Default::default(),
            warnings: Vec::new(),
        };
        let json = serde_json::to_string(&plan).unwrap();
        let decoded: PlanDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, decoded);
    }
}
