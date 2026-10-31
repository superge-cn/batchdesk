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
use kernel::create_dirs::{
    normalize_list_path, plan_f11, target_for_list_path, PlanF11Input, PlanF11Row,
};
use kernel::csv::{plan_f05, CsvSplitBasis, Delimiter, PlanF05File, PlanF05Input};
use kernel::csv_extract::{plan_f07, ColumnPick, PlanF07File, PlanF07Input};
use kernel::csv_merge::{parse_records, plan_f06, PlanF06File, PlanF06Input};
use kernel::csv_range::{plan_f09, PlanF09File, PlanF09Input};
use kernel::csv_rows::{plan_f08, PlanF08File, PlanF08Input, RowWindow};
use kernel::duplicate::{
    group_duplicates, DuplicateCandidate, DuplicateReport, GroupingMode, GroupingOptions,
};
use kernel::encoding::{
    detect, transcode, InvalidSequencePolicy, PlatformPrior, SourceEncoding, TargetEncoding,
};
use kernel::fs::FsAdapter;
use kernel::generic_apply::{
    apply_generic_plan, ContentEditor, ContentRewriter, GenericApplyContext,
};
use kernel::journal::JournalStore;
use kernel::license::AlwaysAllowLicense;
use kernel::line_endings::{
    plan_c02, LineEnding, LineEndingRewriter, LineEndingStructureMode, PlanC02File, PlanC02Input,
};
use kernel::listing::{plan_f10, ListingOptions, PlanF10File, PlanF10Input};
use kernel::merge::{plan_f04, PlanF04File, PlanF04Input};
use kernel::plan::{
    plan_c01, plan_c09, KeepPolicy, PlanC01File, PlanC09Input, PlanDocument, PlanFile,
};
use kernel::regex_rename::{plan_f13, PlanF13Input, PlanF13Item, PlanF13Rule};
use kernel::rename::{plan_f12, PlanF12Input, PlanF12Item, PlanF12Rule, RenameScope};
use kernel::replace_content::{
    plan_c03, CsvMode, PlanC03File, PlanC03Input, ReplaceMode, ReplaceRewriter,
};
use kernel::split::{
    plan_f01, plan_f02, plan_f03, PlanF01File, PlanF01Input, PlanF02Input, PlanF03Input,
};
use platform::csv::{csv_split_layout, CsvScanOptions};
use platform::csv_merge::{csv_merge_inspect, CsvMergeScanOptions};
use platform::csv_range::{csv_range_layout, CsvRangeScanOptions};
use platform::fs::{now_rfc3339, rfc3339_days_ago, CliFsAdapter};
use platform::ids::{is_ulid, new_plan_id, new_ulid};
use platform::journal::FileJournal;
use platform::listing::{inspect_tree, FsDirectoryLister};
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
  batchdesk c02 --file <path>... --to crlf|lf|cr --sample-bytes <bytes>
                [--structure auto|text|csv] [--source <encoding>]
                [--platform-prior chinese-windows|other]
                [--plan-id ULID] [--protected-dir <dir>]... [--json]
  batchdesk c03 --file <path>... --find <text> --replace <text>
                --sample-bytes <bytes> [--mode plain|regex]
                [--csv-mode cell|text] [--delimiter ,|;|tab||]
                [--source <encoding>] [--platform-prior chinese-windows|other]
                [--plan-id ULID] [--protected-dir <dir>]... [--json]
  batchdesk f01 --file <path>... --target-bytes N --output-dir <dir>
                [--plan-id ULID] [--protected-dir <dir>]... [--json]
  batchdesk f02 --file <path>... --lines N --output-dir <dir>
                [--plan-id ULID] [--protected-dir <dir>]... [--json]
  batchdesk f03 --file <path>... --parts N --output-dir <dir>
                [--plan-id ULID] [--protected-dir <dir>]... [--json]
  batchdesk f04 --file <path>... --output <file> --target <encoding>
                --sample-window <bytes> --sample-blocks <n>
                [--source <encoding>] [--invalid replace-and-count|abort]
                [--platform-prior chinese-windows|other]
                [--plan-id ULID] [--protected-dir <dir>]... [--json]
  batchdesk f05 --file <path>... (--target-bytes N | --rows N) --output-dir <dir>
                --sample-bytes <bytes> [--delimiter ,|;|tab||]
                [--no-keep-header] [--plan-id ULID] [--protected-dir <dir>]...
                [--json]
  batchdesk f06 --file <path>... --output <file> --target <encoding>
                --sample-bytes <bytes> [--sample-blocks N]
                [--delimiter ,|;|tab||] [--source <encoding>]
                [--no-header] [--source-column] [--source-column-name NAME]
                [--source-column-relative --base-dir <dir>] [--strict]
                [--invalid replace-and-count|abort]
                [--platform-prior chinese-windows|other]
                [--plan-id ULID] [--protected-dir <dir>]... [--json]
  batchdesk f07 --file <path>... --target <encoding>
                (--column <name> | --column-index <n>)...
                (--output <file> | --output-dir <dir>)
                --sample-bytes <bytes> [--sample-blocks N]
                [--delimiter ,|;|tab||] [--no-header] [--source <encoding>]
                [--invalid replace-and-count|abort]
                [--platform-prior chinese-windows|other]
                [--plan-id ULID] [--protected-dir <dir>]... [--json]
  batchdesk f08 --file <path>... --target <encoding>
                (--first N | --last N)
                (--output <file> | --output-dir <dir>)
                --sample-bytes <bytes> [--sample-blocks N]
                [--delimiter ,|;|tab||] [--no-header] [--source <encoding>]
                [--invalid replace-and-count|abort]
                [--platform-prior chinese-windows|other]
                [--plan-id ULID] [--protected-dir <dir>]... [--json]
  batchdesk f09 --file <path>... --target <encoding> --start X --end Y
                (--output <file> | --output-dir <dir>)
                --sample-bytes <bytes>
                [--delimiter ,|;|tab||] [--no-header] [--source <encoding>]
                [--invalid replace-and-count|abort]
                [--platform-prior chinese-windows|other]
                [--plan-id ULID] [--protected-dir <dir>]... [--json]
  batchdesk f10 --root <dir>... (--output <file> | --output-dir <dir>)
                [--plan-id ULID] [--protected-dir <dir>]... [--json]
  batchdesk f11 --list <csv> --root <dir>
                [--source <encoding>] [--plan-id ULID]
                [--protected-dir <dir>]... [--json]
  batchdesk f12 --dir <dir> (--prefix <text> | --suffix <text> | --find <text>)
                [--replace <text>] [--scope full|stem|ext]
                [--plan-id ULID] [--protected-dir <dir>]... [--json]
  batchdesk f13 --dir <dir> --regex <pattern> --replace <template>
                [--scope full|stem|ext]
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

c03 options:
  --file PATH             repeatable source text/CSV file (regular files only)
  --find TEXT             literal text or regular expression to find (required)
  --replace TEXT          replacement text; regex mode supports $1/${name}, $$ for literal $
  --sample-bytes BYTES    head window for encoding detection (explicit; §5.2)
  --mode plain|regex      default plain; regex uses the Rust regex engine
  --csv-mode cell|text    default cell; cell replaces inside parsed fields and preserves quotes
  --delimiter D           , | ; | tab | pipe; omitted = sniff for CSV cell mode
  --source ENC            optional source encoding override
  --platform-prior PRIOR  chinese-windows | other (default: other)
  --plan-id ULID          optional caller-supplied ULID; generated when omitted
  --protected-dir DIR     repeatable protected directory
  --json                  machine-readable plan output

c02 options:
  --file PATH             repeatable source text/CSV file (regular files only)
  --to crlf|lf|cr         target line ending (required; no default)
  --structure MODE        auto | text | csv (default auto; .csv/.tsv = csv)
  --source ENC            optional source encoding override
  --sample-bytes BYTES    head window for encoding detection (explicit; §5.2)
  --platform-prior PRIOR  chinese-windows | other (default: other)
  --plan-id ULID          optional caller-supplied ULID; generated when omitted
  --protected-dir DIR     repeatable protected directory
  --json                  machine-readable plan output

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

