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

use std::io::{Read, Write};

use crate::apply::{
    execute_reverse, ApplyEntryReport, ApplyOptions, ApplyOutcome, ApplyReport, ApplyState,
    ApplyStatus, OnConflict,
};
use crate::conflict::sequence_target;
use crate::csv::Delimiter;
use crate::csv_dedup::key_of;
use crate::csv_filter::{FilterOp, RowPredicate};
use crate::csv_merge::{
    column_keys, normalized_columns, parse_records, write_record, RecordFeeder,
};
use crate::csv_rows::RowWindow;
use crate::encoding::{
    transcode, InvalidSequencePolicy, SourceEncoding, StreamTranscoder, TargetEncoding,
};
use crate::error::KernelError;
use crate::fs::{CaseSense, ConflictPolicy, EntryKind, FsAdapter, PathOutcomeKind};
use crate::journal::{FsOp, JournalStore};
use crate::license::{Capability, LicenseService};
use crate::listing::{DirectoryLister, ListingOptions};
use crate::path::{is_under, protected_by, try_normalize, ProtectedDir};
use crate::plan::{
    PlanAction, PlanCsvColumns, PlanCsvDedup, PlanCsvExtract, PlanCsvFilter, PlanCsvMerge,
    PlanCsvRows, PlanCsvSort, PlanDocument, PlanParamValue, PlanResolution,
};
use crate::progress::{CancelToken, ProgressPhase, ProgressSink};

/// `edit-content` 的变换结果：新字节 ＋ 可选的**条目级明细**。
///
/// 明细是给执行报告看的一行文本（方案 §5.2"默认替换并计数上报"的落点），由注入方
/// 生成——判定与计数都在纯函数层，执行器只负责把它带到 `ApplyEntryReport.detail`。
pub struct EditedContent {
    pub bytes: Vec<u8>,
    pub detail: Option<String>,
}

impl EditedContent {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            detail: None,
        }
    }

    pub fn with_detail(bytes: Vec<u8>, detail: impl Into<String>) -> Self {
        Self {
            bytes,
            detail: Some(detail.into()),
        }
    }
}

/// `edit-content` 的内容变换函数：输入原始字节，返回变换后的字节与条目级明细。
pub type ContentEditor<'a> =
    &'a (dyn Fn(&[u8]) -> Result<EditedContent, KernelError> + Send + Sync);

/// 流式内容重写的条目级结果：`detail` 与 `EditedContent.detail` 同载体
/// （方案 §5.2"默认替换并计数上报"的落点，随条目级报告进执行明细）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditOutcome {
    pub detail: Option<String>,
}

/// 流式内容重写端口：读取源流、写入目标流；C02/C03/C01 等大文件改写使用。
///
/// **取消**（方案 §7.7）：`cancel` 由执行器注入，实现应在**每个读块边界**检查它；
/// 命中就立即返回 `Err(KernelError::cancelled())`——回调因此以 `Err` 收场，
/// `rewrite_atomic_with` 丢弃临时文件，源文件保持原样（不会被半截内容覆盖）。
/// 取消**不是失败**：执行器据此把整单标成 `Cancelled` 并停止派发。
///
/// 当前只有 C02（换行转换）是单一均匀的读块循环，故只有它接了检查；
/// C01／C03 共用本端口但尚未逐块检查（见 ADR-0097 §5）。
pub trait ContentRewriter: Send + Sync {
    fn rewrite(
        &self,
        entry: &crate::plan::PlanEntry,
        input: &mut dyn Read,
        output: &mut dyn Write,
        cancel: &dyn CancelToken,
    ) -> Result<EditOutcome, KernelError>;
}

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
    /// D1：网络断线的退避等待与自动暂停（None＝不重试）。
    pub network_retry: Option<&'a dyn crate::retry::NetworkRetry>,
    /// `edit-content` 条目的内容变换函数（如转码）；其他动作为 None。
    pub content_editor: Option<ContentEditor<'a>>,
    /// 流式内容重写端口（C02 换行转换）；设置时优先于 `content_editor`。
    pub content_rewriter: Option<&'a dyn ContentRewriter>,
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
    crate::apply::validate_apply_limits(plan, options)?;
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
            | PlanAction::Transcode
            | PlanAction::Split
            | PlanAction::Merge
            | PlanAction::CsvExtract
            | PlanAction::CsvRows
            | PlanAction::CsvDedup
            | PlanAction::CsvFilter
            | PlanAction::CsvColumns
            | PlanAction::CsvSort
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
            PlanAction::Transcode if entry.target.is_some() => {
                return Err(KernelError::invalid_params(
                    "transcode entries edit in place and must not have a target",
                ))
            }
            PlanAction::Rename
            | PlanAction::Move
            | PlanAction::Copy
            | PlanAction::Split
            | PlanAction::Merge
            | PlanAction::CsvExtract
            | PlanAction::CsvColumns
            | PlanAction::CsvSort
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
            PlanAction::CsvColumns => {
                let payload = entry.csv_columns.as_ref().ok_or_else(|| {
                    KernelError::invalid_params("csv-columns entries require a csvColumns payload")
                })?;
                validate_csv_columns(payload)?;
            }
            PlanAction::CsvSort => {
                let payload = entry.csv_sort.as_ref().ok_or_else(|| {
                    KernelError::invalid_params("csv-sort entries require a csvSort payload")
                })?;
                validate_csv_sort(payload)?;
            }
            PlanAction::CsvRows => {
                let payload = entry.csv_rows.as_ref().ok_or_else(|| {
                    KernelError::invalid_params("csv-rows entries require a csvRows payload")
                })?;
                validate_csv_rows(payload)?;
            }
            PlanAction::CsvDedup => {
                let payload = entry.csv_dedup.as_ref().ok_or_else(|| {
                    KernelError::invalid_params("csv-dedup entries require a csvDedup payload")
                })?;
                validate_csv_dedup(payload)?;
            }
            PlanAction::CsvFilter => {
                let payload = entry.csv_filter.as_ref().ok_or_else(|| {
                    KernelError::invalid_params("csv-filter entries require a csvFilter payload")
                })?;
                validate_csv_filter(payload)?;
            }
            _ => {}
        }
    }
    if plan.entries.iter().any(|entry| {
        matches!(
            entry.action,
            PlanAction::EditContent | PlanAction::Transcode
        )
    }) && context.content_editor.is_none()
        && context.content_rewriter.is_none()
    {
        return Err(KernelError::invalid_params(
            "edit-content/transcode entries require a content editor or rewriter on the apply context",
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
    report_progress(context, ProgressPhase::Executing, 0, total, None);

    // 上下文拆解：工作线程只共享执行视图（设计 01 §5.1）；
    // Journal、配额与进度记账留在主执行线程（`&mut` 借用不可跨线程）。
    let mut exec = {
        let GenericApplyContext {
            fs,
            license,
            journal,
            task_id,
            now_rfc3339,
            protected_dirs,
            case_sense,
            progress,
            cancel,
            network_retry,
            content_editor,
            content_rewriter,
            directory_lister,
        } = context;
        ExecContext {
            fs: *fs,
            license: *license,
            journal: *journal,
            now_rfc3339: now_rfc3339.as_str(),
            protected_dirs: protected_dirs.as_slice(),
            case_sense: *case_sense,
            progress: *progress,
            cancel: *cancel,
            network_retry: *network_retry,
            task_id: task_id.as_str(),
            content_editor: *content_editor,
            content_rewriter: *content_rewriter,
            directory_lister: *directory_lister,
        }
    };
    let view = WorkerView {
        fs: exec.fs,
        case_sense: exec.case_sense,
        content_editor: exec.content_editor,
        content_rewriter: exec.content_rewriter,
        directory_lister: exec.directory_lister,
        cancel: exec.cancel,
        network_retry: exec.network_retry,
        task_id: exec.task_id,
        progress: exec.progress,
    };

    let (mut reports, applied, failed, cancelled) = if options.entry_concurrency > 1 {
        execute_pooled(
            &mut exec,
            &view,
            plan,
            options,
            options.entry_concurrency,
            total,
        )?
    } else {
        execute_serial(&mut exec, &view, plan, options, total)?
    };

    if cancelled && options.cancel_policy == crate::apply::CancelPolicy::RollbackApplied {
        report_progress_to(
            exec.progress,
            exec.task_id,
            ProgressPhase::Journaling,
            0,
            applied.len() as u64,
            Some("rolling back applied entries"),
        );
        for (done, (index, seq, reverse)) in applied.iter().rev().enumerate() {
            report_progress_to(
                exec.progress,
                exec.task_id,
                ProgressPhase::Journaling,
                done as u64,
                applied.len() as u64,
                None,
            );
            match execute_reverse(exec.fs, reverse) {
                Ok(()) => {
                    let _ = exec.journal.mark_reverted(&plan.plan_id, &[*seq]);
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
            task_id: exec.task_id.to_string(),
            entries: reports,
            undone_by: None,
        },
    })
}

/// 执行期主线程侧的上下文切片：Journal/配额/进度记账专用字段（设计 01 §5.1）。
/// 由 `GenericApplyContext` 拆解而来；工作线程只拿 `WorkerView`，两者无共享借用。
struct ExecContext<'a> {
    fs: &'a dyn FsAdapter,
    license: &'a dyn LicenseService,
    journal: &'a mut dyn JournalStore,
    now_rfc3339: &'a str,
    protected_dirs: &'a [ProtectedDir],
    case_sense: CaseSense,
    progress: Option<&'a dyn ProgressSink>,
    cancel: Option<&'a dyn CancelToken>,
    network_retry: Option<&'a dyn crate::retry::NetworkRetry>,
    task_id: &'a str,
    content_editor: Option<ContentEditor<'a>>,
    content_rewriter: Option<&'a dyn ContentRewriter>,
    directory_lister: Option<&'a dyn DirectoryLister>,
}

/// 单条预检结果：报告（跳过/失败）或放行执行。
enum Prechecked {
    Skipped(ApplyEntryReport),
    Failed(ApplyEntryReport),
    Proceed { target: String, overwrite: bool },
}

/// 并发分块的一个待执行条目。
struct ChunkItem<'a> {
    entry: &'a crate::plan::PlanEntry,
    target: String,
    overwrite: bool,
    index: usize,
}

fn is_cancelled_token(cancel: Option<&dyn CancelToken>) -> bool {
    cancel.map(|token| token.is_cancelled()).unwrap_or(false)
}

/// 上报**条目内（字节级）**进度（方案 §7.5 双层进度的条目层，ADR-0100）。
///
/// 与条目边界事件的区别：`entry_done/entry_total` 为 `Some`，`done/total` 保持 0——
/// 壳层据此只更新字节字段、不动条目计数（条目计数由派发循环边界事件维护）。
fn report_entry_progress(view: &WorkerView<'_>, entry_done: u64, entry_total: u64) {
    if let Some(sink) = view.progress {
        sink.report(crate::progress::ProgressEvent {
            task_id: view.task_id.to_string(),
            phase: ProgressPhase::Executing,
            done: 0,
            total: 0,
            message: None,
            entry_done: Some(entry_done),
            entry_total: Some(entry_total),
        });
    }
}

fn report_progress_to(
    progress: Option<&dyn ProgressSink>,
    task_id: &str,
    phase: ProgressPhase,
    done: u64,
    total: u64,
    message: Option<&str>,
) {
    if let Some(sink) = progress {
        sink.report(crate::progress::ProgressEvent {
            task_id: task_id.to_string(),
            phase,
            done,
            total,
            message: message.map(str::to_string),
            entry_done: None,
            entry_total: None,
        });
    }
}

fn edit_content_target_param<'a>(
    entry: &'a crate::plan::PlanEntry,
    target: &'a str,
) -> Option<&'a str> {
    if entry.action == PlanAction::EditContent {
        None
    } else {
        Some(target)
    }
}

/// 可并发派发的动作类：1:1 且条目间无顺序依赖（方案 §5.12 条目级并发）。
/// rename/move/split/merge/csv-merge/listing/create-dir 保持串行（条目间有顺序或共享产物）。
fn is_pool_eligible(action: &PlanAction) -> bool {
    matches!(
        action,
        PlanAction::Copy
            | PlanAction::EditContent
            | PlanAction::CsvDedup
            | PlanAction::CsvFilter
            | PlanAction::CsvExtract
            | PlanAction::CsvRows
            | PlanAction::Delete
    )
}

/// 条目占用的资源键（规范化后的源与目标）；键相交的条目不得同块并发。
fn entry_resource_keys(
    entry: &crate::plan::PlanEntry,
    target: &str,
) -> std::collections::BTreeSet<String> {
    let mut keys = std::collections::BTreeSet::new();
    keys.insert(try_normalize(&entry.source).unwrap_or_else(|_| entry.source.clone()));
    if !target.is_empty() {
        keys.insert(try_normalize(target).unwrap_or_else(|_| target.to_string()));
    }
    keys
}

