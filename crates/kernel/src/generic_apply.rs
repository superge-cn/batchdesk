//! 通用文件操作执行器（C05/C06/C18 复用；方案 §5.13、设计 01 §5）。
//!
//! 只执行计划，不做规则生成：新增功能只需产出 `PlanDocument`（参数 → 条目），
//! 冲突二次裁决、Journal、进度、取消回滚、授权复核与指纹复核都在这里统一完成。
//!
//! 覆盖 `rename`／`move`／`delete`／`copy`／`edit-content`；压缩/转码等需要
//! 领域变换的动作用 `content_editor` 注入变换函数（F04/C01–C03 复用本执行器），
//! 其余专用动作遇到即 E1002 明确拒绝，不静默降级。
//!
//! 撤销语义（设计 01 §8、方案 §7.8）：rename/move/delete 的逆由 `FsOp::inverse`
//! 推导；copy 新建目标的逆是"产物进隔离目录"（reverse=`Quarantine`）；copy 覆盖
//! 与 edit-content 先经 `FsAdapter::backup_to_quarantine` 备份原内容，逆是
//! `WriteAtomic{dataRef}` 备份还原。回滚统一走 `apply::execute_reverse`。

use crate::apply::{
    execute_reverse, ApplyEntryReport, ApplyOptions, ApplyOutcome, ApplyReport, ApplyState,
    ApplyStatus, OnConflict,
};
use crate::conflict::sequence_target;
use crate::csv::Delimiter;
use crate::csv_merge::{column_keys, normalized_columns, parse_records, write_record};
use crate::csv_rows::{select_row_window, RowWindow};
use crate::encoding::{transcode, InvalidSequencePolicy, SourceEncoding, TargetEncoding};
use crate::error::KernelError;
use crate::fs::{CaseSense, ConflictPolicy, EntryKind, FsAdapter, PathOutcomeKind};
use crate::journal::{FsOp, JournalStore};
use crate::license::{Capability, LicenseService};
use crate::listing::{DirectoryLister, ListingOptions};
use crate::path::{is_under, protected_by, try_normalize, ProtectedDir};
use crate::plan::{
    PlanAction, PlanCsvExtract, PlanCsvMerge, PlanCsvRows, PlanDocument, PlanParamValue,
    PlanResolution,
};
use crate::progress::{CancelToken, ProgressPhase, ProgressSink};

/// `edit-content` 的内容变换函数：输入原始字节，返回变换后的字节。
pub type ContentEditor<'a> = &'a dyn Fn(&[u8]) -> Result<Vec<u8>, KernelError>;

pub struct GenericApplyContext<'a> {
    pub fs: &'a dyn FsAdapter,
    pub license: &'a dyn LicenseService,
    pub journal: &'a mut dyn JournalStore,
    pub task_id: String,
    pub now_rfc3339: String,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub progress: Option<&'a dyn ProgressSink>,
    pub cancel: Option<&'a dyn CancelToken>,
    /// `edit-content` 条目的内容变换函数（如转码/换行/替换）；其他动作为 None。
    pub content_editor: Option<ContentEditor<'a>>,
    /// F10 目录清单的流式遍历/写出端口；其他动作为 None。
    pub directory_lister: Option<&'a dyn DirectoryLister>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TargetDecision {
    Use(String),
    Overwrite(String),
    Skip(KernelError),
}