f04 options:
  --file PATH             repeatable source text file (merged in the given order)
  --output PATH           merged output file (must not be occupied unless confirmed)
  --target ENC            target encoding: utf-8 | utf-8-bom | gb18030 | utf-16le[-bom] | utf-16be[-bom]
  --sample-window BYTES   sampling window per block; explicit because §5.2 sets no number
  --sample-blocks N       number of evenly spaced sample blocks after the head (0 = head only)
  --source ENC            optional global source override; otherwise each file is detected
  --invalid POLICY        invalid-sequence policy (default: replace-and-count, §5.2)
  --platform-prior PRIOR  chinese-windows | other (default: other; informational only)
  --plan-id ULID          optional caller-supplied ULID; generated when omitted
  --json                  machine-readable plan output

f05 options:
  --file PATH             repeatable source CSV file
  --target-bytes N        target part body size in bytes (record-aligned; §5.3)
  --rows N                records per part (header excluded; §5.3)
  --delimiter D           , | ; | tab | pipe; omitted = sniff from the sample window
  --sample-bytes BYTES    head window for delimiter sniffing/encoding (explicit; §5.2)
  --no-keep-header        do not repeat the header in every part (default: repeated)
  --output-dir DIR        directory for generated parts
  --plan-id ULID          optional caller-supplied ULID; generated when omitted
  --json                  machine-readable plan output

f06 options:
  --file PATH             repeatable source CSV file (merged in the given order)
  --output PATH           merged output file (must not be occupied unless confirmed)
  --target ENC            target encoding: utf-8 | utf-8-bom | gb18030 | utf-16le[-bom] | utf-16be[-bom]
  --sample-bytes BYTES    head window for delimiter sniffing/encoding (explicit; §5.2)
  --sample-blocks N       evenly spaced sample blocks after the head (default: 0)
  --delimiter D           , | ; | tab | pipe; omitted = sniff from the sample window
  --source ENC            optional global source override; otherwise each file is detected
  --no-header             all inputs have no header row (align by column index)
  --source-column         add a leading source-file column (default: off)
  --source-column-name N  source column header (default: 来源文件)
  --source-column-relative
                          write paths relative to --base-dir instead of file names
  --base-dir DIR          base directory for --source-column-relative
  --strict                block when column sets differ (default: fill empty/append)
  --invalid POLICY        invalid-sequence policy (default: replace-and-count, §5.2)
  --platform-prior PRIOR  chinese-windows | other (default: other; informational only)
  --plan-id ULID          optional caller-supplied ULID; generated when omitted
  --json                  machine-readable plan output

f07 options:
  --file PATH             repeatable source CSV file
  --column NAME           repeatable column selected by normalized header name
  --column-index N        repeatable 1-based column selected by position
  --output PATH           single output file (exactly one input)
  --output-dir DIR        one output per input, keeping the source file name
  --target ENC            target encoding: utf-8 | utf-8-bom | gb18030 | utf-16le[-bom] | utf-16be[-bom]
  --sample-bytes BYTES    head window for delimiter sniffing/encoding (explicit; §5.2)
  --sample-blocks N       evenly spaced sample blocks after the head (default: 0)
  --delimiter D           , | ; | tab | pipe; omitted = sniff from the sample window
  --no-header             all inputs have no header row; select by index only
  --source ENC            optional global source override; otherwise each file is detected
  --invalid POLICY        invalid-sequence policy (default: replace-and-count, §5.2)
  --platform-prior PRIOR  chinese-windows | other (default: other; informational only)
  --plan-id ULID          optional caller-supplied ULID; generated when omitted
  --json                  machine-readable plan output

f08 options:
  --file PATH             repeatable source CSV file
  --first N               take the first N data rows (N may be 0)
  --last N                take the last N data rows (N may be 0)
  --output PATH           single output file (exactly one input)
  --output-dir DIR        one output per input, keeping the source file name
  --target ENC            target encoding: utf-8 | utf-8-bom | gb18030 | utf-16le[-bom] | utf-16be[-bom]
  --sample-bytes BYTES    head window for delimiter sniffing/encoding (explicit; §5.2)
  --sample-blocks N       evenly spaced sample blocks after the head (default: 0)
  --delimiter D           , | ; | tab | pipe; omitted = sniff from the sample window
  --no-header             all inputs have no header row
  --source ENC            optional global source override; otherwise each file is detected
  --invalid POLICY        invalid-sequence policy (default: replace-and-count, §5.2)
  --platform-prior PRIOR  chinese-windows | other (default: other; informational only)
  --plan-id ULID          optional caller-supplied ULID; generated when omitted
  --json                  machine-readable plan output

f10 options:
  --root DIR              repeatable directory to list (non-recursive content reads)
  --output PATH           single listing file (exactly one --root)
  --output-dir DIR        one listing per root, named <root>.csv
  --plan-id ULID          optional caller-supplied ULID; generated when omitted
  --protected-dir DIR     repeatable protected directory
  --json                  machine-readable plan output
  Listing output is UTF-8 with BOM; columns: 相对路径,名称,类型,大小,修改时间,层级

f11 options:
  --list PATH             CSV list: F10 format (相对路径/类型) or one path per row
  --root DIR              existing directory root for created folders
  --source ENC            optional list encoding override; otherwise detected
  --plan-id ULID          optional caller-supplied ULID; generated when omitted
  --protected-dir DIR     repeatable protected directory
  --json                  machine-readable plan output

f12 options:
  --dir DIR               directory whose immediate entries are renamed
  --prefix TEXT           prepend text to the selected name scope
  --suffix TEXT           append text to the selected name scope
  --find TEXT             literal text to replace (requires --replace or replaces with empty)
  --replace TEXT          replacement text for --find
  --scope full|stem|ext   default: full; stem preserves the extension
  --plan-id ULID          optional caller-supplied ULID; generated when omitted
  --protected-dir DIR     repeatable protected directory
  --json                  machine-readable plan output

f13 options:
  --dir DIR               directory whose immediate entries are renamed
  --regex PATTERN         regular expression (Rust regex syntax; no backtracking)
  --replace TEMPLATE      replacement; $1/${name} reference capture groups, $$ is literal $
  --scope full|stem|ext   default: full; stem preserves the extension
  --plan-id ULID          optional caller-supplied ULID; generated when omitted
  --protected-dir DIR     repeatable protected directory
  --json                  machine-readable plan output

f09 options:
  --file PATH             repeatable source CSV file
  --start X               first data row (1-based, inclusive; excludes header)
  --end Y                 last data row (1-based, inclusive; Y >= X)
  --output PATH           single output file (exactly one input)
  --output-dir DIR        one output per input, keeping the source file name
  --target ENC            target encoding: utf-8 | utf-8-bom | gb18030 | utf-16le[-bom] | utf-16be[-bom]
  --sample-bytes BYTES    head window for delimiter sniffing/encoding (explicit; §5.2)
  --delimiter D           , | ; | tab | pipe; omitted = sniff from the sample window
  --no-header             all inputs have no header row; rows count from the first record
  --source ENC            optional global source override; otherwise each file is detected
  --invalid POLICY        invalid-sequence policy (default: replace-and-count, §5.2)
  --platform-prior PRIOR  chinese-windows | other (default: other; informational only)
  --plan-id ULID          optional caller-supplied ULID; generated when omitted
  --json                  machine-readable plan output