/// 执行前的逐条预检（与原串行路径逐字同语义）：
/// blocked/skip 短路、源路径保护、授权能力、stat/占位/指纹、目标裁决与目标保护。
fn precheck_entry(
    exec: &ExecContext<'_>,
    view: &WorkerView<'_>,
    plan: &PlanDocument,
    options: &ApplyOptions,
    entry: &crate::plan::PlanEntry,
) -> Prechecked {
    if let Some(blocked) = &entry.blocked {
        return Prechecked::Skipped(skipped_entry(
            entry.entry_id.clone(),
            KernelError::new(
                blocked.code.clone(),
                blocked.reason.clone(),
                blocked.hint.clone(),
                false,
            ),
        ));
    }
    if entry.resolution == Some(PlanResolution::Skip) {
        return Prechecked::Skipped(ApplyEntryReport {
            entry_id: entry.entry_id.clone(),
            status: ApplyStatus::Skipped,
            error: None,
            detail: None,
        });
    }

    if entry.action != PlanAction::CreateDir {
        let normalized = match try_normalize(&entry.source) {
            Ok(normalized) => normalized,
            Err(error) => return Prechecked::Failed(failed_entry(entry.entry_id.clone(), error)),
        };
        if let Some(error) = protected_path_hit(
            exec.fs,
            exec.protected_dirs,
            exec.case_sense,
            &entry.source,
            &normalized,
        ) {
            return Prechecked::Skipped(skipped_entry(entry.entry_id.clone(), error));
        }
    }

    match exec.license.capability(&plan.kind) {
        Capability::Allow => {}
        Capability::Deny { reason } => {
            return Prechecked::Failed(failed_entry(
                entry.entry_id.clone(),
                KernelError::new(
                    "E4001",
                    format!("operation not licensed: {reason}"),
                    None,
                    false,
                ),
            ));
        }
        Capability::Quota { used, limit } => {
            return Prechecked::Failed(failed_entry(
                entry.entry_id.clone(),
                KernelError::new(
                    "E4004",
                    format!("license quota exhausted ({used}/{limit})"),
                    None,
                    false,
                ),
            ));
        }
    }

    if entry.action != PlanAction::CreateDir {
        let stat = match exec.fs.stat(&entry.source) {
            Ok(stat) => stat,
            Err(error) => return Prechecked::Failed(failed_entry(entry.entry_id.clone(), error)),
        };
        if entry.action == PlanAction::Listing {
            if stat.kind != EntryKind::Dir {
                return Prechecked::Failed(failed_entry(
                    entry.entry_id.clone(),
                    KernelError::invalid_params("listing expects a directory source"),
                ));
            }
        } else if entry.action == PlanAction::Rename {
            if !matches!(stat.kind, EntryKind::File | EntryKind::Dir) {
                return Prechecked::Failed(failed_entry(
                    entry.entry_id.clone(),
                    KernelError::invalid_params("rename expects a file or directory source"),
                ));
            }
        } else if entry.action != PlanAction::Delete && stat.kind != EntryKind::File {
            return Prechecked::Failed(failed_entry(
                entry.entry_id.clone(),
                KernelError::invalid_params(format!(
                    "{} expects a file source",
                    entry.action.as_str()
                )),
            ));
        }
        if stat.attrs.placeholder {
            return Prechecked::Skipped(skipped_entry(
                entry.entry_id.clone(),
                KernelError::new("E2006", "placeholder file skipped", None, true),
            ));
        }
        if let Some(error) = fingerprint_mismatch(exec.fs, entry, stat.size) {
            return Prechecked::Skipped(skipped_entry(entry.entry_id.clone(), error));
        }
        if entry.action == PlanAction::Split {
            if let Some(range) = entry.range {
                let required_end = entry
                    .csv
                    .as_ref()
                    .and_then(|csv| csv.header)
                    .map_or(range.end, |header| range.end.max(header.end));
                if required_end > stat.size {
                    return Prechecked::Skipped(skipped_entry(
                        entry.entry_id.clone(),
                        KernelError::new(
                            "E2010",
                            "source file shrank since plan preview; re-plan before applying",
                            None,
                            true,
                        ),
                    ));
                }
            }
        }
        if entry.action == PlanAction::Merge {
            if let Some(error) =
                merge_source_mismatch(view, exec.protected_dirs, exec.case_sense, entry)
            {
                return Prechecked::Skipped(skipped_entry(entry.entry_id.clone(), error));
            }
        }
    }

    let decision = match &entry.target {
        Some(target) => match resolve_target(entry, target, options, exec.fs, exec.case_sense) {
            Ok(decision) => decision,
            Err(error) => return Prechecked::Failed(failed_entry(entry.entry_id.clone(), error)),
        },
        None => TargetDecision::Use(String::new()),
    };
    let (target, overwrite) = match decision {
        TargetDecision::Use(target) => (target, false),
        TargetDecision::Overwrite(target) => (target, true),
        TargetDecision::Skip(error) => {
            return Prechecked::Skipped(skipped_entry(entry.entry_id.clone(), error));
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
            | PlanAction::CsvColumns
            | PlanAction::CsvSort
            | PlanAction::CsvRows
            | PlanAction::Listing
            | PlanAction::CreateDir
    ) {
        if let Some(error) =
            protected_target_hit(exec.fs, exec.protected_dirs, exec.case_sense, &target)
        {
            return Prechecked::Skipped(skipped_entry(entry.entry_id.clone(), error));
        }
    }
    Prechecked::Proceed { target, overwrite }
}

/// OQ-37 裁决①：只有 T v1.12 §3.2 的 E03 行点名的功能族——改名族（F12/F13/C04）与
/// C03 内容重写——把"写不进去"记为**跳过并报告**；其余写臂维持 `Failed`（响亮失败），
/// 因为 F04/C09 之类"产物没写出来"若变安静跳过，用户可能根本不看见。
fn skips_permission_failures(plan: &PlanDocument) -> bool {
    matches!(
        plan.kind.as_str(),
        crate::rename::F12_KIND
            | crate::regex_rename::F13_KIND
            | crate::name_replace::C04_KIND
            | crate::replace_content::C03_KIND
    )
}

/// 方案 §5.13 的"执行时重试一次"只处理 E2003（被占用）；D1 的断线重试单独处理
/// E2005（网络不可达）：最多 3 次、退避 2/4/8 秒，之后交给注入的端口把任务切到
/// 暂停并等待恢复（不直接失败）。两条都只重 `retryable` 的错误。
fn perform_with_retry(
    view: &WorkerView<'_>,
    plan: &PlanDocument,
    entry: &crate::plan::PlanEntry,
    target: Option<&str>,
    overwrite: bool,
) -> Result<Result<Performed, KernelError>, KernelError> {
    let mut network_attempts = 0u32;
    // D1 补全：同一条目的隔离备份在首次尝试后缓存，重试复用——否则 edit-content /
    // transcode 每枪都会 `backup_to_quarantine` 一次，退避重试会累积备份。
    let mut backup: Option<String> = None;
    loop {
        let outcome = perform(view, plan, entry, target, overwrite, &mut backup);
        if let Some(error) = network_error(&outcome) {
            if let Some(retry) = view.network_retry {
                if crate::retry::entry_touches_network(view.fs, entry)
                    && crate::retry::action_can_retry_network(entry.action)
                {
                    if network_attempts < crate::retry::NETWORK_RETRY_ATTEMPTS {
                        let delay = std::time::Duration::from_secs(
                            crate::retry::NETWORK_RETRY_BACKOFF_SECS[network_attempts as usize],
                        );
                        if retry.wait(delay) {
                            network_attempts += 1;
                            continue;
                        }
                    } else {
                        match retry.pause(error) {
                            crate::retry::PauseAction::Resumed => {
                                // 用户恢复后重新从"首次尝试"开始（网络恢复前再次耗尽会再暂停）。
                                network_attempts = 0;
                                continue;
                            }
                            crate::retry::PauseAction::Cancel => {}
                            crate::retry::PauseAction::Abort => {
                                // CLI 没有暂停交互面：中止整个任务，不逐条重试。
                                return Err(error.clone());
                            }
                        }
                    }
                }
            }
        }
        // 磁盘满（ADR-0093）：**不学网络断线退避**——磁盘满不会在秒级自愈，
        // 退避 3 次只是把用户推向"任务失败"。第一次撞上就暂停，等用户清理
        // 空间后恢复并重试本条目；用户取消则按普通条目失败收尾。
        if let Some(error) = disk_full_error(&outcome) {
            if let Some(retry) = view.network_retry {
                match retry.pause(error) {
                    crate::retry::PauseAction::Resumed => continue,
                    crate::retry::PauseAction::Cancel => {}
                    crate::retry::PauseAction::Abort => return Err(error.clone()),
                }
            }
        }
        if let Ok(Err(error)) = &outcome {
            if error.retryable && skips_permission_failures(plan) {
                return perform(view, plan, entry, target, overwrite, &mut backup);
            }
        }
        return outcome;
    }
}

fn network_error(
    outcome: &Result<Result<Performed, KernelError>, KernelError>,
) -> Option<&KernelError> {
    match outcome {
        Ok(Err(error)) if error.code == "E2005" => Some(error),
        Err(error) if error.code == "E2005" => Some(error),
        _ => None,
    }
}

/// 磁盘满（ADR-0093）：致命错误，走"暂停等清理"而不是逐条失败。
fn disk_full_error(
    outcome: &Result<Result<Performed, KernelError>, KernelError>,
) -> Option<&KernelError> {
    match outcome {
        Ok(Err(error)) if error.code == "E2004" => Some(error),
        Err(error) if error.code == "E2004" => Some(error),
        _ => None,
    }
}

/// 执行并记账（主线程调用；串行路径与并发回收共用）：
/// perform 失败释放配额并计失败；Journal 写入失败尽力回退本次动作。
#[allow(clippy::too_many_arguments)]
fn settle_entry(
    exec: &mut ExecContext<'_>,
    plan: &PlanDocument,
    entry: &crate::plan::PlanEntry,
    index: usize,
    overwrite: bool,
    outcome: Result<Result<Performed, KernelError>, KernelError>,
    applied: &mut Vec<(usize, u64, FsOp)>,
    failed: &mut usize,
) -> Result<ApplyEntryReport, KernelError> {
    let performed = match outcome {
        Ok(Ok(performed)) => performed,
        Ok(Err(error)) => {
            exec.license.release(&plan.kind, 1);
            if error.is_cancelled() {
                // 用户取消不是失败（方案 §7.7）：条目如实记 skipped（源文件未被改写），
                // 不进 failed 计数；整单终态由循环边界上的 `cancelled` 标成 Cancelled。
                return Ok(skipped_entry(entry.entry_id.clone(), error));
            }
            if skips_permission_failures(plan) && matches!(error.code.as_str(), "E2002" | "E2003") {
                // 方案 §5.13／§5.14（OQ-37 裁决①）：只读/占用写不进去＝**跳过并报告**，
                // 条目不进 failed 计数，任务终态因此不会被判成整体失败。
                return Ok(skipped_entry(entry.entry_id.clone(), error));
            }
            *failed += 1;
            return Ok(failed_entry(entry.entry_id.clone(), error));
        }
        Err(error) => {
            exec.license.release(&plan.kind, 1);
            return Err(error);
        }
    };
    if performed.skipped {
        exec.license.release(&plan.kind, 1);
        return Ok(skipped_entry(
            entry.entry_id.clone(),
            KernelError::new(
                "E2007",
                "target already exists; skipped by policy",
                None,
                false,
            ),
        ));
    }
    let forward = performed.forward;
    let reverse = performed.reverse;
    let detail = performed.detail;
    match exec.journal.append_with_kind(
        &plan.plan_id,
        &entry.entry_id,
        exec.now_rfc3339,
        &plan.kind,
        forward,
        reverse.clone(),
    ) {
        Ok(journal_entry) => {
            applied.push((index, journal_entry.seq, journal_entry.reverse));
        }
        Err(error) => {
            exec.license.release(&plan.kind, 1);
            // 落盘成功但 Journal 失败：尽力回退本次动作后再报错。
            if execute_reverse(exec.fs, &reverse).is_ok() {
                *failed += 1;
                return Ok(failed_entry(entry.entry_id.clone(), error));
            }
            return Err(KernelError::internal(
                "journal write failed and the operation could not be reverted",
                format!("{}: {}", error.code, error.message),
            ));
        }
    }
    Ok(ApplyEntryReport {
        entry_id: entry.entry_id.clone(),
        status: if overwrite || performed.replaced {
            ApplyStatus::Overwritten
        } else {
            ApplyStatus::Ok
        },
        error: None,
        detail,
    })
}

#[allow(clippy::too_many_arguments)]
fn execute_serial(
    exec: &mut ExecContext<'_>,
    view: &WorkerView<'_>,
    plan: &PlanDocument,
    options: &ApplyOptions,
    total: u64,
) -> Result<Executed, KernelError> {
    let mut reports = Vec::with_capacity(plan.entries.len());
    let mut applied: Vec<(usize, u64, FsOp)> = Vec::new();
    let mut failed = 0usize;
    let mut cancelled = false;
    for (index, entry) in plan.entries.iter().enumerate() {
        report_progress_to(
            exec.progress,
            exec.task_id,
            ProgressPhase::Executing,
            index as u64,
            total,
            None,
        );
        if is_cancelled_token(exec.cancel) {
            cancelled = true;
            break;
        }
        match precheck_entry(exec, view, plan, options, entry) {
            Prechecked::Skipped(report) => reports.push(report),
            Prechecked::Failed(report) => {
                failed += 1;
                reports.push(report);
            }
            Prechecked::Proceed { target, overwrite } => {
                if let Err(error) = exec.license.consume(&plan.kind, 1) {
                    failed += 1;
                    reports.push(failed_entry(entry.entry_id.clone(), error));
                    continue;
                }
                let target_param = edit_content_target_param(entry, &target);
                let outcome = perform_with_retry(view, plan, entry, target_param, overwrite);
                let report = settle_entry(
                    exec,
                    plan,
                    entry,
                    index,
                    overwrite,
                    outcome,
                    &mut applied,
                    &mut failed,
                )?;
                reports.push(report);
                report_progress_to(
                    exec.progress,
                    exec.task_id,
                    ProgressPhase::Executing,
                    (index + 1) as u64,
                    total,
                    Some(&entry.source),
                );
            }
        }
    }
    // mid-stream 取消可能落在**最后一条**里——循环已自然结束，边界检查没机会跑。
    if !cancelled && is_cancelled_token(exec.cancel) {
        cancelled = true;
    }
    Ok((reports, applied, failed, cancelled))
}

/// 结算一个并发分块：块内有界并发执行（条目数 ≤ 并发上限），
/// 按计划序 join 与记账；致命错误仍先收完块内结果再中止（成功条目必须进 Journal）。
#[allow(clippy::too_many_arguments)]
fn flush_chunk(
    exec: &mut ExecContext<'_>,
    view: &WorkerView<'_>,
    plan: &PlanDocument,
    chunk: Vec<ChunkItem<'_>>,
    slots: &mut [Option<ApplyEntryReport>],
    applied: &mut Vec<(usize, u64, FsOp)>,
    failed: &mut usize,
    completed: &mut u64,
    total: u64,
) -> Result<(), KernelError> {
    if chunk.is_empty() {
        return Ok(());
    }
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(chunk.len());
        for item in &chunk {
            let target_param = edit_content_target_param(item.entry, &item.target);
            handles.push(scope.spawn(move || {
                perform_with_retry(view, plan, item.entry, target_param, item.overwrite)
            }));
        }
        let mut error: Option<KernelError> = None;
        for (item, handle) in chunk.iter().zip(handles) {
            let outcome = match handle.join() {
                Ok(outcome) => outcome,
                Err(_) => Err(KernelError::internal(
                    "entry worker panicked",
                    format!("entry {}", item.entry.entry_id),
                )),
            };
            match settle_entry(
                exec,
                plan,
                item.entry,
                item.index,
                item.overwrite,
                outcome,
                applied,
                failed,
            ) {
                Ok(report) => {
                    slots[item.index] = Some(report);
                    *completed += 1;
                    report_progress_to(
                        exec.progress,
                        exec.task_id,
                        ProgressPhase::Executing,
                        *completed,
                        total,
                        Some(&item.entry.source),
                    );
                }
                Err(settle_error) => {
                    if error.is_none() {
                        error = Some(settle_error);
                    }
                }
            }
        }
        if let Some(error) = error {
            return Err(error);
        }
        Ok(())
    })
}

/// 条目级并发的执行路径（方案 §5.12）：任务内独立条目走有界工作池，
/// 资源键相交或顺序敏感的条目先收块再串行执行；Journal 记账按计划序；
/// 条目报告按计划序落槽（回滚定位依赖位置对齐）。
#[allow(clippy::too_many_arguments)]
fn execute_pooled(
    exec: &mut ExecContext<'_>,
    view: &WorkerView<'_>,
    plan: &PlanDocument,
    options: &ApplyOptions,
    concurrency: usize,
    total: u64,
) -> Result<Executed, KernelError> {
    let mut slots: Vec<Option<ApplyEntryReport>> = (0..plan.entries.len()).map(|_| None).collect();
    let mut applied: Vec<(usize, u64, FsOp)> = Vec::new();
    let mut failed = 0usize;
    let mut cancelled = false;
    let mut completed: u64 = 0;
    let mut chunk: Vec<ChunkItem<'_>> = Vec::new();
    let mut chunk_keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for (index, entry) in plan.entries.iter().enumerate() {
        if is_cancelled_token(exec.cancel) {
            cancelled = true;
            break;
        }
        match precheck_entry(exec, view, plan, options, entry) {
            Prechecked::Skipped(report) => slots[index] = Some(report),
            Prechecked::Failed(report) => {
                failed += 1;
                slots[index] = Some(report);
            }
            Prechecked::Proceed { target, overwrite } => {
                let eligible = is_pool_eligible(&entry.action);
                let keys = entry_resource_keys(entry, &target);
                if !eligible || !chunk_keys.is_disjoint(&keys) || chunk.len() >= concurrency {
                    flush_chunk(
                        exec,
                        view,
                        plan,
                        std::mem::take(&mut chunk),
                        &mut slots,
                        &mut applied,
                        &mut failed,
                        &mut completed,
                        total,
                    )?;
                    chunk_keys.clear();
                    // 收块期间取消可能已生效：不再派发新条目。
                    if is_cancelled_token(exec.cancel) {
                        cancelled = true;
                        break;
                    }
                }
                if let Err(error) = exec.license.consume(&plan.kind, 1) {
                    failed += 1;
                    slots[index] = Some(failed_entry(entry.entry_id.clone(), error));
                    continue;
                }
                if eligible {
                    chunk_keys.extend(keys);
                    chunk.push(ChunkItem {
                        entry,
                        target,
                        overwrite,
                        index,
                    });
                } else {
                    let target_param = edit_content_target_param(entry, &target);
                    let outcome = perform_with_retry(view, plan, entry, target_param, overwrite);
                    let report = settle_entry(
                        exec,
                        plan,
                        entry,
                        index,
                        overwrite,
                        outcome,
                        &mut applied,
                        &mut failed,
                    )?;
                    slots[index] = Some(report);
                    completed += 1;
                    report_progress_to(
                        exec.progress,
                        exec.task_id,
                        ProgressPhase::Executing,
                        completed,
                        total,
                        Some(&entry.source),
                    );
                }
            }
        }
    }
    flush_chunk(
        exec,
        view,
        plan,
        chunk,
        &mut slots,
        &mut applied,
        &mut failed,
        &mut completed,
        total,
    )?;
    // mid-stream 取消可能落在最后一条里（同串行路径，见上）。
    if !cancelled && is_cancelled_token(exec.cancel) {
        cancelled = true;
    }
    // 取消/中止时未处理条目不产报告（与串行路径一致）。
    let reports = slots.into_iter().flatten().collect();
    Ok((reports, applied, failed, cancelled))
}

/// 执行器产出：条目报告、已记账（计划序号, Journal seq, reverse）、失败数、是否取消。
type Executed = (Vec<ApplyEntryReport>, Vec<(usize, u64, FsOp)>, usize, bool);

fn resolve_target(
    entry: &crate::plan::PlanEntry,
    target: &str,
    options: &ApplyOptions,
    fs: &dyn FsAdapter,
    case_sense: CaseSense,
) -> Result<TargetDecision, KernelError> {
    let occupied = fs.stat(target).is_ok();
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
                target, &taken, case_sense,
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
pub(crate) struct Performed {
    pub(crate) skipped: bool,
    pub(crate) replaced: bool,
    #[allow(dead_code)]
    pub(crate) final_path: String,
    pub(crate) forward: FsOp,
    pub(crate) reverse: FsOp,
    /// 条目级明细（来自注入的内容变换），随 `ApplyEntryReport.detail` 上报。
    pub(crate) detail: Option<String>,
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

/// 条目执行的工作视图：perform 家族实际使用的字段子集（设计 01 §5.1）。
/// 并发路径把它交给工作线程共享（全部字段 `Send + Sync`）；
/// Journal、配额与进度记账只留在主线程（`&mut` 借用不可跨线程）。
pub(crate) struct WorkerView<'a> {
    pub(crate) fs: &'a dyn FsAdapter,
    case_sense: CaseSense,
    content_editor: Option<ContentEditor<'a>>,
    content_rewriter: Option<&'a dyn ContentRewriter>,
    directory_lister: Option<&'a dyn DirectoryLister>,
    network_retry: Option<&'a dyn crate::retry::NetworkRetry>,
    /// 取消端口：流式内容重写在**读块边界**问它（方案 §7.7 的 mid-stream 取消）。
    cancel: Option<&'a dyn CancelToken>,
    task_id: &'a str,
    /// 进度端口：**条目内（字节级）**进度用它上报（方案 §7.5 双层进度的条目层）。
    /// 与取消端口一样是 `&dyn`（`ProgressSink: Send + Sync`），并发工作线程共享安全。
    progress: Option<&'a dyn ProgressSink>,
}

fn perform_listing(
    view: &WorkerView<'_>,
    plan: &PlanDocument,
    entry: &crate::plan::PlanEntry,
    target: &str,
    overwrite: bool,
    backup: &mut Option<String>,
) -> Result<Performed, KernelError> {
    let options = listing_options(plan)?;
    if let (Ok(root), Ok(target_path)) = (try_normalize(&entry.source), try_normalize(target)) {
        if is_under(&target_path, &root, view.case_sense) {
            return Err(KernelError::invalid_params(
                "listing target must be outside the listed root",
            ));
        }
    }
    let lister = view.directory_lister.ok_or_else(|| {
        KernelError::new(
            "E5001",
            "directory lister is not available for this apply context",
            None,
            false,
        )
    })?;
    let backup = if overwrite {
        Some(backup_once(view.fs, target, backup)?)
    } else {
        None
    };
    view.fs.write_atomic_with(target, &mut |out| {
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
            to: view.fs.quarantine_target_path(&file_name_of(&final_path))?,
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
        detail: None,
    })
}

/// copy 覆盖与 edit-content 先备份原内容（`dataRef`），copy 新建的逆是
/// "产物进隔离目录"（reverse=`Quarantine`，路径由适配层预留）。
/// 把一块字节写入流式 writer；io 错误统一映射为 E3005（与内容改写同码）。
pub(crate) fn write_stream(out: &mut dyn std::io::Write, bytes: &[u8]) -> Result<(), KernelError> {
    out.write_all(bytes).map_err(|error| {
        KernelError::new(
            "E3005",
            "stream write failed",
            Some(error.to_string()),
            false,
        )
    })
}

/// 流式 CSV 源的执行期参数（ADR-0054）：编码/非法序列策略来自计划快照，
/// `expected_size` 仅 F06（csvMerge 载荷带逐源大小）给出。
pub(crate) struct StreamCsvInput<'a> {
    pub(crate) source: &'a str,
    pub(crate) expected_size: Option<u64>,
    pub(crate) encoding: SourceEncoding,
    pub(crate) invalid: InvalidSequencePolicy,
    pub(crate) delimiter: u8,
}

/// A 类流式 CSV 管道（ADR-0054）：`read_stream` → 流式解码 UTF-8 → `RecordFeeder`
/// 增量记录 → 逐条回调。记录语义与"整段 read_bytes＋transcode＋parse_records"
/// 完全一致；内存 O(块＋单条未完记录)。`expected_size` 给出时按 F06 既有口径
/// 复核源大小（增长即止、收尾核对短缺），E2010 与整段路径同码同消息。
pub(crate) fn stream_csv_records(
    view: &WorkerView<'_>,
    input: &StreamCsvInput<'_>,
    on_record: &mut dyn FnMut(Vec<String>) -> Result<(), KernelError>,
) -> Result<(), KernelError> {
    let StreamCsvInput {
        source,
        expected_size,
        encoding,
        invalid,
        delimiter,
    } = *input;
    let mut feeder = RecordFeeder::new(delimiter);
    let mut transcoder =
        StreamTranscoder::new(encoding, TargetEncoding::Utf8 { bom: false }, invalid);
    let mut decoded = Vec::new();
    let mut seen = 0u64;
    view.fs.read_stream(source, &mut |chunk| {
        seen += chunk.len() as u64;
        if let Some(expected) = expected_size {
            if seen > expected {
                return Err(KernelError::new(
                    "E2010",
                    format!("source size changed since plan preview: {source}"),
                    None,
                    false,
                ));
            }
        }
        transcoder.push(chunk, &mut decoded)?;
        for record in feeder.push(&decoded) {
            on_record(record)?;
        }
        // 复用容量：已消费的字节清掉，块间不必反复分配。
        decoded.clear();
        Ok(())
    })?;
    transcoder.finish(&mut decoded)?;
    for record in feeder.push(&decoded) {
        on_record(record)?;
    }
    for record in feeder.finish() {
        on_record(record)?;
    }
    if let Some(expected) = expected_size {
        if seen != expected {
            return Err(KernelError::new(
                "E2010",
                format!("source size changed since plan preview: {source}"),
                None,
                false,
            ));
        }
    }
    Ok(())
}

/// 流式 CSV 产物写出器：逐条把渲染好的 UTF-8 行经流式转码写出（io 错误统一
/// E3005）。目标 BOM 不在这里写——调用方在产物最前面写一次（与 F04 合并臂同
/// 口径），这里恒用 `without_bom`。
pub(crate) struct CsvStreamWriter<'a> {
    pub(crate) transcoder: StreamTranscoder,
    pub(crate) writer: &'a mut dyn std::io::Write,
}

impl CsvStreamWriter<'_> {
    pub(crate) fn emit(&mut self, line: &str) -> Result<(), KernelError> {
        let mut encoded = Vec::new();
        self.transcoder.push(line.as_bytes(), &mut encoded)?;
        write_stream(self.writer, &encoded)
    }

    pub(crate) fn emit_record(
        &mut self,
        fields: &[String],
        delimiter: u8,
    ) -> Result<(), KernelError> {
        let mut line = String::new();
        write_record(fields, delimiter, &mut line);
        self.emit(&line)
    }
}

/// 同一次条目执行（含 D1 重试）内复用同一个隔离备份：首次调用
/// `backup_to_quarantine` 后缓存路径，后续重试不再重复备份。缓存按条目局部持有
/// （`perform_with_retry` 的栈上变量），并发条目之间不共享；成功时该路径作为
/// Journal `dataRef`，条目最终失败时由隔离目录清理按保留期统一回收。
pub(crate) fn backup_once(
    fs: &dyn FsAdapter,
    target: &str,
    backup: &mut Option<String>,
) -> Result<String, KernelError> {
    if let Some(path) = backup.as_deref() {
        return Ok(path.to_string());
    }
    let path = fs.backup_to_quarantine(target)?;
    *backup = Some(path.clone());
    Ok(path)
}