/// 执行通用计划：逐条授权复核 → 预检 → 冲突二次裁决 → 落盘 → Journal。
pub fn apply_generic_plan(
    plan: &PlanDocument,
    options: &ApplyOptions,
    context: &mut GenericApplyContext<'_>,
) -> Result<ApplyOutcome, KernelError> {
    if plan.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if plan.kind.trim().is_empty() {
        return Err(KernelError::missing_param("plan kind is required"));
    }
    for entry in &plan.entries {
        match entry.action {
            PlanAction::Rename
            | PlanAction::Move
            | PlanAction::Delete
            | PlanAction::Copy
            | PlanAction::EditContent
            | PlanAction::Split
            | PlanAction::Merge
            | PlanAction::CsvExtract
            | PlanAction::CsvRows
            | PlanAction::Listing
            | PlanAction::CreateDir => {}
            other => {
                return Err(KernelError::invalid_params(format!(
                    "{} entries need a dedicated executor",
                    other.as_str()
                )))
            }
        }
        match entry.action {
            PlanAction::Delete if entry.target.is_some() => {
                return Err(KernelError::invalid_params(
                    "delete entries must not have a target",
                ))
            }
            PlanAction::EditContent if entry.target.is_some() => {
                return Err(KernelError::invalid_params(
                    "edit-content entries edit in place and must not have a target",
                ))
            }
            PlanAction::Rename
            | PlanAction::Move
            | PlanAction::Copy
            | PlanAction::Split
            | PlanAction::Merge
            | PlanAction::CsvExtract
            | PlanAction::CsvRows
            | PlanAction::Listing
            | PlanAction::CreateDir
                if entry.target.is_none() =>
            {
                return Err(KernelError::invalid_params(format!(
                    "{} entries require a target",
                    entry.action.as_str()
                )))
            }
            PlanAction::Split if entry.range.is_none() => {
                return Err(KernelError::invalid_params(
                    "split entries require a byte range",
                ))
            }
            PlanAction::Split => {
                let range = entry.range.expect("checked above");
                if range.start >= range.end {
                    return Err(KernelError::invalid_params(
                        "split byte range must be non-empty",
                    ));
                }
                if let Some(csv) = &entry.csv {
                    if csv.delimiter.is_empty() {
                        return Err(KernelError::invalid_params(
                            "csv split payload requires a delimiter",
                        ));
                    }
                    if let Some(header) = csv.header {
                        if header.start >= header.end {
                            return Err(KernelError::invalid_params(
                                "csv header range must be non-empty",
                            ));
                        }
                        if header.end > range.start {
                            return Err(KernelError::invalid_params(
                                "csv header range must end before the record range",
                            ));
                        }
                    }
                }
            }
            PlanAction::Merge if entry.merge.is_none() && entry.csv_merge.is_none() => {
                return Err(KernelError::invalid_params(
                    "merge entries require a merge or csvMerge payload",
                ))
            }
            PlanAction::Merge if entry.merge.is_some() && entry.csv_merge.is_some() => {
                return Err(KernelError::invalid_params(
                    "merge entries must not carry both merge and csvMerge payloads",
                ))
            }
            PlanAction::Merge => {
                if let Some(merge) = &entry.merge {
                    if merge.sources.is_empty() {
                        return Err(KernelError::invalid_params(
                            "merge payload requires at least one source",
                        ));
                    }
                    if TargetEncoding::parse(&merge.target_encoding).is_none() {
                        return Err(KernelError::invalid_params(
                            "merge targetEncoding is not a supported encoding",
                        ));
                    }
                    if InvalidSequencePolicy::parse(&merge.invalid).is_none() {
                        return Err(KernelError::invalid_params(
                            "merge invalid policy is not supported",
                        ));
                    }
                } else if let Some(csv_merge) = &entry.csv_merge {
                    validate_csv_merge(csv_merge)?;
                }
            }
            PlanAction::CsvExtract => {
                let payload = entry.csv_extract.as_ref().ok_or_else(|| {
                    KernelError::invalid_params("csv-extract entries require a csvExtract payload")
                })?;
                validate_csv_extract(payload)?;
            }
            PlanAction::CsvRows => {
                let payload = entry.csv_rows.as_ref().ok_or_else(|| {
                    KernelError::invalid_params("csv-rows entries require a csvRows payload")
                })?;
                validate_csv_rows(payload)?;
            }
            _ => {}
        }
    }
    if plan
        .entries
        .iter()
        .any(|entry| entry.action == PlanAction::EditContent)
        && context.content_editor.is_none()
    {
        return Err(KernelError::invalid_params(
            "edit-content entries require a content editor on the apply context",
        ));
    }
    let overwrite_requested = options.on_conflict == OnConflict::Overwrite
        || plan
            .entries
            .iter()
            .any(|entry| entry.resolution == Some(PlanResolution::Overwrite));
    if overwrite_requested && !options.overwrite_confirmed {
        return Err(KernelError::invalid_params(
            "overwrite requires explicit confirmation (overwriteConfirmed)",
        ));
    }

    let total = plan.entries.len() as u64;
    let mut reports = Vec::with_capacity(plan.entries.len());
    let mut applied: Vec<(usize, u64, FsOp)> = Vec::new();
    let mut failed = 0usize;
    let mut cancelled = false;

    report_progress(context, ProgressPhase::Executing, 0, total, None);
    for (index, entry) in plan.entries.iter().enumerate() {
        report_progress(context, ProgressPhase::Executing, index as u64, total, None);
        if is_cancelled(context) {
            cancelled = true;
            break;
        }
        if let Some(blocked) = &entry.blocked {
            reports.push(skipped_entry(
                entry.entry_id.clone(),
                KernelError::new(
                    blocked.code.clone(),
                    blocked.reason.clone(),
                    blocked.hint.clone(),
                    false,
                ),
            ));
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

        if entry.action != PlanAction::CreateDir {
            let normalized = match try_normalize(&entry.source) {
                Ok(normalized) => normalized,
                Err(error) => {
                    failed += 1;
                    reports.push(failed_entry(entry.entry_id.clone(), error));
                    continue;
                }
            };
            if let Some(error) = protected_path_hit(context, &entry.source, &normalized) {
                reports.push(skipped_entry(entry.entry_id.clone(), error));
                continue;
            }
        }

        match context.license.capability(&plan.kind) {
            Capability::Allow => {}
            Capability::Deny { reason } => {
                failed += 1;
                reports.push(failed_entry(
                    entry.entry_id.clone(),
                    KernelError::new(
                        "E4001",
                        format!("operation not licensed: {reason}"),
                        None,
                        false,
                    ),
                ));
                continue;
            }
            Capability::Quota { used, limit } => {
                failed += 1;
                reports.push(failed_entry(
                    entry.entry_id.clone(),
                    KernelError::new(
                        "E4004",
                        format!("license quota exhausted ({used}/{limit})"),
                        None,
                        false,
                    ),
                ));
                continue;
            }
        }

        if entry.action != PlanAction::CreateDir {
            let stat = match context.fs.stat(&entry.source) {
                Ok(stat) => stat,
                Err(error) => {
                    failed += 1;
                    reports.push(failed_entry(entry.entry_id.clone(), error));
                    continue;
                }
            };
            if entry.action == PlanAction::Listing {
                if stat.kind != EntryKind::Dir {
                    failed += 1;
                    reports.push(failed_entry(
                        entry.entry_id.clone(),
                        KernelError::invalid_params("listing expects a directory source"),
                    ));
                    continue;
                }
            } else if entry.action == PlanAction::Rename {
                if !matches!(stat.kind, EntryKind::File | EntryKind::Dir) {
                    failed += 1;
                    reports.push(failed_entry(
                        entry.entry_id.clone(),
                        KernelError::invalid_params("rename expects a file or directory source"),
                    ));
                    continue;
                }
            } else if entry.action != PlanAction::Delete && stat.kind != EntryKind::File {
                failed += 1;
                reports.push(failed_entry(
                    entry.entry_id.clone(),
                    KernelError::invalid_params(format!(
                        "{} expects a file source",
                        entry.action.as_str()
                    )),
                ));
                continue;
            }
            if stat.attrs.placeholder {
                reports.push(skipped_entry(
                    entry.entry_id.clone(),
                    KernelError::new("E2006", "placeholder file skipped", None, true),
                ));
                continue;
            }
            if let Some(error) = fingerprint_mismatch(context, entry, stat.size) {
                reports.push(skipped_entry(entry.entry_id.clone(), error));
                continue;
            }
            if entry.action == PlanAction::Split {
                if let Some(range) = entry.range {
                    let required_end = entry
                        .csv
                        .as_ref()
                        .and_then(|csv| csv.header)
                        .map_or(range.end, |header| range.end.max(header.end));
                    if required_end > stat.size {
                        reports.push(skipped_entry(
                            entry.entry_id.clone(),
                            KernelError::new(
                                "E2010",
                                "source file shrank since plan preview; re-plan before applying",
                                None,
                                true,
                            ),
                        ));
                        continue;
                    }
                }
            }
            if entry.action == PlanAction::Merge {
                if let Some(error) = merge_source_mismatch(context, entry) {
                    reports.push(skipped_entry(entry.entry_id.clone(), error));
                    continue;
                }
            }
        }

        let decision = match &entry.target {
            Some(target) => match resolve_target(entry, target, options, context) {
                Ok(decision) => decision,
                Err(error) => {
                    failed += 1;
                    reports.push(failed_entry(entry.entry_id.clone(), error));
                    continue;
                }
            },
            None => TargetDecision::Use(String::new()),
        };
        let (target, overwrite) = match decision {
            TargetDecision::Use(target) => (target, false),
            TargetDecision::Overwrite(target) => (target, true),
            TargetDecision::Skip(error) => {
                reports.push(skipped_entry(entry.entry_id.clone(), error));
                continue;
            }
        };
        if matches!(
            entry.action,
            PlanAction::Rename
                | PlanAction::Move
                | PlanAction::Copy
                | PlanAction::Split
                | PlanAction::Merge
                | PlanAction::CsvExtract
                | PlanAction::CsvRows
                | PlanAction::Listing
                | PlanAction::CreateDir
        ) {
            if let Some(error) = protected_target_hit(context, &target) {
                reports.push(skipped_entry(entry.entry_id.clone(), error));
                continue;
            }
        }

        if let Err(error) = context.license.consume(&plan.kind, 1) {
            failed += 1;
            reports.push(failed_entry(entry.entry_id.clone(), error));
            continue;
        }

        let target_param = match &entry.action {
            PlanAction::EditContent => None,
            _ => Some(target.as_str()),
        };
        let performed = match perform(context, plan, entry, target_param, overwrite) {
            Ok(Ok(performed)) => performed,
            Ok(Err(error)) => {
                context.license.release(&plan.kind, 1);
                failed += 1;
                reports.push(failed_entry(entry.entry_id.clone(), error));
                continue;
            }
            Err(error) => {
                context.license.release(&plan.kind, 1);
                return Err(error);
            }
        };

        if performed.skipped {
            context.license.release(&plan.kind, 1);
            reports.push(skipped_entry(
                entry.entry_id.clone(),
                KernelError::new(
                    "E2007",
                    "target already exists; skipped by policy",
                    None,
                    false,
                ),
            ));
            continue;
        }

        let forward = performed.forward;
        let reverse = performed.reverse;
        match context.journal.append_with_kind(
            &plan.plan_id,
            &entry.entry_id,
            &context.now_rfc3339,
            &plan.kind,
            forward,
            reverse.clone(),
        ) {
            Ok(journal_entry) => {
                applied.push((index, journal_entry.seq, journal_entry.reverse));
            }
            Err(error) => {
                context.license.release(&plan.kind, 1);
                // 落盘成功但 Journal 失败：尽力回退本次动作后再报错。
                if execute_reverse(context.fs, &reverse).is_ok() {
                    failed += 1;
                    reports.push(failed_entry(entry.entry_id.clone(), error));
                    continue;
                }
                return Err(KernelError::internal(
                    "journal write failed and the operation could not be reverted",
                    format!("{}: {}", error.code, error.message),
                ));
            }
        }
        reports.push(ApplyEntryReport {
            entry_id: entry.entry_id.clone(),
            status: if overwrite || performed.replaced {
                ApplyStatus::Overwritten
            } else {
                ApplyStatus::Ok
            },
            error: None,
        });
    }

    if cancelled {
        report_progress(
            context,
            ProgressPhase::Journaling,
            0,
            applied.len() as u64,
            Some("rolling back applied entries"),
        );
        for (done, (index, seq, reverse)) in applied.iter().rev().enumerate() {
            report_progress(
                context,
                ProgressPhase::Journaling,
                done as u64,
                applied.len() as u64,
                None,
            );
            match execute_reverse(context.fs, reverse) {
                Ok(()) => {
                    let _ = context.journal.mark_reverted(&plan.plan_id, &[*seq]);
                }
                Err(error) => {
                    mark_entry_error(&mut reports, *index, error);
                }
            }
        }
    }

    let state = if cancelled {
        ApplyState::Cancelled
    } else if failed > 0 && reports.iter().any(|entry| entry.status == ApplyStatus::Ok) {
        ApplyState::PartialFailed
    } else if failed > 0 {
        ApplyState::Failed
    } else {
        ApplyState::Completed
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

fn resolve_target(
    entry: &crate::plan::PlanEntry,
    target: &str,
    options: &ApplyOptions,
    context: &GenericApplyContext<'_>,
) -> Result<TargetDecision, KernelError> {
    let occupied = context.fs.stat(target).is_ok();
    if !occupied {
        return Ok(TargetDecision::Use(target.to_string()));
    }
    let resolution = entry.resolution;
    let policy = match resolution {
        Some(PlanResolution::Overwrite) => OnConflict::Overwrite,
        Some(PlanResolution::Sequence) => OnConflict::Sequence,
        Some(PlanResolution::Proceed) => OnConflict::Skip,
        Some(PlanResolution::Skip) => OnConflict::Skip,
        None => options.on_conflict,
    };
    match policy {
        OnConflict::Overwrite => {
            if options.overwrite_confirmed || resolution == Some(PlanResolution::Overwrite) {
                Ok(TargetDecision::Overwrite(target.to_string()))
            } else {
                Ok(TargetDecision::Skip(KernelError::new(
                    "E1002",
                    "overwrite requires explicit confirmation",
                    None,
                    false,
                )))
            }
        }
        OnConflict::Sequence => {
            let mut taken = std::collections::BTreeSet::new();
            taken.insert(target.to_string());
            Ok(TargetDecision::Use(sequence_target(
                target,
                &taken,
                context.case_sense,
            )))
        }
        OnConflict::Skip | OnConflict::Ask => Ok(TargetDecision::Skip(KernelError::new(
            "E2007",
            "target already exists; skipped by policy",
            None,
            false,
        ))),
    }
}

/// 单条执行结果：是否被策略跳过、是否替换了既有目标、落点与 Journal 操作对。
struct Performed {
    skipped: bool,
    replaced: bool,
    #[allow(dead_code)]
    final_path: String,
    forward: FsOp,
    reverse: FsOp,
}

/// 执行单条动作并推导 Journal 的 forward/reverse（设计 01 §8）：
fn listing_options(plan: &PlanDocument) -> Result<ListingOptions, KernelError> {
    match plan.params.get("format") {
        Some(PlanParamValue::String(format)) if format == "csv" => {}
        _ => {
            return Err(KernelError::invalid_params(
                "listing plans require format=csv",
            ))
        }
    }
    let include_files = match plan.params.get("includeFiles") {
        Some(PlanParamValue::Bool(value)) => *value,
        _ => {
            return Err(KernelError::invalid_params(
                "listing plans require includeFiles",
            ))
        }
    };
    let include_dirs = match plan.params.get("includeDirs") {
        Some(PlanParamValue::Bool(value)) => *value,
        _ => {
            return Err(KernelError::invalid_params(
                "listing plans require includeDirs",
            ))
        }
    };
    if !include_files && !include_dirs {
        return Err(KernelError::invalid_params(
            "listing plans must include files or dirs",
        ));
    }
    Ok(ListingOptions {
        include_files,
        include_dirs,
    })
}

fn perform_listing(
    context: &GenericApplyContext<'_>,
    plan: &PlanDocument,
    entry: &crate::plan::PlanEntry,
    target: &str,
    overwrite: bool,
) -> Result<Performed, KernelError> {
    let options = listing_options(plan)?;
    if let (Ok(root), Ok(target_path)) = (try_normalize(&entry.source), try_normalize(target)) {
        if is_under(&target_path, &root, context.case_sense) {
            return Err(KernelError::invalid_params(
                "listing target must be outside the listed root",
            ));
        }
    }
    let lister = context.directory_lister.ok_or_else(|| {
        KernelError::new(
            "E5001",
            "directory lister is not available for this apply context",
            None,
            false,
        )
    })?;
    let backup = if overwrite {
        Some(context.fs.backup_to_quarantine(target)?)
    } else {
        None
    };
    context.fs.write_atomic_with(target, &mut |out| {
        lister
            .write_listing(&entry.source, &options, out)
            .map(|_| ())
    })?;
    let final_path = target.to_string();
    let reverse = match backup {
        Some(backup) => FsOp::WriteAtomic {
            path: final_path.clone(),
            data_ref: Some(backup),
        },
        None => FsOp::Quarantine {
            from: final_path.clone(),
            to: context
                .fs
                .quarantine_target_path(&file_name_of(&final_path))?,
        },
    };
    Ok(Performed {
        skipped: false,
        replaced: overwrite,
        final_path: final_path.clone(),
        forward: FsOp::WriteAtomic {
            path: final_path,
            data_ref: None,
        },
        reverse,
    })
}

/// copy 覆盖与 edit-content 先备份原内容（`dataRef`），copy 新建的逆是
/// "产物进隔离目录"（reverse=`Quarantine`，路径由适配层预留）。
fn perform(
    context: &GenericApplyContext<'_>,
    plan: &PlanDocument,
    entry: &crate::plan::PlanEntry,
    target: Option<&str>,
    overwrite: bool,
) -> Result<Result<Performed, KernelError>, KernelError> {
    let policy = if overwrite {
        ConflictPolicy::Overwrite
    } else {
        ConflictPolicy::Skip
    };
    let run = || -> Result<Performed, KernelError> {
        match entry.action {
            PlanAction::Rename => {
                let target = target.expect("validated: rename requires a target");
                let outcome = context.fs.rename(&entry.source, target, policy)?;
                to_performed(outcome, |final_path| FsOp::Rename {
                    from: entry.source.clone(),
                    to: final_path,
                })
            }
            PlanAction::Move => {
                let target = target.expect("validated: move requires a target");
                let outcome = context.fs.move_path(&entry.source, target, policy)?;
                to_performed(outcome, |final_path| FsOp::Rename {
                    from: entry.source.clone(),
                    to: final_path,
                })
            }
            PlanAction::Copy => {
                let target = target.expect("validated: copy requires a target");
                // 覆盖已有目标：先备份原内容（设计 01 §8 dataRef 语义）。
                let backup = if overwrite {
                    Some(context.fs.backup_to_quarantine(target)?)
                } else {
                    None
                };
                let outcome = context.fs.copy(&entry.source, target, policy)?;
                let final_path = outcome.final_path.clone();
                let reverse = match backup {
                    Some(backup) => FsOp::WriteAtomic {
                        path: final_path.clone(),
                        data_ref: Some(backup),
                    },
                    None => FsOp::Quarantine {
                        from: final_path.clone(),
                        to: context
                            .fs
                            .quarantine_target_path(&file_name_of(&final_path))?,
                    },
                };
                Ok(Performed {
                    skipped: outcome.outcome == PathOutcomeKind::Skipped,
                    replaced: outcome.outcome == PathOutcomeKind::Replaced,
                    final_path,
                    forward: FsOp::Copy {
                        from: entry.source.clone(),
                        to: outcome.final_path,
                    },
                    reverse,
                })
            }
            PlanAction::Split => {
                let target = target.expect("validated: split requires a target");
                let range = entry
                    .range
                    .ok_or_else(|| KernelError::invalid_params("split entries require a range"))?;
                let backup = if overwrite {
                    Some(context.fs.backup_to_quarantine(target)?)
                } else {
                    None
                };
                let outcome = match entry.csv.as_ref().and_then(|csv| csv.header) {
                    // F05：表头区间 + 记录区间一次写成同一分片（CSV 记录边界已在计划期确定）。
                    Some(header) => context.fs.copy_ranges_atomic(
                        &entry.source,
                        &[(header.start, header.end), (range.start, range.end)],
                        target,
                        policy,
                    )?,
                    None => context.fs.copy_range_atomic(
                        &entry.source,
                        range.start,
                        range.end,
                        target,
                        policy,
                    )?,
                };
                let final_path = outcome.final_path.clone();
                let reverse = match backup {
                    Some(backup) => FsOp::WriteAtomic {
                        path: final_path.clone(),
                        data_ref: Some(backup),
                    },
                    None => FsOp::Quarantine {
                        from: final_path.clone(),
                        to: context
                            .fs
                            .quarantine_target_path(&file_name_of(&final_path))?,
                    },
                };
                Ok(Performed {
                    skipped: outcome.outcome == PathOutcomeKind::Skipped,
                    replaced: outcome.outcome == PathOutcomeKind::Replaced,
                    final_path: final_path.clone(),
                    forward: FsOp::WriteAtomic {
                        path: final_path,
                        data_ref: None,
                    },
                    reverse,
                })
            }
            PlanAction::Merge => {
                let target = target.expect("validated: merge requires a target");
                if let Some(csv_merge) = &entry.csv_merge {
                    return perform_csv_merge(context, target, overwrite, csv_merge);
                }
                let merge = entry.merge.as_ref().ok_or_else(|| {
                    KernelError::invalid_params("merge entries require a payload")
                })?;
                let target_encoding =
                    TargetEncoding::parse(&merge.target_encoding).ok_or_else(|| {
                        KernelError::invalid_params("unsupported merge target encoding")
                    })?;
                let invalid = InvalidSequencePolicy::parse(&merge.invalid).ok_or_else(|| {
                    KernelError::invalid_params("unsupported merge invalid-sequence policy")
                })?;
                // 覆盖已有目标：先备份原内容（设计 01 §8 dataRef 语义）。
                let backup = if overwrite {
                    Some(context.fs.backup_to_quarantine(target)?)
                } else {
                    None
                };
                // BOM 只在合并产物最前面写一次；各输入自身 BOM 由 transcode 剥离。
                let mut merged = Vec::new();
                if let Some(prefix) = target_encoding.bom_prefix() {
                    merged.extend_from_slice(prefix);
                }
                for source in &merge.sources {
                    let encoding = source.encoding.ok_or_else(|| {
                        KernelError::new(
                            "E3001",
                            format!(
                                "source encoding requires manual confirmation: {}",
                                source.path
                            ),
                            None,
                            false,
                        )
                    })?;
                    let data = context.fs.read_bytes(&source.path)?;
                    if data.len() as u64 != source.size {
                        return Err(KernelError::new(
                            "E2010",
                            format!("source size changed since plan preview: {}", source.path),
                            None,
                            false,
                        ));
                    }
                    let transcoded =
                        transcode(&data, encoding, target_encoding.without_bom(), invalid)?;
                    merged.extend_from_slice(&transcoded.bytes);
                }
                context.fs.write_atomic(target, &merged)?;
                let final_path = target.to_string();
                let reverse = match backup {
                    Some(backup) => FsOp::WriteAtomic {
                        path: final_path.clone(),
                        data_ref: Some(backup),
                    },
                    None => FsOp::Quarantine {
                        from: final_path.clone(),
                        to: context
                            .fs
                            .quarantine_target_path(&file_name_of(&final_path))?,
                    },
                };
                Ok(Performed {
                    skipped: false,
                    replaced: overwrite,
                    final_path: final_path.clone(),
                    forward: FsOp::WriteAtomic {
                        path: final_path,
                        data_ref: None,
                    },
                    reverse,
                })
            }
            PlanAction::CsvExtract => {
                let target = target.expect("validated: csv-extract requires a target");
                let payload = entry.csv_extract.as_ref().ok_or_else(|| {
                    KernelError::invalid_params("csv-extract entries require a csvExtract payload")
                })?;
                perform_csv_extract(context, &entry.source, target, overwrite, payload)
            }
            PlanAction::CsvRows => {
                let target = target.expect("validated: csv-rows requires a target");
                let payload = entry.csv_rows.as_ref().ok_or_else(|| {
                    KernelError::invalid_params("csv-rows entries require a csvRows payload")
                })?;
                perform_csv_rows(context, &entry.source, target, overwrite, payload)
            }
            PlanAction::Listing => {
                let target = target.expect("validated: listing requires a target");
                perform_listing(context, plan, entry, target, overwrite)
            }
            PlanAction::CreateDir => {
                let target = target.expect("validated: create-dir requires a target");
                context.fs.create_dir(target)?;
                let forward = FsOp::CreateDir {
                    path: target.to_string(),
                };
                let reverse = forward.inverse().ok_or_else(|| {
                    KernelError::internal(
                        "create-dir reverse is not expressible",
                        format!("{forward:?}"),
                    )
                })?;
                Ok(Performed {
                    skipped: false,
                    replaced: false,
                    final_path: target.to_string(),
                    forward,
                    reverse,
                })
            }
            PlanAction::EditContent => {
                // 就地改写：备份 → 读取 → 注入的变换函数 → 原子写回。
                let backup = context.fs.backup_to_quarantine(&entry.source)?;
                let data = context.fs.read_bytes(&entry.source)?;
                let editor = context
                    .content_editor
                    .expect("validated: edit-content requires an editor");
                let edited = editor(&data)?;
                context.fs.write_atomic(&entry.source, &edited)?;
                let forward = FsOp::WriteAtomic {
                    path: entry.source.clone(),
                    data_ref: Some(backup),
                };
                let reverse = forward.inverse().ok_or_else(|| {
                    KernelError::internal(
                        "edit-content reverse is not expressible",
                        format!("{forward:?}"),
                    )
                })?;
                Ok(Performed {
                    skipped: false,
                    replaced: false,
                    final_path: entry.source.clone(),
                    forward,
                    reverse,
                })
            }
            PlanAction::Delete => {
                let quarantined = context.fs.remove_to_quarantine(&entry.source)?;
                Ok(Performed {
                    skipped: false,
                    replaced: false,
                    final_path: quarantined.clone(),
                    forward: FsOp::Quarantine {
                        from: entry.source.clone(),
                        to: quarantined.clone(),
                    },
                    reverse: FsOp::Rename {
                        from: quarantined,
                        to: entry.source.clone(),
                    },
                })
            }
            other => Err(KernelError::invalid_params(format!(
                "{} entries need a dedicated executor",
                other.as_str()
            ))),
        }
    };
    Ok(run())
}

fn to_performed(
    outcome: crate::fs::PathOutcome,
    forward: impl Fn(String) -> FsOp,
) -> Result<Performed, KernelError> {
    let forward_op = forward(outcome.final_path.clone());
    let reverse = forward_op.inverse().ok_or_else(|| {
        KernelError::internal(
            "generic apply journal reverse is not expressible",
            format!("{forward_op:?}"),
        )
    })?;
    Ok(Performed {
        skipped: outcome.outcome == PathOutcomeKind::Skipped,
        replaced: outcome.outcome == PathOutcomeKind::Replaced,
        final_path: outcome.final_path,
        forward: forward_op,
        reverse,
    })
}

fn file_name_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string())
}

fn fingerprint_mismatch(
    context: &GenericApplyContext<'_>,
    entry: &crate::plan::PlanEntry,
    size: u64,
) -> Option<KernelError> {
    // split 的 `expected.size` 是分片输出长度而非源文件大小，按区间单独复核。
    if entry.action == PlanAction::Split {
        return None;
    }
    let expected = entry.expected.as_ref()?;
    if size != expected.size {
        return Some(KernelError::new(
            "E2010",
            "source size changed since plan preview",
            None,
            false,
        ));
    }
    let hash = expected.hash.as_ref()?;
    match context.fs.content_hash(&entry.source, &expected.algorithm) {
        Ok(Some(actual)) if &actual != hash => Some(KernelError::new(
            "E2010",
            "source content changed since plan preview",
            None,
            false,
        )),
        Ok(_) => None,
        Err(error) => Some(error),
    }
}

fn protected_path_hit(
    context: &GenericApplyContext<'_>,
    path: &str,
    normalized: &str,
) -> Option<KernelError> {
    if context.protected_dirs.is_empty() {
        return None;
    }
    let canonical = context.fs.canonicalize(path).ok();
    protected_by(
        normalized,
        canonical.as_deref(),
        &context.protected_dirs,
        context.case_sense,
    )
    .map(|hit| {
        KernelError::new(
            "E2009",
            format!("path is under protected directory {hit}"),
            None,
            false,
        )
    })
}

/// F04/F06：合并的全部输入都要在执行期复核（大小、普通文件、占位、保护目录）。
fn merge_source_mismatch(
    context: &GenericApplyContext<'_>,
    entry: &crate::plan::PlanEntry,
) -> Option<KernelError> {
    let sources: Vec<(&str, u64, Option<SourceEncoding>)> = if let Some(merge) = &entry.merge {
        merge
            .sources
            .iter()
            .map(|source| (source.path.as_str(), source.size, source.encoding))
            .collect()
    } else if let Some(csv_merge) = &entry.csv_merge {
        csv_merge
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.size, file.encoding))
            .collect()
    } else {
        return None;
    };
    for (path, size, encoding) in sources {
        let normalized = match try_normalize(path) {
            Ok(normalized) => normalized,
            Err(error) => return Some(error),
        };
        if let Some(error) = protected_path_hit(context, path, &normalized) {
            return Some(error);
        }
        if entry.csv_merge.is_some() && encoding.is_none() {
            return Some(KernelError::new(
                "E3001",
                format!("source encoding requires manual confirmation: {path}"),
                None,
                false,
            ));
        }
        let stat = match context.fs.stat(path) {
            Ok(stat) => stat,
            Err(error) => return Some(error),
        };
        if stat.kind != EntryKind::File {
            return Some(KernelError::invalid_params(format!(
                "merge expects a file source: {path}"
            )));
        }
        if stat.attrs.placeholder {
            return Some(KernelError::new(
                "E2006",
                "placeholder file skipped",
                None,
                true,
            ));
        }
        if stat.size != size {
            return Some(KernelError::new(
                "E2010",
                "source size changed since plan preview",
                None,
                false,
            ));
        }
    }
    None
}