apply/undo/cleanup:
  apply consumes a previously generated plan: c09 entries move to quarantine; c01
  entries transcode in place; c02/c03 entries rewrite content in place (all keep a
  quarantine backup and undo restores from the Journal);
  f01/f02/f03 entries write line-aligned parts (undo moves the parts into quarantine);
  f04 entries merge sources in order into one output with a single BOM;
  f05 entries write CSV parts on record boundaries, repeating the header by default;
  f06 entries align CSV columns by header name (or column index without headers),
  fill missing values and append extra columns, optionally adding a source column;
  f07 entries export selected columns in the given order, preserving row order;
  f08 entries take the first or last N data rows, keeping the header;
  f09 entries take data rows X-Y (1-based, header excluded), reading only the
  header and selected byte range at apply time;
  f10 entries stream a directory tree into a UTF-8 BOM CSV listing
  (相对路径/名称/类型/大小/修改时间/层级); placeholders and symlinks are skipped;
  f11 entries create relative folders under --root from a CSV list; parents are
  created before children and undo removes created empty directories in reverse;
  f12 entries rename immediate children of --dir using prefix/suffix/literal replace;
  f13 entries rewrite names with a regular expression and capture-group template;
  cycles and case-only renames use a temporary-name two-phase plan;
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
    if args[1] == "f04" {
        std::process::exit(run_f04(&args));
    }
    if args[1] == "f05" {
        std::process::exit(run_f05(&args));
    }
    if args[1] == "f06" {
        std::process::exit(run_f06(&args));
    }
    if args[1] == "f07" {
        std::process::exit(run_f07(&args));
    }
    if args[1] == "f08" {
        std::process::exit(run_f08(&args));
    }
    if args[1] == "f09" {
        std::process::exit(run_f09(&args));
    }
    if args[1] == "f10" {
        std::process::exit(run_f10(&args));
    }
    if args[1] == "f11" {
        std::process::exit(run_f11(&args));
    }
    if args[1] == "f12" {
        std::process::exit(run_f12(&args));
    }
    if args[1] == "f13" {
        std::process::exit(run_f13(&args));
    }
    if args[1] == "c02" {
        std::process::exit(run_c02(&args));
    }
    if args[1] == "c03" {
        std::process::exit(run_c03(&args));
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
        // 通用执行器：c01/c02/c03/f 系列 rename/move/copy/delete/edit-content 计划。
        let is_c02 = plan.kind == kernel::line_endings::C02_KIND;
        let is_c03 = plan.kind == kernel::replace_content::C03_KIND;
        let streaming = is_c02 || is_c03;
        let has_edit_content = plan
            .entries
            .iter()
            .any(|entry| entry.action == kernel::plan::PlanAction::EditContent);
        let encoding = match has_edit_content && !streaming {
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
        let content_editor =
            (has_edit_content && !streaming).then_some(&editor as ContentEditor<'_>);
        let rewriter: Option<Box<dyn ContentRewriter>> = if is_c02 {
            match c02_rewriter_from_plan(&plan) {
                Ok(rewriter) => Some(Box::new(rewriter)),
                Err(error) => return print_kernel_error(&error),
            }
        } else if is_c03 {
            match c03_rewriter_from_plan(&plan) {
                Ok(rewriter) => Some(Box::new(rewriter)),
                Err(error) => return print_kernel_error(&error),
            }
        } else {
            None
        };
        let content_rewriter = rewriter.as_deref();
        let lister = FsDirectoryLister;
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
            content_rewriter,
            directory_lister: Some(&lister),
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

fn c02_rewriter_from_plan(
    plan: &PlanDocument,
) -> Result<LineEndingRewriter, kernel::error::KernelError> {
    let string_param = |name: &str| -> Result<&str, kernel::error::KernelError> {
        match plan.params.get(name) {
            Some(kernel::plan::PlanParamValue::String(value)) => Ok(value.as_str()),
            _ => Err(kernel::error::KernelError::missing_param(format!(
                "plan param {name} is required"
            ))),
        }
    };
    let target = LineEnding::parse(string_param("target")?).ok_or_else(|| {
        kernel::error::KernelError::invalid_params(
            "plan param target is not a supported line ending",
        )
    })?;
    let structure =
        LineEndingStructureMode::parse(string_param("structure")?).ok_or_else(|| {
            kernel::error::KernelError::invalid_params(
                "plan param structure is not a supported structure mode",
            )
        })?;
    Ok(LineEndingRewriter::new(target, structure))
}

fn c03_rewriter_from_plan(
    plan: &PlanDocument,
) -> Result<ReplaceRewriter, kernel::error::KernelError> {
    let string_param = |name: &str| -> Result<&str, kernel::error::KernelError> {
        match plan.params.get(name) {
            Some(kernel::plan::PlanParamValue::String(value)) => Ok(value.as_str()),
            _ => Err(kernel::error::KernelError::missing_param(format!(
                "plan param {name} is required"
            ))),
        }
    };
    let mode = ReplaceMode::parse(string_param("mode")?).ok_or_else(|| {
        kernel::error::KernelError::invalid_params("plan param mode is not a supported mode")
    })?;
    let csv_mode = CsvMode::parse(string_param("csvMode")?).ok_or_else(|| {
        kernel::error::KernelError::invalid_params("plan param csvMode is not a supported mode")
    })?;
    let delimiter = match string_param("delimiter")? {
        "auto" => None,
        value => Some(Delimiter::parse(value).ok_or_else(|| {
            kernel::error::KernelError::invalid_params(
                "plan param delimiter is not a supported delimiter",
            )
        })?),
    };
    Ok(ReplaceRewriter::new(
        mode,
        string_param("find")?.to_string(),
        string_param("replace")?.to_string(),
        csv_mode,
        delimiter,
    ))
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

fn run_c02(args: &[String]) -> i32 {
    let mut files = Vec::new();
    let mut target = None;
    let mut structure = LineEndingStructureMode::Auto;
    let mut source = None;
    let mut prior = PlatformPrior::Other;
    let mut sample_bytes = None;
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
            "--to" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --to requires crlf|lf|cr");
                };
                let Some(parsed) = LineEnding::parse(value) else {
                    return input_error(format!("E1002: unknown line ending: {value}"));
                };
                target = Some(parsed);
            }
            "--structure" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --structure requires auto|text|csv");
                };
                let Some(parsed) = LineEndingStructureMode::parse(value) else {
                    return input_error(format!("E1002: unknown structure mode: {value}"));
                };
                structure = parsed;
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
            "--sample-bytes" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --sample-bytes requires bytes");
                };
                match value.parse::<usize>() {
                    Ok(bytes) if bytes > 0 => sample_bytes = Some(bytes),
                    _ => {
                        return input_error(format!(
                            "E1002: --sample-bytes must be a positive byte count: {value}"
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
            other => return input_error(format!("E1002: unknown c02 argument: {other}")),
        }
        index += 1;
    }

    if files.is_empty() {
        return input_error("E1001: c02 requires at least one --file");
    }
    let Some(target) = target else {
        return input_error("E1001: c02 requires --to crlf|lf|cr");
    };
    let Some(sample_bytes) = sample_bytes else {
        return input_error("E1001: c02 requires --sample-bytes <bytes>");
    };
    let protected_dirs = match protected_dirs_from_paths(&protected_dirs) {
        Ok(protected_dirs) => protected_dirs,
        Err(error) => {
            eprintln!("{}: {}", error.code, error.message);
            return 2;
        }
    };

    let mut plan_files = Vec::with_capacity(files.len());
    for file in &files {
        let absolute = make_absolute(file);
        let resolved = structure.resolve(&absolute.to_string_lossy());
        let inspect =
            match platform::line_endings::inspect_line_endings(&absolute, resolved, sample_bytes) {
                Ok(inspect) => inspect,
                Err(error) => {
                    eprintln!("{}: {}", error.code, error.message);
                    return 2;
                }
            };
        let canonical_path = if protected_dirs.is_empty() {
            None
        } else {
            platform::fs::canonicalize_display(&absolute)
        };
        plan_files.push(PlanC02File {
            path: absolute.to_string_lossy().into_owned(),
            canonical_path,
            file_size: inspect.file_size,
            sample: inspect.sample,
            counts: inspect.counts,
            structure: resolved,
        });
    }

    let plan_id = plan_id.unwrap_or_else(|| new_ulid("c02"));
    let planned = plan_c02(PlanC02Input {
        plan_id,
        target,
        structure,
        files: plan_files,
        protected_dirs,
        case_sense: CliFsAdapter::new(None).case_sensitivity(),
        confirmed_source: source,
        prior,
    });
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

fn run_c03(args: &[String]) -> i32 {
    let mut files = Vec::new();
    let mut find = None;
    let mut replacement = None;
    let mut mode = ReplaceMode::Plain;
    let mut csv_mode = CsvMode::Cell;
    let mut delimiter = None;
    let mut source = None;
    let mut prior = PlatformPrior::Other;
    let mut sample_bytes = None;
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
            "--find" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --find requires text");
                };
                find = Some(value.clone());
            }
            "--replace" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --replace requires text");
                };
                replacement = Some(value.clone());
            }
            "--mode" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --mode requires plain|regex");
                };
                let Some(parsed) = ReplaceMode::parse(value) else {
                    return input_error(format!("E1002: unknown replace mode: {value}"));
                };
                mode = parsed;
            }
            "--csv-mode" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --csv-mode requires cell|text");
                };
                let Some(parsed) = CsvMode::parse(value) else {
                    return input_error(format!("E1002: unknown CSV mode: {value}"));
                };
                csv_mode = parsed;
            }
            "--delimiter" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --delimiter requires ,|;|tab|pipe");
                };
                let Some(parsed) = Delimiter::parse(value) else {
                    return input_error(format!("E1002: unknown delimiter: {value}"));
                };
                delimiter = Some(parsed);
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
            "--sample-bytes" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --sample-bytes requires bytes");
                };
                match value.parse::<usize>() {
                    Ok(bytes) if bytes > 0 => sample_bytes = Some(bytes),
                    _ => {
                        return input_error(format!(
                            "E1002: --sample-bytes must be a positive byte count: {value}"
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
            other => return input_error(format!("E1002: unknown c03 argument: {other}")),
        }
        index += 1;
    }

    if files.is_empty() {
        return input_error("E1001: c03 requires at least one --file");
    }
    let Some(find) = find else {
        return input_error("E1001: c03 requires --find <text>");
    };
    let Some(replacement) = replacement else {
        return input_error("E1001: c03 requires --replace <text>");
    };
    let Some(sample_bytes) = sample_bytes else {
        return input_error("E1001: c03 requires --sample-bytes <bytes>");
    };
    let protected_dirs = match protected_dirs_from_paths(&protected_dirs) {
        Ok(protected_dirs) => protected_dirs,
        Err(error) => {
            eprintln!("{}: {}", error.code, error.message);
            return 2;
        }
    };

    let mut plan_files = Vec::with_capacity(files.len());
    for file in &files {
        let absolute = make_absolute(file);
        let inspect = match platform::replace_content::inspect_file(
            &absolute,
            mode,
            &find,
            &replacement,
            csv_mode,
            delimiter,
            sample_bytes,
        ) {
            Ok(inspect) => inspect,
            Err(error) => return print_kernel_error(&error),
        };
        let canonical_path = if protected_dirs.is_empty() {
            None
        } else {
            platform::fs::canonicalize_display(&absolute)
        };
        plan_files.push(PlanC03File {
            path: absolute.to_string_lossy().into_owned(),
            canonical_path,
            file_size: inspect.file_size,
            sample: inspect.sample,
            structure: inspect.structure,
            inspect: inspect.inspect,
        });
    }

    let plan_id = plan_id.unwrap_or_else(|| new_ulid("c03"));
    let planned = plan_c03(PlanC03Input {
        plan_id,
        mode,
        find,
        replacement,
        csv_mode,
        files: plan_files,
        protected_dirs,
        case_sense: CliFsAdapter::new(None).case_sensitivity(),
        confirmed_source: source,
        prior,
        delimiter,
    });
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

fn run_f04(args: &[String]) -> i32 {
    let mut files = Vec::new();
    let mut output = None;
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
            "--output" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --output requires a file path");
                };
                output = Some(PathBuf::from(value));
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
            other => return input_error(format!("E1002: unknown f04 argument: {other}")),
        }
        index += 1;
    }

    if files.is_empty() {
        return input_error("E1001: f04 requires at least one --file <path>");
    }
    let Some(output) = output else {
        return input_error("E1001: f04 requires --output <file>");
    };
    let Some(target) = target else {
        return input_error("E1001: f04 requires --target <encoding>");
    };
    let spec = match (source, sample_window, sample_blocks) {
        (Some(_), _, _) => None,
        (None, Some(window), Some(blocks)) => Some(SampleSpec {
            window_bytes: window,
            blocks,
        }),
        (None, _, _) => {
            return input_error(
                "E1001: f04 requires --sample-window and --sample-blocks without --source \
                 (方案 §5.2 sets no numeric window)",
            )
        }
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
            eprintln!("E1002: f04 requires regular files: {}", absolute.display());
            return 2;
        }
        let canonical_path = if protected_dirs.is_empty() {
            None
        } else {
            platform::fs::canonicalize_display(&absolute)
        };
        let source_encoding = match source {
            Some(encoding) => Some(encoding),
            None => match sample_bytes(&absolute, spec.expect("checked above")) {
                Ok(sampled) => detect(&sampled.bytes, prior).require_encoding().ok(),
                Err(error) => {
                    eprintln!(
                        "E1004: cannot sample file ({}): {error}",
                        absolute.display()
                    );
                    return 2;
                }
            },
        };
        plan_files.push(PlanF04File {
            path: absolute.to_string_lossy().into_owned(),
            canonical_path,
            file_size: metadata.len(),
            source_encoding,
        });
    }

    let output = make_absolute(&output);
    let output = output.to_string_lossy().into_owned();
    let target_occupied = std::path::Path::new(&output).exists();
    let plan_id = plan_id.unwrap_or_else(|| new_ulid("f04"));
    let input = PlanF04Input {
        plan_id,
        target: output,
        target_encoding: target,
        invalid,
        protected_dirs,
        case_sense,
        target_occupied,
        files: plan_files,
    };
    match plan_f04(input) {
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

fn run_f05(args: &[String]) -> i32 {
    let mut files = Vec::new();
    let mut target_bytes = None;
    let mut rows = None;
    let mut output_dir = None;
    let mut delimiter = None;
    let mut keep_header = true;
    let mut sample_window = None;
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
            "--target-bytes" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --target-bytes requires a byte count");
                };
                match value.parse::<u64>() {
                    Ok(bytes) if bytes > 0 => target_bytes = Some(bytes),
                    _ => {
                        return input_error(format!(
                            "E1002: --target-bytes must be a positive byte count: {value}"
                        ))
                    }
                }
            }
            "--rows" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --rows requires a record count");
                };
                match value.parse::<u64>() {
                    Ok(count) if count > 0 => rows = Some(count),
                    _ => {
                        return input_error(format!(
                            "E1002: --rows must be a positive record count: {value}"
                        ))
                    }
                }
            }
            "--delimiter" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --delimiter requires , ; tab | or pipe");
                };
                match Delimiter::parse(value) {
                    Some(parsed) => delimiter = Some(parsed),
                    None => {
                        return input_error(format!(
                            "E1002: --delimiter must be one of , ; tab | pipe: {value}"
                        ))
                    }
                }
            }
            "--no-keep-header" => keep_header = false,
            "--sample-bytes" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --sample-bytes requires a window size");
                };
                match value.parse::<u64>() {
                    Ok(bytes) if bytes > 0 => sample_window = Some(bytes),
                    _ => {
                        return input_error(format!(
                            "E1002: --sample-bytes must be a positive byte count: {value}"
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
            other => return input_error(format!("E1002: unknown f05 argument: {other}")),
        }
        index += 1;
    }

    if files.is_empty() {
        return input_error("E1001: f05 requires at least one --file <path>");
    }
    let basis = match (target_bytes, rows) {
        (Some(bytes), None) => CsvSplitBasis::Bytes(bytes),
        (None, Some(count)) => CsvSplitBasis::Rows(count),
        _ => {
            return input_error(
                "E1001: f05 requires exactly one of --target-bytes <n> or --rows <n>",
            )
        }
    };
    let Some(sample_window) = sample_window else {
        return input_error(
            "E1001: f05 requires --sample-bytes <n> (方案 §5.2 sets no numeric window)",
        );
    };
    let Some(output_dir) = output_dir else {
        return input_error("E1001: f05 requires --output-dir <dir>");
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
            eprintln!("E1002: f05 requires regular files: {}", absolute.display());
            return 2;
        }
        let layout = match csv_split_layout(
            &absolute,
            CsvScanOptions {
                delimiter,
                keep_header,
                basis,
                sample_window_bytes: sample_window,
            },
        ) {
            Ok(layout) => layout,
            Err(error) => {
                eprintln!(
                    "E1004: cannot scan CSV for split layout ({}): {error}",
                    absolute.display()
                );
                return 2;
            }
        };
        let file_name = absolute
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("file");
        let total = layout.shards.len() as u32;
        for shard in &layout.shards {
            let target = output_dir.join(kernel::split::shard_name(file_name, shard.index, total));
            if target.exists() {
                existing_targets.push(target.to_string_lossy().into_owned());
            }
        }
        let canonical_path = if protected_dirs.is_empty() {
            None
        } else {
            platform::fs::canonicalize_display(&absolute)
        };
        plan_files.push(PlanF05File {
            path: absolute.to_string_lossy().into_owned(),
            canonical_path,
            group_id: new_ulid("f05"),
            layout,
        });
    }

    let plan_id = plan_id.unwrap_or_else(|| new_ulid("f05"));
    let planned = plan_f05(PlanF05Input {
        plan_id,
        basis,
        keep_header,
        delimiter,
        output_dir: output_dir.to_string_lossy().into_owned(),
        protected_dirs,
        case_sense,
        existing_targets,
        files: plan_files,
    });
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

fn run_f06(args: &[String]) -> i32 {
    let mut files = Vec::new();
    let mut output = None;
    let mut target = None;
    let mut source = None;
    let mut delimiter = None;
    let mut invalid = InvalidSequencePolicy::ReplaceAndCount;
    let mut prior = PlatformPrior::Other;
    let mut sample_window = None;
    let mut sample_blocks = 0usize;
    let mut has_header = true;
    let mut add_source_column = false;
    let mut source_column_name = "来源文件".to_string();
    let mut source_column_relative = false;
    let mut base_dir = None;
    let mut strict = false;
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
            "--output" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --output requires a path");
                };
                output = Some(PathBuf::from(value));
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
            "--delimiter" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --delimiter requires a value");
                };
                match Delimiter::parse(value) {
                    Some(parsed) => delimiter = Some(parsed),
                    None => {
                        return input_error(format!(
                            "E1002: --delimiter must be one of , ; tab | pipe: {value}"
                        ))
                    }
                }
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
            "--sample-bytes" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --sample-bytes requires a window size");
                };
                match value.parse::<u64>() {
                    Ok(bytes) if bytes > 0 => sample_window = Some(bytes),
                    _ => {
                        return input_error(format!(
                            "E1002: --sample-bytes must be a positive byte count: {value}"
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
                    Ok(blocks) => sample_blocks = blocks,
                    Err(_) => {
                        return input_error(format!(
                            "E1002: --sample-blocks must be a non-negative integer: {value}"
                        ))
                    }
                }
            }
            "--no-header" => has_header = false,
            "--source-column" => add_source_column = true,
            "--source-column-name" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --source-column-name requires a value");
                };
                source_column_name = value.clone();
            }
            "--source-column-relative" => {
                add_source_column = true;
                source_column_relative = true;
            }
            "--base-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --base-dir requires a path");
                };
                base_dir = Some(PathBuf::from(value));
            }
            "--strict" => strict = true,
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
            other => return input_error(format!("E1002: unknown f06 argument: {other}")),
        }
        index += 1;
    }

    if files.is_empty() {
        return input_error("E1001: f06 requires at least one --file <path>");
    }
    let Some(output) = output else {
        return input_error("E1001: f06 requires --output <file>");
    };
    let Some(target) = target else {
        return input_error("E1001: f06 requires --target <encoding>");
    };
    let Some(sample_window) = sample_window else {
        return input_error(
            "E1001: f06 requires --sample-bytes <n> (方案 §5.2 sets no numeric window)",
        );
    };
    if source_column_name.trim().is_empty() {
        return input_error("E1001: --source-column-name must not be empty");
    }
    let base = if source_column_relative {
        let Some(base_dir) = base_dir else {
            return input_error("E1001: --source-column-relative requires --base-dir <dir>");
        };
        Some(make_absolute(&base_dir))
    } else {
        None
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
            eprintln!("E1002: f06 requires regular files: {}", absolute.display());
            return 2;
        }
        let inspection = match csv_merge_inspect(
            &absolute,
            CsvMergeScanOptions {
                delimiter,
                has_header,
                sample_window_bytes: sample_window,
                sample_blocks,
                platform_prior: prior,
                source_encoding: source,
            },
        ) {
            Ok(inspection) => inspection,
            Err(error) => {
                eprintln!(
                    "E1004: cannot inspect CSV for merge ({}): {error}",
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
        let source_label = if add_source_column {
            if source_column_relative {
                let base = base.as_ref().expect("checked above");
                match absolute.strip_prefix(base) {
                    Ok(relative) if !relative.as_os_str().is_empty() => {
                        Some(relative.to_string_lossy().into_owned())
                    }
                    _ => {
                        eprintln!(
                            "E1002: source is outside --base-dir: {}",
                            absolute.display()
                        );
                        return 2;
                    }
                }
            } else {
                Some(
                    absolute
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("file")
                        .to_string(),
                )
            }
        } else {
            None
        };
        plan_files.push(PlanF06File {
            path: absolute.to_string_lossy().into_owned(),
            canonical_path,
            file_size: inspection.file_size,
            source_encoding: source.or(inspection.source_encoding),
            delimiter: inspection.delimiter,
            has_header: inspection.has_header,
            columns: inspection.columns,
            column_count: inspection.column_count,
            data_rows: inspection.data_rows,
            dirty_rows: inspection.dirty_rows,
            source_label,
        });
    }

    let output = make_absolute(&output);
    let output = output.to_string_lossy().into_owned();
    let target_occupied = std::path::Path::new(&output).exists();
    let plan_id = plan_id.unwrap_or_else(|| new_ulid("f06"));
    let input = PlanF06Input {
        plan_id,
        target: output,
        target_encoding: target,
        invalid,
        output_delimiter: delimiter,
        add_source_column,
        source_column_name,
        strict,
        protected_dirs,
        case_sense,
        target_occupied,
        files: plan_files,
    };
    match plan_f06(input) {
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

fn run_f07(args: &[String]) -> i32 {
    let mut files = Vec::new();
    let mut output = None;
    let mut output_dir = None;
    let mut selections = Vec::new();
    let mut target = None;
    let mut source = None;
    let mut delimiter = None;
    let mut invalid = InvalidSequencePolicy::ReplaceAndCount;
    let mut prior = PlatformPrior::Other;
    let mut sample_window = None;
    let mut sample_blocks = 0usize;
    let mut has_header = true;
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
            "--column" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --column requires a header name");
                };
                selections.push(ColumnPick::Name(value.clone()));
            }
            "--column-index" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --column-index requires a 1-based index");
                };
                match value.parse::<u64>() {
                    Ok(parsed) if parsed > 0 => selections.push(ColumnPick::Index(parsed)),
                    _ => {
                        return input_error(format!(
                            "E1002: --column-index must be a positive integer: {value}"
                        ))
                    }
                }
            }
            "--output" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --output requires a path");
                };
                output = Some(PathBuf::from(value));
            }
            "--output-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --output-dir requires a path");
                };
                output_dir = Some(PathBuf::from(value));
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
            "--delimiter" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --delimiter requires a value");
                };
                match Delimiter::parse(value) {
                    Some(parsed) => delimiter = Some(parsed),
                    None => {
                        return input_error(format!(
                            "E1002: --delimiter must be one of , ; tab | pipe: {value}"
                        ))
                    }
                }
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
            "--sample-bytes" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --sample-bytes requires a window size");
                };
                match value.parse::<u64>() {
                    Ok(bytes) if bytes > 0 => sample_window = Some(bytes),
                    _ => {
                        return input_error(format!(
                            "E1002: --sample-bytes must be a positive byte count: {value}"
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
                    Ok(blocks) => sample_blocks = blocks,
                    Err(_) => {
                        return input_error(format!(
                            "E1002: --sample-blocks must be a non-negative integer: {value}"
                        ))
                    }
                }
            }
            "--no-header" => has_header = false,
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
            other => return input_error(format!("E1002: unknown f07 argument: {other}")),
        }
        index += 1;
    }

    if files.is_empty() {
        return input_error("E1001: f07 requires at least one --file <path>");
    }
    if selections.is_empty() {
        return input_error("E1001: f07 requires at least one --column or --column-index");
    }
    let Some(target) = target else {
        return input_error("E1001: f07 requires --target <encoding>");
    };
    let Some(sample_window) = sample_window else {
        return input_error(
            "E1001: f07 requires --sample-bytes <n> (方案 §5.2 sets no numeric window)",
        );
    };
    if output.is_some() && output_dir.is_some() {
        return input_error("E1001: use either --output or --output-dir, not both");
    }
    let targets: Vec<PathBuf> = match (output, output_dir) {
        (Some(output), None) => {
            if files.len() != 1 {
                return input_error("E1002: --output requires exactly one --file");
            }
            vec![make_absolute(&output)]
        }
        (None, Some(dir)) => {
            let dir = make_absolute(&dir);
            files
                .iter()
                .map(|path| {
                    let name = path
                        .file_name()
                        .map(|value| value.to_os_string())
                        .unwrap_or_else(|| "output.csv".into());
                    dir.join(name)
                })
                .collect()
        }
        _ => return input_error("E1001: f07 requires --output <file> or --output-dir <dir>"),
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
    let mut plan_files = Vec::with_capacity(files.len());
    for (path, target) in files.iter().zip(targets.iter()) {
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
            eprintln!("E1002: f07 requires regular files: {}", absolute.display());
            return 2;
        }
        let inspection = match csv_merge_inspect(
            &absolute,
            CsvMergeScanOptions {
                delimiter,
                has_header,
                sample_window_bytes: sample_window,
                sample_blocks,
                platform_prior: prior,
                source_encoding: source,
            },
        ) {
            Ok(inspection) => inspection,
            Err(error) => {
                eprintln!(
                    "E1004: cannot inspect CSV for extraction ({}): {error}",
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
        plan_files.push(PlanF07File {
            path: absolute.to_string_lossy().into_owned(),
            canonical_path,
            file_size: inspection.file_size,
            source_encoding: inspection.source_encoding,
            delimiter: inspection.delimiter,
            has_header,
            columns: inspection.columns,
            column_count: inspection.column_count,
            data_rows: inspection.data_rows,
            dirty_rows: inspection.dirty_rows,
            target: target.to_string_lossy().into_owned(),
            target_occupied: target.exists(),
        });
    }

    let plan_id = plan_id.unwrap_or_else(|| new_ulid("f07"));
    let planned = plan_f07(PlanF07Input {
        plan_id,
        target_encoding: target,
        invalid,
        protected_dirs,
        case_sense,
        selections,
        files: plan_files,
    });
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

fn run_f08(args: &[String]) -> i32 {
    let mut files = Vec::new();
    let mut first = None;
    let mut last = None;
    let mut output = None;
    let mut output_dir = None;
    let mut target = None;
    let mut source = None;
    let mut delimiter = None;
    let mut invalid = InvalidSequencePolicy::ReplaceAndCount;
    let mut prior = PlatformPrior::Other;
    let mut sample_window = None;
    let mut sample_blocks = 0usize;
    let mut has_header = true;
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
            "--first" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --first requires a row count");
                };
                match value.parse::<u64>() {
                    Ok(count) => first = Some(count),
                    Err(_) => {
                        return input_error(format!(
                            "E1002: --first must be a non-negative integer: {value}"
                        ))
                    }
                }
            }
            "--last" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --last requires a row count");
                };
                match value.parse::<u64>() {
                    Ok(count) => last = Some(count),
                    Err(_) => {
                        return input_error(format!(
                            "E1002: --last must be a non-negative integer: {value}"
                        ))
                    }
                }
            }
            "--output" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --output requires a path");
                };
                output = Some(PathBuf::from(value));
            }
            "--output-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --output-dir requires a path");
                };
                output_dir = Some(PathBuf::from(value));
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
            "--delimiter" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --delimiter requires a value");
                };
                match Delimiter::parse(value) {
                    Some(parsed) => delimiter = Some(parsed),
                    None => {
                        return input_error(format!(
                            "E1002: --delimiter must be one of , ; tab | pipe: {value}"
                        ))
                    }
                }
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
            "--sample-bytes" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --sample-bytes requires a window size");
                };
                match value.parse::<u64>() {
                    Ok(bytes) if bytes > 0 => sample_window = Some(bytes),
                    _ => {
                        return input_error(format!(
                            "E1002: --sample-bytes must be a positive byte count: {value}"
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
                    Ok(blocks) => sample_blocks = blocks,
                    Err(_) => {
                        return input_error(format!(
                            "E1002: --sample-blocks must be a non-negative integer: {value}"
                        ))
                    }
                }
            }
            "--no-header" => has_header = false,
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
            other => return input_error(format!("E1002: unknown f08 argument: {other}")),
        }
        index += 1;
    }

    if files.is_empty() {
        return input_error("E1001: f08 requires at least one --file <path>");
    }
    let (window, count) = match (first, last) {
        (Some(count), None) => (RowWindow::First, count),
        (None, Some(count)) => (RowWindow::Last, count),
        _ => {
            return input_error("E1001: f08 requires exactly one of --first <n> or --last <n>");
        }
    };
    let Some(target) = target else {
        return input_error("E1001: f08 requires --target <encoding>");
    };
    let Some(sample_window) = sample_window else {
        return input_error(
            "E1001: f08 requires --sample-bytes <n> (方案 §5.2 sets no numeric window)",
        );
    };
    if output.is_some() && output_dir.is_some() {
        return input_error("E1001: use either --output or --output-dir, not both");
    }
    let targets: Vec<PathBuf> = match (output, output_dir) {
        (Some(output), None) => {
            if files.len() != 1 {
                return input_error("E1002: --output requires exactly one --file");
            }
            vec![make_absolute(&output)]
        }
        (None, Some(dir)) => {
            let dir = make_absolute(&dir);
            files
                .iter()
                .map(|path| {
                    let name = path
                        .file_name()
                        .map(|value| value.to_os_string())
                        .unwrap_or_else(|| "output.csv".into());
                    dir.join(name)
                })
                .collect()
        }
        _ => return input_error("E1001: f08 requires --output <file> or --output-dir <dir>"),
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
    let mut plan_files = Vec::with_capacity(files.len());
    for (path, target) in files.iter().zip(targets.iter()) {
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
            eprintln!("E1002: f08 requires regular files: {}", absolute.display());
            return 2;
        }
        let inspection = match csv_merge_inspect(
            &absolute,
            CsvMergeScanOptions {
                delimiter,
                has_header,
                sample_window_bytes: sample_window,
                sample_blocks,
                platform_prior: prior,
                source_encoding: source,
            },
        ) {
            Ok(inspection) => inspection,
            Err(error) => {
                eprintln!(
                    "E1004: cannot inspect CSV for row extraction ({}): {error}",
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
        plan_files.push(PlanF08File {
            path: absolute.to_string_lossy().into_owned(),
            canonical_path,
            file_size: inspection.file_size,
            source_encoding: inspection.source_encoding,
            delimiter: inspection.delimiter,
            has_header,
            columns: inspection.columns,
            column_count: inspection.column_count,
            data_rows: inspection.data_rows,
            dirty_rows: inspection.dirty_rows,
            target: target.to_string_lossy().into_owned(),
            target_occupied: target.exists(),
        });
    }

    let plan_id = plan_id.unwrap_or_else(|| new_ulid("f08"));
    let planned = plan_f08(PlanF08Input {
        plan_id,
        target_encoding: target,
        invalid,
        window,
        count,
        protected_dirs,
        case_sense,
        files: plan_files,
    });
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

fn run_f09(args: &[String]) -> i32 {
    let mut files = Vec::new();
    let mut start = None;
    let mut end = None;
    let mut output = None;
    let mut output_dir = None;
    let mut target = None;
    let mut source = None;
    let mut delimiter = None;
    let mut invalid = InvalidSequencePolicy::ReplaceAndCount;
    let mut prior = PlatformPrior::Other;
    let mut sample_window = None;
    let mut has_header = true;
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
            "--start" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --start requires a row number");
                };
                match value.parse::<u64>() {
                    Ok(parsed) if parsed > 0 => start = Some(parsed),
                    _ => {
                        return input_error(format!(
                            "E1002: --start must be a positive 1-based row number: {value}"
                        ))
                    }
                }
            }
            "--end" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --end requires a row number");
                };
                match value.parse::<u64>() {
                    Ok(parsed) if parsed > 0 => end = Some(parsed),
                    _ => {
                        return input_error(format!(
                            "E1002: --end must be a positive 1-based row number: {value}"
                        ))
                    }
                }
            }
            "--output" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --output requires a path");
                };
                output = Some(PathBuf::from(value));
            }
            "--output-dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --output-dir requires a path");
                };
                output_dir = Some(PathBuf::from(value));
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
            "--delimiter" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --delimiter requires a value");
                };
                match Delimiter::parse(value) {
                    Some(parsed) => delimiter = Some(parsed),
                    None => {
                        return input_error(format!(
                            "E1002: --delimiter must be one of , ; tab | pipe: {value}"
                        ))
                    }
                }
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
            "--sample-bytes" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --sample-bytes requires a window size");
                };
                match value.parse::<u64>() {
                    Ok(bytes) if bytes > 0 => sample_window = Some(bytes),
                    _ => {
                        return input_error(format!(
                            "E1002: --sample-bytes must be a positive byte count: {value}"
                        ))
                    }
                }
            }
            "--no-header" => has_header = false,
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
            other => return input_error(format!("E1002: unknown f09 argument: {other}")),
        }
        index += 1;
    }

    if files.is_empty() {
        return input_error("E1001: f09 requires at least one --file <path>");
    }
    let (Some(start_row), Some(end_row)) = (start, end) else {
        return input_error("E1001: f09 requires --start <X> and --end <Y>");
    };
    if end_row < start_row {
        return input_error("E1002: f09 requires --end >= --start");
    }
    let Some(target) = target else {
        return input_error("E1001: f09 requires --target <encoding>");
    };
    let Some(sample_window) = sample_window else {
        return input_error(
            "E1001: f09 requires --sample-bytes <n> (方案 §5.2 sets no numeric window)",
        );
    };
    if output.is_some() && output_dir.is_some() {
        return input_error("E1001: use either --output or --output-dir, not both");
    }
    let targets: Vec<PathBuf> = match (output, output_dir) {
        (Some(output), None) => {
            if files.len() != 1 {
                return input_error("E1002: --output requires exactly one --file");
            }
            vec![make_absolute(&output)]
        }
        (None, Some(dir)) => {
            let dir = make_absolute(&dir);
            files
                .iter()
                .map(|path| {
                    let name = path
                        .file_name()
                        .map(|value| value.to_os_string())
                        .unwrap_or_else(|| "output.csv".into());
                    dir.join(name)
                })
                .collect()
        }
        _ => return input_error("E1001: f09 requires --output <file> or --output-dir <dir>"),
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
    let mut plan_files = Vec::with_capacity(files.len());
    for (path, target) in files.iter().zip(targets.iter()) {
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
            eprintln!("E1002: f09 requires regular files: {}", absolute.display());
            return 2;
        }
        let layout = match csv_range_layout(
            &absolute,
            CsvRangeScanOptions {
                delimiter,
                has_header,
                start_row,
                end_row,
                sample_window_bytes: sample_window,
                platform_prior: prior,
                source_encoding: source,
            },
        ) {
            Ok(layout) => layout,
            Err(error) => {
                eprintln!(
                    "E1004: cannot inspect CSV for row range ({}): {error}",
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
        plan_files.push(PlanF09File {
            path: absolute.to_string_lossy().into_owned(),
            canonical_path,
            file_size: layout.file_size,
            source_encoding: layout.source_encoding,
            delimiter: layout.delimiter,
            delimiter_detected: layout.delimiter_detected,
            has_header,
            columns: layout.header_columns,
            column_count: layout.column_count,
            data_rows: layout.data_rows,
            total_known: layout.total_known,
            selected_rows: layout.selected_rows,
            header_range: layout.header.map(|span| (span.start, span.end)),
            data_range: layout.selected.map(|span| (span.start, span.end)),
            dirty_rows: layout.dirty_rows,
            multi_line_header: layout.multi_line_header,
            unclosed_quote: layout.unclosed_quote,
            target: target.to_string_lossy().into_owned(),
            target_occupied: target.exists(),
        });
    }

    let plan_id = plan_id.unwrap_or_else(|| new_ulid("f09"));
    let planned = plan_f09(PlanF09Input {
        plan_id,
        target_encoding: target,
        invalid,
        start_row,
        end_row,
        protected_dirs,
        case_sense,
        files: plan_files,
    });
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

fn run_f10(args: &[String]) -> i32 {
    let mut roots = Vec::new();
    let mut output = None;
    let mut output_dir = None;
    let mut plan_id = None;
    let mut protected_dirs = Vec::new();
    let mut json = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--root" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --root requires a directory");
                };
                roots.push(PathBuf::from(value));
            }
            "--output" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --output requires a path");
                };
                output = Some(PathBuf::from(value));
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
            other => return input_error(format!("E1002: unknown f10 argument: {other}")),
        }
        index += 1;
    }

    if roots.is_empty() {
        return input_error("E1001: f10 requires at least one --root <dir>");
    }
    if output.is_some() && output_dir.is_some() {
        return input_error("E1001: use either --output or --output-dir, not both");
    }
    let targets: Vec<PathBuf> = match (output, output_dir) {
        (Some(output), None) => {
            if roots.len() != 1 {
                return input_error("E1002: --output requires exactly one --root");
            }
            vec![make_absolute(&output)]
        }
        (None, Some(dir)) => {
            let dir = make_absolute(&dir);
            roots
                .iter()
                .map(|root| {
                    let name = root
                        .file_name()
                        .map(|value| value.to_os_string())
                        .unwrap_or_else(|| "listing.csv".into());
                    let name = PathBuf::from(name);
                    dir.join(name.with_extension("csv"))
                })
                .collect()
        }
        _ => return input_error("E1001: f10 requires --output <file> or --output-dir <dir>"),
    };

    let protected_dirs = match protected_dirs_from_paths(&protected_dirs) {
        Ok(protected_dirs) => protected_dirs,
        Err(error) => {
            eprintln!("{}: {}", error.code, error.message);
            return 2;
        }
    };
    let options = ListingOptions::default();
    let mut plan_files = Vec::with_capacity(roots.len());
    for (root, target) in roots.iter().zip(targets.iter()) {
        let absolute = make_absolute(root);
        let metadata = match std::fs::metadata(&absolute) {
            Ok(metadata) => metadata,
            Err(error) => {
                eprintln!(
                    "E1004: cannot read directory metadata ({}): {error}",
                    absolute.display()
                );
                return 2;
            }
        };
        if !metadata.is_dir() {
            eprintln!(
                "E1002: f10 requires directory roots: {}",
                absolute.display()
            );
            return 2;
        }
        let stats = match inspect_tree(&absolute, &options) {
            Ok(stats) => stats,
            Err(error) => {
                eprintln!(
                    "E1004: cannot inspect directory tree ({}): {error}",
                    absolute.display()
                );
                return 2;
            }
        };
        let canonical_root = if protected_dirs.is_empty() {
            None
        } else {
            platform::fs::canonicalize_display(&absolute)
        };
        plan_files.push(PlanF10File {
            root: absolute.to_string_lossy().into_owned(),
            canonical_root,
            target: target.to_string_lossy().into_owned(),
            target_occupied: target.exists(),
            files: stats.files,
            dirs: stats.dirs,
            placeholders: stats.placeholders,
            skipped: stats.skipped,
            max_depth: stats.max_depth,
        });
    }

    let plan_id = plan_id.unwrap_or_else(|| new_ulid("f10"));
    let planned = plan_f10(PlanF10Input {
        plan_id,
        protected_dirs,
        case_sense: CliFsAdapter::new(None).case_sensitivity(),
        files: plan_files,
    });
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