fn perform(
    view: &WorkerView<'_>,
    plan: &PlanDocument,
    entry: &crate::plan::PlanEntry,
    target: Option<&str>,
    overwrite: bool,
    backup: &mut Option<String>,
) -> Result<Result<Performed, KernelError>, KernelError> {
    let policy = if overwrite {
        ConflictPolicy::Overwrite
    } else {
        ConflictPolicy::Skip
    };
    let mut run = || -> Result<Performed, KernelError> {
        match entry.action {
            PlanAction::Rename => {
                let target = target.expect("validated: rename requires a target");
                let outcome = view.fs.rename(&entry.source, target, policy)?;
                to_performed(outcome, |final_path| FsOp::Rename {
                    from: entry.source.clone(),
                    to: final_path,
                })
            }
            PlanAction::Move => {
                let target = target.expect("validated: move requires a target");
                ensure_target_dir(view, target)?;
                if view.fs.same_volume(&entry.source, target) {
                    // 同盘：改名即完成（瞬间）。
                    let outcome = view.fs.move_path(&entry.source, target, policy)?;
                    to_performed(outcome, |final_path| FsOp::Rename {
                        from: entry.source.clone(),
                        to: final_path,
                    })
                } else {
                    // 跨盘移动＝复制→校验大小与哈希→删源；校验不通过绝不删源（§5.8）。
                    let outcome = view.fs.copy(&entry.source, target, policy)?;
                    if outcome.outcome == PathOutcomeKind::Skipped {
                        let final_path = outcome.final_path.clone();
                        return Ok(Performed {
                            skipped: true,
                            replaced: false,
                            final_path: final_path.clone(),
                            forward: FsOp::Rename {
                                from: entry.source.clone(),
                                to: final_path.clone(),
                            },
                            reverse: FsOp::Rename {
                                from: final_path,
                                to: entry.source.clone(),
                            },
                            detail: None,
                        });
                    }
                    if let Err(error) =
                        verify_move_integrity(view, &entry.source, &outcome.final_path)
                    {
                        // 校验不通过绝不删源；坏副本尽力清除后按失败条目报告（§5.8）。
                        let _ = view.fs.remove_file(&outcome.final_path);
                        return Err(error);
                    }
                    view.fs.remove_file(&entry.source)?;
                    let final_path = outcome.final_path.clone();
                    let forward = FsOp::Rename {
                        from: entry.source.clone(),
                        to: final_path.clone(),
                    };
                    let reverse = forward.inverse().ok_or_else(|| {
                        KernelError::internal(
                            "move reverse is not expressible",
                            format!("{forward:?}"),
                        )
                    })?;
                    Ok(Performed {
                        skipped: false,
                        replaced: outcome.outcome == PathOutcomeKind::Replaced,
                        final_path,
                        forward,
                        reverse,
                        detail: None,
                    })
                }
            }
            PlanAction::Copy => {
                let target = target.expect("validated: copy requires a target");
                ensure_target_dir(view, target)?;
                // 覆盖已有目标：先备份原内容（设计 01 §8 dataRef 语义）。
                let backup = if overwrite {
                    Some(backup_once(view.fs, target, backup)?)
                } else {
                    None
                };
                let outcome = view.fs.copy(&entry.source, target, policy)?;
                let final_path = outcome.final_path.clone();
                let reverse = match backup {
                    Some(backup) => FsOp::WriteAtomic {
                        path: final_path.clone(),
                        data_ref: Some(backup),
                    },
                    None => FsOp::Quarantine {
                        from: final_path.clone(),
                        to: view.fs.quarantine_target_path(&file_name_of(&final_path))?,
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
                    detail: None,
                })
            }
            PlanAction::Split => {
                let target = target.expect("validated: split requires a target");
                let range = entry
                    .range
                    .ok_or_else(|| KernelError::invalid_params("split entries require a range"))?;
                let backup = if overwrite {
                    Some(backup_once(view.fs, target, backup)?)
                } else {
                    None
                };
                // 每片都必须以 BOM 开头（UTF-16 源，方案 §5.1）：该片不含源文件开头时补写前缀。
                let prefix: &[u8] = entry
                    .csv
                    .as_ref()
                    .and_then(|csv| csv.shard_bom)
                    .map_or(&[], crate::encoding::Bom::bytes);
                let header = entry.csv.as_ref().and_then(|csv| csv.header);
                let outcome = if prefix.is_empty() && header.is_none() {
                    view.fs.copy_range_atomic(
                        &entry.source,
                        range.start,
                        range.end,
                        target,
                        policy,
                    )?
                } else {
                    // F05：表头区间 + 记录区间一次写成同一分片（CSV 记录边界已在计划期确定）。
                    let mut ranges = Vec::with_capacity(2);
                    if let Some(header) = header {
                        ranges.push((header.start, header.end));
                    }
                    ranges.push((range.start, range.end));
                    view.fs
                        .copy_ranges_atomic(&entry.source, &ranges, prefix, target, policy)?
                };
                let final_path = outcome.final_path.clone();
                let reverse = match backup {
                    Some(backup) => FsOp::WriteAtomic {
                        path: final_path.clone(),
                        data_ref: Some(backup),
                    },
                    None => FsOp::Quarantine {
                        from: final_path.clone(),
                        to: view.fs.quarantine_target_path(&file_name_of(&final_path))?,
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
                    detail: None,
                })
            }
            PlanAction::Merge => {
                let target = target.expect("validated: merge requires a target");
                if let Some(csv_merge) = &entry.csv_merge {
                    return perform_csv_merge(view, target, overwrite, csv_merge, backup);
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
                    Some(backup_once(view.fs, target, backup)?)
                } else {
                    None
                };
                // 逐源流式解码并追加：内存占用与源大小无关（O(块)）。
                // BOM 只在产物最前面写一次；各输入自身 BOM 由解码器剥离。
                // 条目内进度的总量＝各源字节数之和（计划期已探明，方案 §9.1 允许预估口径）。
                let entry_total: u64 = merge.sources.iter().map(|source| source.size).sum();
                let mut entry_done = 0u64;
                view.fs.write_atomic_with(target, &mut |writer| {
                    if let Some(prefix) = target_encoding.bom_prefix() {
                        write_stream(writer, prefix)?;
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
                        let mut transcoder =
                            StreamTranscoder::new(encoding, target_encoding.without_bom(), invalid);
                        // 分块解码出的字节先落到小缓冲，再交给 writer；缓冲在块之间复用。
                        let mut buffer = Vec::new();
                        let mut seen = 0u64;
                        view.fs.read_stream(&source.path, &mut |chunk| {
                            seen += chunk.len() as u64;
                            entry_done += chunk.len() as u64;
                            transcoder.push(chunk, &mut buffer)?;
                            write_stream(writer, &buffer)?;
                            buffer.clear();
                            // 条目内进度：合并是一条 entry，不报字节的话界面长时间停在 0/1。
                            report_entry_progress(view, entry_done, entry_total);
                            Ok(())
                        })?;
                        if seen != source.size {
                            return Err(KernelError::new(
                                "E2010",
                                format!("source size changed since plan preview: {}", source.path),
                                None,
                                false,
                            ));
                        }
                        transcoder.finish(&mut buffer)?;
                        write_stream(writer, &buffer)?;
                    }
                    Ok(())
                })?;
                let final_path = target.to_string();
                let reverse = match backup {
                    Some(backup) => FsOp::WriteAtomic {
                        path: final_path.clone(),
                        data_ref: Some(backup),
                    },
                    None => FsOp::Quarantine {
                        from: final_path.clone(),
                        to: view.fs.quarantine_target_path(&file_name_of(&final_path))?,
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
                    detail: None,
                })
            }
            PlanAction::CsvExtract => {
                let target = target.expect("validated: csv-extract requires a target");
                let payload = entry.csv_extract.as_ref().ok_or_else(|| {
                    KernelError::invalid_params("csv-extract entries require a csvExtract payload")
                })?;
                perform_csv_extract(view, &entry.source, target, overwrite, payload, backup)
            }
            PlanAction::CsvColumns => {
                let target = target.expect("validated: csv-columns requires a target");
                let payload = entry.csv_columns.as_ref().ok_or_else(|| {
                    KernelError::invalid_params("csv-columns entries require a csvColumns payload")
                })?;
                perform_csv_columns(view, &entry.source, target, overwrite, payload, backup)
            }
            PlanAction::CsvSort => {
                let target = target.expect("validated: csv-sort requires a target");
                let payload = entry.csv_sort.as_ref().ok_or_else(|| {
                    KernelError::invalid_params("csv-sort entries require a csvSort payload")
                })?;
                crate::csv_sort::perform_csv_sort(
                    view,
                    &entry.source,
                    target,
                    overwrite,
                    payload,
                    backup,
                )
            }
            PlanAction::CsvRows => {
                let target = target.expect("validated: csv-rows requires a target");
                let payload = entry.csv_rows.as_ref().ok_or_else(|| {
                    KernelError::invalid_params("csv-rows entries require a csvRows payload")
                })?;
                perform_csv_rows(view, &entry.source, target, overwrite, payload, backup)
            }
            PlanAction::CsvDedup => {
                let target = target.expect("validated: csv-dedup requires a target");
                let payload = entry.csv_dedup.as_ref().ok_or_else(|| {
                    KernelError::invalid_params("csv-dedup entries require a csvDedup payload")
                })?;
                perform_csv_dedup(view, &entry.source, target, overwrite, payload, backup)
            }
            PlanAction::CsvFilter => {
                let target = target.expect("validated: csv-filter requires a target");
                let payload = entry.csv_filter.as_ref().ok_or_else(|| {
                    KernelError::invalid_params("csv-filter entries require a csvFilter payload")
                })?;
                perform_csv_filter(view, &entry.source, target, overwrite, payload, backup)
            }
            PlanAction::Listing => {
                let target = target.expect("validated: listing requires a target");
                perform_listing(view, plan, entry, target, overwrite, backup)
            }
            PlanAction::CreateDir => {
                let target = target.expect("validated: create-dir requires a target");
                view.fs.create_dir(target)?;
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
                    detail: None,
                })
            }
            PlanAction::EditContent | PlanAction::Transcode => {
                // 就地改写：备份 →（流式重写或整段变换）→ 原子写回。
                // `transcode` 是与 `edit-content` 同一执行路径的旧动作名（C01），
                // 变换由壳层按计划 kind 注入的编辑器/重写器完成；两者共用备份复用。
                let backup = backup_once(view.fs, &entry.source, backup)?;
                let detail = if let Some(rewriter) = view.content_rewriter {
                    // 明细经闭包局部变量带出（rewrite_atomic_with 的回调签名固定为 ()）。
                    let mut rewrite_detail = None;
                    view.fs
                        .rewrite_atomic_with(&entry.source, &mut |reader, writer| {
                            let outcome = rewriter.rewrite(
                                entry,
                                reader,
                                writer,
                                view.cancel.unwrap_or(&crate::progress::NoCancel),
                            )?;
                            rewrite_detail = outcome.detail;
                            Ok(())
                        })?;
                    rewrite_detail
                } else {
                    let data = view.fs.read_bytes(&entry.source)?;
                    let editor = view
                        .content_editor
                        .expect("validated: edit-content requires an editor");
                    let edited = editor(&data)?;
                    let detail = edited.detail;
                    view.fs.write_atomic(&entry.source, &edited.bytes)?;
                    detail
                };
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
                    detail,
                })
            }
            PlanAction::Delete => {
                let quarantined = view.fs.remove_to_quarantine(&entry.source)?;
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
                    detail: None,
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

/// 执行 `csv-columns` 条目（与 F07 同构：备份 → 解析 → 变换 → 原子写出）。
fn perform_csv_columns(
    view: &WorkerView<'_>,
    source: &str,
    target: &str,
    overwrite: bool,
    payload: &PlanCsvColumns,
    backup: &mut Option<String>,
) -> Result<crate::generic_apply::Performed, KernelError> {
    use crate::csv_columns::{split_value, SplitBy};
    use crate::journal::FsOp;

    let encoding = payload.encoding.ok_or_else(|| {
        KernelError::new(
            "E3001",
            "source encoding requires manual confirmation",
            None,
            false,
        )
    })?;
    let target_encoding = TargetEncoding::parse(&payload.target_encoding)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvColumns target encoding"))?;
    let invalid = InvalidSequencePolicy::parse(&payload.invalid).ok_or_else(|| {
        KernelError::invalid_params("unsupported csvColumns invalid-sequence policy")
    })?;
    let delimiter = Delimiter::parse(&payload.delimiter)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvColumns delimiter"))?;

    let backup = if overwrite {
        Some(backup_once(view.fs, target, backup)?)
    } else {
        None
    };
    // 流式执行（ADR-0055）：逐行拆分/合并即读即写，内存 O(块＋单条记录)。
    // 模式与拆分方式先于读源判定（与整段路径同码，顺序无观察面）。
    enum ColumnsMode {
        Split {
            index: u64,
            parts: u64,
            new_names: Vec<String>,
            by: SplitBy,
        },
        Merge {
            picks: Vec<crate::plan::PlanCsvExtractColumn>,
            joiner: String,
            target_name: String,
        },
    }
    let mode = match (payload.mode.as_str(), &payload.split, &payload.merge) {
        ("split", Some(split), _) => {
            let by = match (&split.separator, &split.regex) {
                (Some(separator), _) => SplitBy::Separator(separator.clone()),
                (None, Some(pattern)) => SplitBy::Regex(pattern.clone()),
                _ => {
                    return Err(KernelError::invalid_params(
                        "csvColumns split requires a separator or regex",
                    ))
                }
            };
            ColumnsMode::Split {
                index: split.index,
                parts: split.parts,
                new_names: split.new_names.clone(),
                by,
            }
        }
        ("merge", _, Some(merge)) => ColumnsMode::Merge {
            picks: merge.picks.clone(),
            joiner: merge.joiner.clone(),
            target_name: merge.target_name.clone(),
        },
        _ => {
            return Err(KernelError::invalid_params(
                "csvColumns payload does not match its mode",
            ))
        }
    };
    view.fs.write_atomic_with(target, &mut |writer| {
        let mut sink = CsvStreamWriter {
            transcoder: StreamTranscoder::new(
                SourceEncoding::Utf8,
                target_encoding.without_bom(),
                invalid,
            ),
            writer,
        };
        if let Some(prefix) = target_encoding.bom_prefix() {
            write_stream(sink.writer, prefix)?;
        }
        let mut header_checked = false;
        stream_csv_records(
            view,
            &StreamCsvInput {
                source,
                expected_size: None,
                encoding,
                invalid,
                delimiter: delimiter.as_byte(),
            },
            &mut |row| {
                if payload.has_header && !header_checked {
                    header_checked = true;
                    if normalized_columns(&row) != payload.columns {
                        return Err(KernelError::new(
                            "E2010",
                            "source header changed since plan preview; re-plan before applying",
                            None,
                            true,
                        ));
                    }
                    // 表头行按模式派生新列名（与整段路径一致：原始表头＋追加/插入）。
                    let mut names = row;
                    match &mode {
                        ColumnsMode::Split {
                            index, new_names, ..
                        } => {
                            for (offset, name) in new_names.iter().enumerate() {
                                let position = *index as usize + offset;
                                names.insert(position.min(names.len()), name.clone());
                            }
                        }
                        ColumnsMode::Merge { target_name, .. } => {
                            names.push(target_name.clone());
                        }
                    }
                    return sink.emit_record(&names, delimiter.as_byte());
                }
                match &mode {
                    ColumnsMode::Split {
                        index,
                        parts: expected_parts,
                        by,
                        ..
                    } => {
                        let mut fields = row;
                        // 源列取值用 1-based 序号换算的 0-based 下标（差一曾致取错列）。
                        let source_value =
                            fields.get(*index as usize - 1).cloned().unwrap_or_default();
                        let mut parts = split_value(&source_value, by);
                        // 不足评估段数的行补空串（§5.15 空值处理）；超出部分原样保留不截断。
                        while parts.len() < *expected_parts as usize {
                            parts.push(String::new());
                        }
                        // 0-based 插入点＝源列之后（源列 1-based index 的 0-based 位置）。
                        for (offset, part) in parts.iter().enumerate() {
                            let position = *index as usize + offset;
                            fields.insert(position.min(fields.len()), part.clone());
                        }
                        sink.emit_record(&fields, delimiter.as_byte())
                    }
                    ColumnsMode::Merge { picks, joiner, .. } => {
                        let joined = picks
                            .iter()
                            .map(|pick| {
                                // 1-based 序号换算 0-based 下标（差一曾致取错列）。
                                row.get(pick.index as usize - 1)
                                    .cloned()
                                    .unwrap_or_default()
                            })
                            .collect::<Vec<_>>()
                            .join(joiner);
                        let mut fields = row;
                        fields.push(joined);
                        sink.emit_record(&fields, delimiter.as_byte())
                    }
                }
            },
        )?;
        if payload.has_header && !header_checked {
            // 与整段路径同口径：有表头而源里没有表头 ⇒ E2010。
            return Err(KernelError::new(
                "E2010",
                "source header changed since plan preview; re-plan before applying",
                None,
                true,
            ));
        }
        Ok(())
    })?;
    let final_path = target.to_string();
    let reverse = match backup {
        Some(backup) => FsOp::WriteAtomic {
            path: final_path.clone(),
            data_ref: Some(backup),
        },
        None => FsOp::Quarantine {
            from: final_path.clone(),
            to: view
                .fs
                .quarantine_target_path(crate::conflict::file_name_of(&final_path))?,
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
        detail: None,
    })
}

/// 目标子目录自动创建（§5.8"目标目录不存在→按配置自动创建"）。
/// 目录创建是移动/复制的实现细节：不单独记 Journal，撤销后空目录残留可接受
/// （§7.8 回滚语义不含目录删除）。
fn ensure_target_dir(view: &WorkerView<'_>, target: &str) -> Result<(), KernelError> {
    match target.rfind(['/', '\\']) {
        Some(index) if index > 0 => view.fs.create_dir_all(&target[..index]),
        _ => Ok(()),
    }
}

/// 跨盘移动的完整性校验（§5.8"复制→校验大小与哈希→删除源"）：
/// 大小必须一致；两侧适配器都能提供 blake3 时进一步比对哈希——任一侧不可
/// 哈希则退化为大小校验（ADR-0036）。失败返回 E3005，调用方绝不删源。
fn verify_move_integrity(
    view: &WorkerView<'_>,
    source: &str,
    target: &str,
) -> Result<(), KernelError> {
    let source_size = view.fs.stat(source)?.size;
    let target_size = view.fs.stat(target)?.size;
    if source_size != target_size {
        return Err(KernelError::new(
            "E3005",
            "cross-volume move verification failed: size mismatch; source is kept",
            Some(format!("{source} → {target}")),
            true,
        ));
    }
    let source_hash = view.fs.content_hash(source, "blake3")?;
    let target_hash = view.fs.content_hash(target, "blake3")?;
    if let (Some(source_hash), Some(target_hash)) = (source_hash, target_hash) {
        if source_hash != target_hash {
            return Err(KernelError::new(
                "E3005",
                "cross-volume move verification failed: hash mismatch; source is kept",
                Some(format!("{source} → {target}")),
                true,
            ));
        }
    }
    Ok(())
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
        detail: None,
    })
}

fn file_name_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string())
}