fn validate_csv_merge(payload: &PlanCsvMerge) -> Result<(), KernelError> {
    if payload.files.is_empty() {
        return Err(KernelError::invalid_params(
            "csvMerge payload requires at least one source",
        ));
    }
    if TargetEncoding::parse(&payload.target_encoding).is_none() {
        return Err(KernelError::invalid_params(
            "csvMerge targetEncoding is not a supported encoding",
        ));
    }
    if InvalidSequencePolicy::parse(&payload.invalid).is_none() {
        return Err(KernelError::invalid_params(
            "csvMerge invalid policy is not supported",
        ));
    }
    if Delimiter::parse(&payload.delimiter).is_none() {
        return Err(KernelError::invalid_params(
            "csvMerge delimiter is not supported",
        ));
    }
    if payload.has_header {
        if payload.union.is_empty() {
            return Err(KernelError::invalid_params(
                "csvMerge with headers requires a non-empty column union",
            ));
        }
        if payload.positional_columns as usize != payload.union.len() {
            return Err(KernelError::invalid_params(
                "csvMerge positionalColumns must match the column union length",
            ));
        }
    } else if payload.positional_columns == 0 {
        return Err(KernelError::invalid_params(
            "csvMerge without headers requires a positive positional column width",
        ));
    }
    if payload
        .source_column
        .as_ref()
        .is_some_and(|name| name.trim().is_empty())
    {
        return Err(KernelError::invalid_params(
            "csvMerge sourceColumn must not be empty",
        ));
    }
    for file in &payload.files {
        if Delimiter::parse(&file.delimiter).is_none() {
            return Err(KernelError::invalid_params(format!(
                "csvMerge source delimiter is not supported: {}",
                file.path
            )));
        }
        if file.encoding.is_none() {
            return Err(KernelError::new(
                "E3001",
                format!(
                    "source encoding requires manual confirmation: {}",
                    file.path
                ),
                None,
                false,
            ));
        }
        if payload.has_header && file.columns.is_empty() {
            return Err(KernelError::invalid_params(format!(
                "csvMerge source has no header columns: {}",
                file.path
            )));
        }
    }
    Ok(())
}