fn run_f11(args: &[String]) -> i32 {
    let mut list_path = None;
    let mut root = None;
    let mut source = None;
    let mut plan_id = None;
    let mut protected_dirs = Vec::new();
    let mut json = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--list" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --list requires a CSV path");
                };
                list_path = Some(PathBuf::from(value));
            }
            "--root" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --root requires a directory");
                };
                root = Some(PathBuf::from(value));
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
            other => return input_error(format!("E1002: unknown f11 argument: {other}")),
        }
        index += 1;
    }

    let Some(list_path) = list_path else {
        return input_error("E1001: f11 requires --list <csv>");
    };
    let Some(root) = root else {
        return input_error("E1001: f11 requires --root <dir>");
    };
    let root = make_absolute(&root);
    match std::fs::metadata(&root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return input_error("E1002: --root must be a directory"),
        Err(error) => {
            eprintln!(
                "E1004: cannot read target root ({}): {error}",
                root.display()
            );
            return 2;
        }
    }
    let root_text = root.to_string_lossy().into_owned();
    let list_path = make_absolute(&list_path);
    let bytes = match std::fs::read(&list_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!(
                "E1004: cannot read list file ({}): {error}",
                list_path.display()
            );
            return 2;
        }
    };
    let list_encoding = match source {
        Some(source) => source,
        None => match detect(&bytes, PlatformPrior::Other).require_encoding() {
            Ok(encoding) => encoding,
            Err(error) => return print_kernel_error(&error),
        },
    };
    let decoded = match transcode(
        &bytes,
        list_encoding,
        TargetEncoding::Utf8 { bom: false },
        InvalidSequencePolicy::Abort,
    ) {
        Ok(decoded) => decoded,
        Err(error) => return print_kernel_error(&error),
    };
    let records = parse_records(&decoded.bytes, b',');
    let mut path_index = 0usize;
    let mut type_index = None;
    let mut start = 0usize;
    if let Some(first) = records.first() {
        let header: Vec<&str> = first
            .iter()
            .map(|cell| cell.trim().trim_start_matches('\u{feff}'))
            .collect();
        if let Some(found) = header
            .iter()
            .position(|cell| matches!(*cell, "相对路径" | "路径" | "目录" | "path"))
        {
            path_index = found;
            type_index = header
                .iter()
                .position(|cell| matches!(*cell, "类型" | "type"));
            start = 1;
        }
    }
    let mut rows = Vec::new();
    for (line_index, record) in records.iter().enumerate().skip(start) {
        if let Some(type_index) = type_index {
            let kind = record
                .get(type_index)
                .map(|value| value.trim())
                .unwrap_or_default();
            if kind != "目录" && kind != "dir" {
                continue;
            }
        }
        let path = record.get(path_index).cloned().unwrap_or_default();
        rows.push(PlanF11Row {
            line: line_index as u64 + 1,
            path,
        });
    }
    let mut existing_targets = Vec::new();
    for row in &rows {
        let Ok(normalized) = normalize_list_path(&row.path) else {
            continue;
        };
        let target = target_for_list_path(&root_text, &normalized);
        if std::fs::metadata(&target).is_ok() {
            existing_targets.push(target);
        }
    }
    let protected_dirs = match protected_dirs_from_paths(&protected_dirs) {
        Ok(protected_dirs) => protected_dirs,
        Err(error) => {
            eprintln!("{}: {}", error.code, error.message);
            return 2;
        }
    };
    let canonical_root = if protected_dirs.is_empty() {
        None
    } else {
        platform::fs::canonicalize_display(&root)
    };
    let plan_id = plan_id.unwrap_or_else(|| new_ulid("f11"));
    let planned = plan_f11(PlanF11Input {
        plan_id,
        root: root_text,
        canonical_root,
        rows,
        existing_targets,
        protected_dirs,
        case_sense: CliFsAdapter::new(None).case_sensitivity(),
    });
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

