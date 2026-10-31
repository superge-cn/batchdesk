//!
//! C10 批量解压计划与执行（方案 §5.11）。
//!
//! 本切片覆盖 ZIP 解压：Z01 的 GBK 名回退、Z02 的路径穿越拒绝、解压总量/比例
//! 限额与逐文件 Journal 撤销。压缩与 GBK 名写入留待后续切片。

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use crate::apply::{
    execute_reverse, ApplyEntryReport, ApplyOptions, ApplyOutcome, ApplyReport, ApplyState,
    ApplyStatus, OnConflict,
};
use crate::conflict::validate_target_name;
use crate::error::KernelError;
use crate::fs::{CaseSense, FsAdapter};
use crate::journal::{FsOp, JournalStore};
use crate::license::{Capability, LicenseService};
use crate::path::{is_under, protected_by, try_normalize, ProtectedDir};
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanDocument, PlanEntry, PlanExpected, PlanParamValue,
    PlanResolution, PlanStats, PlanWarning, RiskLevel,
};
use crate::progress::{CancelToken, ProgressEvent, ProgressPhase, ProgressSink};

pub const C10_KIND: &str = "c10";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveEntryKind {
    File,
    Dir,
}

impl ArchiveEntryKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Dir => "dir",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "file" => Some(Self::File),
            "dir" => Some(Self::Dir),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveCompression {
    Store,
    Deflate,
}

impl ArchiveCompression {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Store => "store",
            Self::Deflate => "deflate",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "store" | "stored" => Some(Self::Store),
            "deflate" | "deflated" => Some(Self::Deflate),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ArchiveEntryInfo {
    pub index: u64,
    /// GBK/UTF-8 解码后的相对名；无法解码时为 None。
    pub name: Option<String>,
    pub kind: ArchiveEntryKind,
    pub size: u64,
    pub compressed_size: u64,
    pub gbk_fallback: bool,
    pub encrypted: bool,
    pub supported: bool,
}

#[derive(Clone, Debug)]
pub struct ArchiveInspect {
    pub archive_size: u64,
    pub entries: Vec<ArchiveEntryInfo>,
}

/// 解压端口：平台实现 ZIP 读取，kernel 负责路径/限额/Journal。
pub trait ArchiveService: Send + Sync {
    fn inspect(&self, archive: &str) -> Result<ArchiveInspect, KernelError>;
    fn extract_entry(
        &self,
        archive: &str,
        index: u64,
        writer: &mut dyn Write,
    ) -> Result<(), KernelError>;
    /// 将输入的源文件/目录打包为 ZIP 字节（UTF-8 文件名）。
    fn compress(
        &self,
        sources: &[String],
        compression: ArchiveCompression,
    ) -> Result<Vec<u8>, KernelError>;
}

#[derive(Clone, Debug)]
pub struct PlanC10Input {
    pub plan_id: String,
    pub archive: String,
    pub archive_size: u64,
    pub entries: Vec<ArchiveEntryInfo>,
    pub output_dir: String,
    pub max_total_bytes: u64,
    pub max_ratio: f64,
    pub existing_targets: Vec<String>,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
}

#[derive(Clone, Debug)]
struct SelectedEntry {
    index: u64,
    name: String,
    kind: ArchiveEntryKind,
    size: u64,
}

#[derive(Default)]
struct Selection {
    selected: Vec<SelectedEntry>,
    unsafe_names: Vec<String>,
    existing_names: Vec<String>,
    protected_names: Vec<String>,
    gbk_fallback: bool,
}

fn normalize_entry_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return None;
    }
    let replaced = trimmed.replace('\\', "/");
    if replaced.starts_with('/') {
        return None;
    }
    let bytes = replaced.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return None;
    }
    let mut parts = Vec::new();
    for part in replaced.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains('\0') {
            return None;
        }
        if !validate_target_name(part).is_empty() {
            return None;
        }
        parts.push(part);
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

fn join_target(root: &str, name: &str, sense: CaseSense) -> Option<String> {
    let root = root.trim_end_matches(['/', '\\']);
    let target = try_normalize(&format!("{root}/{name}")).ok()?;
    let normalized_root = try_normalize(root).ok()?;
    (target != normalized_root && is_under(&target, &normalized_root, sense)).then_some(target)
}