fn fingerprint_mismatch(
    fs: &dyn FsAdapter,
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
    match fs.content_hash(&entry.source, &expected.algorithm) {
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
    fs: &dyn FsAdapter,
    protected_dirs: &[ProtectedDir],
    case_sense: CaseSense,
    path: &str,
    normalized: &str,
) -> Option<KernelError> {
    if protected_dirs.is_empty() {
        return None;
    }
    let canonical = fs.canonicalize(path).ok();
    protected_by(normalized, canonical.as_deref(), protected_dirs, case_sense).map(|hit| {
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
    view: &WorkerView<'_>,
    protected_dirs: &[ProtectedDir],
    case_sense: CaseSense,
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
        if let Some(error) =
            protected_path_hit(view.fs, protected_dirs, case_sense, path, &normalized)
        {
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
        let stat = match view.fs.stat(path) {
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

fn validate_csv_sort(payload: &PlanCsvSort) -> Result<(), KernelError> {
    if payload.encoding.is_none() {
        return Err(KernelError::new(
            "E3001",
            "source encoding requires manual confirmation",
            None,
            false,
        ));
    }
    if payload.keys.is_empty() {
        return Err(KernelError::invalid_params(
            "csvSort payload requires at least one sort key",
        ));
    }
    for key in &payload.keys {
        if key.index == 0 || key.index > payload.column_count {
            return Err(KernelError::new(
                "E3004",
                format!("sort key column out of range: {}", key.index),
                None,
                false,
            ));
        }
        if key.order != "asc" && key.order != "desc" {
            return Err(KernelError::invalid_params(format!(
                "unsupported sort order: {}",
                key.order
            )));
        }
        if key.kind != "numeric" && key.kind != "date" && key.kind != "lexicographic" {
            return Err(KernelError::invalid_params(format!(
                "unsupported sort key kind: {}",
                key.kind
            )));
        }
    }
    Ok(())
}

fn validate_csv_columns(payload: &PlanCsvColumns) -> Result<(), KernelError> {
    if payload.encoding.is_none() {
        return Err(KernelError::new(
            "E3001",
            "source encoding requires manual confirmation",
            None,
            false,
        ));
    }
    match (payload.mode.as_str(), &payload.split, &payload.merge) {
        ("split", Some(split), None) => {
            if split.parts == 0 {
                return Err(KernelError::invalid_params(
                    "csvColumns split requires an evaluated parts count of at least 1",
                ));
            }
            match (&split.separator, &split.regex) {
                (Some(separator), None) if !separator.is_empty() => {}
                (None, Some(pattern)) => {
                    crate::regex_rename::compile_regex(pattern)?;
                }
                _ => {
                    return Err(KernelError::invalid_params(
                        "csvColumns split requires exactly one of separator or regex",
                    ))
                }
            }
            if payload.has_header && split.new_names.len() != split.parts as usize {
                return Err(KernelError::invalid_params(
                    "csvColumns split new column names must match the evaluated parts count",
                ));
            }
        }
        ("merge", None, Some(merge)) => {
            if merge.picks.len() < 2 {
                return Err(KernelError::invalid_params(
                    "csvColumns merge requires at least two source columns",
                ));
            }
        }
        _ => {
            return Err(KernelError::invalid_params(
                "csvColumns payload does not match its mode",
            ))
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

fn validate_csv_dedup(payload: &PlanCsvDedup) -> Result<(), KernelError> {
    if TargetEncoding::parse(&payload.target_encoding).is_none() {
        return Err(KernelError::invalid_params(
            "csv-dedup targetEncoding is not a supported encoding",
        ));
    }
    if InvalidSequencePolicy::parse(&payload.invalid).is_none() {
        return Err(KernelError::invalid_params(
            "csv-dedup invalid policy is not supported",
        ));
    }
    if Delimiter::parse(&payload.delimiter).is_none() {
        return Err(KernelError::invalid_params(
            "csv-dedup delimiter is not supported",
        ));
    }
    match payload.key.as_str() {
        "row" => {}
        "columns" if !payload.key_columns.is_empty() => {}
        _ => {
            return Err(KernelError::invalid_params(
                "csv-dedup key/columns selection is invalid",
            ))
        }
    }
    if payload
        .key_columns
        .iter()
        .any(|column| column.index >= payload.column_count)
    {
        return Err(KernelError::invalid_params(
            "csv-dedup key column index is out of range",
        ));
    }
    Ok(())
}

fn validate_csv_filter(payload: &PlanCsvFilter) -> Result<(), KernelError> {
    if TargetEncoding::parse(&payload.target_encoding).is_none() {
        return Err(KernelError::invalid_params(
            "csv-filter targetEncoding is not a supported encoding",
        ));
    }
    if InvalidSequencePolicy::parse(&payload.invalid).is_none() {
        return Err(KernelError::invalid_params(
            "csv-filter invalid policy is not supported",
        ));
    }
    if Delimiter::parse(&payload.delimiter).is_none() {
        return Err(KernelError::invalid_params(
            "csv-filter delimiter is not supported",
        ));
    }
    if FilterOp::parse(&payload.condition.op).is_none() {
        return Err(KernelError::invalid_params(
            "csv-filter condition operator is not supported",
        ));
    }
    if !matches!(payload.mode.as_str(), "keep" | "drop") {
        return Err(KernelError::invalid_params(
            "csv-filter mode must be keep or drop",
        ));
    }
    if payload.condition.column.index >= payload.column_count {
        return Err(KernelError::invalid_params(
            "csv-filter condition column index is out of range",
        ));
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
    view: &WorkerView<'_>,
    source: &str,
    target: &str,
    overwrite: bool,
    payload: &PlanCsvExtract,
    backup: &mut Option<String>,
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
        Some(backup_once(view.fs, target, backup)?)
    } else {
        None
    };
    // 流式执行（ADR-0054）：逐条记录即读即写，内存 O(块＋单条记录)。
    view.fs.write_atomic_with(target, &mut |writer| {
        let mut sink = CsvStreamWriter {
            transcoder: StreamTranscoder::new(
                SourceEncoding::Utf8,
                target_encoding.without_bom(),
                invalid,
            ),
            writer,
        };
        if let Some(prefix) = target_encoding.bom_prefix() {
            write_stream(sink.writer, prefix)?;
        }
        if payload.has_header {
            let names: Vec<String> = payload
                .selections
                .iter()
                .map(|selection| selection.name.clone().unwrap_or_default())
                .collect();
            sink.emit_record(&names, delimiter.as_byte())?;
        }
        let mut header_checked = false;
        stream_csv_records(
            view,
            &StreamCsvInput {
                source,
                expected_size: None,
                encoding,
                invalid,
                delimiter: delimiter.as_byte(),
            },
            &mut |row| {
                if payload.has_header && !header_checked {
                    header_checked = true;
                    if normalized_columns(&row) != payload.columns {
                        return Err(KernelError::new(
                            "E2010",
                            "source header changed since plan preview; re-plan before applying",
                            None,
                            true,
                        ));
                    }
                    return Ok(());
                }
                let fields: Vec<String> = payload
                    .selections
                    .iter()
                    .map(|selection| {
                        row.get(selection.index as usize)
                            .cloned()
                            .unwrap_or_default()
                    })
                    .collect();
                sink.emit_record(&fields, delimiter.as_byte())
            },
        )?;
        if payload.has_header && !header_checked {
            // 与整段路径同口径：有表头而源里没有表头 ⇒ E2010。
            return Err(KernelError::new(
                "E2010",
                "source header changed since plan preview; re-plan before applying",
                None,
                true,
            ));
        }
        Ok(())
    })?;
    let final_path = target.to_string();
    let reverse = match backup {
        Some(backup) => FsOp::WriteAtomic {
            path: final_path.clone(),
            data_ref: Some(backup),
        },
        None => FsOp::Quarantine {
            from: final_path.clone(),
            to: view.fs.quarantine_target_path(&file_name_of(&final_path))?,
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
        detail: None,
    })
}

/// F09：只读取表头与选中的字节区间，解码/转码后原子写出。
fn perform_csv_range(
    view: &WorkerView<'_>,
    source: &str,
    target: &str,
    overwrite: bool,
    payload: &PlanCsvRows,
    backup: &mut Option<String>,
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
        Some(backup_once(view.fs, target, backup)?)
    } else {
        None
    };

    // 表头仍按需读取（要解析列名与计划期列清单核对），但它是**一行**、不是整个选中区间；
    // 数据区间改为流式解码＋原子写——内存 O(块)，不随选中区间线性增长
    // （E5 判据①，TC-F09-EXE-05 的缺口）。表头的转码结果缓存在小 Vec 里复用给写出。
    let mut header_out = Vec::new();
    if let Some(header) = payload.header_range {
        let header_raw = view.fs.read_ranges(source, &[(header.start, header.end)])?;
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
        // 目标编码的 BOM 由下面的流式写统一加在产物最前面，表头这里用 without_bom。
        header_out = transcode(
            &header_raw,
            encoding,
            target_encoding.without_bom(),
            invalid,
        )?
        .bytes;
    }

    view.fs.write_atomic_with(target, &mut |writer| {
        if let Some(prefix) = target_encoding.bom_prefix() {
            write_stream(writer, prefix)?;
        }
        write_stream(writer, &header_out)?;
        if let Some(data) = payload.data_range {
            let mut transcoder =
                StreamTranscoder::new(encoding, target_encoding.without_bom(), invalid);
            // 分块解码出的字节先落到小缓冲再交给 writer；缓冲在块之间复用。
            let mut buffer = Vec::new();
            view.fs
                .read_ranges_stream(source, &[(data.start, data.end)], &mut |chunk| {
                    transcoder.push(chunk, &mut buffer)?;
                    write_stream(writer, &buffer)?;
                    buffer.clear();
                    Ok(())
                })?;
            transcoder.finish(&mut buffer)?;
            write_stream(writer, &buffer)?;
        }
        Ok(())
    })?;
    let final_path = target.to_string();
    let reverse = match backup {
        Some(backup) => FsOp::WriteAtomic {
            path: final_path.clone(),
            data_ref: Some(backup),
        },
        None => FsOp::Quarantine {
            from: final_path.clone(),
            to: view.fs.quarantine_target_path(&file_name_of(&final_path))?,
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
        detail: None,
    })
}

/// C12：解码后按条件求值并按 keep/drop 写出，表头与选中行原样输出。
fn perform_csv_filter(
    view: &WorkerView<'_>,
    source: &str,
    target: &str,
    overwrite: bool,
    payload: &PlanCsvFilter,
    backup: &mut Option<String>,
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
        .ok_or_else(|| KernelError::invalid_params("unsupported csvFilter target encoding"))?;
    let invalid = InvalidSequencePolicy::parse(&payload.invalid).ok_or_else(|| {
        KernelError::invalid_params("unsupported csvFilter invalid-sequence policy")
    })?;
    let delimiter = Delimiter::parse(&payload.delimiter)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvFilter delimiter"))?;
    let op = FilterOp::parse(&payload.condition.op)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvFilter operator"))?;
    let mode_keep = match payload.mode.as_str() {
        "keep" => true,
        "drop" => false,
        _ => return Err(KernelError::invalid_params("unsupported csvFilter mode")),
    };
    let backup = if overwrite {
        Some(backup_once(view.fs, target, backup)?)
    } else {
        None
    };
    // 流式执行（ADR-0054）：逐行判定即读即写，内存 O(块＋单条记录)。
    let predicate = RowPredicate::new(
        payload.condition.column.index as usize,
        op,
        &payload.condition.value,
    );
    view.fs.write_atomic_with(target, &mut |writer| {
        let mut sink = CsvStreamWriter {
            transcoder: StreamTranscoder::new(
                SourceEncoding::Utf8,
                target_encoding.without_bom(),
                invalid,
            ),
            writer,
        };
        if let Some(prefix) = target_encoding.bom_prefix() {
            write_stream(sink.writer, prefix)?;
        }
        let mut header_checked = false;
        stream_csv_records(
            view,
            &StreamCsvInput {
                source,
                expected_size: None,
                encoding,
                invalid,
                delimiter: delimiter.as_byte(),
            },
            &mut |row| {
                if payload.has_header && !header_checked {
                    header_checked = true;
                    if normalized_columns(&row) != payload.columns {
                        return Err(KernelError::new(
                            "E2010",
                            "source header changed since plan preview; re-plan before applying",
                            None,
                            true,
                        ));
                    }
                    // 表头行原样输出（与 render_records 的整段路径一致）。
                    return sink.emit_record(&row, delimiter.as_byte());
                }
                if predicate.matches(&row) == mode_keep {
                    sink.emit_record(&row, delimiter.as_byte())?;
                }
                Ok(())
            },
        )?;
        if payload.has_header && !header_checked {
            // 与整段路径同口径：有表头而源里没有表头 ⇒ E2010。
            return Err(KernelError::new(
                "E2010",
                "source header changed since plan preview; re-plan before applying",
                None,
                true,
            ));
        }
        Ok(())
    })?;
    let final_path = target.to_string();
    let reverse = match backup {
        Some(backup) => FsOp::WriteAtomic {
            path: final_path.clone(),
            data_ref: Some(backup),
        },
        None => FsOp::Quarantine {
            from: final_path.clone(),
            to: view.fs.quarantine_target_path(&file_name_of(&final_path))?,
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
        detail: None,
    })
}

/// F08：解码后按前 N / 后 N 选择数据行，表头与行内容原样输出。
fn perform_csv_dedup(
    view: &WorkerView<'_>,
    source: &str,
    target: &str,
    overwrite: bool,
    payload: &PlanCsvDedup,
    backup: &mut Option<String>,
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
        .ok_or_else(|| KernelError::invalid_params("unsupported csvDedup target encoding"))?;
    let invalid = InvalidSequencePolicy::parse(&payload.invalid).ok_or_else(|| {
        KernelError::invalid_params("unsupported csvDedup invalid-sequence policy")
    })?;
    let delimiter = Delimiter::parse(&payload.delimiter)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvDedup delimiter"))?;
    let backup = if overwrite {
        Some(backup_once(view.fs, target, backup)?)
    } else {
        None
    };
    // 流式执行（ADR-0054）：判重键集合驻留内存 O(唯一键)，记录本身即读即写。
    view.fs.write_atomic_with(target, &mut |writer| {
        let mut sink = CsvStreamWriter {
            transcoder: StreamTranscoder::new(
                SourceEncoding::Utf8,
                target_encoding.without_bom(),
                invalid,
            ),
            writer,
        };
        if let Some(prefix) = target_encoding.bom_prefix() {
            write_stream(sink.writer, prefix)?;
        }
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut header_checked = false;
        stream_csv_records(
            view,
            &StreamCsvInput {
                source,
                expected_size: None,
                encoding,
                invalid,
                delimiter: delimiter.as_byte(),
            },
            &mut |row| {
                if payload.has_header && !header_checked {
                    header_checked = true;
                    if normalized_columns(&row) != payload.columns {
                        return Err(KernelError::new(
                            "E2010",
                            "source header changed since plan preview; re-plan before applying",
                            None,
                            true,
                        ));
                    }
                    // 表头行原样输出（与 render_records 的整段路径一致）。
                    return sink.emit_record(&row, delimiter.as_byte());
                }
                // 与 dedup_records 同口径：保留首次出现（key_of 的 JSON 序列化键）。
                if seen.insert(key_of(&row, &payload.key_columns)) {
                    sink.emit_record(&row, delimiter.as_byte())?;
                }
                Ok(())
            },
        )?;
        if payload.has_header && !header_checked {
            // 与整段路径同口径：有表头而源里没有表头 ⇒ E2010。
            return Err(KernelError::new(
                "E2010",
                "source header changed since plan preview; re-plan before applying",
                None,
                true,
            ));
        }
        Ok(())
    })?;
    let final_path = target.to_string();
    let reverse = match backup {
        Some(backup) => FsOp::WriteAtomic {
            path: final_path.clone(),
            data_ref: Some(backup),
        },
        None => FsOp::Quarantine {
            from: final_path.clone(),
            to: view.fs.quarantine_target_path(&file_name_of(&final_path))?,
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
        detail: None,
    })
}

fn perform_csv_rows(
    view: &WorkerView<'_>,
    source: &str,
    target: &str,
    overwrite: bool,
    payload: &PlanCsvRows,
    backup: &mut Option<String>,
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
        return perform_csv_range(view, source, target, overwrite, payload, backup);
    }
    let target_encoding = TargetEncoding::parse(&payload.target_encoding)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvRows target encoding"))?;
    let invalid = InvalidSequencePolicy::parse(&payload.invalid).ok_or_else(|| {
        KernelError::invalid_params("unsupported csvRows invalid-sequence policy")
    })?;
    let delimiter = Delimiter::parse(&payload.delimiter)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvRows delimiter"))?;

    let backup = if overwrite {
        Some(backup_once(view.fs, target, backup)?)
    } else {
        None
    };
    // 流式执行（ADR-0054）：first 逐条直通、last 有界环形缓冲（内存 O(N＋单条记录)）。
    view.fs.write_atomic_with(target, &mut |writer| {
        let mut sink = CsvStreamWriter {
            transcoder: StreamTranscoder::new(
                SourceEncoding::Utf8,
                target_encoding.without_bom(),
                invalid,
            ),
            writer,
        };
        if let Some(prefix) = target_encoding.bom_prefix() {
            write_stream(sink.writer, prefix)?;
        }
        if payload.has_header {
            let names: Vec<String> = payload.columns.clone();
            sink.emit_record(&names, delimiter.as_byte())?;
        }
        let mut header_checked = false;
        if window == RowWindow::First {
            let mut emitted = 0u64;
            stream_csv_records(
                view,
                &StreamCsvInput {
                    source,
                    expected_size: None,
                    encoding,
                    invalid,
                    delimiter: delimiter.as_byte(),
                },
                &mut |row| {
                    if payload.has_header && !header_checked {
                        header_checked = true;
                        if normalized_columns(&row) != payload.columns {
                            return Err(KernelError::new(
                                "E2010",
                                "source header changed since plan preview; re-plan before applying",
                                None,
                                true,
                            ));
                        }
                        return Ok(());
                    }
                    if emitted < payload.count {
                        sink.emit_record(&row, delimiter.as_byte())?;
                        emitted += 1;
                    }
                    Ok(())
                },
            )?;
        } else {
            // Last：环形缓冲保留最后 N 条数据行，EOF 后按序写出（与
            // select_row_window 的语义一致）。
            let capacity = usize::try_from(payload.count).unwrap_or(usize::MAX);
            let mut ring: std::collections::VecDeque<Vec<String>> =
                std::collections::VecDeque::with_capacity(capacity.min(1024));
            stream_csv_records(
                view,
                &StreamCsvInput {
                    source,
                    expected_size: None,
                    encoding,
                    invalid,
                    delimiter: delimiter.as_byte(),
                },
                &mut |row| {
                    if payload.has_header && !header_checked {
                        header_checked = true;
                        if normalized_columns(&row) != payload.columns {
                            return Err(KernelError::new(
                                "E2010",
                                "source header changed since plan preview; re-plan before applying",
                                None,
                                true,
                            ));
                        }
                        return Ok(());
                    }
                    if payload.count == 0 {
                        return Ok(());
                    }
                    if ring.len() == capacity {
                        ring.pop_front();
                    }
                    ring.push_back(row);
                    Ok(())
                },
            )?;
            for row in &ring {
                sink.emit_record(row, delimiter.as_byte())?;
            }
        }
        if payload.has_header && !header_checked {
            // 与整段路径同口径：有表头而源里没有表头 ⇒ E2010。
            return Err(KernelError::new(
                "E2010",
                "source header changed since plan preview; re-plan before applying",
                None,
                true,
            ));
        }
        Ok(())
    })?;
    let final_path = target.to_string();
    let reverse = match backup {
        Some(backup) => FsOp::WriteAtomic {
            path: final_path.clone(),
            data_ref: Some(backup),
        },
        None => FsOp::Quarantine {
            from: final_path.clone(),
            to: view.fs.quarantine_target_path(&file_name_of(&final_path))?,
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
        detail: None,
    })
}

fn perform_csv_merge(
    view: &WorkerView<'_>,
    target: &str,
    overwrite: bool,
    payload: &PlanCsvMerge,
    backup: &mut Option<String>,
) -> Result<Performed, KernelError> {
    let target_encoding = TargetEncoding::parse(&payload.target_encoding)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvMerge target encoding"))?;
    let invalid = InvalidSequencePolicy::parse(&payload.invalid).ok_or_else(|| {
        KernelError::invalid_params("unsupported csvMerge invalid-sequence policy")
    })?;
    let output_delimiter = Delimiter::parse(&payload.delimiter)
        .ok_or_else(|| KernelError::invalid_params("unsupported csvMerge delimiter"))?;
    let backup = if overwrite {
        Some(backup_once(view.fs, target, backup)?)
    } else {
        None
    };

    // 流式执行（ADR-0054）：逐源 `read_stream`→增量记录→按并集映射即读即写；
    // 内存 O(块＋单条记录)，逐源大小与表头复核沿用整段路径的 E2010 口径。
    view.fs.write_atomic_with(target, &mut |writer| {
        let mut sink = CsvStreamWriter {
            transcoder: StreamTranscoder::new(
                SourceEncoding::Utf8,
                target_encoding.without_bom(),
                invalid,
            ),
            writer,
        };
        if let Some(prefix) = target_encoding.bom_prefix() {
            write_stream(sink.writer, prefix)?;
        }
        if payload.has_header {
            let mut header: Vec<String> = Vec::new();
            if let Some(source_column) = &payload.source_column {
                header.push(source_column.clone());
            }
            header.extend(payload.union.iter().cloned());
            sink.emit_record(&header, output_delimiter.as_byte())?;
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
                KernelError::invalid_params(format!(
                    "unsupported source delimiter for {}",
                    file.path
                ))
            })?;
            let mut header_checked = false;
            let mut field_indexes: Option<Vec<Option<usize>>> = None;
            let mut known = if payload.has_header {
                0usize
            } else {
                payload.positional_columns as usize
            };
            stream_csv_records(
                view,
                &StreamCsvInput {
                    source: &file.path,
                    expected_size: Some(file.size),
                    encoding,
                    invalid,
                    delimiter: file_delimiter.as_byte(),
                },
                &mut |row| {
                    if payload.has_header && !header_checked {
                        header_checked = true;
                        let normalized = normalized_columns(&row);
                        if normalized != file.columns {
                            return Err(KernelError::new(
                                "E2010",
                                format!("source header changed since plan preview: {}", file.path),
                                None,
                                false,
                            ));
                        }
                        // 整段路径的 `known`＝原始表头字段数；映射键由该表头求出。
                        known = row.len();
                        if let Some(union) = &union_keys {
                            let header_keys = column_keys(&normalized_columns(&row));
                            field_indexes = Some(
                                union
                                    .iter()
                                    .map(|key| {
                                        header_keys.iter().position(|header_key| header_key == key)
                                    })
                                    .collect::<Vec<_>>(),
                            );
                        }
                        return Ok(());
                    }
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
                    if row.len() > known {
                        fields.extend(row[known..].iter().cloned());
                    }
                    sink.emit_record(&fields, output_delimiter.as_byte())
                },
            )?;
            // 整段路径对空文件（无首条记录）不做表头核对，这里同样跳过。
        }
        Ok(())
    })?;
    let final_path = target.to_string();
    let reverse = match backup {
        Some(backup) => FsOp::WriteAtomic {
            path: final_path.clone(),
            data_ref: Some(backup),
        },
        None => FsOp::Quarantine {
            from: final_path.clone(),
            to: view.fs.quarantine_target_path(&file_name_of(&final_path))?,
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
        detail: None,
    })
}

fn protected_target_hit(
    fs: &dyn FsAdapter,
    protected_dirs: &[ProtectedDir],
    case_sense: CaseSense,
    target: &str,
) -> Option<KernelError> {
    if protected_dirs.is_empty() {
        return None;
    }
    let normalized = match try_normalize(target) {
        Ok(normalized) => normalized,
        Err(error) => return Some(error),
    };
    let canonical = fs.canonicalize(target).ok();
    protected_by(
        &normalized,
        canonical.as_deref(),
        protected_dirs,
        case_sense,
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
            entry_done: None,
            entry_total: None,
        });
    }
}

fn skipped_entry(entry_id: String, error: KernelError) -> ApplyEntryReport {
    ApplyEntryReport {
        entry_id,
        status: ApplyStatus::Skipped,
        error: Some(error),
        detail: None,
    }
}

fn failed_entry(entry_id: String, error: KernelError) -> ApplyEntryReport {
    ApplyEntryReport {
        entry_id,
        status: ApplyStatus::Failed,
        error: Some(error),
        detail: None,
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
    use crate::apply::{ApplyOptions, OnConflict, MAX_ENTRY_CONCURRENCY, MAX_PLAN_ENTRIES};
    use crate::encoding::{SourceEncoding, TargetEncoding};
    use crate::fs::{DirEntryInfo, EntryAttrs, PathOutcome, ScanFilter, StatInfo};
    use crate::journal::{JournalEntry, JournalSink, JournalStatus};
    use crate::plan::{
        PlanCsvColumnsSplit, PlanCsvDedupColumn, PlanCsvExtract, PlanCsvExtractColumn,
        PlanCsvFilterColumn, PlanCsvFilterCondition, PlanCsvMerge, PlanCsvMergeFile, PlanCsvRows,
        PlanEntry, PlanExpected, PlanMerge, PlanMergeSource, PlanRange, PlanStats,
    };
    use crate::progress::{CancelToken, ProgressEvent, ProgressSink};
    use crate::split::{plan_f01, PlanF01File, PlanF01Input, SplitAccumulator};
    use std::collections::{BTreeMap, HashMap};
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    const PLAN_ID: &str = "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8";

    #[derive(Default)]
    struct TestFs {
        files: Mutex<HashMap<String, Vec<u8>>>,
        next: Mutex<u64>,
        read_bytes_calls: AtomicU64,
        /// 计数：流式区间读被调用了几次——F09 的"内存不随输入线性增长"要数得出
        /// 走的是流式路径而不是 read_ranges 的整段入内存。
        read_ranges_stream_calls: AtomicU64,
        /// 模拟跨盘：`true` 时 same_volume 返回 false（走校验复制路径）。
        cross_volume: Mutex<bool>,
        /// 故障注入：`true` 时 copy 落盘损坏内容（校验失败路径测试）。
        copy_corrupts: Mutex<bool>,
        /// 故障注入：写这些路径时返回 E5001（模拟磁盘满/权限失败的条目级失败）。
        fail_writes: Mutex<Vec<String>>,
        /// 故障注入：路径 →（错误码, retryable, 剩余可失败次数）。权限类终态与
        /// 「执行时重试一次」只能用真实错误码驱动，E5001 那条注入口顶不了这个坑。
        permission_failures: Mutex<HashMap<String, (String, bool, u64)>>,
        /// 计数：rename 被调用了几次——"执行时重试一次"必须数得出次数才不空转。
        rename_calls: std::sync::atomic::AtomicU64,
        /// 计数：copy 被调用了几次——D1 的 3 次退避重试必须数得出次数。
        copy_calls: std::sync::atomic::AtomicU64,
        /// 内存目录集合：支撑 create_dir / remove_dir_if_empty 臂（不模拟非空判定）。
        dirs: Mutex<Vec<String>>,
        /// read_stream 每次回调的块大小：既证"走了流式口"，也刻意把多字节序列切开。
        stream_chunks: Mutex<Vec<usize>>,
        /// open_read_stream 次数：C13 归并相必须走拉取式口（ADR-0058）。
        open_stream_calls: AtomicU64,
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

        fn set_cross_volume(&self, value: bool) {
            *self.cross_volume.lock().unwrap() = value;
        }

        fn set_copy_corrupts(&self, value: bool) {
            *self.copy_corrupts.lock().unwrap() = value;
        }

        fn read_ranges_stream_calls(&self) -> u64 {
            self.read_ranges_stream_calls.load(Ordering::Relaxed)
        }

        fn read_bytes_calls(&self) -> u64 {
            self.read_bytes_calls.load(Ordering::Relaxed)
        }

        fn copy_calls(&self) -> u64 {
            self.copy_calls.load(Ordering::Relaxed)
        }

        fn open_stream_calls(&self) -> u64 {
            self.open_stream_calls.load(Ordering::Relaxed)
        }

        /// 注入权限类写失败：`times` 次后自动放行（＝1 才能证明"重试一次"真发生了）。
        fn set_permission_failure(&self, path: &str, code: &str, retryable: bool, times: u64) {
            self.permission_failures
                .lock()
                .unwrap()
                .insert(path.to_string(), (code.to_string(), retryable, times));
        }

        /// 命中则返回错误并消耗一次额度（剩余次数为 0 时后续写成功）——
        /// 「重试一次」只有能数得出次数才算被证明。
        fn permission_error(&self, path: &str) -> Option<KernelError> {
            let mut guard = self.permission_failures.lock().unwrap();
            let (code, retryable, remaining) = guard.get_mut(path)?;
            if *remaining == 0 {
                return None;
            }
            *remaining -= 1;
            Some(KernelError::new(
                code.clone(),
                "injected permission failure",
                None,
                *retryable,
            ))
        }

        fn set_fail_writes(&self, paths: &[&str]) {
            *self.fail_writes.lock().unwrap() =
                paths.iter().map(|path| (*path).to_string()).collect();
        }

        fn dir_exists(&self, path: &str) -> bool {
            self.dirs.lock().unwrap().iter().any(|dir| dir == path)
        }
    }

    #[derive(Default)]
    struct RecordingRetry {
        waits: Mutex<Vec<Duration>>,
        pauses: AtomicU64,
    }

    impl crate::retry::NetworkRetry for RecordingRetry {
        fn wait(&self, delay: Duration) -> bool {
            self.waits.lock().unwrap().push(delay);
            true
        }

        fn pause(&self, _error: &crate::error::KernelError) -> crate::retry::PauseAction {
            self.pauses.fetch_add(1, Ordering::SeqCst);
            crate::retry::PauseAction::Resumed
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

        fn open_read_stream(
            &self,
            path: &str,
        ) -> Result<Box<dyn std::io::Read + Send>, KernelError> {
            let data = self
                .files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))?;
            self.open_stream_calls.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(std::io::Cursor::new(data)))
        }

        fn read_stream(
            &self,
            path: &str,
            read: &mut dyn FnMut(&[u8]) -> Result<(), KernelError>,
        ) -> Result<(), KernelError> {
            let data = self
                .files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))?;
            // 3 字节一片：故意在 GB18030 多字节序列中间切开，逼解码器跨块保持状态。
            for chunk in data.chunks(3) {
                self.stream_chunks.lock().unwrap().push(chunk.len());
                read(chunk)?;
            }
            Ok(())
        }

        fn write_atomic(&self, path: &str, data: &[u8]) -> Result<(), KernelError> {
            if let Some(error) = self.permission_error(path) {
                return Err(error);
            }
            if self
                .fail_writes
                .lock()
                .unwrap()
                .iter()
                .any(|blocked| blocked == path)
            {
                return Err(KernelError::new(
                    "E5001",
                    "injected write failure",
                    None,
                    true,
                ));
            }
            self.files
                .lock()
                .unwrap()
                .insert(path.to_string(), data.to_vec());
            Ok(())
        }

        fn create_dir(&self, path: &str) -> Result<(), KernelError> {
            let mut dirs = self.dirs.lock().unwrap();
            if dirs.iter().any(|dir| dir == path) {
                return Err(KernelError::new("E2007", "directory exists", None, false));
            }
            dirs.push(path.to_string());
            Ok(())
        }

        fn remove_dir_if_empty(&self, path: &str) -> Result<(), KernelError> {
            let mut dirs = self.dirs.lock().unwrap();
            match dirs.iter().position(|dir| dir == path) {
                Some(index) => {
                    dirs.remove(index);
                    Ok(())
                }
                None => Err(KernelError::new("E2001", "no such directory", None, false)),
            }
        }

        fn write_atomic_with(
            &self,
            path: &str,
            write: &mut dyn FnMut(&mut dyn std::io::Write) -> Result<(), KernelError>,
        ) -> Result<(), KernelError> {
            if let Some(error) = self.permission_error(path) {
                return Err(error);
            }
            if self
                .fail_writes
                .lock()
                .unwrap()
                .iter()
                .any(|blocked| blocked == path)
            {
                return Err(KernelError::new(
                    "E5001",
                    "injected write failure",
                    None,
                    true,
                ));
            }
            struct Sink(Vec<u8>);
            impl std::io::Write for Sink {
                fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
                    self.0.extend_from_slice(buffer);
                    Ok(buffer.len())
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            let mut sink = Sink(Vec::new());
            write(&mut sink)?;
            self.files.lock().unwrap().insert(path.to_string(), sink.0);
            Ok(())
        }

        fn rewrite_atomic_with(
            &self,
            path: &str,
            rewrite: &mut crate::fs::RewriteCallback<'_>,
        ) -> Result<(), KernelError> {
            if let Some(error) = self.permission_error(path) {
                return Err(error);
            }
            if self
                .fail_writes
                .lock()
                .unwrap()
                .iter()
                .any(|blocked| blocked == path)
            {
                return Err(KernelError::new(
                    "E5001",
                    "injected write failure",
                    None,
                    true,
                ));
            }
            let original = self
                .files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))?;
            struct Sink(Vec<u8>);
            impl std::io::Write for Sink {
                fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
                    self.0.extend_from_slice(buffer);
                    Ok(buffer.len())
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            // reader 覆盖原字节；回调失败不提交（原子语义与平台实现一致）。
            let mut sink = Sink(Vec::new());
            let mut reader: &[u8] = &original;
            rewrite(&mut reader, &mut sink)?;
            self.files.lock().unwrap().insert(path.to_string(), sink.0);
            Ok(())
        }

        fn rename(
            &self,
            from: &str,
            to: &str,
            policy: ConflictPolicy,
        ) -> Result<PathOutcome, KernelError> {
            self.rename_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Some(error) = self.permission_error(to) {
                return Err(error);
            }
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
            self.copy_calls.fetch_add(1, Ordering::Relaxed);
            if let Some(error) = self.permission_error(to) {
                return Err(error);
            }
            if self
                .fail_writes
                .lock()
                .unwrap()
                .iter()
                .any(|blocked| blocked == to)
            {
                return Err(KernelError::new(
                    "E5001",
                    "injected write failure",
                    None,
                    true,
                ));
            }
            let mut files = self.files.lock().unwrap();
            let corrupts = *self.copy_corrupts.lock().unwrap();
            let mut data = files
                .get(from)
                .cloned()
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))?;
            if corrupts {
                data.push(b'!');
            }
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

        fn read_ranges_stream(
            &self,
            source: &str,
            ranges: &[(u64, u64)],
            read: &mut dyn FnMut(&[u8]) -> Result<(), KernelError>,
        ) -> Result<(), KernelError> {
            self.read_ranges_stream_calls
                .fetch_add(1, Ordering::Relaxed);
            let files = self.files.lock().unwrap();
            let data = files
                .get(source)
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))?;
            for (start, end) in ranges {
                if start >= end || *end > data.len() as u64 {
                    return Err(KernelError::new(
                        "E2010",
                        "range is out of bounds",
                        None,
                        false,
                    ));
                }
                read(&data[*start as usize..*end as usize])?;
            }
            Ok(())
        }

        fn copy_ranges_atomic(
            &self,
            source: &str,
            ranges: &[(u64, u64)],
            prefix: &[u8],
            target: &str,
            policy: ConflictPolicy,
        ) -> Result<PathOutcome, KernelError> {
            let mut files = self.files.lock().unwrap();
            let data = files
                .get(source)
                .cloned()
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))?;
            let mut joined = prefix.to_vec();
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

        fn same_volume(&self, _a: &str, _b: &str) -> bool {
            !*self.cross_volume.lock().unwrap()
        }

        fn remove_file(&self, path: &str) -> Result<(), KernelError> {
            self.files
                .lock()
                .unwrap()
                .remove(path)
                .map(|_| ())
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))
        }

        fn create_dir_all(&self, path: &str) -> Result<(), KernelError> {
            // 内存文件系统：目录不参与查找，仅接受合法路径。
            if path.is_empty() {
                return Err(KernelError::new("E1003", "empty directory", None, false));
            }
            Ok(())
        }

        fn content_hash(&self, path: &str, algorithm: &str) -> Result<Option<String>, KernelError> {
            if algorithm != "blake3" {
                return Ok(None);
            }
            self.files
                .lock()
                .unwrap()
                .get(path)
                .map(|data| Some(format!("h:{}", String::from_utf8_lossy(data))))
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))
        }

        fn case_sensitivity(&self) -> CaseSense {
            CaseSense::Sensitive
        }

        // 测试替身模拟 POSIX 临时目录：与 `Sensitive` 同一个前提，不是"随便填"。
        fn name_rules(&self) -> crate::conflict::NameRules {
            crate::conflict::NameRules::posix()
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
            csv_dedup: None,
            csv_filter: None,
            csv_columns: None,
            csv_sort: None,
        }
    }

    fn csv_sort_entry(
        entry_id: &str,
        source: &str,
        target: &str,
        size: u64,
        columns: &[&str],
        key: (u64, &str),
        chunk_rows: Option<u64>,
    ) -> PlanEntry {
        PlanEntry {
            entry_id: entry_id.to_string(),
            source: source.to_string(),
            target: Some(target.to_string()),
            action: PlanAction::CsvSort,
            relation: None,
            reason: "csv sort".to_string(),
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
            csv_rows: None,
            csv_dedup: None,
            csv_filter: None,
            csv_columns: None,
            csv_sort: Some(PlanCsvSort {
                encoding: Some(SourceEncoding::Utf8),
                size,
                delimiter: ",".to_string(),
                has_header: true,
                columns: columns.iter().map(|value| value.to_string()).collect(),
                column_count: columns.len() as u64,
                keys: vec![crate::plan::PlanCsvSortKey {
                    index: key.0,
                    name: Some(columns[(key.0 - 1) as usize].to_string()),
                    order: "asc".to_string(),
                    kind: key.1.to_string(),
                }],
                external_chunk_rows: chunk_rows,
                target_encoding: "utf-8".to_string(),
                invalid: "replace-and-count".to_string(),
                dirty_rows: 0,
            }),
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
            network_retry: None,
            content_editor: None,
            content_rewriter: None,
            directory_lister: None,
        }
    }

    #[test]
    fn cross_volume_move_copies_verifies_removes_source_and_undoes() {
        let fs = TestFs::with_files(&["a"]);
        fs.set_cross_volume(true);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan(vec![action_entry("e1", "a", Some("b"), PlanAction::Move)]),
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert!(fs.exists("b"), "copy landed on the target volume");
        assert!(!fs.exists("a"), "source removed only after verified copy");
        // Journal 语义仍是移动：撤销＝反向移动（§5.8）。
        let undo = crate::apply::undo_plan_with_ports(
            PLAN_ID,
            &fs,
            &license,
            &mut journal,
            crate::apply::UndoPorts::default(),
        )
        .unwrap();
        assert_eq!(undo.reverted, 1);
        assert!(fs.exists("a") && !fs.exists("b"));
    }

    #[test]
    fn cross_volume_move_verification_failure_keeps_the_source() {
        let fs = TestFs::with_files(&["a"]);
        fs.set_cross_volume(true);
        fs.set_copy_corrupts(true);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan(vec![action_entry("e1", "a", Some("b"), PlanAction::Move)]),
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Failed);
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Failed);
        assert_eq!(
            outcome.report.entries[0].error.as_ref().unwrap().code,
            "E3005",
            "hash mismatch must surface as verification failure"
        );
        // "校验不通过绝不删源"：源保留；坏副本尽力清除。
        assert!(fs.exists("a"));
        assert!(!fs.exists("b"), "the corrupt copy is cleaned up");
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

    /// OQ-37 裁决①的落点：改名族遇到写不进去（E2002 只读）＝跳过并报告，
    /// 不记 failed、不写 Journal、源文件留在原地，任务终态因此不是整体失败。
    #[test]
    fn readonly_rename_is_skipped_and_reported_without_failing_the_task() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        fs.set_permission_failure("/data/b.txt", "E2002", false, 1);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut plan_doc = plan(vec![action_entry(
            "e1",
            "/data/a.txt",
            Some("/data/b.txt"),
            PlanAction::Rename,
        )]);
        plan_doc.kind = crate::rename::F12_KIND.to_string();
        let outcome = apply_generic_plan(
            &plan_doc,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed, "只读不得把整任务判死");
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Skipped);
        assert_eq!(
            outcome.report.entries[0]
                .error
                .as_ref()
                .expect("跳过也要带着码位上报")
                .code,
            "E2002"
        );
        assert_eq!(journal.entries.len(), 0, "没写成就不该有撤销记录");
        assert!(fs.exists("/data/a.txt"), "改名被拒后源文件必须留在原地");
        assert!(!fs.exists("/data/b.txt"));
    }

    /// 反向对照：裁决①的范围只到改名族与 C03。其余写臂（这里取未点名的 kind）
    /// 遇到同一种失败仍须记 Failed——"产物没写出来"不能变安静跳过。
    #[test]
    fn permission_failure_still_fails_arms_outside_the_named_scope() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        fs.set_permission_failure("/data/b.txt", "E2002", false, 1);
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
        assert_eq!(outcome.state, ApplyState::Failed);
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Failed);
    }

    /// 方案 §5.13 的"执行时重试一次"：E2003（被占用）按设计 06 是 retryable，
    /// 注入点只放行一次——第一枪失败、第二枪成功才说明真的重过。
    #[test]
    fn occupied_target_is_retried_once_and_then_applies() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        fs.set_permission_failure("/data/b.txt", "E2003", true, 1);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut plan_doc = plan(vec![action_entry(
            "e1",
            "/data/a.txt",
            Some("/data/b.txt"),
            PlanAction::Rename,
        )]);
        plan_doc.kind = crate::rename::F12_KIND.to_string();
        let outcome = apply_generic_plan(
            &plan_doc,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);
        assert_eq!(
            fs.rename_calls.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "E2003 可重试＝必须真的再打过一次，而不是恰好一次成功"
        );
        assert!(fs.exists("/data/b.txt"), "重试一次后应改名成功");
        assert_eq!(journal.entries.len(), 1, "成功条目必须留撤销记录");
    }

    /// D1：UNC 断线（E2005）最多重 3 次、退避 2/4/8 秒，之后暂停等待恢复；
    /// 恢复后从首次尝试重新计数。这里第 5 枪成功，证明暂停后确实重试了。
    #[test]
    fn network_disconnect_is_retried_three_times_then_pauses_and_resumes() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        // 前 4 枪失败（首次＋3 次重试），暂停恢复后的第 5 枪成功。
        fs.set_permission_failure("//server/share/b.txt", "E2005", true, 4);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let retry = RecordingRetry::default();
        let mut plan_doc = plan(vec![action_entry(
            "e1",
            "/data/a.txt",
            Some("//server/share/b.txt"),
            PlanAction::Copy,
        )]);
        plan_doc.kind = crate::c07::C07_KIND.to_string();
        let mut ctx = context(&fs, &mut journal, &license);
        ctx.network_retry = Some(&retry);
        let outcome = apply_generic_plan(&plan_doc, &ApplyOptions::default(), &mut ctx).unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(fs.copy_calls(), 5, "首次＋3 次退避重试＋暂停恢复后 1 次");
        assert_eq!(
            *retry.waits.lock().unwrap(),
            vec![
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8)
            ],
            "退避必须按 2/4/8 秒，而不是忙重试"
        );
        assert_eq!(
            retry.pauses.load(Ordering::SeqCst),
            1,
            "3 次重试耗尽后必须切到暂停，而不是直接失败"
        );
    }

    /// ADR-0093：磁盘满（E2004）是致命错误，**第一次撞上就暂停**——不学网络断线
    /// 退避 3 次（磁盘满不会秒级自愈）。恢复后重试本条目，已完成的不重做。
    #[test]
    fn disk_full_pauses_once_without_backoff_then_resumes_and_retries() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        fs.write_file("/data/a.txt", b"abc");
        // 首枪失败（模拟写盘撞上 ENOSPC），暂停恢复后的第二枪成功。
        fs.set_permission_failure("/data/b.txt", "E2004", true, 1);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let retry = RecordingRetry::default();
        let mut ctx = context(&fs, &mut journal, &license);
        ctx.network_retry = Some(&retry);
        let outcome = apply_generic_plan(
            &plan(vec![action_entry(
                "e1",
                "/data/a.txt",
                Some("/data/b.txt"),
                PlanAction::Copy,
            )]),
            &ApplyOptions::default(),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(
            fs.copy_calls(),
            2,
            "首枪失败后应只重试一次（暂停恢复后），不得忙重试"
        );
        assert_eq!(
            retry.waits.lock().unwrap().len(),
            0,
            "磁盘满不得走 2/4/8 秒退避——那不是等待，是把用户推向任务失败"
        );
        assert_eq!(retry.pauses.load(Ordering::SeqCst), 1);
        assert_eq!(fs.read_file("/data/b.txt"), b"abc");
    }

    /// ADR-0093：暂停期间用户取消 → 该条目按**普通条目失败**收尾，
    /// 不阻塞整体（方案 §5.13），也不把任务判成"已暂停"。
    #[test]
    fn disk_full_pause_cancel_ends_as_a_normal_entry_failure() {
        #[derive(Default)]
        struct CancellingRetry;
        impl crate::retry::NetworkRetry for CancellingRetry {
            fn wait(&self, _delay: Duration) -> bool {
                true
            }
            fn pause(&self, _error: &crate::error::KernelError) -> crate::retry::PauseAction {
                crate::retry::PauseAction::Cancel
            }
        }

        let fs = TestFs::with_files(&["/data/a.txt", "/data/ok.txt"]);
        fs.write_file("/data/a.txt", b"abc");
        fs.write_file("/data/ok.txt", b"ok");
        fs.set_permission_failure("/data/b.txt", "E2004", true, u64::MAX);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let retry = CancellingRetry;
        let mut ctx = context(&fs, &mut journal, &license);
        ctx.network_retry = Some(&retry);
        let outcome = apply_generic_plan(
            &plan(vec![
                action_entry("e1", "/data/a.txt", Some("/data/b.txt"), PlanAction::Copy),
                action_entry(
                    "e2",
                    "/data/ok.txt",
                    Some("/data/ok2.txt"),
                    PlanAction::Copy,
                ),
            ]),
            &ApplyOptions::default(),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(
            outcome.state,
            ApplyState::PartialFailed,
            "一条失败一条成功＝部分失败，不是整体失败"
        );
        let entries = &outcome.report.entries;
        assert_eq!(entries[0].status, ApplyStatus::Failed);
        assert_eq!(
            entries[0].error.as_ref().map(|error| error.code.as_str()),
            Some("E2004")
        );
        assert_eq!(entries[1].status, ApplyStatus::Ok, "另一条不该被牵连");
        assert_eq!(fs.read_file("/data/ok2.txt"), b"ok");
    }

    /// D1 补全：`edit-content` 的网络断线重试复用同一份隔离备份——重试不得
    /// 每枪都 `backup_to_quarantine` 一次（旧口径把它排除在重试族之外）。
    #[test]
    fn edit_content_network_retry_reuses_a_single_quarantine_backup() {
        let fs = TestFs::with_files(&["//server/share/a.txt"]);
        fs.write_file("//server/share/a.txt", b"abc");
        // 前 4 枪失败（首次＋3 次重试），暂停恢复后的第 5 枪成功。
        fs.set_permission_failure("//server/share/a.txt", "E2005", true, 4);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let retry = RecordingRetry::default();
        let mut ctx = context(&fs, &mut journal, &license);
        ctx.network_retry = Some(&retry);
        ctx.content_editor = Some(&|data: &[u8]| {
            Ok(EditedContent::new(
                data.iter().map(|byte| byte.to_ascii_uppercase()).collect(),
            ))
        });
        let outcome = apply_generic_plan(
            &plan(vec![action_entry(
                "e1",
                "//server/share/a.txt",
                None,
                PlanAction::EditContent,
            )]),
            &ApplyOptions::default(),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(fs.read_file("//server/share/a.txt"), b"ABC");
        assert_eq!(
            *retry.waits.lock().unwrap(),
            vec![
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8)
            ],
            "edit-content 的断线重试必须按 2/4/8 秒退避"
        );
        assert_eq!(retry.pauses.load(Ordering::SeqCst), 1);
        let backups = fs
            .files
            .lock()
            .unwrap()
            .keys()
            .filter(|path| path.ends_with(".bak"))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            backups.len(),
            1,
            "重试必须复用同一份隔离备份，不得每枪累积一份：{backups:?}"
        );
        let data_ref = match &journal.entries[0].forward {
            FsOp::WriteAtomic {
                data_ref: Some(path),
                ..
            } => path,
            other => panic!("edit-content forward must be write-atomic: {other:?}"),
        };
        assert_eq!(
            data_ref, &backups[0],
            "Journal 的 dataRef 必须指向重试复用的那份备份"
        );
    }

    /// `transcode` 的旧动作名走与 `edit-content` 相同的就地改写执行路径：
    /// 备份 → 变换 → 原子写回 → Journal `WriteAtomic{dataRef}` → undo 还原。
    #[test]
    fn transcode_entries_run_the_in_place_rewrite_and_undo_restores_the_bytes() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        fs.write_file("/data/a.txt", b"abc");
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut ctx = context(&fs, &mut journal, &license);
        ctx.content_editor = Some(&|data: &[u8]| {
            Ok(EditedContent::new(
                data.iter().map(|byte| byte.to_ascii_uppercase()).collect(),
            ))
        });
        let outcome = apply_generic_plan(
            &plan(vec![action_entry(
                "e1",
                "/data/a.txt",
                None,
                PlanAction::Transcode,
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

    /// `transcode` 同样要求上下文注入编辑器/重写器；缺失时在执行前拒绝（E1002），
    /// 不能落到执行臂里的 `expect`。
    #[test]
    fn transcode_without_a_content_editor_is_rejected_before_touching_files() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let error = apply_generic_plan(
            &plan(vec![action_entry(
                "e1",
                "/data/a.txt",
                None,
                PlanAction::Transcode,
            )]),
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap_err();
        assert_eq!(error.code, "E1002");
        assert_eq!(
            fs.read_file("/data/a.txt"),
            b"content",
            "拒绝必须发生在落盘前"
        );
    }

    /// 只读不重试：设计 06 里 `E2002.retryable=false`——再试一次也不会变可写。
    #[test]
    fn non_retryable_permission_error_is_not_retried() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        // 只注入一次额度：若执行器错误地去重试，第二枪就会成功并把条目做成 Ok。
        fs.set_permission_failure("/data/b.txt", "E2002", false, 1);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut plan_doc = plan(vec![action_entry(
            "e1",
            "/data/a.txt",
            Some("/data/b.txt"),
            PlanAction::Rename,
        )]);
        plan_doc.kind = crate::rename::F12_KIND.to_string();
        let outcome = apply_generic_plan(
            &plan_doc,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Skipped);
        assert_eq!(journal.entries.len(), 0);
    }

    /// 裁决①的范围另一头：内容重写族（C03）就地改写失败同样"跳过并报告"。
    #[test]
    fn readonly_source_is_skipped_for_content_rewrites() {
        struct NoopRewriter;
        impl ContentRewriter for NoopRewriter {
            fn rewrite(
                &self,
                _entry: &crate::plan::PlanEntry,
                _input: &mut dyn std::io::Read,
                _output: &mut dyn std::io::Write,
                _cancel: &dyn CancelToken,
            ) -> Result<EditOutcome, KernelError> {
                Ok(EditOutcome { detail: None })
            }
        }
        let fs = TestFs::with_files(&["/data/a.txt"]);
        fs.set_permission_failure("/data/a.txt", "E2002", false, 1);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut plan_doc = plan(vec![action_entry(
            "e1",
            "/data/a.txt",
            None,
            PlanAction::EditContent,
        )]);
        plan_doc.kind = crate::replace_content::C03_KIND.to_string();
        let mut ctx = GenericApplyContext {
            fs: &fs,
            license: &license,
            journal: &mut journal,
            task_id: "task-1".to_string(),
            now_rfc3339: "2026-11-04T00:00:00Z".to_string(),
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            progress: None,
            cancel: None,
            network_retry: None,
            content_editor: None,
            content_rewriter: Some(&NoopRewriter),
            directory_lister: None,
        };
        let outcome = apply_generic_plan(&plan_doc, &ApplyOptions::default(), &mut ctx).unwrap();
        assert_eq!(outcome.state, ApplyState::Completed, "只读不得判死整任务");
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Skipped);
        assert_eq!(journal.entries.len(), 0, "没写成就不该有撤销记录");
        assert!(fs.exists("/data/a.txt"), "源文件必须仍在原地");
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
        // 流式证据：合并路径不得走 read_bytes（整文件读入），且确实按块喂入。
        assert_eq!(
            fs.read_bytes_calls.load(Ordering::Relaxed),
            0,
            "F04 合并必须走 read_stream，不得整文件读入"
        );
        let chunks = fs.stream_chunks.lock().unwrap().clone();
        assert!(!chunks.is_empty(), "read_stream 必须真的被调用");
        assert!(
            chunks.iter().all(|size| *size <= 3),
            "块大小应符合读取口的分块行为：{chunks:?}"
        );
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

    /// 记录式进度端口：把内核上报的 ProgressEvent 全留下来供断言。
    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<ProgressEvent>>,
    }

    impl ProgressSink for RecordingSink {
        fn report(&self, event: ProgressEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    /// F04 合并的**条目内（字节级）进度**（方案 §7.5 双层进度、TC-F04-EXE-05）。
    ///
    /// 合并是 N:1——整条任务只有一条 entry，只报条目计数的话界面长时间停在 0/1。
    /// 本测试钉住：大文件合并必须报字节进度，且条目边界事件不得携带字节字段。
    #[test]
    fn merge_reports_byte_level_entry_progress_for_large_inputs() {
        let fs = TestFs::default();
        let first: Vec<u8> = vec![b'a'; 200_000];
        let second: Vec<u8> = vec![b'b'; 100_000];
        fs.write_file("/d/a.txt", &first);
        fs.write_file("/d/b.txt", &second);
        let plan = plan(vec![merge_entry(
            "e1",
            "/o/merged.txt",
            TargetEncoding::Utf8 { bom: false },
            vec![
                ("/d/a.txt", SourceEncoding::Utf8, first.len() as u64),
                ("/d/b.txt", SourceEncoding::Utf8, second.len() as u64),
            ],
        )]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut ctx = context(&fs, &mut journal, &license);
        let sink = RecordingSink::default();
        ctx.progress = Some(&sink);
        let outcome = apply_generic_plan(&plan, &ApplyOptions::default(), &mut ctx).unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);

        let events = sink.events.lock().unwrap().clone();
        let byte_events: Vec<&ProgressEvent> = events
            .iter()
            .filter(|event| event.entry_total.is_some())
            .collect();
        assert!(
            !byte_events.is_empty(),
            "合并必须报条目内字节进度，否则大文件合并期间界面停在 0/1"
        );
        let last = byte_events.last().unwrap();
        assert_eq!(last.entry_total, Some(300_000), "总量＝各源字节之和");
        assert_eq!(last.entry_done, Some(300_000), "最后一块应报满");
        assert_eq!(last.phase, ProgressPhase::Executing);
        // 条目边界事件不携带字节字段（壳层据此区分两类事件）
        for event in events.iter().filter(|event| event.entry_total.is_none()) {
            assert!(event.entry_done.is_none());
        }
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
        // 流式证据：csv-merge 必须走 read_stream，不得整文件读入（ADR-0054）。
        assert_eq!(
            fs.read_bytes_calls(),
            0,
            "csv-merge must stream sources instead of read_bytes"
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
        // 流式证据：csv-extract 必须走 read_stream，不得整文件读入（ADR-0054）。
        assert_eq!(
            fs.read_bytes_calls(),
            0,
            "csv-extract must stream the source instead of read_bytes"
        );
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
            csv_dedup: None,
            csv_filter: None,
            csv_columns: None,
            csv_sort: None,
        }
    }

    fn csv_dedup_entry(
        entry_id: &str,
        source: &str,
        target: &str,
        size: u64,
        has_header: bool,
        columns: &[&str],
        key_columns: Vec<(u64, Option<&str>)>,
    ) -> PlanEntry {
        PlanEntry {
            entry_id: entry_id.to_string(),
            source: source.to_string(),
            target: Some(target.to_string()),
            action: PlanAction::CsvDedup,
            relation: None,
            reason: "csv dedup".to_string(),
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
            csv_rows: None,
            csv_dedup: Some(PlanCsvDedup {
                encoding: Some(SourceEncoding::Utf8),
                size,
                delimiter: ",".to_string(),
                has_header,
                columns: columns.iter().map(|value| value.to_string()).collect(),
                column_count: columns.len() as u64,
                key: if key_columns.is_empty() {
                    "row"
                } else {
                    "columns"
                }
                .to_string(),
                key_columns: key_columns
                    .into_iter()
                    .map(|(index, name)| PlanCsvDedupColumn {
                        index,
                        name: name.map(str::to_string),
                    })
                    .collect(),
                target_encoding: "utf-8".to_string(),
                invalid: "replace-and-count".to_string(),
                data_rows: 0,
                unique_rows: 0,
                duplicate_rows: 0,
                dirty_rows: 0,
            }),
            csv_filter: None,
            csv_columns: None,
            csv_sort: None,
        }
    }

    fn csv_columns_split_entry(
        entry_id: &str,
        source: &str,
        target: &str,
        size: u64,
        columns: &[&str],
        split: PlanCsvColumnsSplit,
    ) -> PlanEntry {
        PlanEntry {
            entry_id: entry_id.to_string(),
            source: source.to_string(),
            target: Some(target.to_string()),
            action: PlanAction::CsvColumns,
            relation: None,
            reason: "csv columns split".to_string(),
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
            csv_rows: None,
            csv_dedup: None,
            csv_filter: None,
            csv_columns: Some(PlanCsvColumns {
                mode: "split".to_string(),
                encoding: Some(SourceEncoding::Utf8),
                size,
                delimiter: ",".to_string(),
                has_header: true,
                columns: columns.iter().map(|value| value.to_string()).collect(),
                column_count: columns.len() as u64,
                split: Some(split),
                merge: None,
                target_encoding: "utf-8".to_string(),
                invalid: "replace-and-count".to_string(),
                dirty_rows: 0,
            }),
            csv_sort: None,
        }
    }

    fn csv_filter_entry(
        entry_id: &str,
        source: &str,
        target: &str,
        size: u64,
        has_header: bool,
        columns: &[&str],
        condition: (u64, &str, &str),
    ) -> PlanEntry {
        let (column_index, op, value) = condition;
        PlanEntry {
            entry_id: entry_id.to_string(),
            source: source.to_string(),
            target: Some(target.to_string()),
            action: PlanAction::CsvFilter,
            relation: None,
            reason: "csv filter".to_string(),
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
            csv_rows: None,
            csv_dedup: None,
            csv_filter: Some(PlanCsvFilter {
                encoding: Some(SourceEncoding::Utf8),
                size,
                delimiter: ",".to_string(),
                has_header,
                columns: columns.iter().map(|value| value.to_string()).collect(),
                column_count: columns.len() as u64,
                condition: PlanCsvFilterCondition {
                    column: PlanCsvFilterColumn {
                        index: column_index,
                        name: None,
                    },
                    op: op.to_string(),
                    value: value.to_string(),
                },
                mode: "keep".to_string(),
                target_encoding: "utf-8".to_string(),
                invalid: "replace-and-count".to_string(),
                data_rows: 0,
                matched_rows: 0,
                kept_rows: 0,
                non_numeric_cells: 0,
                dirty_rows: 0,
            }),
            csv_columns: None,
            csv_sort: None,
        }
    }

    #[test]
    fn csv_columns_arm_streams_instead_of_whole_file_reads() {
        // C14 与 CSV 家族其余各臂复用同一流式底座（ADR-0055）：
        // 断言 read_bytes 0 次（TestFs 的 read_stream 按 3 字节分块切开记录边界）。
        let fs = TestFs::default();
        fs.write_file("/data/a.csv", b"a,b\nx-1,y\np-2,q\n");
        let split = PlanCsvColumnsSplit {
            index: 1,
            name: Some("a".to_string()),
            separator: Some("-".to_string()),
            regex: None,
            parts: 2,
            new_names: vec!["a.1".to_string(), "a.2".to_string()],
        };
        let plan = plan(vec![csv_columns_split_entry(
            "e1",
            "/data/a.csv",
            "/out/split.csv",
            16,
            &["a", "b"],
            split,
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
            fs.read_file("/out/split.csv"),
            b"a,a.1,a.2,b\nx-1,x,1,y\np-2,p,2,q\n"
        );
        assert_eq!(
            fs.read_bytes_calls(),
            0,
            "csv-columns must stream the source instead of read_bytes"
        );
        assert!(matches!(
            journal.entries[0].reverse,
            FsOp::Quarantine { .. }
        ));
    }

    #[test]
    fn edit_content_via_rewriter_carries_detail_per_entry() {
        // ContentRewriter 的明细通道（ADR-0055）：与 ContentEditor 的
        // EditedContent.detail 同载体，随条目报告进执行明细。
        let fs = TestFs::default();
        fs.write_file("/d/one.txt", b"abc\n");
        let plan = plan(vec![action_entry(
            "e1",
            "/d/one.txt",
            None,
            PlanAction::EditContent,
        )]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut ctx = context(&fs, &mut journal, &license);
        let rewriter = UpperRewriter;
        ctx.content_rewriter = Some(&rewriter);
        let outcome = apply_generic_plan(&plan, &ApplyOptions::default(), &mut ctx).unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(fs.read_file("/d/one.txt"), b"ABC\n");
        assert_eq!(
            outcome.report.entries[0].detail.as_deref(),
            Some("upper rewriter ran")
        );
    }

    #[test]
    fn csv_sort_external_merge_streams_the_source_and_pulls_each_chunk() {
        // C13 外排（ADR-0058）：分块排序相走 read_stream（TestFs 按 3 字节切片，
        // 连引号内换行的记录也被切开），归并相每块开一个拉取式流逐条取最小——
        // 两相都不再整块读回。旧实现两相都用 read_bytes，本断言在其上必红。
        let fs = TestFs::default();
        let source = b"id,note,value\n1,a,30\n2,\"multi\nline\",10\n3,c,40\n4,d,20\n5,e,50\n";
        fs.write_file("/data/big.csv", source);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let sort = plan(vec![csv_sort_entry(
            "e1",
            "/data/big.csv",
            "/out/sorted.csv",
            source.len() as u64,
            &["id", "note", "value"],
            (3, "numeric"),
            Some(2),
        )]);
        let outcome = apply_generic_plan(
            &sort,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(
            fs.read_file("/out/sorted.csv"),
            b"id,note,value\n2,\"multi\nline\",10\n4,d,20\n1,a,30\n3,c,40\n5,e,50\n"
        );
        assert_eq!(
            fs.read_bytes_calls(),
            0,
            "csv-sort must stream the source instead of read_bytes"
        );
        assert_eq!(
            fs.open_stream_calls(),
            3,
            "every spilled chunk must be pulled through open_read_stream"
        );
        assert!(
            fs.files
                .lock()
                .unwrap()
                .keys()
                .all(|path| !path.contains(".batchdesk-sort-")),
            "chunk temp files must be removed once the merge is done"
        );
        assert!(matches!(
            journal.entries[0].reverse,
            FsOp::Quarantine { .. }
        ));
    }

    #[test]
    fn csv_sort_external_chunks_are_removed_even_when_the_output_write_fails() {
        // 守卫必须在分块相之前建立（ADR-0058）：输入相/产物相中途失败都不得残留
        // `.batchdesk-sort-*.tmp`。这里让产物写入失败，断言块文件仍被清掉。
        let fs = TestFs::default();
        let source = b"id,value\n1,30\n2,10\n3,40\n";
        fs.write_file("/data/big.csv", source);
        fs.set_fail_writes(&["/out/sorted.csv"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let sort = plan(vec![csv_sort_entry(
            "e1",
            "/data/big.csv",
            "/out/sorted.csv",
            source.len() as u64,
            &["id", "value"],
            (2, "numeric"),
            Some(2),
        )]);
        let outcome = apply_generic_plan(
            &sort,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Failed);
        assert!(
            fs.files
                .lock()
                .unwrap()
                .keys()
                .all(|path| !path.contains(".batchdesk-sort-")),
            "chunk temp files must be removed on the failure path too"
        );
    }

    #[test]
    fn csv_dedup_and_filter_arms_stream_instead_of_whole_file_reads() {
        // C11/C12 与 F06/F07/F08 复用同一流式底座（ADR-0054）：
        // 断言 read_bytes 0 次（TestFs 的 read_stream 按 3 字节分块，记录边界被切开）。
        let fs = TestFs::default();
        fs.write_file("/data/a.csv", b"id,value\n1,10\n2,20\n1,10\n3,30\n");
        let license = crate::license::AlwaysAllowLicense;

        let mut journal = TestJournal::default();
        let dedup = plan(vec![csv_dedup_entry(
            "e1",
            "/data/a.csv",
            "/out/dedup.csv",
            29,
            true,
            &["id", "value"],
            vec![],
        )]);
        let outcome = apply_generic_plan(
            &dedup,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(
            fs.read_file("/out/dedup.csv"),
            b"id,value\n1,10\n2,20\n3,30\n"
        );
        assert_eq!(
            fs.read_bytes_calls(),
            0,
            "csv-dedup must stream the source instead of read_bytes"
        );
        assert!(matches!(
            journal.entries[0].reverse,
            FsOp::Quarantine { .. }
        ));

        let mut journal = TestJournal::default();
        let filter = plan(vec![csv_filter_entry(
            "e1",
            "/data/a.csv",
            "/out/filtered.csv",
            29,
            true,
            &["id", "value"],
            (1, "ge", "20"),
        )]);
        let outcome = apply_generic_plan(
            &filter,
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(fs.read_file("/out/filtered.csv"), b"id,value\n2,20\n3,30\n");
        assert_eq!(
            fs.read_bytes_calls(),
            0,
            "csv-filter must stream the source instead of read_bytes"
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
        assert!(!fs.exists("/out/filtered.csv"));
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
        // 流式证据：csv-rows 必须走 read_stream，不得整文件读入（ADR-0054）。
        assert_eq!(
            fs.read_bytes_calls(),
            0,
            "csv-rows must stream the source instead of read_bytes"
        );
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
        assert_eq!(
            fs.read_ranges_stream_calls(),
            1,
            "数据区间必须走流式读取（内存 O(块)，不随选中区间线性增长）"
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
            csv_dedup: None,
            csv_filter: None,
            csv_columns: None,
            csv_sort: None,
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
            csv_dedup: None,
            csv_filter: None,
            csv_columns: None,
            csv_sort: None,
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
            csv_dedup: None,
            csv_filter: None,
            csv_columns: None,
            csv_sort: None,
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
            csv_dedup: None,
            csv_filter: None,
            csv_columns: None,
            csv_sort: None,
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
            entry_concurrency: 1,
            ..ApplyOptions::default()
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
            Ok(EditedContent::new(
                data.iter().map(|byte| byte.to_ascii_uppercase()).collect(),
            ))
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
    fn edit_content_carries_detail_per_entry_and_keeps_it_absent_when_not_produced() {
        // 方案 §5.2"默认替换并计数上报"的执行侧落点：注入变换产出的条目级 detail 必须随
        // 该条目的执行报告出现；两条目的 detail 不得互相串用；未产出明细时保持 None。
        let fs = TestFs::with_files(&["/data/a.txt", "/data/b.txt"]);
        fs.write_file("/data/a.txt", b"abc");
        fs.write_file("/data/b.txt", b"xy");
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut ctx = context(&fs, &mut journal, &license);
        ctx.content_editor = Some(&|data: &[u8]| {
            Ok(EditedContent::with_detail(
                data.to_vec(),
                format!("invalid sequences replaced: {}", data.len()),
            ))
        });
        let outcome = apply_generic_plan(
            &plan(vec![
                action_entry("e1", "/data/a.txt", None, PlanAction::EditContent),
                action_entry("e2", "/data/b.txt", None, PlanAction::EditContent),
            ]),
            &ApplyOptions::default(),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(
            outcome.report.entries[0].detail.as_deref(),
            Some("invalid sequences replaced: 3"),
            "第一条条目要带自己的计数"
        );
        assert_eq!(
            outcome.report.entries[1].detail.as_deref(),
            Some("invalid sequences replaced: 2"),
            "第二条条目不得复用第一条的明细"
        );
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);

        // 对照：注入方不产出明细时 detail 保持 None（不得凭空造一行）。
        let mut journal = TestJournal::default();
        let mut ctx = context(&fs, &mut journal, &license);
        ctx.content_editor = Some(&|data: &[u8]| Ok(EditedContent::new(data.to_vec())));
        let outcome = apply_generic_plan(
            &plan(vec![action_entry(
                "e3",
                "/data/a.txt",
                None,
                PlanAction::EditContent,
            )]),
            &ApplyOptions::default(),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);
        assert_eq!(outcome.report.entries[0].detail, None);
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

    #[test]
    fn cancel_keeps_applied_entries_under_keep_policy_and_undo_restores_them() {
        // 方案 §7.7：壳层先问、再决定。keepApplied 下取消只停派发：
        // 已完成的改名与产物留在原处并保持 Journal 可撤销，undo 时一并还原。
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
            &ApplyOptions {
                cancel_policy: crate::apply::CancelPolicy::KeepApplied,
                ..ApplyOptions::default()
            },
            &mut ctx,
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Cancelled);
        assert_eq!(outcome.report.undone_by, None, "保留处置不声称已撤销");
        assert!(!fs.exists("/data/a.txt"), "已完成条目保留在目标位置");
        assert!(fs.exists("/data/a1.txt"));
        assert!(fs.exists("/data/b.txt"), "未派发条目原地不动");
        assert!(!fs.exists("/data/b1.txt"));
        assert_eq!(
            journal.entries[0].status,
            JournalStatus::Applied,
            "保留的条目必须仍可撤销"
        );

        let undo = crate::apply::undo_plan(PLAN_ID, &fs, &license, &mut journal).unwrap();
        assert_eq!(undo.reverted, 1);
        assert!(fs.exists("/data/a.txt"));
        assert!(!fs.exists("/data/a1.txt"));
        assert_eq!(journal.entries[0].status, JournalStatus::Reverted);
    }

    /// 取消在第几条条目进度上报后触发（Ports.cancel_after 的语义）。
    fn cancel_ports(after: u64) -> Ports {
        let ports = Ports::default();
        *ports.cancel_after.lock().unwrap() = Some(after);
        ports
    }

    fn assert_cancelled(outcome: &ApplyOutcome, arm: &str) {
        assert_eq!(
            outcome.state,
            ApplyState::Cancelled,
            "{arm} 臂应在条目边界取消"
        );
    }

    #[test]
    fn cancel_rolls_back_applied_entries_in_each_write_arm() {
        // E4（任务控制）此前只在 Rename 与并发池上有断言；本测试把"取消后已完成条目回滚"
        // 逐执行臂钉住：copy / move / edit-content / split(1:N) / csv-extract(1:1 产物) /
        // create-dir / delete（隔离目录）。
        let license = crate::license::AlwaysAllowLicense;

        // ① copy：新建产物的逆向是移入隔离目录
        {
            let fs = TestFs::with_files(&["/d/a.txt", "/d/b.txt"]);
            let mut journal = TestJournal::default();
            let ports = cancel_ports(1);
            let mut ctx = context(&fs, &mut journal, &license);
            ctx.progress = Some(&ports);
            ctx.cancel = Some(&ports);
            let outcome = apply_generic_plan(
                &plan(vec![
                    action_entry("e1", "/d/a.txt", Some("/o/a.txt"), PlanAction::Copy),
                    action_entry("e2", "/d/b.txt", Some("/o/b.txt"), PlanAction::Copy),
                ]),
                &ApplyOptions::default(),
                &mut ctx,
            )
            .unwrap();
            assert_cancelled(&outcome, "copy");
            assert!(!fs.exists("/o/a.txt"), "取消不留半成品产物");
            assert!(
                fs.exists("/d/a.txt") && fs.exists("/d/b.txt"),
                "copy 不动源"
            );
            assert!(
                !journal
                    .entries
                    .iter()
                    .any(|entry| entry.status == JournalStatus::Applied),
                "取消后不得残留已应用状态的记账"
            );
        }

        // ② move：改名语义的逆向是反向移动
        {
            let fs = TestFs::with_files(&["/d/c.txt", "/d/d.txt"]);
            let mut journal = TestJournal::default();
            let ports = cancel_ports(1);
            let mut ctx = context(&fs, &mut journal, &license);
            ctx.progress = Some(&ports);
            ctx.cancel = Some(&ports);
            let outcome = apply_generic_plan(
                &plan(vec![
                    action_entry("e1", "/d/c.txt", Some("/o/c.txt"), PlanAction::Move),
                    action_entry("e2", "/d/d.txt", Some("/o/d.txt"), PlanAction::Move),
                ]),
                &ApplyOptions::default(),
                &mut ctx,
            )
            .unwrap();
            assert_cancelled(&outcome, "move");
            assert!(fs.exists("/d/c.txt"), "已移动的首条目被还原回源路径");
            assert!(!fs.exists("/o/c.txt"));
        }

        // ③ edit-content：先备份再改写，逆向是按 dataRef 还原
        {
            let fs = TestFs::default();
            fs.write_file("/d/e.txt", b"abc");
            fs.write_file("/d/f.txt", b"def");
            let mut journal = TestJournal::default();
            let ports = cancel_ports(1);
            let mut ctx = context(&fs, &mut journal, &license);
            ctx.progress = Some(&ports);
            ctx.cancel = Some(&ports);
            ctx.content_editor = Some(&|data: &[u8]| {
                Ok(EditedContent::new(
                    data.iter().map(|byte| byte.to_ascii_uppercase()).collect(),
                ))
            });
            let outcome = apply_generic_plan(
                &plan(vec![
                    action_entry("e1", "/d/e.txt", None, PlanAction::EditContent),
                    action_entry("e2", "/d/f.txt", None, PlanAction::EditContent),
                ]),
                &ApplyOptions::default(),
                &mut ctx,
            )
            .unwrap();
            assert_cancelled(&outcome, "edit-content");
            assert_eq!(fs.read_file("/d/e.txt"), b"abc", "内容改写取消后按备份还原");
        }

        // ④ split：一个源文件产 1:N 条目，取消发生在第 1 片之后
        {
            let fs = TestFs::default();
            fs.write_file("/d/big.txt", b"aaa\nbbb\nccc\n");
            let mut accumulator = SplitAccumulator::new(6).unwrap();
            for _ in 0..3 {
                accumulator.push_line(4, true);
            }
            let plan = plan_f01(PlanF01Input {
                plan_id: PLAN_ID.to_string(),
                target_bytes: 6,
                output_dir: "/o".to_string(),
                protected_dirs: Vec::new(),
                case_sense: CaseSense::Sensitive,
                existing_targets: Vec::new(),
                files: vec![PlanF01File {
                    path: "/d/big.txt".to_string(),
                    canonical_path: None,
                    group_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_string(),
                    layout: accumulator.finish(12),
                }],
            })
            .unwrap();
            assert_eq!(plan.entries.len(), 2, "两条分片条目才有取消空间");
            let mut journal = TestJournal::default();
            let ports = cancel_ports(1);
            let mut ctx = context(&fs, &mut journal, &license);
            ctx.progress = Some(&ports);
            ctx.cancel = Some(&ports);
            let outcome = apply_generic_plan(&plan, &ApplyOptions::default(), &mut ctx).unwrap();
            assert_cancelled(&outcome, "split");
            assert!(!fs.exists("/o/big.1.txt"), "已写分片随取消进隔离目录");
            assert!(!fs.exists("/o/big.2.txt"));
        }

        // ⑤ csv-extract：1:1 产物臂（与 csv-rows/csv-dedup/csv-filter/csv-columns/csv-sort 同管道）
        {
            let fs = TestFs::default();
            let first = b"id,name\n1,Alice\n";
            let second = b"id,name\n2,Bob\n";
            fs.write_file("/d/a.csv", first);
            fs.write_file("/d/b.csv", second);
            let build = |id: &str, source: &str, target: &str, size: u64| {
                csv_extract_entry(
                    id,
                    source,
                    target,
                    size,
                    true,
                    &["id", "name"],
                    2,
                    vec![(1, Some("name"))],
                )
            };
            let mut journal = TestJournal::default();
            let ports = cancel_ports(1);
            let mut ctx = context(&fs, &mut journal, &license);
            ctx.progress = Some(&ports);
            ctx.cancel = Some(&ports);
            let outcome = apply_generic_plan(
                &plan(vec![
                    build("e1", "/d/a.csv", "/o/a.csv", first.len() as u64),
                    build("e2", "/d/b.csv", "/o/b.csv", second.len() as u64),
                ]),
                &ApplyOptions::default(),
                &mut ctx,
            )
            .unwrap();
            assert_cancelled(&outcome, "csv-extract");
            assert!(!fs.exists("/o/a.csv"), "抽取产物随取消进隔离目录");
            assert_eq!(fs.read_file("/d/a.csv"), first.to_vec(), "源 CSV 未被改写");
        }

        // ⑥ create-dir：目录链的逆向是只删空目录
        {
            let fs = TestFs::default();
            let mut journal = TestJournal::default();
            let ports = cancel_ports(1);
            let mut ctx = context(&fs, &mut journal, &license);
            ctx.progress = Some(&ports);
            ctx.cancel = Some(&ports);
            let outcome = apply_generic_plan(
                &plan(vec![
                    action_entry("e1", "/d", Some("/o/one"), PlanAction::CreateDir),
                    action_entry("e2", "/d", Some("/o/two"), PlanAction::CreateDir),
                ]),
                &ApplyOptions::default(),
                &mut ctx,
            )
            .unwrap();
            assert_cancelled(&outcome, "create-dir");
            assert!(!fs.dir_exists("/o/one"), "已建目录随取消删除");
        }

        // ⑦ delete：条目先移入隔离目录，取消须把它取回原位（C09 清理走这条臂）
        {
            let fs = TestFs::with_files(&["/d/one.txt", "/d/two.txt"]);
            let mut journal = TestJournal::default();
            let ports = cancel_ports(1);
            let mut ctx = context(&fs, &mut journal, &license);
            ctx.progress = Some(&ports);
            ctx.cancel = Some(&ports);
            let outcome = apply_generic_plan(
                &plan(vec![
                    action_entry("e1", "/d/one.txt", None, PlanAction::Delete),
                    action_entry("e2", "/d/two.txt", None, PlanAction::Delete),
                ]),
                &ApplyOptions::default(),
                &mut ctx,
            )
            .unwrap();
            assert_cancelled(&outcome, "delete");
            assert!(
                fs.exists("/d/one.txt"),
                "已隔离的首条目随取消取回原位，不得留在隔离目录"
            );
            assert!(fs.exists("/d/two.txt"), "未派发的条目保持原状");
            assert!(!fs.exists("/quarantine/1"), "回滚后隔离目录不得残留该条目");

            // 对照：不取消时首条目确实进隔离目录（防止上面的断言空转）
            let control = TestFs::with_files(&["/d/one.txt", "/d/two.txt"]);
            let mut control_journal = TestJournal::default();
            let control_ports = Ports::default();
            let mut control_ctx = context(&control, &mut control_journal, &license);
            control_ctx.progress = Some(&control_ports);
            control_ctx.cancel = Some(&control_ports);
            let done = apply_generic_plan(
                &plan(vec![
                    action_entry("e1", "/d/one.txt", None, PlanAction::Delete),
                    action_entry("e2", "/d/two.txt", None, PlanAction::Delete),
                ]),
                &ApplyOptions::default(),
                &mut control_ctx,
            )
            .unwrap();
            assert_eq!(done.state, ApplyState::Completed);
            assert!(!control.exists("/d/one.txt"), "未取消时条目应移入隔离目录");
            assert!(control.exists("/quarantine/1"), "隔离目录应持有该条目");
        }
    }

    #[test]
    fn a_failing_entry_marks_partial_and_undo_reverts_only_applied_entries() {
        // RBK-02（部分失败）：条目级失败不把整单变成"已应用"，撤销只回滚成功条目。
        let fs = TestFs::default();
        fs.write_file("/d/a.txt", b"aaa");
        fs.write_file("/d/b.txt", b"bbb");
        fs.set_fail_writes(&["/o/b.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_generic_plan(
            &plan(vec![
                action_entry("e1", "/d/a.txt", Some("/o/a.txt"), PlanAction::Copy),
                action_entry("e2", "/d/b.txt", Some("/o/b.txt"), PlanAction::Copy),
            ]),
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::PartialFailed);
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);
        assert_eq!(outcome.report.entries[1].status, ApplyStatus::Failed);
        assert!(fs.exists("/o/a.txt"), "成功条目已落盘，等待撤销或重试");
        assert!(!fs.exists("/o/b.txt"), "失败条目不得留下半成品");
        assert_eq!(
            journal
                .entries
                .iter()
                .filter(|entry| entry.status == JournalStatus::Applied)
                .count(),
            1,
            "只有成功条目进入 Journal"
        );

        let undo = crate::apply::undo_plan_with_ports(
            PLAN_ID,
            &fs,
            &license,
            &mut journal,
            crate::apply::UndoPorts::default(),
        )
        .unwrap();
        assert_eq!(undo.reverted, 1, "撤销只回滚已应用条目，不因失败条目报错");
        assert!(!fs.exists("/o/a.txt"));
    }

    struct UpperRewriter;

    impl ContentRewriter for UpperRewriter {
        fn rewrite(
            &self,
            _entry: &crate::plan::PlanEntry,
            input: &mut dyn std::io::Read,
            output: &mut dyn std::io::Write,
            _cancel: &dyn CancelToken,
        ) -> Result<EditOutcome, KernelError> {
            let mut buffer = Vec::new();
            input
                .read_to_end(&mut buffer)
                .map_err(|error| KernelError::new("E5001", error.to_string(), None, false))?;
            let upper: Vec<u8> = buffer
                .iter()
                .map(|byte| byte.to_ascii_uppercase())
                .collect();
            output
                .write_all(&upper)
                .map_err(|error| KernelError::new("E5001", error.to_string(), None, false))?;
            Ok(EditOutcome {
                detail: Some("upper rewriter ran".to_string()),
            })
        }
    }

    struct TwoRowLister;

    impl crate::listing::DirectoryLister for TwoRowLister {
        fn write_listing(
            &self,
            root: &str,
            _options: &crate::listing::ListingOptions,
            out: &mut dyn std::io::Write,
        ) -> Result<crate::listing::ListingStats, KernelError> {
            out.write_all(format!("相对路径,名称,类型\n{root},a.txt,文件\n").as_bytes())
                .map_err(|error| KernelError::new("E5001", error.to_string(), None, false))?;
            Ok(crate::listing::ListingStats {
                files: 1,
                dirs: 0,
                placeholders: 0,
                skipped: 0,
                max_depth: 1,
            })
        }
    }

    #[test]
    fn cancel_rolls_back_the_batch_and_streaming_write_arms() {
        // F 类补齐：merge／csv-merge（N:1 单条目）、csv-rows、csv-range（区间读）、
        // ContentRewriter（c02/c03 流式重写）与 listing（流式写出）各臂的取消回滚。
        let license = crate::license::AlwaysAllowLicense;

        // ① merge（N:1 产物）
        {
            let fs = TestFs::default();
            fs.write_file("/d/a1.txt", b"A\n");
            fs.write_file("/d/a2.txt", b"B\n");
            fs.write_file("/d/b1.txt", b"C\n");
            fs.write_file("/d/b2.txt", b"D\n");
            let plan = plan(vec![
                merge_entry(
                    "e1",
                    "/o/one.txt",
                    TargetEncoding::Utf8 { bom: false },
                    vec![
                        ("/d/a1.txt", SourceEncoding::Utf8, 2),
                        ("/d/a2.txt", SourceEncoding::Utf8, 2),
                    ],
                ),
                merge_entry(
                    "e2",
                    "/o/two.txt",
                    TargetEncoding::Utf8 { bom: false },
                    vec![
                        ("/d/b1.txt", SourceEncoding::Utf8, 2),
                        ("/d/b2.txt", SourceEncoding::Utf8, 2),
                    ],
                ),
            ]);
            let mut journal = TestJournal::default();
            let ports = cancel_ports(1);
            let mut ctx = context(&fs, &mut journal, &license);
            ctx.progress = Some(&ports);
            ctx.cancel = Some(&ports);
            let outcome = apply_generic_plan(&plan, &ApplyOptions::default(), &mut ctx).unwrap();
            assert_cancelled(&outcome, "merge");
            assert!(!fs.exists("/o/one.txt"), "已合并产物随取消进隔离目录");
            assert!(fs.exists("/d/a1.txt"), "merge 不动源");
        }

        // ② csv-merge（F06 载荷同走 Merge 臂）
        {
            let fs = TestFs::default();
            fs.write_file("/d/a.csv", b"id,name\n1,Alice\n");
            fs.write_file("/d/b.csv", b"name,extra\nBob,x\n");
            fs.write_file("/d/c.csv", b"id,name\n2,Bob\n");
            fs.write_file("/d/d.csv", b"name,extra\nCara,y\n");
            let build = |id: &str, target: &str, first: &str, second: &str, columns: &[&str]| {
                csv_merge_entry(
                    id,
                    target,
                    vec![
                        csv_file(first, 16, columns, Some("a.csv")),
                        csv_file(second, 17, columns, Some("b.csv")),
                    ],
                    vec!["id", "name", "extra"],
                    true,
                    3,
                    None,
                )
            };
            let plan = plan(vec![
                build("e1", "/o/one.csv", "/d/a.csv", "/d/b.csv", &["id", "name"]),
                build("e2", "/o/two.csv", "/d/c.csv", "/d/d.csv", &["id", "name"]),
            ]);
            let mut journal = TestJournal::default();
            let ports = cancel_ports(1);
            let mut ctx = context(&fs, &mut journal, &license);
            ctx.progress = Some(&ports);
            ctx.cancel = Some(&ports);
            let outcome = apply_generic_plan(&plan, &ApplyOptions::default(), &mut ctx).unwrap();
            assert_cancelled(&outcome, "csv-merge");
            assert!(!fs.exists("/o/one.csv"), "合并 CSV 随取消进隔离目录");
        }

        // ③ csv-rows（前/后 N 行）
        {
            let fs = TestFs::default();
            fs.write_file("/d/one.csv", b"id,name\n1,A\n2,B\n");
            fs.write_file("/d/two.csv", b"id,name\n3,C\n4,D\n");
            let plan = plan(vec![
                csv_rows_entry(
                    "e1",
                    "/d/one.csv",
                    "/o/one.csv",
                    15,
                    true,
                    &["id", "name"],
                    2,
                    2,
                    "first",
                    1,
                ),
                csv_rows_entry(
                    "e2",
                    "/d/two.csv",
                    "/o/two.csv",
                    15,
                    true,
                    &["id", "name"],
                    2,
                    2,
                    "first",
                    1,
                ),
            ]);
            let mut journal = TestJournal::default();
            let ports = cancel_ports(1);
            let mut ctx = context(&fs, &mut journal, &license);
            ctx.progress = Some(&ports);
            ctx.cancel = Some(&ports);
            let outcome = apply_generic_plan(&plan, &ApplyOptions::default(), &mut ctx).unwrap();
            assert_cancelled(&outcome, "csv-rows");
            assert!(!fs.exists("/o/one.csv"));
        }

        // ④ csv-range（只读表头＋选中区间）
        {
            let fs = TestFs::default();
            let data = b"id,name\n1,Alice\n2,Bob\n3,Cara\n";
            fs.write_file("/d/one.csv", data);
            fs.write_file("/d/two.csv", data);
            let build = |id: &str, source: &str, target: &str| {
                csv_range_entry(
                    id,
                    source,
                    target,
                    data.len() as u64,
                    true,
                    &["id", "name"],
                    2,
                    3,
                    2,
                    3,
                    Some((0, 8)),
                    Some((8, data.len() as u64)),
                    2,
                )
            };
            let plan = plan(vec![
                build("e1", "/d/one.csv", "/o/one.csv"),
                build("e2", "/d/two.csv", "/o/two.csv"),
            ]);
            let mut journal = TestJournal::default();
            let ports = cancel_ports(1);
            let mut ctx = context(&fs, &mut journal, &license);
            ctx.progress = Some(&ports);
            ctx.cancel = Some(&ports);
            let outcome = apply_generic_plan(&plan, &ApplyOptions::default(), &mut ctx).unwrap();
            assert_cancelled(&outcome, "csv-range");
            assert!(!fs.exists("/o/one.csv"));
            assert_eq!(fs.read_file("/d/one.csv"), data.to_vec(), "区间读不改源");
        }

        // ⑤ ContentRewriter 流式重写（c02/c03 的编辑通道）
        {
            let fs = TestFs::default();
            fs.write_file("/d/one.txt", b"abc\n");
            fs.write_file("/d/two.txt", b"def\n");
            let plan = plan(vec![
                action_entry("e1", "/d/one.txt", None, PlanAction::EditContent),
                action_entry("e2", "/d/two.txt", None, PlanAction::EditContent),
            ]);
            let mut journal = TestJournal::default();
            let ports = cancel_ports(1);
            let mut ctx = context(&fs, &mut journal, &license);
            ctx.progress = Some(&ports);
            ctx.cancel = Some(&ports);
            let rewriter = UpperRewriter;
            ctx.content_rewriter = Some(&rewriter);
            let outcome = apply_generic_plan(&plan, &ApplyOptions::default(), &mut ctx).unwrap();
            assert_cancelled(&outcome, "content-rewriter");
            assert_eq!(
                fs.read_file("/d/one.txt"),
                b"abc\n",
                "重写取消后按备份还原原字节"
            );
        }

        // ⑥ listing（流式写出）
        {
            let fs = TestFs::default();
            let listing_plan = |root: &str, target: &str| {
                crate::listing::plan_f10(crate::listing::PlanF10Input {
                    plan_id: PLAN_ID.to_string(),
                    protected_dirs: Vec::new(),
                    case_sense: CaseSense::Sensitive,
                    files: vec![crate::listing::PlanF10File {
                        root: root.to_string(),
                        canonical_root: None,
                        target: target.to_string(),
                        target_occupied: false,
                        files: 1,
                        dirs: 0,
                        placeholders: 0,
                        skipped: 0,
                        max_depth: 1,
                    }],
                })
                .unwrap()
            };
            // 两个根目录＝两条 listing 条目，才有条目边界的取消窗口
            let mut merged = listing_plan("/d/one", "/o/one.csv");
            let second = listing_plan("/d/two", "/o/two.csv");
            merged.entries.extend(second.entries.clone());
            let mut journal = TestJournal::default();
            let ports = cancel_ports(1);
            let mut ctx = context(&fs, &mut journal, &license);
            ctx.progress = Some(&ports);
            ctx.cancel = Some(&ports);
            let lister = TwoRowLister;
            ctx.directory_lister = Some(&lister);
            let outcome = apply_generic_plan(&merged, &ApplyOptions::default(), &mut ctx).unwrap();
            assert_cancelled(&outcome, "listing");
            assert!(!fs.exists("/o/one.csv"), "已写清单随取消进隔离目录");
        }
    }

    struct QuotaLicense {
        remaining: std::sync::atomic::AtomicU32,
    }

    impl crate::license::LicenseService for QuotaLicense {
        fn capability(&self, _op: &str) -> crate::license::Capability {
            crate::license::Capability::Allow
        }

        fn consume(&self, _op: &str, n: u32) -> Result<(), KernelError> {
            use std::sync::atomic::Ordering;
            let mut current = self.remaining.load(Ordering::SeqCst);
            loop {
                if current < n {
                    return Err(KernelError::new(
                        "E4004",
                        "license quota exhausted",
                        None,
                        false,
                    ));
                }
                match self.remaining.compare_exchange(
                    current,
                    current - n,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) => return Ok(()),
                    Err(actual) => current = actual,
                }
            }
        }

        fn release(&self, _op: &str, n: u32) {
            self.remaining
                .fetch_add(n, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn pooled_options(concurrency: usize) -> ApplyOptions {
        ApplyOptions {
            on_conflict: OnConflict::Skip,
            overwrite_confirmed: false,
            entry_concurrency: concurrency,
            ..ApplyOptions::default()
        }
    }

    /// 按 `is_cancelled()` 的**询问次数**翻标志的取消端口。
    ///
    /// 条目边界的取消用 `Ports`（按进度事件计数）就够；**单条内部 mid-stream** 的
    /// 取消窗口只在流式重写器的读块循环里，只能按询问次数定时——询问顺序是
    /// 「每读一块一次」，因此 `after = N` 表示"第 N 块读完后、第 N+1 块开始前"。
    /// 若将来重写器改了询问位置，这条断言会立刻失败，不会静默放过。
    struct ChunkCancel {
        after: usize,
        asks: AtomicUsize,
    }

    impl CancelToken for ChunkCancel {
        fn is_cancelled(&self) -> bool {
            self.asks.fetch_add(1, Ordering::SeqCst) >= self.after
        }
    }

    /// TC-C02-EXE-04：**单条内部 mid-stream 取消**。
    ///
    /// 此前 `ContentRewriter` 不接收 cancel token，取消只能在条目边界生效，
    /// 因此"大文件重写跑到一半取消"这条路径根本不存在（缺口原文：真要支持需先改 trait）。
    /// 现在 C02 在读块边界问一次：命中即返回 E3010，回调以 Err 收场、
    /// `rewrite_atomic_with` 丢弃临时文件，**源文件保持原样**。
    #[test]
    fn mid_stream_cancel_aborts_the_rewrite_and_leaves_the_source_untouched() {
        use crate::line_endings::LineEndingRewriter;
        use crate::plan::{PlanAction, PlanParamValue};
        let fs = TestFs::default();
        // 4 个 64 KiB 块以上：取消落在第 3 次询问时一定在条目**内部**。
        let original: Vec<u8> = b"line\n".repeat(64 * 1024);
        fs.write_file("/d/big.txt", &original);

        let mut params = std::collections::BTreeMap::new();
        params.insert(
            "target".to_string(),
            PlanParamValue::String("crlf".to_string()),
        );
        params.insert(
            "structure".to_string(),
            PlanParamValue::String("text".to_string()),
        );
        let mut document = plan(vec![action_entry(
            "e1",
            "/d/big.txt",
            None,
            PlanAction::EditContent,
        )]);
        document.kind = crate::line_endings::C02_KIND.to_string();
        document.params = params;

        let rewriter = LineEndingRewriter::from_plan(&document).expect("build rewriter");
        let mut journal = TestJournal::default();
        let license = crate::license::AlwaysAllowLicense;
        let mut ctx = context(&fs, &mut journal, &license);
        let cancel = ChunkCancel {
            after: 3,
            asks: AtomicUsize::new(0),
        };
        ctx.cancel = Some(&cancel);
        ctx.content_rewriter = Some(&rewriter);

        let outcome = apply_generic_plan(&document, &ApplyOptions::default(), &mut ctx)
            .expect("cancelled apply still returns an outcome");

        assert_eq!(
            outcome.state,
            ApplyState::Cancelled,
            "mid-stream 取消必须把整单标成 Cancelled"
        );
        assert_eq!(
            fs.read_file("/d/big.txt"),
            original.as_slice(),
            "源文件必须逐字节保持原样——临时文件被丢弃，半截内容不得落盘"
        );
        assert_eq!(outcome.report.entries.len(), 1);
        assert_eq!(
            outcome.report.entries[0].status,
            ApplyStatus::Skipped,
            "被取消的那一条记 skipped（未改动），不是 failed"
        );
        assert_eq!(
            outcome.report.entries[0]
                .error
                .as_ref()
                .map(|error| error.code.as_str()),
            Some("E3010"),
            "取消走专用码 E3010，与普通写回失败可区分"
        );
        assert!(
            cancel.asks.load(Ordering::SeqCst) >= 3,
            "取消确实发生在读块循环内（询问过至少 3 次）"
        );

        // 对照：不取消时同一计划把 LF 全部转成 CRLF（证明上面的"未改动"是取消所致，
        // 而不是计划本身没效果）。
        let fs2 = TestFs::default();
        fs2.write_file("/d/big.txt", &original);
        let mut journal2 = TestJournal::default();
        let mut ctx2 = context(&fs2, &mut journal2, &license);
        let never = ChunkCancel {
            after: usize::MAX,
            asks: AtomicUsize::new(0),
        };
        ctx2.cancel = Some(&never);
        ctx2.content_rewriter = Some(&rewriter);
        let ok = apply_generic_plan(&document, &ApplyOptions::default(), &mut ctx2).unwrap();
        assert_eq!(ok.state, ApplyState::Completed);
        assert_eq!(
            fs2.read_file("/d/big.txt"),
            b"line\r\n".repeat(64 * 1024).as_slice(),
            "对照：不取消时重写确实生效"
        );
    }

    #[test]
    fn pooled_apply_matches_serial_results_and_undo() {
        let fs = TestFs::with_files(&["/data/a.txt", "/data/b.txt", "/data/c.txt"]);
        fs.write_file("/data/a.txt", b"aaa");
        fs.write_file("/data/b.txt", b"bbb");
        fs.write_file("/data/c.txt", b"ccc");
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut ctx = context(&fs, &mut journal, &license);
        let outcome = apply_generic_plan(
            &plan(vec![
                action_entry("e1", "/data/a.txt", Some("/out/a.txt"), PlanAction::Copy),
                action_entry("e2", "/data/b.txt", Some("/out/b.txt"), PlanAction::Copy),
                action_entry("e3", "/data/c.txt", Some("/out/c.txt"), PlanAction::Copy),
            ]),
            &pooled_options(4),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(fs.read_file("/out/a.txt"), b"aaa");
        assert_eq!(fs.read_file("/out/b.txt"), b"bbb");
        assert_eq!(fs.read_file("/out/c.txt"), b"ccc");
        assert_eq!(journal.entries.len(), 3);
        assert!(journal
            .entries
            .iter()
            .all(|entry| entry.status == JournalStatus::Applied));

        let undo = crate::apply::undo_plan_with_ports(
            PLAN_ID,
            &fs,
            &license,
            &mut journal,
            crate::apply::UndoPorts::default(),
        )
        .unwrap();
        assert_eq!(undo.reverted, 3);
        assert!(!fs.exists("/out/a.txt"));
        assert!(!fs.exists("/out/b.txt"));
        assert!(!fs.exists("/out/c.txt"));
    }

    #[test]
    fn pooled_quota_is_enforced_deterministically() {
        let fs = TestFs::with_files(&["/data/a.txt", "/data/b.txt", "/data/c.txt", "/data/d.txt"]);
        let license = QuotaLicense {
            remaining: std::sync::atomic::AtomicU32::new(2),
        };
        let mut journal = TestJournal::default();
        let mut ctx = context(&fs, &mut journal, &license);
        let outcome = apply_generic_plan(
            &plan(vec![
                action_entry("e1", "/data/a.txt", Some("/out/a.txt"), PlanAction::Copy),
                action_entry("e2", "/data/b.txt", Some("/out/b.txt"), PlanAction::Copy),
                action_entry("e3", "/data/c.txt", Some("/out/c.txt"), PlanAction::Copy),
                action_entry("e4", "/data/d.txt", Some("/out/d.txt"), PlanAction::Copy),
            ]),
            &pooled_options(4),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::PartialFailed);
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);
        assert_eq!(outcome.report.entries[1].status, ApplyStatus::Ok);
        assert_eq!(outcome.report.entries[2].status, ApplyStatus::Failed);
        assert_eq!(outcome.report.entries[3].status, ApplyStatus::Failed);
        assert_eq!(
            outcome.report.entries[2].error.as_ref().unwrap().code,
            "E4004"
        );
        assert_eq!(journal.entries.len(), 2);
    }

    #[test]
    fn pooled_same_source_edit_content_is_serialized() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        fs.write_file("/data/a.txt", b"x");
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut ctx = context(&fs, &mut journal, &license);
        let editor = |data: &[u8]| {
            let mut owned = data.to_vec();
            owned.extend_from_slice(b"+");
            Ok(EditedContent::with_detail(
                owned,
                "appended a marker byte by the test editor",
            ))
        };
        ctx.content_editor = Some(&editor);
        let outcome = apply_generic_plan(
            &plan(vec![
                action_entry("e1", "/data/a.txt", None, PlanAction::EditContent),
                action_entry("e2", "/data/a.txt", None, PlanAction::EditContent),
            ]),
            &pooled_options(4),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        // 同源条目资源键相交 → 拆块串行：第二次改写看到第一次的结果。
        assert_eq!(fs.read_file("/data/a.txt"), b"x++");
        assert_eq!(journal.entries.len(), 2);
    }

    #[test]
    fn pooled_cancel_stops_dispatch_and_rolls_back() {
        let fs = TestFs::with_files(&["/data/a.txt", "/data/b.txt", "/data/c.txt", "/data/d.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let ports = Ports::default();
        *ports.cancel_after.lock().unwrap() = Some(2);
        let mut ctx = context(&fs, &mut journal, &license);
        ctx.progress = Some(&ports);
        ctx.cancel = Some(&ports);
        let outcome = apply_generic_plan(
            &plan(vec![
                action_entry("e1", "/data/a.txt", Some("/out/a.txt"), PlanAction::Copy),
                action_entry("e2", "/data/b.txt", Some("/out/b.txt"), PlanAction::Copy),
                action_entry("e3", "/data/c.txt", Some("/out/c.txt"), PlanAction::Copy),
                action_entry("e4", "/data/d.txt", Some("/out/d.txt"), PlanAction::Copy),
            ]),
            &pooled_options(2),
            &mut ctx,
        )
        .unwrap();
        // 块大小＝并发上限：首块两条完成后取消生效，e3/e4 不再派发。
        assert_eq!(outcome.state, ApplyState::Cancelled);
        assert_eq!(journal.entries.len(), 2);
        assert!(fs.exists("/data/a.txt"));
        assert!(fs.exists("/data/b.txt"));
        assert!(fs.exists("/data/c.txt"));
        assert!(fs.exists("/data/d.txt"));
        assert!(!fs.exists("/out/a.txt"));
        assert!(!fs.exists("/out/b.txt"));
        assert!(!fs.exists("/out/c.txt"));
        assert!(!fs.exists("/out/d.txt"));
        assert!(journal
            .entries
            .iter()
            .all(|entry| entry.status == JournalStatus::Reverted));
    }

    #[test]
    fn entry_concurrency_out_of_range_is_rejected() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        for bad in [0usize, MAX_ENTRY_CONCURRENCY + 1] {
            let mut journal = TestJournal::default();
            let mut ctx = context(&fs, &mut journal, &license);
            let error = apply_generic_plan(
                &plan(vec![action_entry(
                    "e1",
                    "/data/a.txt",
                    Some("/out/a.txt"),
                    PlanAction::Copy,
                )]),
                &pooled_options(bad),
                &mut ctx,
            )
            .unwrap_err();
            assert_eq!(error.code, "E1002");
        }
    }

    #[test]
    fn plan_entry_count_limit_is_rejected() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut ctx = context(&fs, &mut journal, &license);
        let entries = (0..=MAX_PLAN_ENTRIES)
            .map(|i| {
                let mut entry = action_entry(
                    &format!("e-{i:06}"),
                    "/data/a.txt",
                    Some("/out/a.txt"),
                    PlanAction::Copy,
                );
                entry.blocked = Some(crate::plan::PlanBlocked {
                    code: "E1002".to_string(),
                    reason: "test".to_string(),
                    hint: None,
                });
                entry
            })
            .collect();
        let error = apply_generic_plan(&plan(entries), &pooled_options(1), &mut ctx).unwrap_err();
        assert_eq!(error.code, "E3007");
    }
}