fn run_f12(args: &[String]) -> i32 {
    let mut dir = None;
    let mut prefix = None;
    let mut suffix = None;
    let mut find = None;
    let mut replace = None;
    let mut scope = RenameScope::Full;
    let mut plan_id = None;
    let mut protected_dirs = Vec::new();
    let mut json = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --dir requires a directory");
                };
                dir = Some(PathBuf::from(value));
            }
            "--prefix" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --prefix requires text");
                };
                prefix = Some(value.clone());
            }
            "--suffix" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --suffix requires text");
                };
                suffix = Some(value.clone());
            }
            "--find" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --find requires text");
                };
                find = Some(value.clone());
            }
            "--replace" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --replace requires text");
                };
                replace = Some(value.clone());
            }
            "--scope" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --scope requires full|stem|ext");
                };
                let Some(parsed) = RenameScope::parse(value) else {
                    return input_error(format!("E1002: unknown scope: {value}"));
                };
                scope = parsed;
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
            other => return input_error(format!("E1002: unknown f12 argument: {other}")),
        }
        index += 1;
    }

    let Some(dir) = dir else {
        return input_error("E1001: f12 requires --dir <dir>");
    };
    if prefix.is_none() && suffix.is_none() && find.is_none() {
        return input_error("E1001: f12 requires at least one of --prefix/--suffix/--find");
    }
    if replace.is_some() && find.is_none() {
        return input_error("E1001: --replace requires --find");
    }
    let dir = make_absolute(&dir);
    let metadata = match std::fs::metadata(&dir) {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!(
                "E1004: cannot read rename directory ({}): {error}",
                dir.display()
            );
            return 2;
        }
    };
    if !metadata.is_dir() {
        return input_error("E1002: --dir must be a directory");
    }
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!(
                "E1004: cannot list rename directory ({}): {error}",
                dir.display()
            );
            return 2;
        }
    };
    let mut items = Vec::new();
    let mut existing_targets = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        existing_targets.push(path.to_string_lossy().into_owned());
        items.push(PlanF12Item {
            path: path.to_string_lossy().into_owned(),
            name,
        });
    }
    items.sort_by(|left, right| left.name.cmp(&right.name));
    let protected_dirs = match protected_dirs_from_paths(&protected_dirs) {
        Ok(protected_dirs) => protected_dirs,
        Err(error) => {
            eprintln!("{}: {}", error.code, error.message);
            return 2;
        }
    };
    let plan_id = plan_id.unwrap_or_else(|| new_ulid("f12"));
    let planned = plan_f12(PlanF12Input {
        plan_id,
        items,
        existing_targets,
        protected_dirs,
        case_sense: CliFsAdapter::new(None).case_sensitivity(),
        long_path_supported: platform::preflight::long_path_supported(),
        rule: PlanF12Rule {
            prefix,
            suffix,
            find,
            replace: replace.unwrap_or_default(),
            scope,
        },
    });
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