fn select_entries(
    entries: &[ArchiveEntryInfo],
    output_dir: &str,
    existing: &BTreeSet<String>,
    protected: &[ProtectedDir],
    sense: CaseSense,
) -> Selection {
    let mut selection = Selection::default();
    for entry in entries {
        if entry.encrypted || !entry.supported {
            continue;
        }
        if entry.gbk_fallback {
            selection.gbk_fallback = true;
        }
        let Some(name) = entry.name.as_deref().and_then(normalize_entry_name) else {
            selection.unsafe_names.push(
                entry
                    .name
                    .clone()
                    .unwrap_or_else(|| "<undecodable>".to_string()),
            );
            continue;
        };
        let Some(target) = join_target(output_dir, &name, sense) else {
            selection.unsafe_names.push(name);
            continue;
        };
        let key = crate::path::normalize(&target);
        if existing.contains(&key) {
            selection.existing_names.push(name);
            continue;
        }
        if protected_by(&target, None, protected, sense).is_some() {
            selection.protected_names.push(name);
            continue;
        }
        selection.selected.push(SelectedEntry {
            index: entry.index,
            name,
            kind: entry.kind,
            size: entry.size,
        });
    }
    selection
}

pub fn plan_c10(input: PlanC10Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.archive.trim().is_empty() {
        return Err(KernelError::missing_param("archive path is required"));
    }
    if input.output_dir.trim().is_empty() {
        return Err(KernelError::missing_param("output directory is required"));
    }
    if input.max_total_bytes == 0 {
        return Err(KernelError::invalid_params(
            "maxTotalBytes must be a positive byte count",
        ));
    }
    if !input.max_ratio.is_finite() || input.max_ratio <= 0.0 {
        return Err(KernelError::invalid_params(
            "maxRatio must be a positive finite number",
        ));
    }

    let existing: BTreeSet<String> = input
        .existing_targets
        .iter()
        .map(|path| crate::path::normalize(path))
        .collect();
    let selection = select_entries(
        &input.entries,
        &input.output_dir,
        &existing,
        &input.protected_dirs,
        input.case_sense,
    );
    let total: u64 = selection.selected.iter().map(|entry| entry.size).sum();
    let ratio = if input.archive_size == 0 {
        if total > 0 {
            f64::INFINITY
        } else {
            0.0
        }
    } else {
        total as f64 / input.archive_size as f64
    };

    let mut blocked = input
        .entries
        .iter()
        .find(|entry| entry.encrypted)
        .map(|_| PlanBlocked {
            code: "E3003".to_string(),
            reason: "encrypted archives are not supported in this slice".to_string(),
            hint: Some("remove the password and retry".to_string()),
        })
        .or_else(|| {
            input
                .entries
                .iter()
                .find(|entry| !entry.supported)
                .map(|_| PlanBlocked {
                    code: "E3003".to_string(),
                    reason: "archive entry uses an unsupported compression method".to_string(),
                    hint: Some("only stored/deflated ZIP entries are supported".to_string()),
                })
        });
    if blocked.is_none() && (total > input.max_total_bytes || ratio > input.max_ratio) {
        blocked = Some(PlanBlocked {
            code: "E3007".to_string(),
            reason: format!(
                "archive extraction limits exceeded (total {total} bytes, ratio {ratio:.2})"
            ),
            hint: Some(
                "raise --max-total-bytes/--max-ratio or choose a smaller archive".to_string(),
            ),
        });
    }
    let no_safe_entries = selection.selected.is_empty();
    let resolution = if blocked.is_none() && no_safe_entries {
        Some(PlanResolution::Skip)
    } else {
        None
    };

    let files = selection
        .selected
        .iter()
        .filter(|entry| entry.kind == ArchiveEntryKind::File)
        .count();
    let dirs = selection.selected.len().saturating_sub(files);
    let reason = match &blocked {
        Some(block) => block.reason.clone(),
        None if no_safe_entries => "no safe entries to extract".to_string(),
        None => format!(
            "extract {files} files, {dirs} dirs, total {total} bytes; skipped unsafe={} existing={} protected={}",
            selection.unsafe_names.len(),
            selection.existing_names.len(),
            selection.protected_names.len()
        ),
    };

    let mut warnings = Vec::new();
    if !selection.unsafe_names.is_empty() {
        warnings.push(PlanWarning {
            kind: "unsafe-entry-skipped".to_string(),
            suggestion: Some(format!(
                "unsafe or undecodable entries are skipped: {}",
                selection.unsafe_names.join(", ")
            )),
            risk_level: RiskLevel::Danger,
        });
    }
    if !selection.existing_names.is_empty() {
        warnings.push(PlanWarning {
            kind: "existing-target-skipped".to_string(),
            suggestion: Some(format!(
                "existing targets are skipped: {}",
                selection.existing_names.join(", ")
            )),
            risk_level: RiskLevel::Caution,
        });
    }
    if !selection.protected_names.is_empty() {
        warnings.push(PlanWarning {
            kind: "protected-target-skipped".to_string(),
            suggestion: Some(format!(
                "protected targets are skipped: {}",
                selection.protected_names.join(", ")
            )),
            risk_level: RiskLevel::Danger,
        });
    }
    if selection.gbk_fallback {
        warnings.push(PlanWarning {
            kind: "gbk-filename-fallback".to_string(),
            suggestion: Some("entries without the UTF-8 flag were decoded as GBK".to_string()),
            risk_level: RiskLevel::Caution,
        });
    }
    if no_safe_entries && blocked.is_none() {
        warnings.push(PlanWarning {
            kind: "no-safe-entries".to_string(),
            suggestion: Some("archive contains no extractable entries".to_string()),
            risk_level: RiskLevel::Caution,
        });
    }

    let mut params = BTreeMap::new();
    params.insert(
        "maxTotalBytes".to_string(),
        PlanParamValue::Number(input.max_total_bytes),
    );
    params.insert(
        "maxRatio".to_string(),
        PlanParamValue::String(format!("{}", input.max_ratio)),
    );
    params.insert(
        "entryCount".to_string(),
        PlanParamValue::Number(input.entries.len() as u64),
    );
    params.insert(
        "safeCount".to_string(),
        PlanParamValue::Number(selection.selected.len() as u64),
    );

    let entries = vec![PlanEntry {
        entry_id: format!("{}-0001", input.plan_id),
        source: input.archive.clone(),
        target: Some(input.output_dir.clone()),
        action: PlanAction::Extract,
        relation: None,
        reason,
        conflict: None,
        resolution,
        blocked,
        expected: Some(PlanExpected {
            size: input.archive_size,
            algorithm: "size".to_string(),
            hash: None,
        }),
        range: None,
        merge: None,
        csv: None,
        csv_merge: None,
        csv_extract: None,
        csv_rows: None,
    }];
    let stats = PlanStats {
        total: 1,
        conflict: 0,
        warning: warnings.len(),
        blocked: entries
            .iter()
            .filter(|entry| entry.blocked.is_some())
            .count(),
    };
    Ok(PlanDocument {
        plan_id: input.plan_id,
        kind: C10_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

#[derive(Clone, Debug)]
pub struct PlanC10CompressInput {
    pub plan_id: String,
    pub output: String,
    pub sources: Vec<String>,
    pub compression: ArchiveCompression,
    pub target_exists: bool,
    pub source_file_count: u64,
    pub source_bytes: u64,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
}

pub fn plan_c10_compress(input: PlanC10CompressInput) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.output.trim().is_empty() {
        return Err(KernelError::missing_param("output path is required"));
    }
    if input.sources.is_empty() {
        return Err(KernelError::missing_param(
            "c10 compression requires at least one source",
        ));
    }
    let mut blocked = None;
    for source in &input.sources {
        let same = crate::path::normalize(source) == crate::path::normalize(&input.output);
        if same || is_under(&input.output, source, input.case_sense) {
            blocked = Some(PlanBlocked {
                code: "E1002".to_string(),
                reason: "output must not be one of the sources or inside a source directory"
                    .to_string(),
                hint: Some("choose an output path outside the input tree".to_string()),
            });
            break;
        }
    }
    if blocked.is_none() {
        blocked =
            crate::csv_merge::target_block(&input.output, &input.protected_dirs, input.case_sense);
    }
    let resolution = if blocked.is_none() && input.target_exists {
        Some(PlanResolution::Skip)
    } else {
        None
    };
    let reason = match &blocked {
        Some(block) => block.reason.clone(),
        None if input.target_exists => "target exists; skipped by default".to_string(),
        None => format!(
            "compress {} files, {} bytes -> {} ZIP (UTF-8 names)",
            input.source_file_count,
            input.source_bytes,
            input.compression.as_str()
        ),
    };
    let mut warnings = Vec::new();
    if input.target_exists && blocked.is_none() {
        warnings.push(PlanWarning {
            kind: "target-exists-skipped".to_string(),
            suggestion: Some("existing output files are skipped by default".to_string()),
            risk_level: RiskLevel::Caution,
        });
    }
    let mut params = BTreeMap::new();
    params.insert(
        "sources".to_string(),
        PlanParamValue::String(serde_json::to_string(&input.sources).map_err(|error| {
            KernelError::internal("source list is not serializable", error.to_string())
        })?),
    );
    params.insert(
        "compression".to_string(),
        PlanParamValue::String(input.compression.as_str().to_string()),
    );
    params.insert(
        "sourceCount".to_string(),
        PlanParamValue::Number(input.source_file_count),
    );
    params.insert(
        "sourceBytes".to_string(),
        PlanParamValue::Number(input.source_bytes),
    );
    let entries = vec![PlanEntry {
        entry_id: format!("{}-0001", input.plan_id),
        source: input.sources[0].clone(),
        target: Some(input.output),
        action: PlanAction::Archive,
        relation: None,
        reason,
        conflict: None,
        resolution,
        blocked,
        expected: None,
        range: None,
        merge: None,
        csv: None,
        csv_merge: None,
        csv_extract: None,
        csv_rows: None,
    }];
    let stats = PlanStats {
        total: 1,
        conflict: 0,
        warning: warnings.len(),
        blocked: entries
            .iter()
            .filter(|entry| entry.blocked.is_some())
            .count(),
    };
    Ok(PlanDocument {
        plan_id: input.plan_id,
        kind: C10_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

pub struct ArchiveApplyContext<'a> {
    pub fs: &'a dyn FsAdapter,
    pub license: &'a dyn LicenseService,
    pub journal: &'a mut dyn JournalStore,
    pub archive: &'a dyn ArchiveService,
    pub task_id: String,
    pub now_rfc3339: String,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub progress: Option<&'a dyn ProgressSink>,
    pub cancel: Option<&'a dyn CancelToken>,
}

pub fn apply_archive_plan(
    plan: &PlanDocument,
    options: &ApplyOptions,
    context: &mut ArchiveApplyContext<'_>,
) -> Result<ApplyOutcome, KernelError> {
    if plan.kind != C10_KIND {
        return Err(KernelError::invalid_params(
            "only c10 plans are supported by the archive executor",
        ));
    }
    if plan.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if plan
        .entries
        .iter()
        .any(|entry| !matches!(entry.action, PlanAction::Extract | PlanAction::Archive))
    {
        return Err(KernelError::invalid_params(
            "c10 plans only support extract/archive entries",
        ));
    }
    if options.on_conflict == OnConflict::Overwrite && !options.overwrite_confirmed {
        return Err(KernelError::invalid_params(
            "overwrite requires explicit confirmation (overwriteConfirmed)",
        ));
    }
    let mut reports = Vec::with_capacity(plan.entries.len());
    let mut failed = 0usize;
    let mut ok = 0usize;
    let mut cancelled = false;
    let total_entries = plan.entries.len() as u64;
    report_progress(context, 0, total_entries);
    for (position, entry) in plan.entries.iter().enumerate() {
        report_progress(context, position as u64, total_entries);
        if is_cancelled(context) {
            cancelled = true;
            break;
        }
        if let Some(block) = &entry.blocked {
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Skipped,
                error: Some(KernelError::new(
                    block.code.clone(),
                    block.reason.clone(),
                    block.hint.clone(),
                    false,
                )),
            });
            continue;
        }
        if entry.resolution == Some(PlanResolution::Skip) {
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Skipped,
                error: None,
            });
            continue;
        }
        let source_stat = match context.fs.stat(&entry.source) {
            Ok(stat) => stat,
            Err(error) => {
                failed += 1;
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Failed,
                    error: Some(error),
                });
                continue;
            }
        };
        if entry
            .expected
            .as_ref()
            .is_some_and(|expected| expected.size != source_stat.size)
        {
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Skipped,
                error: Some(KernelError::new(
                    "E2010",
                    "archive changed since plan preview; re-plan before applying",
                    None,
                    true,
                )),
            });
            continue;
        }
        match context.license.capability(&plan.kind) {
            Capability::Allow => {}
            Capability::Deny { reason } => {
                failed += 1;
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Failed,
                    error: Some(KernelError::new(
                        "E4001",
                        format!("operation not licensed: {reason}"),
                        None,
                        false,
                    )),
                });
                continue;
            }
            Capability::Quota { used, limit } => {
                failed += 1;
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Failed,
                    error: Some(KernelError::new(
                        "E4004",
                        format!("license quota exhausted ({used}/{limit})"),
                        None,
                        false,
                    )),
                });
                continue;
            }
        }
        if let Err(error) = context.license.consume(&plan.kind, 1) {
            failed += 1;
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Failed,
                error: Some(error),
            });
            continue;
        }
        if entry.action == PlanAction::Archive {
            let Some(target) = entry.target.clone() else {
                context.license.release(&plan.kind, 1);
                failed += 1;
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Failed,
                    error: Some(KernelError::missing_param("archive target is required")),
                });
                continue;
            };
            if protected_by(&target, None, &context.protected_dirs, context.case_sense).is_some() {
                context.license.release(&plan.kind, 1);
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Skipped,
                    error: Some(KernelError::new(
                        "E2009",
                        "target is under a protected directory",
                        None,
                        false,
                    )),
                });
                continue;
            }
            if context.fs.stat(&target).is_ok() {
                context.license.release(&plan.kind, 1);
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Skipped,
                    error: None,
                });
                continue;
            }
            let sources = match plan_param_sources(plan) {
                Ok(sources) => sources,
                Err(error) => {
                    context.license.release(&plan.kind, 1);
                    failed += 1;
                    reports.push(ApplyEntryReport {
                        entry_id: entry.entry_id.clone(),
                        status: ApplyStatus::Failed,
                        error: Some(error),
                    });
                    continue;
                }
            };
            let compression = match plan_param_compression(plan) {
                Ok(compression) => compression,
                Err(error) => {
                    context.license.release(&plan.kind, 1);
                    failed += 1;
                    reports.push(ApplyEntryReport {
                        entry_id: entry.entry_id.clone(),
                        status: ApplyStatus::Failed,
                        error: Some(error),
                    });
                    continue;
                }
            };
            let bytes = match context.archive.compress(&sources, compression) {
                Ok(bytes) => bytes,
                Err(error) => {
                    context.license.release(&plan.kind, 1);
                    failed += 1;
                    reports.push(ApplyEntryReport {
                        entry_id: entry.entry_id.clone(),
                        status: ApplyStatus::Failed,
                        error: Some(error),
                    });
                    continue;
                }
            };
            if let Err(error) = context.fs.write_atomic(&target, &bytes) {
                context.license.release(&plan.kind, 1);
                failed += 1;
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Failed,
                    error: Some(error),
                });
                continue;
            }
            let forward = FsOp::WriteAtomic {
                path: target.clone(),
                data_ref: None,
            };
            let reverse = match context
                .fs
                .quarantine_target_path(crate::conflict::file_name_of(&target))
            {
                Ok(path) => FsOp::Quarantine {
                    from: target.clone(),
                    to: path,
                },
                Err(error) => {
                    context.license.release(&plan.kind, 1);
                    failed += 1;
                    reports.push(ApplyEntryReport {
                        entry_id: entry.entry_id.clone(),
                        status: ApplyStatus::Failed,
                        error: Some(error),
                    });
                    continue;
                }
            };
            if let Err(error) = append_journal(context, plan, entry, forward, reverse.clone()) {
                let _ = execute_reverse(context.fs, &reverse);
                context.license.release(&plan.kind, 1);
                failed += 1;
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Failed,
                    error: Some(error),
                });
                continue;
            }
            ok += 1;
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Ok,
                error: None,
            });
            continue;
        }
        let inspect = match context.archive.inspect(&entry.source) {
            Ok(inspect) => inspect,
            Err(error) => {
                context.license.release(&plan.kind, 1);
                failed += 1;
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Failed,
                    error: Some(error),
                });
                continue;
            }
        };
        if inspect
            .entries
            .iter()
            .any(|item| item.encrypted || !item.supported)
        {
            context.license.release(&plan.kind, 1);
            failed += 1;
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Failed,
                error: Some(KernelError::new(
                    "E3003",
                    "encrypted or unsupported archive entries cannot be extracted",
                    None,
                    false,
                )),
            });
            continue;
        }
        let max_total = match plan.params.get("maxTotalBytes") {
            Some(PlanParamValue::Number(value)) => *value,
            _ => {
                context.license.release(&plan.kind, 1);
                failed += 1;
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Failed,
                    error: Some(KernelError::missing_param(
                        "plan param maxTotalBytes is required",
                    )),
                });
                continue;
            }
        };
        let max_ratio = match plan.params.get("maxRatio") {
            Some(PlanParamValue::String(value)) => value
                .parse::<f64>()
                .map_err(|_| KernelError::invalid_params("plan param maxRatio is not a number"))?,
            _ => {
                context.license.release(&plan.kind, 1);
                failed += 1;
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Failed,
                    error: Some(KernelError::missing_param(
                        "plan param maxRatio is required",
                    )),
                });
                continue;
            }
        };
        let output_dir = entry.target.clone().unwrap_or_default();
        let selection = select_entries(
            &inspect.entries,
            &output_dir,
            &BTreeSet::new(),
            &context.protected_dirs,
            context.case_sense,
        );
        let total: u64 = selection.selected.iter().map(|item| item.size).sum();
        let ratio = if inspect.archive_size == 0 {
            if total > 0 {
                f64::INFINITY
            } else {
                0.0
            }
        } else {
            total as f64 / inspect.archive_size as f64
        };
        if total > max_total || ratio > max_ratio {
            context.license.release(&plan.kind, 1);
            failed += 1;
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Failed,
                error: Some(KernelError::new(
                    "E3007",
                    "archive extraction limits exceeded",
                    Some(format!("total {total} bytes, ratio {ratio:.2}")),
                    false,
                )),
            });
            continue;
        }
        let mut first_error = None;
        let mut failures = 0usize;
        for item in &selection.selected {
            if is_cancelled(context) {
                cancelled = true;
                break;
            }
            let Some(target) = join_target(&output_dir, &item.name, context.case_sense) else {
                continue;
            };
            let occupied = context.fs.stat(&target).is_ok();
            if occupied {
                continue;
            }
            let relative_parent = item.name.rsplit_once('/').map(|(parent, _)| parent);
            if let Some(relative_parent) = relative_parent {
                let mut current = output_dir.trim_end_matches(['/', '\\']).to_string();
                let mut created_ok = true;
                for part in relative_parent.split('/') {
                    current.push('/');
                    current.push_str(part);
                    if context.fs.stat(&current).is_err() {
                        if let Err(error) = context.fs.create_dir(&current) {
                            first_error.get_or_insert(error);
                            failures += 1;
                            created_ok = false;
                            break;
                        }
                        let forward = FsOp::CreateDir {
                            path: current.clone(),
                        };
                        let reverse = FsOp::RemoveDir {
                            path: current.clone(),
                        };
                        if let Err(error) = append_journal(context, plan, entry, forward, reverse) {
                            first_error.get_or_insert(error);
                            failures += 1;
                            created_ok = false;
                            break;
                        }
                    }
                }
                if !created_ok {
                    continue;
                }
            }
            if item.kind == ArchiveEntryKind::Dir {
                if let Err(error) = context.fs.create_dir(&target) {
                    first_error.get_or_insert(error);
                    failures += 1;
                    continue;
                }
                let forward = FsOp::CreateDir {
                    path: target.clone(),
                };
                let reverse = FsOp::RemoveDir {
                    path: target.clone(),
                };
                if let Err(error) = append_journal(context, plan, entry, forward, reverse) {
                    first_error.get_or_insert(error);
                    failures += 1;
                }
                continue;
            }
            let write = context.fs.write_atomic_with(&target, &mut |writer| {
                context
                    .archive
                    .extract_entry(&entry.source, item.index, writer)
            });
            if let Err(error) = write {
                first_error.get_or_insert(error);
                failures += 1;
                continue;
            }
            let forward = FsOp::WriteAtomic {
                path: target.clone(),
                data_ref: None,
            };
            let reverse = match context
                .fs
                .quarantine_target_path(crate::conflict::file_name_of(&target))
            {
                Ok(path) => FsOp::Quarantine {
                    from: target.clone(),
                    to: path,
                },
                Err(error) => {
                    first_error.get_or_insert(error);
                    failures += 1;
                    continue;
                }
            };
            if let Err(error) =
                append_journal(context, plan, entry, forward.clone(), reverse.clone())
            {
                let _ = execute_reverse(context.fs, &reverse);
                first_error.get_or_insert(error);
                failures += 1;
            }
        }
        if failures > 0 {
            failed += 1;
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Failed,
                error: first_error,
            });
        } else {
            ok += 1;
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Ok,
                error: None,
            });
        }
    }
    let state = if cancelled {
        ApplyState::Cancelled
    } else if failed == 0 {
        ApplyState::Completed
    } else if ok > 0 {
        ApplyState::PartialFailed
    } else {
        ApplyState::Failed
    };
    Ok(ApplyOutcome {
        state,
        report: ApplyReport {
            plan_id: plan.plan_id.clone(),
            task_id: context.task_id.clone(),
            entries: reports,
            undone_by: None,
        },
    })
}