fn validate_csv_extract(payload: &PlanCsvExtract) -> Result<(), KernelError> {
    if payload.selections.is_empty() {
        return Err(KernelError::invalid_params(
            "csvExtract payload requires at least one selected column",
        ));
    }
    if payload.encoding.is_none() {
        return Err(KernelError::new(
            "E3001",
            "source encoding requires manual confirmation",
            None,
            false,
        ));
    }
    if TargetEncoding::parse(&payload.target_encoding).is_none() {
        return Err(KernelError::invalid_params(
            "csvExtract targetEncoding is not a supported encoding",
        ));
    }
    if InvalidSequencePolicy::parse(&payload.invalid).is_none() {
        return Err(KernelError::invalid_params(
            "csvExtract invalid policy is not supported",
        ));
    }
    if Delimiter::parse(&payload.delimiter).is_none() {
        return Err(KernelError::invalid_params(
            "csvExtract delimiter is not supported",
        ));
    }
    if payload.has_header {
        if payload.columns.is_empty() {
            return Err(KernelError::invalid_params(
                "csvExtract with headers requires non-empty source columns",
            ));
        }
        if payload.column_count as usize != payload.columns.len() {
            return Err(KernelError::invalid_params(
                "csvExtract columnCount must match the header width",
            ));
        }
    } else if payload.column_count == 0 {
        return Err(KernelError::invalid_params(
            "csvExtract without headers requires a positive column width",
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for selection in &payload.selections {
        if selection.index >= payload.column_count {
            return Err(KernelError::invalid_params(format!(
                "csvExtract column index {} is out of range",
                selection.index + 1
            )));
        }
        if !seen.insert(selection.index) {
            return Err(KernelError::invalid_params(format!(
                "csvExtract column {} is selected more than once",
                selection.index + 1
            )));
        }
        if payload.has_header {
            let Some(name) = &selection.name else {
                return Err(KernelError::invalid_params(
                    "csvExtract header selections require a column name",
                ));
            };
            if payload.columns.get(selection.index as usize) != Some(name) {
                return Err(KernelError::invalid_params(
                    "csvExtract selection does not match the source header",
                ));
            }
        } else if selection.name.is_some() {
            return Err(KernelError::invalid_params(
                "csvExtract without headers must not carry column names",
            ));
        }
    }
    Ok(())
}

fn validate_csv_rows(payload: &PlanCsvRows) -> Result<(), KernelError> {
    if payload.encoding.is_none() {
        return Err(KernelError::new(
            "E3001",
            "source encoding requires manual confirmation",
            None,
            false,
        ));
    }
    if TargetEncoding::parse(&payload.target_encoding).is_none() {
        return Err(KernelError::invalid_params(
            "csvRows targetEncoding is not a supported encoding",
        ));
    }
    if InvalidSequencePolicy::parse(&payload.invalid).is_none() {
        return Err(KernelError::invalid_params(
            "csvRows invalid policy is not supported",
        ));
    }
    if Delimiter::parse(&payload.delimiter).is_none() {
        return Err(KernelError::invalid_params(
            "csvRows delimiter is not supported",
        ));
    }
    let Some(window) = RowWindow::parse(&payload.window) else {
        return Err(KernelError::invalid_params(format!(
            "csvRows window must be first, last, or range: {}",
            payload.window
        )));
    };
    if window == RowWindow::Range {
        let (Some(start), Some(end)) = (payload.start_row, payload.end_row) else {
            return Err(KernelError::invalid_params(
                "csvRows range mode requires startRow and endRow",
            ));
        };
        if start == 0 || end < start {
            return Err(KernelError::invalid_params(
                "csvRows range must be 1-based with endRow >= startRow",
            ));
        }
        if payload.has_header && payload.header_range.is_none() && payload.size > 0 {
            return Err(KernelError::invalid_params(
                "csvRows range with headers requires a header byte range",
            ));
        }
        if !payload.has_header && payload.header_range.is_some() {
            return Err(KernelError::invalid_params(
                "csvRows range without headers must not carry a header byte range",
            ));
        }
        if payload.data_range.is_none() && payload.count > 0 {
            return Err(KernelError::invalid_params(
                "csvRows range with selected rows requires a data byte range",
            ));
        }
        if payload.data_range.is_some() && payload.count == 0 {
            return Err(KernelError::invalid_params(
                "csvRows range data byte range requires selected rows",
            ));
        }
        if let (Some(header), Some(data)) = (payload.header_range, payload.data_range) {
            if header.start >= header.end || data.start >= data.end || header.end > data.start {
                return Err(KernelError::invalid_params(
                    "csvRows range byte spans are invalid",
                ));
            }
        }
    }
    if payload.has_header {
        if payload.columns.is_empty() {
            return Err(KernelError::invalid_params(
                "csvRows with headers requires non-empty source columns",
            ));
        }
        if payload.column_count as usize != payload.columns.len() {
            return Err(KernelError::invalid_params(
                "csvRows columnCount must match the header width",
            ));
        }
    } else if payload.column_count == 0 {
        return Err(KernelError::invalid_params(
            "csvRows without headers requires a positive column width",
        ));
    }
    Ok(())
}

/// F07：逐个文件解码后按已解析的列序号抽取，输出保留选择顺序与原行序。
fn perform_csv_extract(
    context: &GenericApplyContext<'_>,
    source: &str,
    target: &str,
    overwrite: bool,
    payload: &PlanCsvExtract,
) -> Result<Performed, KernelError> {
    let encoding = payload.encoding.ok_or_else(|| {
        KernelError::new(
            "E3001",
            "source encoding requires manual confirmation",
            None,
            false,
        )
    })?;
    let target_encoding = TargetEncoding::parse(&payload.target_encoding)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvExtract target encoding"))?;
    let invalid = InvalidSequencePolicy::parse(&payload.invalid).ok_or_else(|| {
        KernelError::invalid_params("unsupported csvExtract invalid-sequence policy")
    })?;
    let delimiter = Delimiter::parse(&payload.delimiter)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvExtract delimiter"))?;

    let backup = if overwrite {
        Some(context.fs.backup_to_quarantine(target)?)
    } else {
        None
    };
    let data = context.fs.read_bytes(source)?;
    let decoded = transcode(
        &data,
        encoding,
        TargetEncoding::Utf8 { bom: false },
        invalid,
    )?;
    let records = parse_records(&decoded.bytes, delimiter.as_byte());
    let (has_header, rows): (bool, &[Vec<String>]) = if payload.has_header {
        let Some((header, rows)) = records.split_first() else {
            return Err(KernelError::new(
                "E2010",
                "source header changed since plan preview; re-plan before applying",
                None,
                true,
            ));
        };
        if normalized_columns(header) != payload.columns {
            return Err(KernelError::new(
                "E2010",
                "source header changed since plan preview; re-plan before applying",
                None,
                true,
            ));
        }
        (true, rows)
    } else {
        (false, &records)
    };

    let mut output = String::new();
    if has_header {
        let names: Vec<String> = payload
            .selections
            .iter()
            .map(|selection| selection.name.clone().unwrap_or_default())
            .collect();
        write_record(&names, delimiter.as_byte(), &mut output);
    }
    for row in rows {
        let fields: Vec<String> = payload
            .selections
            .iter()
            .map(|selection| {
                row.get(selection.index as usize)
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        write_record(&fields, delimiter.as_byte(), &mut output);
    }

    let transcoded = transcode(
        output.as_bytes(),
        SourceEncoding::Utf8,
        target_encoding,
        invalid,
    )?;
    context.fs.write_atomic(target, &transcoded.bytes)?;
    let final_path = target.to_string();
    let reverse = match backup {
        Some(backup) => FsOp::WriteAtomic {
            path: final_path.clone(),
            data_ref: Some(backup),
        },
        None => FsOp::Quarantine {
            from: final_path.clone(),
            to: context
                .fs
                .quarantine_target_path(&file_name_of(&final_path))?,
        },
    };
    Ok(Performed {
        skipped: false,
        replaced: overwrite,
        final_path: final_path.clone(),
        forward: FsOp::WriteAtomic {
            path: final_path,
            data_ref: None,
        },
        reverse,
    })
}

/// F09：只读取表头与选中的字节区间，解码/转码后原子写出。
fn perform_csv_range(
    context: &GenericApplyContext<'_>,
    source: &str,
    target: &str,
    overwrite: bool,
    payload: &PlanCsvRows,
) -> Result<Performed, KernelError> {
    let encoding = payload.encoding.ok_or_else(|| {
        KernelError::new(
            "E3001",
            "source encoding requires manual confirmation",
            None,
            false,
        )
    })?;
    let target_encoding = TargetEncoding::parse(&payload.target_encoding)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvRows target encoding"))?;
    let invalid = InvalidSequencePolicy::parse(&payload.invalid).ok_or_else(|| {
        KernelError::invalid_params("unsupported csvRows invalid-sequence policy")
    })?;
    let delimiter = Delimiter::parse(&payload.delimiter)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvRows delimiter"))?;

    let (Some(start), Some(end)) = (payload.start_row, payload.end_row) else {
        return Err(KernelError::invalid_params(
            "csvRows range mode requires startRow and endRow",
        ));
    };
    if start == 0 || end < start {
        return Err(KernelError::invalid_params(
            "csvRows range must be 1-based with endRow >= startRow",
        ));
    }
    if payload.has_header && payload.header_range.is_none() && payload.size > 0 {
        return Err(KernelError::new(
            "E2010",
            "source header range is missing; re-plan before applying",
            None,
            true,
        ));
    }
    if !payload.has_header && payload.header_range.is_some() {
        return Err(KernelError::invalid_params(
            "csvRows range without headers must not carry a header byte range",
        ));
    }
    if payload.data_range.is_none() && payload.count > 0 {
        return Err(KernelError::new(
            "E2010",
            "source data range is missing; re-plan before applying",
            None,
            true,
        ));
    }
    if payload.data_range.is_some() && payload.count == 0 {
        return Err(KernelError::invalid_params(
            "csvRows range data byte range requires selected rows",
        ));
    }
    if let (Some(header), Some(data)) = (payload.header_range, payload.data_range) {
        if header.start >= header.end || data.start >= data.end || header.end > data.start {
            return Err(KernelError::new(
                "E2010",
                "source byte ranges changed since planning; re-plan before applying",
                None,
                true,
            ));
        }
    }

    let backup = if overwrite {
        Some(context.fs.backup_to_quarantine(target)?)
    } else {
        None
    };

    let mut raw = Vec::new();
    if let Some(header) = payload.header_range {
        let header_raw = context
            .fs
            .read_ranges(source, &[(header.start, header.end)])?;
        let decoded = transcode(
            &header_raw,
            encoding,
            TargetEncoding::Utf8 { bom: false },
            invalid,
        )?;
        let records = parse_records(&decoded.bytes, delimiter.as_byte());
        let fields = records.first().ok_or_else(|| {
            KernelError::new(
                "E2010",
                "source header changed since plan preview; re-plan before applying",
                None,
                true,
            )
        })?;
        if normalized_columns(fields) != payload.columns {
            return Err(KernelError::new(
                "E2010",
                "source header changed since plan preview; re-plan before applying",
                None,
                true,
            ));
        }
        raw.extend_from_slice(&header_raw);
    }
    if let Some(data) = payload.data_range {
        raw.extend_from_slice(&context.fs.read_ranges(source, &[(data.start, data.end)])?);
    }

    let transcoded = transcode(&raw, encoding, target_encoding, invalid)?;
    context.fs.write_atomic(target, &transcoded.bytes)?;
    let final_path = target.to_string();
    let reverse = match backup {
        Some(backup) => FsOp::WriteAtomic {
            path: final_path.clone(),
            data_ref: Some(backup),
        },
        None => FsOp::Quarantine {
            from: final_path.clone(),
            to: context
                .fs
                .quarantine_target_path(&file_name_of(&final_path))?,
        },
    };
    Ok(Performed {
        skipped: false,
        replaced: overwrite,
        final_path: final_path.clone(),
        forward: FsOp::WriteAtomic {
            path: final_path,
            data_ref: None,
        },
        reverse,
    })
}

/// F08：解码后按前 N / 后 N 选择数据行，表头与行内容原样输出。
fn perform_csv_rows(
    context: &GenericApplyContext<'_>,
    source: &str,
    target: &str,
    overwrite: bool,
    payload: &PlanCsvRows,
) -> Result<Performed, KernelError> {
    let encoding = payload.encoding.ok_or_else(|| {
        KernelError::new(
            "E3001",
            "source encoding requires manual confirmation",
            None,
            false,
        )
    })?;
    let window = RowWindow::parse(&payload.window)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvRows window"))?;
    if window == RowWindow::Range {
        return perform_csv_range(context, source, target, overwrite, payload);
    }
    let target_encoding = TargetEncoding::parse(&payload.target_encoding)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvRows target encoding"))?;
    let invalid = InvalidSequencePolicy::parse(&payload.invalid).ok_or_else(|| {
        KernelError::invalid_params("unsupported csvRows invalid-sequence policy")
    })?;
    let delimiter = Delimiter::parse(&payload.delimiter)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvRows delimiter"))?;

    let backup = if overwrite {
        Some(context.fs.backup_to_quarantine(target)?)
    } else {
        None
    };
    let data = context.fs.read_bytes(source)?;
    let decoded = transcode(
        &data,
        encoding,
        TargetEncoding::Utf8 { bom: false },
        invalid,
    )?;
    let records = parse_records(&decoded.bytes, delimiter.as_byte());
    let (has_header, rows): (bool, &[Vec<String>]) = if payload.has_header {
        let Some((header, rows)) = records.split_first() else {
            return Err(KernelError::new(
                "E2010",
                "source header changed since plan preview; re-plan before applying",
                None,
                true,
            ));
        };
        if normalized_columns(header) != payload.columns {
            return Err(KernelError::new(
                "E2010",
                "source header changed since plan preview; re-plan before applying",
                None,
                true,
            ));
        }
        (true, rows)
    } else {
        (false, &records)
    };

    let selected = select_row_window(rows, window, payload.count);
    let mut output = String::new();
    if has_header {
        let names: Vec<String> = payload.columns.clone();
        write_record(&names, delimiter.as_byte(), &mut output);
    }
    for row in &selected {
        write_record(row, delimiter.as_byte(), &mut output);
    }

    let transcoded = transcode(
        output.as_bytes(),
        SourceEncoding::Utf8,
        target_encoding,
        invalid,
    )?;
    context.fs.write_atomic(target, &transcoded.bytes)?;
    let final_path = target.to_string();
    let reverse = match backup {
        Some(backup) => FsOp::WriteAtomic {
            path: final_path.clone(),
            data_ref: Some(backup),
        },
        None => FsOp::Quarantine {
            from: final_path.clone(),
            to: context
                .fs
                .quarantine_target_path(&file_name_of(&final_path))?,
        },
    };
    Ok(Performed {
        skipped: false,
        replaced: overwrite,
        final_path: final_path.clone(),
        forward: FsOp::WriteAtomic {
            path: final_path,
            data_ref: None,
        },
        reverse,
    })
}

fn perform_csv_merge(
    context: &GenericApplyContext<'_>,
    target: &str,
    overwrite: bool,
    payload: &PlanCsvMerge,
) -> Result<Performed, KernelError> {
    let target_encoding = TargetEncoding::parse(&payload.target_encoding)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvMerge target encoding"))?;
    let invalid = InvalidSequencePolicy::parse(&payload.invalid).ok_or_else(|| {
        KernelError::invalid_params("unsupported csvMerge invalid-sequence policy")
    })?;
    let output_delimiter = Delimiter::parse(&payload.delimiter)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvMerge delimiter"))?;
    let backup = if overwrite {
        Some(context.fs.backup_to_quarantine(target)?)
    } else {
        None
    };

    let mut output = String::new();
    if payload.has_header {
        let mut header: Vec<String> = Vec::new();
        if let Some(source_column) = &payload.source_column {
            header.push(source_column.clone());
        }
        header.extend(payload.union.iter().cloned());
        write_record(&header, output_delimiter.as_byte(), &mut output);
    }
    let union_keys = if payload.has_header {
        Some(column_keys(&payload.union))
    } else {
        None
    };

    for file in &payload.files {
        let encoding = file.encoding.ok_or_else(|| {
            KernelError::new(
                "E3001",
                format!(
                    "source encoding requires manual confirmation: {}",
                    file.path
                ),
                None,
                false,
            )
        })?;
        let file_delimiter = Delimiter::parse(&file.delimiter).ok_or_else(|| {
            KernelError::invalid_params(format!("unsupported source delimiter for {}", file.path))
        })?;
        let data = context.fs.read_bytes(&file.path)?;
        if data.len() as u64 != file.size {
            return Err(KernelError::new(
                "E2010",
                format!("source size changed since plan preview: {}", file.path),
                None,
                false,
            ));
        }
        let decoded = transcode(
            &data,
            encoding,
            TargetEncoding::Utf8 { bom: false },
            invalid,
        )?;
        let records = parse_records(&decoded.bytes, file_delimiter.as_byte());
        let (header, rows): (&[String], &[Vec<String>]) = if payload.has_header {
            match records.split_first() {
                Some((header, rows)) => (header, rows),
                None => (&[], &[]),
            }
        } else {
            (&[], &records)
        };
        if payload.has_header {
            let normalized = normalized_columns(header);
            if normalized != file.columns {
                return Err(KernelError::new(
                    "E2010",
                    format!("source header changed since plan preview: {}", file.path),
                    None,
                    false,
                ));
            }
        }
        let field_indexes = union_keys.as_ref().map(|union| {
            let header_keys = column_keys(&normalized_columns(header));
            union
                .iter()
                .map(|key| header_keys.iter().position(|header_key| header_key == key))
                .collect::<Vec<_>>()
        });

        for row in rows {
            let mut fields: Vec<String> = Vec::new();
            if payload.source_column.is_some() {
                let label = file.source_label.as_deref().ok_or_else(|| {
                    KernelError::invalid_params(format!(
                        "csvMerge source label is missing: {}",
                        file.path
                    ))
                })?;
                fields.push(label.to_string());
            }
            match &field_indexes {
                Some(indexes) => {
                    for position in indexes {
                        fields.push(
                            position
                                .and_then(|index| row.get(index))
                                .cloned()
                                .unwrap_or_default(),
                        );
                    }
                }
                None => {
                    for index in 0..payload.positional_columns as usize {
                        fields.push(row.get(index).cloned().unwrap_or_default());
                    }
                }
            }
            let known = if payload.has_header {
                header.len()
            } else {
                payload.positional_columns as usize
            };
            if row.len() > known {
                fields.extend(row[known..].iter().cloned());
            }
            write_record(&fields, output_delimiter.as_byte(), &mut output);
        }
    }

    let encoded = transcode(
        output.as_bytes(),
        SourceEncoding::Utf8,
        target_encoding,
        invalid,
    )?;
    context.fs.write_atomic(target, &encoded.bytes)?;
    let final_path = target.to_string();
    let reverse = match backup {
        Some(backup) => FsOp::WriteAtomic {
            path: final_path.clone(),
            data_ref: Some(backup),
        },
        None => FsOp::Quarantine {
            from: final_path.clone(),
            to: context
                .fs
                .quarantine_target_path(&file_name_of(&final_path))?,
        },
    };
    Ok(Performed {
        skipped: false,
        replaced: overwrite,
        final_path: final_path.clone(),
        forward: FsOp::WriteAtomic {
            path: final_path,
            data_ref: None,
        },
        reverse,
    })
}

fn protected_target_hit(context: &GenericApplyContext<'_>, target: &str) -> Option<KernelError> {
    if context.protected_dirs.is_empty() {
        return None;
    }
    let normalized = match try_normalize(target) {
        Ok(normalized) => normalized,
        Err(error) => return Some(error),
    };
    let canonical = context.fs.canonicalize(target).ok();
    protected_by(
        &normalized,
        canonical.as_deref(),
        &context.protected_dirs,
        context.case_sense,
    )
    .map(|hit| {
        KernelError::new(
            "E2009",
            format!("target is under protected directory {hit}"),
            None,
            false,
        )
    })
}

fn is_cancelled(context: &GenericApplyContext<'_>) -> bool {
    context
        .cancel
        .map(CancelToken::is_cancelled)
        .unwrap_or(false)
}

fn report_progress(
    context: &GenericApplyContext<'_>,
    phase: ProgressPhase,
    done: u64,
    total: u64,
    message: Option<&str>,
) {
    if let Some(sink) = context.progress {
        sink.report(crate::progress::ProgressEvent {
            task_id: context.task_id.clone(),
            phase,
            done,
            total,
            message: message.map(str::to_string),
        });
    }
}

fn skipped_entry(entry_id: String, error: KernelError) -> ApplyEntryReport {
    ApplyEntryReport {
        entry_id,
        status: ApplyStatus::Skipped,
        error: Some(error),
    }
}

fn failed_entry(entry_id: String, error: KernelError) -> ApplyEntryReport {
    ApplyEntryReport {
        entry_id,
        status: ApplyStatus::Failed,
        error: Some(error),
    }
}

fn mark_entry_error(reports: &mut [ApplyEntryReport], index: usize, error: KernelError) {
    if let Some(report) = reports.get_mut(index) {
        report.status = ApplyStatus::Failed;
        report.error = Some(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::{ApplyOptions, OnConflict};
    use crate::encoding::{SourceEncoding, TargetEncoding};
    use crate::fs::{DirEntryInfo, EntryAttrs, PathOutcome, ScanFilter, StatInfo};
    use crate::journal::{JournalEntry, JournalSink, JournalStatus};
    use crate::plan::{
        PlanCsvExtract, PlanCsvExtractColumn, PlanCsvMerge, PlanCsvMergeFile, PlanCsvRows,
        PlanEntry, PlanExpected, PlanMerge, PlanMergeSource, PlanRange, PlanStats,
    };
    use crate::progress::{CancelToken, ProgressEvent, ProgressSink};
    use crate::split::{plan_f01, PlanF01File, PlanF01Input, SplitAccumulator};
    use std::collections::{BTreeMap, HashMap};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Mutex;

    const PLAN_ID: &str = "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8";

    #[derive(Default)]
    struct TestFs {
        files: Mutex<HashMap<String, Vec<u8>>>,
        next: Mutex<u64>,
        read_bytes_calls: AtomicU64,
    }

    impl TestFs {
        fn with_files(paths: &[&str]) -> Self {
            let fs = Self::default();
            {
                let mut files = fs.files.lock().unwrap();
                for path in paths {
                    files.insert((*path).to_string(), b"content".to_vec());
                }
            }
            fs
        }

        fn exists(&self, path: &str) -> bool {
            self.files.lock().unwrap().contains_key(path)
        }

        fn write_file(&self, path: &str, data: &[u8]) {
            self.files
                .lock()
                .unwrap()
                .insert(path.to_string(), data.to_vec());
        }

        fn read_file(&self, path: &str) -> Vec<u8> {
            self.files.lock().unwrap().get(path).cloned().unwrap()
        }

        fn read_bytes_calls(&self) -> u64 {
            self.read_bytes_calls.load(Ordering::Relaxed)
        }
    }

    impl FsAdapter for TestFs {
        fn read_bytes(&self, path: &str) -> Result<Vec<u8>, KernelError> {
            self.read_bytes_calls.fetch_add(1, Ordering::Relaxed);
            self.files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))
        }

        fn write_atomic(&self, path: &str, data: &[u8]) -> Result<(), KernelError> {
            self.files
                .lock()
                .unwrap()
                .insert(path.to_string(), data.to_vec());
            Ok(())
        }

        fn rename(
            &self,
            from: &str,
            to: &str,
            policy: ConflictPolicy,
        ) -> Result<PathOutcome, KernelError> {
            let mut files = self.files.lock().unwrap();
            let data = files
                .remove(from)
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))?;
            if files.contains_key(to) {
                if policy == ConflictPolicy::Overwrite {
                    files.insert(to.to_string(), data);
                    return Ok(PathOutcome {
                        outcome: PathOutcomeKind::Replaced,
                        final_path: to.to_string(),
                    });
                }
                files.insert(from.to_string(), data);
                return Ok(PathOutcome {
                    outcome: PathOutcomeKind::Skipped,
                    final_path: to.to_string(),
                });
            }
            files.insert(to.to_string(), data);
            Ok(PathOutcome {
                outcome: PathOutcomeKind::Applied,
                final_path: to.to_string(),
            })
        }

        fn copy(
            &self,
            from: &str,
            to: &str,
            policy: ConflictPolicy,
        ) -> Result<PathOutcome, KernelError> {
            let mut files = self.files.lock().unwrap();
            let data = files
                .get(from)
                .cloned()
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))?;
            if files.contains_key(to) {
                if policy == ConflictPolicy::Overwrite {
                    files.insert(to.to_string(), data);
                    return Ok(PathOutcome {
                        outcome: PathOutcomeKind::Replaced,
                        final_path: to.to_string(),
                    });
                }
                return Ok(PathOutcome {
                    outcome: PathOutcomeKind::Skipped,
                    final_path: to.to_string(),
                });
            }
            files.insert(to.to_string(), data);
            Ok(PathOutcome {
                outcome: PathOutcomeKind::Applied,
                final_path: to.to_string(),
            })
        }

        fn move_path(
            &self,
            from: &str,
            to: &str,
            policy: ConflictPolicy,
        ) -> Result<PathOutcome, KernelError> {
            self.rename(from, to, policy)
        }

        fn copy_range_atomic(
            &self,
            source: &str,
            start: u64,
            end: u64,
            target: &str,
            policy: ConflictPolicy,
        ) -> Result<PathOutcome, KernelError> {
            let mut files = self.files.lock().unwrap();
            let data = files
                .get(source)
                .cloned()
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))?;
            if start >= end || end > data.len() as u64 {
                return Err(KernelError::new(
                    "E2010",
                    "range is out of bounds",
                    None,
                    false,
                ));
            }
            let slice = data[start as usize..end as usize].to_vec();
            if files.contains_key(target) {
                if policy != ConflictPolicy::Overwrite {
                    return Ok(PathOutcome {
                        outcome: PathOutcomeKind::Skipped,
                        final_path: target.to_string(),
                    });
                }
                files.insert(target.to_string(), slice);
                return Ok(PathOutcome {
                    outcome: PathOutcomeKind::Replaced,
                    final_path: target.to_string(),
                });
            }
            files.insert(target.to_string(), slice);
            Ok(PathOutcome {
                outcome: PathOutcomeKind::Applied,
                final_path: target.to_string(),
            })
        }

        fn read_ranges(&self, source: &str, ranges: &[(u64, u64)]) -> Result<Vec<u8>, KernelError> {
            let files = self.files.lock().unwrap();
            let data = files
                .get(source)
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))?;
            let mut joined = Vec::new();
            for (start, end) in ranges {
                if start >= end || *end > data.len() as u64 {
                    return Err(KernelError::new(
                        "E2010",
                        "range is out of bounds",
                        None,
                        false,
                    ));
                }
                joined.extend_from_slice(&data[*start as usize..*end as usize]);
            }
            Ok(joined)
        }

        fn copy_ranges_atomic(
            &self,
            source: &str,
            ranges: &[(u64, u64)],
            target: &str,
            policy: ConflictPolicy,
        ) -> Result<PathOutcome, KernelError> {
            let mut files = self.files.lock().unwrap();
            let data = files
                .get(source)
                .cloned()
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))?;
            let mut joined = Vec::new();
            for (start, end) in ranges {
                if start >= end || *end > data.len() as u64 {
                    return Err(KernelError::new(
                        "E2010",
                        "range is out of bounds",
                        None,
                        false,
                    ));
                }
                joined.extend_from_slice(&data[*start as usize..*end as usize]);
            }
            if files.contains_key(target) {
                if policy != ConflictPolicy::Overwrite {
                    return Ok(PathOutcome {
                        outcome: PathOutcomeKind::Skipped,
                        final_path: target.to_string(),
                    });
                }
                files.insert(target.to_string(), joined);
                return Ok(PathOutcome {
                    outcome: PathOutcomeKind::Replaced,
                    final_path: target.to_string(),
                });
            }
            files.insert(target.to_string(), joined);
            Ok(PathOutcome {
                outcome: PathOutcomeKind::Applied,
                final_path: target.to_string(),
            })
        }

        fn backup_to_quarantine(&self, path: &str) -> Result<String, KernelError> {
            let mut files = self.files.lock().unwrap();
            let data = files
                .get(path)
                .cloned()
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))?;
            let mut next = self.next.lock().unwrap();
            *next += 1;
            let backup = format!("/quarantine/{next}.bak");
            files.insert(backup.clone(), data);
            Ok(backup)
        }

        fn quarantine_target_path(&self, file_name: &str) -> Result<String, KernelError> {
            let mut next = self.next.lock().unwrap();
            *next += 1;
            Ok(format!("/quarantine/{file_name}.{next}"))
        }

        fn remove_to_quarantine(&self, path: &str) -> Result<String, KernelError> {
            let mut files = self.files.lock().unwrap();
            let data = files
                .remove(path)
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))?;
            let mut next = self.next.lock().unwrap();
            *next += 1;
            let destination = format!("/quarantine/{next}");
            files.insert(destination.clone(), data);
            Ok(destination)
        }

        fn list_dir(
            &self,
            _path: &str,
            _filter: &ScanFilter,
        ) -> Result<Vec<DirEntryInfo>, KernelError> {
            Ok(Vec::new())
        }

        fn stat(&self, path: &str) -> Result<StatInfo, KernelError> {
            let files = self.files.lock().unwrap();
            let data = files
                .get(path)
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))?;
            Ok(StatInfo {
                kind: EntryKind::File,
                size: data.len() as u64,
                mtime: "2026-10-29T00:00:00Z".to_string(),
                attrs: EntryAttrs {
                    readonly: false,
                    hidden: false,
                    placeholder: false,
                },
            })
        }

        fn canonicalize(&self, path: &str) -> Result<String, KernelError> {
            Ok(path.to_string())
        }

        fn case_sensitivity(&self) -> CaseSense {
            CaseSense::Sensitive
        }
    }

    #[derive(Default)]
    struct TestJournal {
        entries: Vec<JournalEntry>,
    }

    impl JournalSink for TestJournal {
        fn append(
            &mut self,
            plan_id: &str,
            entry_id: &str,
            ts: &str,
            forward: FsOp,
            reverse: FsOp,
        ) -> Result<JournalEntry, KernelError> {
            let seq = self.entries.len() as u64 + 1;
            let entry = JournalEntry {
                seq,
                plan_id: plan_id.to_string(),
                entry_id: entry_id.to_string(),
                ts: ts.to_string(),
                plan_kind: None,
                forward,
                reverse,
                status: JournalStatus::Applied,
            };
            self.entries.push(entry.clone());
            Ok(entry)
        }

        fn append_with_kind(
            &mut self,
            plan_id: &str,
            entry_id: &str,
            ts: &str,
            plan_kind: &str,
            forward: FsOp,
            reverse: FsOp,
        ) -> Result<JournalEntry, KernelError> {
            let seq = self.entries.len() as u64 + 1;
            let entry = JournalEntry {
                seq,
                plan_id: plan_id.to_string(),
                entry_id: entry_id.to_string(),
                ts: ts.to_string(),
                plan_kind: Some(plan_kind.to_string()),
                forward,
                reverse,
                status: JournalStatus::Applied,
            };
            self.entries.push(entry.clone());
            Ok(entry)
        }
    }

    impl JournalStore for TestJournal {
        fn entries_for_plan(&self, plan_id: &str) -> Result<Vec<JournalEntry>, KernelError> {
            Ok(self
                .entries
                .iter()
                .filter(|entry| entry.plan_id == plan_id)
                .cloned()
                .collect())
        }

        fn mark_reverted(&mut self, plan_id: &str, seqs: &[u64]) -> Result<(), KernelError> {
            for entry in self.entries.iter_mut() {
                if entry.plan_id == plan_id && seqs.contains(&entry.seq) {
                    entry.status = JournalStatus::Reverted;
                }
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct Ports {
        cancel: AtomicBool,
        cancel_after: Mutex<Option<u64>>,
    }

    impl CancelToken for Ports {
        fn is_cancelled(&self) -> bool {
            self.cancel.load(Ordering::SeqCst)
        }
    }

    impl ProgressSink for Ports {
        fn report(&self, event: ProgressEvent) {
            if let Some(after) = *self.cancel_after.lock().unwrap() {
                if event.done >= after {
                    self.cancel.store(true, Ordering::SeqCst);
                }
            }
        }
    }

    fn action_entry(
        entry_id: &str,
        source: &str,
        target: Option<&str>,
        action: PlanAction,
    ) -> PlanEntry {
        PlanEntry {
            entry_id: entry_id.to_string(),
            source: source.to_string(),
            target: target.map(str::to_string),
            action,
            relation: None,
            reason: "test".to_string(),
            conflict: None,
            resolution: None,
            blocked: None,
            expected: None,
            range: None,
            merge: None,
            csv: None,
            csv_merge: None,
            csv_extract: None,
            csv_rows: None,
        }
    }

    fn plan(entries: Vec<PlanEntry>) -> PlanDocument {
        PlanDocument {
            plan_id: PLAN_ID.to_string(),
            kind: "c05".to_string(),
            params: BTreeMap::new(),
            entries,
            stats: PlanStats::default(),
            warnings: Vec::new(),
        }
    }

    fn context<'a>(
        fs: &'a TestFs,
        journal: &'a mut TestJournal,
        license: &'a dyn crate::license::LicenseService,
    ) -> GenericApplyContext<'a> {
        GenericApplyContext {
            fs,
            license,
            journal,
            task_id: "task-1".to_string(),
            now_rfc3339: "2026-10-29T00:00:00Z".to_string(),
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            progress: None,
            cancel: None,
            content_editor: None,
            directory_lister: None,
        }
    }

    #[test]
    fn rename_applies_and_writes_a_reversible_journal_entry() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan(vec![action_entry(
                "e1",
                "/data/a.txt",
                Some("/data/b.txt"),
                PlanAction::Rename,
            )]),
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);
        assert!(!fs.exists("/data/a.txt"));
        assert!(fs.exists("/data/b.txt"));
        assert_eq!(journal.entries.len(), 1);
        assert_eq!(
            journal.entries[0].reverse,
            FsOp::Rename {
                from: "/data/b.txt".to_string(),
                to: "/data/a.txt".to_string()
            }
        );
    }

    #[test]
    fn move_and_delete_use_their_own_operations() {
        let fs = TestFs::with_files(&["/data/a.txt", "/data/old.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan(vec![
                action_entry(
                    "e1",
                    "/data/a.txt",
                    Some("/archive/a.txt"),
                    PlanAction::Move,
                ),
                action_entry("e2", "/data/old.txt", None, PlanAction::Delete),
            ]),
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert!(fs.exists("/archive/a.txt"));
        assert!(!fs.exists("/data/old.txt"));
        assert_eq!(journal.entries.len(), 2);
        assert!(matches!(
            journal.entries[1].forward,
            FsOp::Quarantine { .. }
        ));
    }

    #[test]
    fn split_plan_writes_line_aligned_parts_and_undo_quarantines_them() {
        let fs = TestFs::default();
        fs.write_file("/data/big.txt", b"aaa\nbbb\nccc\n");
        let mut accumulator = SplitAccumulator::new(6).unwrap();
        for _ in 0..3 {
            accumulator.push_line(4, true);
        }
        let layout = accumulator.finish(12);
        let plan = plan_f01(PlanF01Input {
            plan_id: PLAN_ID.to_string(),
            target_bytes: 6,
            output_dir: "/out".to_string(),
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            existing_targets: Vec::new(),
            files: vec![PlanF01File {
                path: "/data/big.txt".to_string(),
                canonical_path: None,
                group_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_string(),
                layout,
            }],
        })
        .unwrap();

        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(outcome.report.entries.len(), 2);
        assert_eq!(fs.read_file("/out/big.1.txt"), b"aaa\nbbb\n");
        assert_eq!(fs.read_file("/out/big.2.txt"), b"ccc\n");
        assert_eq!(journal.entries.len(), 2);
        assert!(matches!(
            &journal.entries[0].forward,
            FsOp::WriteAtomic { path, data_ref: None } if path == "/out/big.1.txt"
        ));
        assert!(matches!(
            &journal.entries[0].reverse,
            FsOp::Quarantine { from, .. } if from == "/out/big.1.txt"
        ));

        let undo = crate::apply::undo_plan_with_ports(
            PLAN_ID,
            &fs,
            &license,
            &mut journal,
            crate::apply::UndoPorts::default(),
        )
        .unwrap();
        assert_eq!(undo.reverted, 2);
        assert!(!fs.exists("/out/big.1.txt"), "undo quarantines the part");
        assert!(!fs.exists("/out/big.2.txt"), "undo quarantines the part");
        assert!(fs.exists("/data/big.txt"), "split keeps the source");
    }

    #[test]
    fn merge_transcodes_inputs_in_order_and_undo_removes_the_output() {
        let fs = TestFs::default();
        fs.write_file("/data/a.txt", b"A\n");
        let gb_bytes = encoding_rs::GB18030.encode("中\n").0.to_vec();
        fs.write_file("/data/b.txt", &gb_bytes);
        let plan = plan(vec![merge_entry(
            "e1",
            "/out/all.txt",
            TargetEncoding::Utf8 { bom: false },
            vec![
                ("/data/a.txt", SourceEncoding::Utf8, 2),
                (
                    "/data/b.txt",
                    SourceEncoding::Gb18030,
                    gb_bytes.len() as u64,
                ),
            ],
        )]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);
        assert_eq!(fs.read_file("/out/all.txt"), "A\n中\n".as_bytes());
        assert_eq!(journal.entries.len(), 1);
        assert!(matches!(
            &journal.entries[0].forward,
            FsOp::WriteAtomic { path, data_ref: None } if path == "/out/all.txt"
        ));
        assert!(matches!(
            &journal.entries[0].reverse,
            FsOp::Quarantine { from, .. } if from == "/out/all.txt"
        ));

        let undo = crate::apply::undo_plan_with_ports(
            PLAN_ID,
            &fs,
            &license,
            &mut journal,
            crate::apply::UndoPorts::default(),
        )
        .unwrap();
        assert_eq!(undo.reverted, 1);
        assert!(!fs.exists("/out/all.txt"));
        assert!(fs.exists("/data/a.txt"));
        assert!(fs.exists("/data/b.txt"));
    }

    #[test]
    fn merge_writes_the_bom_only_once() {
        let fs = TestFs::default();
        fs.write_file("/data/a.txt", b"A\n");
        fs.write_file("/data/b.txt", b"B\n");
        let plan = plan(vec![merge_entry(
            "e1",
            "/out/all.txt",
            TargetEncoding::Utf8 { bom: true },
            vec![
                ("/data/a.txt", SourceEncoding::Utf8, 2),
                ("/data/b.txt", SourceEncoding::Utf8, 2),
            ],
        )]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        apply_generic_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        let bytes = fs.read_file("/out/all.txt");
        assert_eq!(bytes, b"\xEF\xBB\xBFA\nB\n");
        assert_eq!(
            bytes
                .windows(3)
                .filter(|window| *window == [0xEF, 0xBB, 0xBF])
                .count(),
            1,
            "BOM must appear exactly once at the start"
        );
    }

    #[test]
    fn merge_skips_when_a_source_size_changed_since_plan() {
        let fs = TestFs::default();
        fs.write_file("/data/a.txt", b"A\n");
        fs.write_file("/data/b.txt", b"BB\n");
        let plan = plan(vec![merge_entry(
            "e1",
            "/out/all.txt",
            TargetEncoding::Utf8 { bom: false },
            vec![
                ("/data/a.txt", SourceEncoding::Utf8, 2),
                // 计划期记录 4 字节，执行期实际 3 字节。
                ("/data/b.txt", SourceEncoding::Utf8, 4),
            ],
        )]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Skipped);
        assert_eq!(
            outcome.report.entries[0].error.as_ref().unwrap().code,
            "E2010"
        );
        assert!(!fs.exists("/out/all.txt"));
        assert!(journal.entries.is_empty());
    }

    #[test]
    fn merge_does_not_overwrite_an_existing_target_by_default() {
        let fs = TestFs::default();
        fs.write_file("/data/a.txt", b"A\n");
        fs.write_file("/out/all.txt", b"old");
        let plan = plan(vec![merge_entry(
            "e1",
            "/out/all.txt",
            TargetEncoding::Utf8 { bom: false },
            vec![("/data/a.txt", SourceEncoding::Utf8, 2)],
        )]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Skipped);
        assert_eq!(
            outcome.report.entries[0].error.as_ref().unwrap().code,
            "E2007"
        );
        assert_eq!(fs.read_file("/out/all.txt"), b"old".to_vec());
        assert!(journal.entries.is_empty());
    }

    #[test]
    fn merge_rejects_an_ambiguous_source_encoding() {
        let fs = TestFs::default();
        fs.write_file("/data/a.txt", b"A\n");
        let base = plan(vec![merge_entry(
            "e1",
            "/out/all.txt",
            TargetEncoding::Utf8 { bom: false },
            vec![("/data/a.txt", SourceEncoding::Utf8, 2)],
        )]);
        let mut entry = base.entries.into_iter().next().unwrap();
        entry.merge = Some(PlanMerge {
            sources: vec![PlanMergeSource {
                path: "/data/a.txt".to_string(),
                encoding: None,
                size: 2,
            }],
            target_encoding: "utf-8".to_string(),
            invalid: "replace-and-count".to_string(),
        });
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan(vec![entry]),
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Failed);
        assert_eq!(
            outcome.report.entries[0].error.as_ref().unwrap().code,
            "E3001"
        );
        assert!(!fs.exists("/out/all.txt"));
    }

    #[test]
    fn csv_merge_aligns_columns_and_prepends_source_column() {
        let fs = TestFs::default();
        fs.write_file("/data/a.csv", b"id,name\n1,Alice\n");
        fs.write_file("/data/b.csv", b"name,extra\nBob,x\n");
        let plan = plan(vec![csv_merge_entry(
            "e1",
            "/out/all.csv",
            vec![
                csv_file("/data/a.csv", 16, &["id", "name"], Some("a.csv")),
                csv_file("/data/b.csv", 17, &["name", "extra"], Some("b.csv")),
            ],
            vec!["id", "name", "extra"],
            true,
            3,
            Some("来源文件"),
        )]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);
        assert_eq!(
            fs.read_file("/out/all.csv"),
            "来源文件,id,name,extra\na.csv,1,Alice,\nb.csv,,Bob,x\n".as_bytes()
        );
        assert!(matches!(
            journal.entries[0].reverse,
            FsOp::Quarantine { .. }
        ));
    }

    #[test]
    fn csv_merge_maps_by_header_name_not_position() {
        let fs = TestFs::default();
        fs.write_file("/data/a.csv", b"id,name\n1,Alice\n");
        fs.write_file("/data/b.csv", b"name,id\nBob,2\n");
        let plan = plan(vec![csv_merge_entry(
            "e1",
            "/out/all.csv",
            vec![
                csv_file("/data/a.csv", 16, &["id", "name"], None),
                csv_file("/data/b.csv", 14, &["name", "id"], None),
            ],
            vec!["id", "name"],
            true,
            2,
            None,
        )]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);
        assert_eq!(
            fs.read_file("/out/all.csv"),
            "id,name\n1,Alice\n2,Bob\n".as_bytes()
        );
    }

    #[test]
    fn csv_merge_without_headers_aligns_by_position() {
        let fs = TestFs::default();
        fs.write_file("/data/a.csv", b"1,2\n");
        fs.write_file("/data/b.csv", b"a,b,c\n");
        let plan = plan(vec![csv_merge_entry(
            "e1",
            "/out/all.csv",
            vec![
                PlanCsvMergeFile {
                    columns: Vec::new(),
                    ..csv_file("/data/a.csv", 4, &[], None)
                },
                PlanCsvMergeFile {
                    columns: Vec::new(),
                    ..csv_file("/data/b.csv", 6, &[], None)
                },
            ],
            Vec::new(),
            false,
            3,
            None,
        )]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);
        assert_eq!(fs.read_file("/out/all.csv"), b"1,2,\na,b,c\n");
    }

    #[test]
    fn csv_merge_rejects_a_header_changed_since_plan() {
        let fs = TestFs::default();
        fs.write_file("/data/a.csv", b"id,title\n1,x\n");
        let plan = plan(vec![csv_merge_entry(
            "e1",
            "/out/all.csv",
            vec![csv_file("/data/a.csv", 13, &["id", "name"], None)],
            vec!["id", "name"],
            true,
            2,
            None,
        )]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Failed);
        assert_eq!(
            outcome.report.entries[0].error.as_ref().unwrap().code,
            "E2010"
        );
        assert!(!fs.exists("/out/all.csv"));
        assert!(journal.entries.is_empty());
    }

    fn csv_file(path: &str, size: u64, columns: &[&str], label: Option<&str>) -> PlanCsvMergeFile {
        PlanCsvMergeFile {
            path: path.to_string(),
            encoding: Some(SourceEncoding::Utf8),
            size,
            delimiter: ",".to_string(),
            columns: columns.iter().map(|value| value.to_string()).collect(),
            source_label: label.map(str::to_string),
            dirty_rows: 0,
        }
    }

    #[test]
    fn csv_extract_selects_columns_in_order_and_undo_removes_the_output() {
        let fs = TestFs::default();
        fs.write_file("/data/a.csv", b"id,name,age\n1,Alice,30\n2,Bob,25\n");
        let plan = plan(vec![csv_extract_entry(
            "e1",
            "/data/a.csv",
            "/out/subset.csv",
            32,
            true,
            &["id", "name", "age"],
            3,
            vec![(2, Some("age")), (0, Some("id"))],
        )]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(fs.read_file("/out/subset.csv"), b"age,id\n30,1\n25,2\n");
        assert_eq!(
            fs.read_file("/data/a.csv"),
            b"id,name,age\n1,Alice,30\n2,Bob,25\n"
        );

        let undo = crate::apply::undo_plan_with_ports(
            PLAN_ID,
            &fs,
            &license,
            &mut journal,
            crate::apply::UndoPorts::default(),
        )
        .unwrap();
        assert_eq!(undo.reverted, 1);
        assert!(!fs.exists("/out/subset.csv"));
        assert!(fs.exists("/data/a.csv"));
    }

    #[test]
    fn csv_extract_without_headers_uses_column_positions() {
        let fs = TestFs::default();
        fs.write_file("/data/a.csv", b"1,Alice,30\n2,Bob,25\n");
        let plan = plan(vec![csv_extract_entry(
            "e1",
            "/data/a.csv",
            "/out/subset.csv",
            20,
            false,
            &[],
            3,
            vec![(1, None)],
        )]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        apply_generic_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(fs.read_file("/out/subset.csv"), b"Alice\nBob\n");
    }

    #[test]
    fn csv_extract_rejects_a_header_changed_since_plan() {
        let fs = TestFs::default();
        fs.write_file("/data/a.csv", b"id,city\n1,NY\n");
        let plan = plan(vec![csv_extract_entry(
            "e1",
            "/data/a.csv",
            "/out/subset.csv",
            13,
            true,
            &["id", "name"],
            2,
            vec![(1, Some("name"))],
        )]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Failed);
        assert_eq!(
            outcome.report.entries[0].error.as_ref().unwrap().code,
            "E2010"
        );
        assert!(!fs.exists("/out/subset.csv"));
        assert!(journal.entries.is_empty());
    }

    #[allow(clippy::too_many_arguments)]
    fn csv_extract_entry(
        entry_id: &str,
        source: &str,
        target: &str,
        size: u64,
        has_header: bool,
        columns: &[&str],
        column_count: u64,
        selections: Vec<(u64, Option<&str>)>,
    ) -> PlanEntry {
        PlanEntry {
            entry_id: entry_id.to_string(),
            source: source.to_string(),
            target: Some(target.to_string()),
            action: PlanAction::CsvExtract,
            relation: None,
            reason: "csv extract".to_string(),
            conflict: None,
            resolution: None,
            blocked: None,
            expected: Some(PlanExpected {
                size,
                algorithm: "size".to_string(),
                hash: None,
            }),
            range: None,
            merge: None,
            csv: None,
            csv_merge: None,
            csv_extract: Some(PlanCsvExtract {
                encoding: Some(SourceEncoding::Utf8),
                size,
                delimiter: ",".to_string(),
                has_header,
                columns: columns.iter().map(|value| value.to_string()).collect(),
                column_count,
                selections: selections
                    .into_iter()
                    .map(|(index, name)| PlanCsvExtractColumn {
                        index,
                        name: name.map(str::to_string),
                    })
                    .collect(),
                target_encoding: "utf-8".to_string(),
                invalid: "replace-and-count".to_string(),
                dirty_rows: 0,
            }),
            csv_rows: None,
        }
    }

    #[test]
    fn csv_rows_first_window_writes_rows_and_undo_removes_the_output() {
        let fs = TestFs::default();
        fs.write_file("/data/a.csv", b"id,name\n1,Alice\n2,Bob\n3,Cara\n");
        let plan = plan(vec![csv_rows_entry(
            "e1",
            "/data/a.csv",
            "/out/first.csv",
            29,
            true,
            &["id", "name"],
            2,
            3,
            "first",
            2,
        )]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(fs.read_file("/out/first.csv"), b"id,name\n1,Alice\n2,Bob\n");

        let undo = crate::apply::undo_plan_with_ports(
            PLAN_ID,
            &fs,
            &license,
            &mut journal,
            crate::apply::UndoPorts::default(),
        )
        .unwrap();
        assert_eq!(undo.reverted, 1);
        assert!(!fs.exists("/out/first.csv"));
        assert!(fs.exists("/data/a.csv"));
    }

    #[test]
    fn csv_rows_last_window_keeps_the_tail_and_clamps_to_available_rows() {
        let fs = TestFs::default();
        fs.write_file("/data/a.csv", b"id,name\n1,Alice\n2,Bob\n3,Cara\n");
        let plan = plan(vec![csv_rows_entry(
            "e1",
            "/data/a.csv",
            "/out/last.csv",
            29,
            true,
            &["id", "name"],
            2,
            3,
            "last",
            9,
        )]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        apply_generic_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(
            fs.read_file("/out/last.csv"),
            b"id,name\n1,Alice\n2,Bob\n3,Cara\n",
            "count beyond the row count writes all data rows"
        );
    }

    #[test]
    fn csv_rows_rejects_a_header_changed_since_plan() {
        let fs = TestFs::default();
        fs.write_file("/data/a.csv", b"id,title\n1,x\n");
        let plan = plan(vec![csv_rows_entry(
            "e1",
            "/data/a.csv",
            "/out/first.csv",
            13,
            true,
            &["id", "name"],
            2,
            1,
            "first",
            1,
        )]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Failed);
        assert_eq!(
            outcome.report.entries[0].error.as_ref().unwrap().code,
            "E2010"
        );
        assert!(!fs.exists("/out/first.csv"));
    }

    #[test]
    fn csv_range_reads_only_header_and_selected_range_and_undo_removes_the_output() {
        let fs = TestFs::default();
        let data = b"id,name\n1,Alice\n2,Bob\n3,Cara\n";
        fs.write_file("/data/a.csv", data);
        let plan = plan(vec![csv_range_entry(
            "e1",
            "/data/a.csv",
            "/out/range.csv",
            data.len() as u64,
            true,
            &["id", "name"],
            2,
            3,
            2,
            3,
            Some((0, 8)),
            Some((16, data.len() as u64)),
            2,
        )]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(
            fs.read_bytes_calls(),
            0,
            "range apply must not read the whole file"
        );
        assert_eq!(fs.read_file("/out/range.csv"), b"id,name\n2,Bob\n3,Cara\n");

        let undo = crate::apply::undo_plan_with_ports(
            PLAN_ID,
            &fs,
            &license,
            &mut journal,
            crate::apply::UndoPorts::default(),
        )
        .unwrap();
        assert_eq!(undo.reverted, 1);
        assert!(!fs.exists("/out/range.csv"));
        assert!(fs.exists("/data/a.csv"));
    }

    #[test]
    fn csv_range_rejects_a_same_size_header_change() {
        let fs = TestFs::default();
        let data = b"id,name\n1,Alice\n2,Bob\n3,Cara\n";
        fs.write_file("/data/a.csv", data);
        let plan = plan(vec![csv_range_entry(
            "e1",
            "/data/a.csv",
            "/out/range.csv",
            data.len() as u64,
            true,
            &["id", "name"],
            2,
            3,
            1,
            1,
            Some((0, 8)),
            Some((8, 16)),
            1,
        )]);
        fs.write_file("/data/a.csv", b"id,nAme\n1,Alice\n2,Bob\n3,Cara\n");
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Failed);
        assert_eq!(
            outcome.report.entries[0].error.as_ref().unwrap().code,
            "E2010"
        );
        assert!(!fs.exists("/out/range.csv"));
    }

    #[allow(clippy::too_many_arguments)]
    fn csv_range_entry(
        entry_id: &str,
        source: &str,
        target: &str,
        size: u64,
        has_header: bool,
        columns: &[&str],
        column_count: u64,
        data_rows: u64,
        start_row: u64,
        end_row: u64,
        header_range: Option<(u64, u64)>,
        data_range: Option<(u64, u64)>,
        count: u64,
    ) -> PlanEntry {
        PlanEntry {
            entry_id: entry_id.to_string(),
            source: source.to_string(),
            target: Some(target.to_string()),
            action: PlanAction::CsvRows,
            relation: None,
            reason: "csv range".to_string(),
            conflict: None,
            resolution: None,
            blocked: None,
            expected: Some(PlanExpected {
                size,
                algorithm: "size".to_string(),
                hash: None,
            }),
            range: None,
            merge: None,
            csv: None,
            csv_merge: None,
            csv_extract: None,
            csv_rows: Some(PlanCsvRows {
                encoding: Some(SourceEncoding::Utf8),
                size,
                delimiter: ",".to_string(),
                has_header,
                columns: columns.iter().map(|value| value.to_string()).collect(),
                column_count,
                data_rows,
                window: "range".to_string(),
                count,
                start_row: Some(start_row),
                end_row: Some(end_row),
                header_range: header_range.map(|(start, end)| PlanRange { start, end }),
                data_range: data_range.map(|(start, end)| PlanRange { start, end }),
                total_known: true,
                target_encoding: "utf-8".to_string(),
                invalid: "replace-and-count".to_string(),
                dirty_rows: 0,
            }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn csv_rows_entry(
        entry_id: &str,
        source: &str,
        target: &str,
        size: u64,
        has_header: bool,
        columns: &[&str],
        column_count: u64,
        data_rows: u64,
        window: &str,
        count: u64,
    ) -> PlanEntry {
        PlanEntry {
            entry_id: entry_id.to_string(),
            source: source.to_string(),
            target: Some(target.to_string()),
            action: PlanAction::CsvRows,
            relation: None,
            reason: "csv rows".to_string(),
            conflict: None,
            resolution: None,
            blocked: None,
            expected: Some(PlanExpected {
                size,
                algorithm: "size".to_string(),
                hash: None,
            }),
            range: None,
            merge: None,
            csv: None,
            csv_merge: None,
            csv_extract: None,
            csv_rows: Some(PlanCsvRows {
                encoding: Some(SourceEncoding::Utf8),
                size,
                delimiter: ",".to_string(),
                has_header,
                columns: columns.iter().map(|value| value.to_string()).collect(),
                column_count,
                data_rows,
                window: window.to_string(),
                count,
                start_row: None,
                end_row: None,
                header_range: None,
                data_range: None,
                total_known: false,
                target_encoding: "utf-8".to_string(),
                invalid: "replace-and-count".to_string(),
                dirty_rows: 0,
            }),
        }
    }

    fn csv_merge_entry(
        entry_id: &str,
        target: &str,
        files: Vec<PlanCsvMergeFile>,
        union: Vec<&str>,
        has_header: bool,
        positional_columns: u64,
        source_column: Option<&str>,
    ) -> PlanEntry {
        let first = &files[0];
        PlanEntry {
            entry_id: entry_id.to_string(),
            source: first.path.clone(),
            target: Some(target.to_string()),
            action: PlanAction::Merge,
            relation: None,
            reason: "csv merge".to_string(),
            conflict: None,
            resolution: None,
            blocked: None,
            expected: Some(PlanExpected {
                size: first.size,
                algorithm: "size".to_string(),
                hash: None,
            }),
            range: None,
            merge: None,
            csv: None,
            csv_merge: Some(PlanCsvMerge {
                files,
                union: union.iter().map(|value| value.to_string()).collect(),
                positional_columns,
                has_header,
                source_column: source_column.map(str::to_string),
                target_encoding: "utf-8".to_string(),
                invalid: "replace-and-count".to_string(),
                delimiter: ",".to_string(),
                strict: false,
            }),
            csv_extract: None,
            csv_rows: None,
        }
    }

    fn merge_entry(
        entry_id: &str,
        target: &str,
        target_encoding: TargetEncoding,
        sources: Vec<(&str, SourceEncoding, u64)>,
    ) -> PlanEntry {
        let merge_sources: Vec<PlanMergeSource> = sources
            .iter()
            .map(|(path, encoding, size)| PlanMergeSource {
                path: (*path).to_string(),
                encoding: Some(*encoding),
                size: *size,
            })
            .collect();
        PlanEntry {
            entry_id: entry_id.to_string(),
            source: merge_sources[0].path.clone(),
            target: Some(target.to_string()),
            action: PlanAction::Merge,
            relation: None,
            reason: "merge sources".to_string(),
            conflict: None,
            resolution: None,
            blocked: None,
            expected: Some(PlanExpected {
                size: merge_sources[0].size,
                algorithm: "size".to_string(),
                hash: None,
            }),
            range: None,
            merge: Some(PlanMerge {
                sources: merge_sources,
                target_encoding: target_encoding.as_str().to_string(),
                invalid: "replace-and-count".to_string(),
            }),
            csv: None,
            csv_merge: None,
            csv_extract: None,
            csv_rows: None,
        }
    }

    #[test]
    fn existing_target_is_skipped_by_default() {
        let fs = TestFs::with_files(&["/data/a.txt", "/data/b.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan(vec![action_entry(
                "e1",
                "/data/a.txt",
                Some("/data/b.txt"),
                PlanAction::Rename,
            )]),
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Skipped);
        assert_eq!(
            outcome.report.entries[0].error.as_ref().unwrap().code,
            "E2007"
        );
        assert!(fs.exists("/data/a.txt"));
        assert!(journal.entries.is_empty());
    }

    #[test]
    fn sequence_resolution_picks_a_free_name() {
        let fs = TestFs::with_files(&["/data/a.txt", "/data/b.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut entry = action_entry("e1", "/data/a.txt", Some("/data/b.txt"), PlanAction::Rename);
        entry.resolution = Some(PlanResolution::Sequence);
        let outcome = apply_generic_plan(
            &plan(vec![entry]),
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);
        assert!(fs.exists("/data/b (1).txt"));
        assert!(fs.exists("/data/b.txt"));
        assert!(!fs.exists("/data/a.txt"));
    }

    #[test]
    fn overwrite_without_confirmation_is_rejected_before_any_change() {
        let fs = TestFs::with_files(&["/data/a.txt", "/data/b.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let options = ApplyOptions {
            on_conflict: OnConflict::Overwrite,
            ..Default::default()
        };
        let error = apply_generic_plan(
            &plan(vec![action_entry(
                "e1",
                "/data/a.txt",
                Some("/data/b.txt"),
                PlanAction::Rename,
            )]),
            &options,
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap_err();
        assert_eq!(error.code, "E1002");
        assert!(fs.exists("/data/a.txt"));
    }

    #[test]
    fn copy_applies_with_a_quarantine_reverse_and_undo_removes_the_output() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan(vec![action_entry(
                "e1",
                "/data/a.txt",
                Some("/data/copy.txt"),
                PlanAction::Copy,
            )]),
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);
        assert!(fs.exists("/data/a.txt"), "copy keeps the source");
        assert!(fs.exists("/data/copy.txt"));
        // Journal：forward=Copy，reverse=产物进隔离目录（设计 01 §8 新建产物语义）。
        assert!(matches!(journal.entries[0].forward, FsOp::Copy { .. }));
        assert!(matches!(
            &journal.entries[0].reverse,
            FsOp::Quarantine { from, .. } if from == "/data/copy.txt"
        ));

        // undo 按字面执行 reverse：产物移入隔离目录，源文件不动。
        let license = crate::license::AlwaysAllowLicense;
        let undo = crate::apply::undo_plan_with_ports(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            &fs,
            &license,
            &mut journal,
            crate::apply::UndoPorts::default(),
        )
        .unwrap();
        assert_eq!(undo.reverted, 1);
        assert!(!fs.exists("/data/copy.txt"), "undo quarantines the output");
        assert!(fs.exists("/data/a.txt"));
    }

    #[test]
    fn copy_overwrite_backs_up_the_target_and_undo_restores_its_content() {
        let fs = TestFs::with_files(&["/data/a.txt", "/data/b.txt"]);
        fs.write_file("/data/b.txt", b"old-target");
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let options = ApplyOptions {
            on_conflict: OnConflict::Overwrite,
            overwrite_confirmed: true,
        };
        let outcome = apply_generic_plan(
            &plan(vec![action_entry(
                "e1",
                "/data/a.txt",
                Some("/data/b.txt"),
                PlanAction::Copy,
            )]),
            &options,
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Overwritten);
        assert_eq!(
            fs.read_file("/data/b.txt"),
            b"content",
            "target now holds the copied content"
        );
        // Journal：reverse = dataRef 备份还原（设计 01 §8）。
        let (path, backup) = match &journal.entries[0].reverse {
            FsOp::WriteAtomic {
                path,
                data_ref: Some(backup),
            } => (path.clone(), backup.clone()),
            _ => panic!("overwrite copy must reverse via a dataRef write_atomic"),
        };
        assert_eq!(path, "/data/b.txt");

        let license = crate::license::AlwaysAllowLicense;
        let undo = crate::apply::undo_plan_with_ports(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            &fs,
            &license,
            &mut journal,
            crate::apply::UndoPorts::default(),
        )
        .unwrap();
        assert_eq!(undo.reverted, 1);
        assert_eq!(
            fs.read_file(&path),
            b"old-target",
            "undo restores the pre-overwrite content from the backup"
        );
        assert!(fs.exists(&backup), "the backup file itself stays recorded");
    }

    #[test]
    fn edit_content_rewrites_in_place_and_undo_restores_the_bytes() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        fs.write_file("/data/a.txt", b"abc");
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut ctx = context(&fs, &mut journal, &license);
        ctx.content_editor = Some(&|data: &[u8]| {
            Ok(data
                .iter()
                .map(|byte| byte.to_ascii_uppercase())
                .collect::<Vec<u8>>())
        });
        let outcome = apply_generic_plan(
            &plan(vec![action_entry(
                "e1",
                "/data/a.txt",
                None,
                PlanAction::EditContent,
            )]),
            &ApplyOptions::default(),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);
        assert_eq!(fs.read_file("/data/a.txt"), b"ABC");
        assert!(matches!(
            &journal.entries[0].forward,
            FsOp::WriteAtomic {
                data_ref: Some(_),
                ..
            }
        ));

        let license = crate::license::AlwaysAllowLicense;
        let undo = crate::apply::undo_plan_with_ports(
            "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
            &fs,
            &license,
            &mut journal,
            crate::apply::UndoPorts::default(),
        )
        .unwrap();
        assert_eq!(undo.reverted, 1);
        assert_eq!(fs.read_file("/data/a.txt"), b"abc");
    }

    #[test]
    fn edit_content_requires_an_editor_on_the_context() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let error = apply_generic_plan(
            &plan(vec![action_entry(
                "e1",
                "/data/a.txt",
                None,
                PlanAction::EditContent,
            )]),
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap_err();
        assert_eq!(error.code, "E1002");
        assert_eq!(fs.read_file("/data/a.txt"), b"content");
    }

    #[test]
    fn changed_size_is_skipped_with_e2010() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut entry = action_entry("e1", "/data/a.txt", Some("/data/b.txt"), PlanAction::Rename);
        entry.expected = Some(PlanExpected {
            size: 99,
            algorithm: "blake3".to_string(),
            hash: None,
        });
        let outcome = apply_generic_plan(
            &plan(vec![entry]),
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Skipped);
        assert_eq!(
            outcome.report.entries[0].error.as_ref().unwrap().code,
            "E2010"
        );
        assert!(fs.exists("/data/a.txt"));
    }

    #[test]
    fn protected_target_is_skipped_before_any_change() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut ctx = context(&fs, &mut journal, &license);
        ctx.protected_dirs = vec![ProtectedDir::new("/protected")];
        let outcome = apply_generic_plan(
            &plan(vec![action_entry(
                "e1",
                "/data/a.txt",
                Some("/protected/a.txt"),
                PlanAction::Rename,
            )]),
            &ApplyOptions::default(),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Skipped);
        assert_eq!(
            outcome.report.entries[0].error.as_ref().unwrap().code,
            "E2009"
        );
        assert!(fs.exists("/data/a.txt"));
        assert!(!fs.exists("/protected/a.txt"));
        assert!(journal.entries.is_empty());
    }

    #[test]
    fn protected_source_is_rechecked_during_execution() {
        let fs = TestFs::with_files(&["/protected/a.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut ctx = context(&fs, &mut journal, &license);
        ctx.protected_dirs = vec![ProtectedDir::new("/protected")];
        let outcome = apply_generic_plan(
            &plan(vec![action_entry(
                "e1",
                "/protected/a.txt",
                Some("/data/b.txt"),
                PlanAction::Rename,
            )]),
            &ApplyOptions::default(),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Skipped);
        assert_eq!(
            outcome.report.entries[0].error.as_ref().unwrap().code,
            "E2009"
        );
        assert!(fs.exists("/protected/a.txt"));
        assert!(!fs.exists("/data/b.txt"));
    }

    #[derive(Default)]
    struct RecordingLicense {
        released: Mutex<Vec<String>>,
    }

    impl crate::license::LicenseService for RecordingLicense {
        fn capability(&self, _op: &str) -> crate::license::Capability {
            crate::license::Capability::Allow
        }

        fn consume(&self, _op: &str, _n: u32) -> Result<(), KernelError> {
            Ok(())
        }

        fn release(&self, op: &str, _n: u32) {
            self.released.lock().unwrap().push(op.to_string());
        }
    }

    #[test]
    fn undo_releases_the_plan_kind_recorded_in_the_journal() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        let license = RecordingLicense::default();
        let mut journal = TestJournal::default();
        apply_generic_plan(
            &plan(vec![action_entry(
                "e1",
                "/data/a.txt",
                Some("/data/b.txt"),
                PlanAction::Rename,
            )]),
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(journal.entries[0].plan_kind.as_deref(), Some("c05"));

        let outcome = crate::apply::undo_plan(PLAN_ID, &fs, &license, &mut journal).unwrap();
        assert_eq!(outcome.reverted, 1);
        assert_eq!(*license.released.lock().unwrap(), vec!["c05".to_string()]);
    }

    #[test]
    fn cancel_mid_batch_rolls_back_applied_entries() {
        let fs = TestFs::with_files(&["/data/a.txt", "/data/b.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let ports = Ports::default();
        *ports.cancel_after.lock().unwrap() = Some(1);
        let mut ctx = context(&fs, &mut journal, &license);
        ctx.progress = Some(&ports);
        ctx.cancel = Some(&ports);
        let outcome = apply_generic_plan(
            &plan(vec![
                action_entry(
                    "e1",
                    "/data/a.txt",
                    Some("/data/a1.txt"),
                    PlanAction::Rename,
                ),
                action_entry(
                    "e2",
                    "/data/b.txt",
                    Some("/data/b1.txt"),
                    PlanAction::Rename,
                ),
            ]),
            &ApplyOptions::default(),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Cancelled);
        assert!(fs.exists("/data/a.txt"), "first entry is rolled back");
        assert!(fs.exists("/data/b.txt"));
        assert!(!fs.exists("/data/a1.txt"));
        assert_eq!(journal.entries[0].status, JournalStatus::Reverted);
    }
}