fn run_f13(args: &[String]) -> i32 {
    let mut dir = None;
    let mut pattern = None;
    let mut replacement = None;
    let mut scope = RenameScope::Full;
    let mut plan_id = None;
    let mut protected_dirs = Vec::new();
    let mut json = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--dir" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --dir requires a directory");
                };
                dir = Some(PathBuf::from(value));
            }
            "--regex" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --regex requires a pattern");
                };
                pattern = Some(value.clone());
            }
            "--replace" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --replace requires a template");
                };
                replacement = Some(value.clone());
            }
            "--scope" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return input_error("E1001: --scope requires full|stem|ext");
                };
                let Some(parsed) = RenameScope::parse(value) else {
                    return input_error(format!("E1002: unknown scope: {value}"));
                };
                scope = parsed;
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
            other => return input_error(format!("E1002: unknown f13 argument: {other}")),
        }
        index += 1;
    }

    let Some(dir) = dir else {
        return input_error("E1001: f13 requires --dir <dir>");
    };
    let Some(pattern) = pattern else {
        return input_error("E1001: f13 requires --regex <pattern>");
    };
    let Some(replacement) = replacement else {
        return input_error("E1001: f13 requires --replace <template>");
    };
    let dir = make_absolute(&dir);
    let metadata = match std::fs::metadata(&dir) {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!(
                "E1004: cannot read rename directory ({}): {error}",
                dir.display()
            );
            return 2;
        }
    };
    if !metadata.is_dir() {
        return input_error("E1002: --dir must be a directory");
    }
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!(
                "E1004: cannot list rename directory ({}): {error}",
                dir.display()
            );
            return 2;
        }
    };
    let mut items = Vec::new();
    let mut existing_targets = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        existing_targets.push(path.to_string_lossy().into_owned());
        items.push(PlanF13Item {
            path: path.to_string_lossy().into_owned(),
            name,
        });
    }
    items.sort_by(|left, right| left.name.cmp(&right.name));
    let protected_dirs = match protected_dirs_from_paths(&protected_dirs) {
        Ok(protected_dirs) => protected_dirs,
        Err(error) => {
            eprintln!("{}: {}", error.code, error.message);
            return 2;
        }
    };
    let plan_id = plan_id.unwrap_or_else(|| new_ulid("f13"));
    let planned = plan_f13(PlanF13Input {
        plan_id,
        items,
        existing_targets,
        protected_dirs,
        case_sense: CliFsAdapter::new(None).case_sensitivity(),
        long_path_supported: platform::preflight::long_path_supported(),
        rule: PlanF13Rule {
            pattern,
            replacement,
            scope,
        },
    });
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