fn plan_param_sources(plan: &PlanDocument) -> Result<Vec<String>, KernelError> {
    let raw = match plan.params.get("sources") {
        Some(PlanParamValue::String(value)) => value,
        _ => return Err(KernelError::missing_param("plan param sources is required")),
    };
    serde_json::from_str(raw).map_err(|error| {
        KernelError::invalid_params(format!("plan param sources is not a string list: {error}"))
    })
}

fn plan_param_compression(plan: &PlanDocument) -> Result<ArchiveCompression, KernelError> {
    let raw = match plan.params.get("compression") {
        Some(PlanParamValue::String(value)) => value,
        _ => {
            return Err(KernelError::missing_param(
                "plan param compression is required",
            ))
        }
    };
    ArchiveCompression::parse(raw)
        .ok_or_else(|| KernelError::invalid_params("plan param compression is not supported"))
}

fn append_journal(
    context: &mut ArchiveApplyContext<'_>,
    plan: &PlanDocument,
    entry: &PlanEntry,
    forward: FsOp,
    reverse: FsOp,
) -> Result<(), KernelError> {
    context
        .journal
        .append_with_kind(
            &plan.plan_id,
            &entry.entry_id,
            &context.now_rfc3339,
            &plan.kind,
            forward,
            reverse,
        )
        .map(|_| ())
}

fn is_cancelled(context: &ArchiveApplyContext<'_>) -> bool {
    context
        .cancel
        .map(CancelToken::is_cancelled)
        .unwrap_or(false)
}

fn report_progress(context: &ArchiveApplyContext<'_>, done: u64, total: u64) {
    if let Some(progress) = context.progress {
        progress.report(ProgressEvent {
            task_id: context.task_id.clone(),
            phase: ProgressPhase::Executing,
            done,
            total,
            message: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_names_reject_traversal_and_absolute_paths() {
        assert_eq!(
            normalize_entry_name("safe/ok.txt").as_deref(),
            Some("safe/ok.txt")
        );
        assert_eq!(normalize_entry_name("safe/../escape.txt"), None);
        assert_eq!(normalize_entry_name("../escape.txt"), None);
        assert_eq!(normalize_entry_name("/etc/passwd"), None);
        assert_eq!(normalize_entry_name(r"C:\windows\x"), None);
        assert_eq!(normalize_entry_name("CON.txt"), None);
        assert_eq!(normalize_entry_name("dir/"), Some("dir".to_string()));
    }

    #[test]
    fn target_join_stays_under_output_root() {
        assert_eq!(
            join_target("/out", "safe/a.txt", CaseSense::Sensitive).as_deref(),
            Some("/out/safe/a.txt")
        );
        assert!(join_target("/out", "safe/a.txt", CaseSense::Sensitive).is_some());
        assert!(join_target("/out", "../a.txt", CaseSense::Sensitive).is_none());
    }

    fn entry(index: u64, name: &str, kind: ArchiveEntryKind, size: u64) -> ArchiveEntryInfo {
        ArchiveEntryInfo {
            index,
            name: Some(name.to_string()),
            kind,
            size,
            compressed_size: size,
            gbk_fallback: false,
            encrypted: false,
            supported: true,
        }
    }

    fn input(entries: Vec<ArchiveEntryInfo>) -> PlanC10Input {
        PlanC10Input {
            plan_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            archive: "/data/a.zip".to_string(),
            archive_size: 100,
            entries,
            output_dir: "/out".to_string(),
            max_total_bytes: 10_000,
            max_ratio: 100.0,
            existing_targets: Vec::new(),
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
        }
    }

    #[test]
    fn plan_skips_unsafe_entries_and_extracts_safe_ones() {
        let plan = plan_c10(input(vec![
            entry(0, "safe/ok.txt", ArchiveEntryKind::File, 10),
            entry(1, "../escape.txt", ArchiveEntryKind::File, 10),
            entry(2, "safe/../../escape2.txt", ArchiveEntryKind::File, 10),
        ]))
        .unwrap();
        assert_eq!(plan.kind, C10_KIND);
        assert_eq!(plan.entries[0].action, PlanAction::Extract);
        assert!(plan.entries[0].blocked.is_none());
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "unsafe-entry-skipped"));
        assert!(plan.entries[0].reason.contains("skipped unsafe=2"));
    }

    #[test]
    fn plan_blocks_when_limits_are_exceeded() {
        let mut input = input(vec![entry(0, "a.txt", ArchiveEntryKind::File, 1_000)]);
        input.max_total_bytes = 100;
        let plan = plan_c10(input).unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3007");
    }

    #[test]
    fn plan_blocks_encrypted_entries() {
        let mut item = entry(0, "a.txt", ArchiveEntryKind::File, 1);
        item.encrypted = true;
        let plan = plan_c10(input(vec![item])).unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3003");
    }

    #[test]
    fn existing_and_protected_targets_are_skipped() {
        let mut input = input(vec![
            entry(0, "a.txt", ArchiveEntryKind::File, 1),
            entry(1, "b.txt", ArchiveEntryKind::File, 1),
            entry(2, "c.txt", ArchiveEntryKind::File, 1),
        ]);
        input.existing_targets = vec!["/out/a.txt".to_string()];
        input.protected_dirs = vec![ProtectedDir::new("/out/b.txt")];
        let plan = plan_c10(input).unwrap();
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "existing-target-skipped"));
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "protected-target-skipped"));
        assert!(plan.entries[0]
            .reason
            .contains("skipped unsafe=0 existing=1 protected=1"));
    }

    fn compress_input(sources: Vec<&str>, output: &str) -> PlanC10CompressInput {
        PlanC10CompressInput {
            plan_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            output: output.to_string(),
            sources: sources.into_iter().map(str::to_string).collect(),
            compression: ArchiveCompression::Deflate,
            target_exists: false,
            source_file_count: 2,
            source_bytes: 20,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
        }
    }

    #[test]
    fn compression_plan_blocks_self_inclusion() {
        let plan = plan_c10_compress(compress_input(vec!["/data/a"], "/data/a.zip")).unwrap();
        assert!(plan.entries[0].blocked.is_none());
        let inside =
            plan_c10_compress(compress_input(vec!["/data/dir"], "/data/dir/out.zip")).unwrap();
        assert_eq!(inside.entries[0].blocked.as_ref().unwrap().code, "E1002");
        let same = plan_c10_compress(compress_input(vec!["/data/a"], "/data/a")).unwrap();
        assert_eq!(same.entries[0].blocked.as_ref().unwrap().code, "E1002");
    }

    #[test]
    fn compression_plan_skips_existing_target() {
        let mut input = compress_input(vec!["/data/a", "/data/b"], "/data/out.zip");
        input.target_exists = true;
        let plan = plan_c10_compress(input).unwrap();
        assert_eq!(plan.entries[0].resolution, Some(PlanResolution::Skip));
        match &plan.params["sources"] {
            PlanParamValue::String(sources) => assert!(sources.contains("/data/a")),
            other => panic!("unexpected sources param: {other:?}"),
        }
    }

    #[test]
    fn gbk_fallback_is_reported() {
        let mut item = entry(0, "a.txt", ArchiveEntryKind::File, 1);
        item.gbk_fallback = true;
        let plan = plan_c10(input(vec![item])).unwrap();
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "gbk-filename-fallback"));
    }
}
