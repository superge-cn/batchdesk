//! Apply and undo orchestration for C09.
//!
//! The kernel only drives ports: `FsAdapter` performs path operations,
//! `LicenseService` is checked per entry, and `JournalSink` records the
//! forward/reverse operation before the next entry. No clock, network or
//! environment access happens here; the caller supplies task/plan IDs and the
//! RFC 3339 timestamp.

use serde::{Deserialize, Serialize};

use crate::error::KernelError;
use crate::fs::{CaseSense, ConflictPolicy, EntryKind, FsAdapter, PathOutcomeKind, ScanFilter};
use crate::journal::{FsOp, JournalStatus, JournalStore};
use crate::license::{Capability, LicenseService, C09};
use crate::path::{protected_by, try_normalize, ProtectedDir};
use crate::plan::{PlanAction, PlanDocument, PlanParamValue, PlanResolution};
use crate::progress::{CancelToken, ProgressEvent, ProgressPhase, ProgressSink};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnConflict {
    #[default]
    Skip,
    Sequence,
    Overwrite,
    Ask,
}

/// 单任务条目上限（方案 §5.12：超出提示分批，避免内存失控）。
pub const MAX_PLAN_ENTRIES: usize = 200_000;

/// 条目级并发上限（方案 §5.12：用户可调范围 1–16）。
pub const MAX_ENTRY_CONCURRENCY: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOptions {
    #[serde(default)]
    pub on_conflict: OnConflict,
    #[serde(default)]
    pub overwrite_confirmed: bool,
    /// 任务内条目级并发（有界工作池，方案 §5.12）；1＝串行（缺省，行为不变）。
    #[serde(default = "default_entry_concurrency")]
    pub entry_concurrency: usize,
    /// 取消时对「已完成条目」的处置（方案 §7.7）。
    /// 缺省 `rollbackApplied` 供没有交互面的调用方使用；桌面壳在取消后要询问用户，
    /// 因此显式传 `keepApplied`，等答复再走同一本 Journal 的撤销通道。
    #[serde(default)]
    pub cancel_policy: CancelPolicy,
}

/// 取消时已完成条目的处置（方案 §7.7：停止派发＋安全收尾＋询问是否回滚已完成项）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CancelPolicy {
    /// 立即按 Journal 反向回滚已应用条目（无交互面的调用方的缺省行为）。
    #[default]
    RollbackApplied,
    /// 保留已应用条目并留在 Journal 里，由调用方询问用户后再决定是否撤销。
    KeepApplied,
}

fn default_entry_concurrency() -> usize {
    1
}

/// 方案 §5.12 的工作类别：决定并发默认值走哪一档。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkClass {
    /// I/O 密集（复制／移动／解压）：SSD/NVMe 取 4，机械盘与网络盘取 2。
    IoBound,
    /// CPU 密集（哈希／编码检测／压缩）：核数减一，上限 8。
    CpuBound,
}

/// 方案 §5.12「并发的默认值（决策）：按介质自适应」的纯函数实现。
///
/// - I/O 密集：SSD/NVMe → 4；机械盘／网络盘／判别不了 → 2（保守）。
/// - CPU 密集：`cpus - 1`，夹在 1–8。
/// - 结果统一夹进 `MAX_ENTRY_CONCURRENCY`（1–16）。
pub fn adaptive_concurrency(medium: crate::fs::MediumKind, class: WorkClass, cpus: usize) -> usize {
    let base = match class {
        WorkClass::IoBound => match medium {
            crate::fs::MediumKind::SsdNvme => 4,
            crate::fs::MediumKind::Hdd
            | crate::fs::MediumKind::Network
            | crate::fs::MediumKind::Unknown => 2,
        },
        WorkClass::CpuBound => cpus.saturating_sub(1).clamp(1, 8),
    };
    base.clamp(1, MAX_ENTRY_CONCURRENCY)
}

/// 方案 §5.12 的「超大文件（>2 GB）自动把并发降为 1–2」。
pub const HUGE_FILE_ENTRY_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// 计划里是否存在超大条目（体积取自 `PlanExpected.size`）。
pub fn plan_has_huge_entry(plan: &PlanDocument) -> bool {
    plan.entries.iter().any(|entry| {
        entry
            .expected
            .as_ref()
            .is_some_and(|expected| expected.size > HUGE_FILE_ENTRY_BYTES)
    })
}

/// 超大文件降并发的上限：SSD/NVMe 取 2，其余（机械盘／网络盘／判别不了）取 1。
///
/// 方案 §5.12 给的是区间「1–2」，此处按既有的介质分档把它落成确定值——
/// 与 I/O 密集档「SSD 4／其余 2」同一套判别，保持单调（SSD: 4→2，其余: 2→1）。
pub fn huge_entry_cap(medium: crate::fs::MediumKind) -> usize {
    match medium {
        crate::fs::MediumKind::SsdNvme => 2,
        _ => 1,
    }
}

/// 计划的工作类别（方案 §5.12 的两档）。
///
/// CPU 密集＝哈希／编码检测／压缩：转码（C01）、换行与替换重写（C02/C03）、
/// 哈希去重（C09/C11）。c10 按动作分：压缩＝CPU、解压＝I/O。
/// 其余（复制／移动／改名／分片／合并／CSV 抽取／目录清单）都是 I/O 主导。
pub fn plan_work_class(plan: &PlanDocument) -> WorkClass {
    use crate::plan::PlanAction;
    match plan.kind.as_str() {
        crate::plan::C01_KIND
        | crate::line_endings::C02_KIND
        | crate::replace_content::C03_KIND
        | crate::plan::C09_KIND
        | crate::csv_dedup::C11_KIND => WorkClass::CpuBound,
        crate::archive::C10_KIND => {
            if plan
                .entries
                .iter()
                .any(|entry| entry.action == PlanAction::Archive)
            {
                WorkClass::CpuBound
            } else {
                WorkClass::IoBound
            }
        }
        _ => WorkClass::IoBound,
    }
}

/// 方案 §5.12 的执行期限额：单任务条目上限与条目级并发可调范围。
/// 计划生成不设上限，执行层统一复核（不信任壳层/入口门禁）。
pub fn validate_apply_limits(
    plan: &PlanDocument,
    options: &ApplyOptions,
) -> Result<(), KernelError> {
    if plan.entries.len() > MAX_PLAN_ENTRIES {
        return Err(KernelError::new(
            "E3007",
            "plan entry count exceeds the per-task limit; split the task",
            Some(format!(
                "plan has {} entries; limit is {MAX_PLAN_ENTRIES}",
                plan.entries.len()
            )),
            false,
        ));
    }
    if options.entry_concurrency == 0 || options.entry_concurrency > MAX_ENTRY_CONCURRENCY {
        return Err(KernelError::invalid_params(format!(
            "entryConcurrency must be between 1 and {MAX_ENTRY_CONCURRENCY}"
        )));
    }
    Ok(())
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            on_conflict: OnConflict::Skip,
            overwrite_confirmed: false,
            entry_concurrency: default_entry_concurrency(),
            cancel_policy: CancelPolicy::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApplyStatus {
    Ok,
    Overwritten,
    Skipped,
    Failed,
}

impl ApplyStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Overwritten => "overwritten",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyEntryReport {
    pub entry_id: String,
    pub status: ApplyStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<KernelError>,
    /// 条目级明细：由执行器注入的变换产出（方案 §5.2 的"替换并计数上报"落在这里）。
    /// 与 `error` 互不冲突——成功条目也可带明细；缺省时省略字段，旧报告仍可反序列化。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    pub plan_id: String,
    pub task_id: String,
    pub entries: Vec<ApplyEntryReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undone_by: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApplyState {
    Completed,
    PartialFailed,
    Failed,
    Cancelled,
}

impl ApplyState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::PartialFailed => "partial-failed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOutcome {
    pub state: ApplyState,
    pub report: ApplyReport,
}

pub struct ApplyContext<'a> {
    pub fs: &'a dyn FsAdapter,
    pub license: &'a dyn LicenseService,
    pub journal: &'a mut dyn JournalStore,
    pub task_id: String,
    pub now_rfc3339: String,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    /// 目标名合法性规则（平台能力；设计 04 §2.1）。apply 期的名称复核
    /// 必须与计划期同一套规则，否则会重演"计划选中、执行判不安全"的分叉。
    pub name_rules: crate::conflict::NameRules,
    pub progress: Option<&'a dyn ProgressSink>,
    pub cancel: Option<&'a dyn CancelToken>,
    /// D1：网络断线的退避等待与自动暂停（None＝不重试）。
    pub network_retry: Option<&'a dyn crate::retry::NetworkRetry>,
}

pub fn apply_plan(
    plan: &PlanDocument,
    options: &ApplyOptions,
    context: &mut ApplyContext<'_>,
) -> Result<ApplyOutcome, KernelError> {
    validate_apply_limits(plan, options)?;
    if plan.kind.as_str() != C09 {
        return Err(KernelError::invalid_params(
            "only c09 plans are supported by this apply slice",
        ));
    }
    if plan.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    match plan.params.get("mode") {
        Some(PlanParamValue::String(mode)) if mode == "full" => {}
        _ => {
            return Err(KernelError::invalid_params(
                "c09 apply requires a full-mode plan",
            ));
        }
    }
    for entry in &plan.entries {
        if entry.action != PlanAction::Delete {
            return Err(KernelError::invalid_params(
                "c09 apply supports only delete entries",
            ));
        }
        if entry.target.is_some() {
            return Err(KernelError::invalid_params(
                "c09 delete entries must not have a target",
            ));
        }
    }

    let total = plan.entries.len() as u64;
    let mut entries = Vec::with_capacity(plan.entries.len());
    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut applied = Vec::new();
    let mut cancelled = false;
    let mut stopped_at = plan.entries.len();

    report_progress(context, ProgressPhase::Executing, 0, total, None);
    for (index, entry) in plan.entries.iter().enumerate() {
        report_progress(context, ProgressPhase::Executing, index as u64, total, None);
        if is_cancelled(context) {
            cancelled = true;
            stopped_at = index;
            break;
        }
        if let Some(blocked) = &entry.blocked {
            entries.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Skipped,
                error: Some(KernelError::new(
                    blocked.code.clone(),
                    blocked.reason.clone(),
                    blocked.hint.clone(),
                    false,
                )),
                detail: None,
            });
            continue;
        }
        if entry.resolution == Some(PlanResolution::Skip) {
            entries.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Skipped,
                error: None,
                detail: None,
            });
            continue;
        }

        let normalized = match try_normalize(&entry.source) {
            Ok(normalized) => normalized,
            Err(error) => {
                failed += 1;
                entries.push(failed_entry(&entry.entry_id, error));
                continue;
            }
        };
        if !context.protected_dirs.is_empty() {
            let canonical = context.fs.canonicalize(&entry.source).ok();
            if let Some(hit) = protected_by(
                &normalized,
                canonical.as_deref(),
                &context.protected_dirs,
                context.case_sense,
            ) {
                entries.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Skipped,
                    error: Some(KernelError::new(
                        "E2009",
                        format!("path is under protected directory {hit}"),
                        None,
                        false,
                    )),
                    detail: None,
                });
                continue;
            }
        }

        match context.license.capability(C09) {
            Capability::Allow => {}
            Capability::Deny { reason } => {
                failed += 1;
                entries.push(failed_entry(
                    &entry.entry_id,
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
                entries.push(failed_entry(
                    &entry.entry_id,
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

        // D1 实机补全（2026-11-08）：网络盘上的执行前 stat 与哈希复核同样会因
        // 断线失败；此前只包了隔离移动，导致 C09 在断线时逐条立即失败并跑完，
        // 界面看不到自动暂停。stat/哈希与 rename 共用同一重试与暂停语义。
        let network = context.fs.is_network_path(&entry.source);
        let network_retry = context.network_retry;
        let fs = context.fs;
        let stat = match crate::retry::with_network_retry(network_retry, network, || {
            fs.stat(&entry.source)
        }) {
            Ok(stat) => stat,
            Err(failure) if failure.abort => return Err(failure.error),
            Err(failure) => {
                failed += 1;
                entries.push(failed_entry(&entry.entry_id, failure.error));
                continue;
            }
        };
        if stat.kind != EntryKind::File {
            failed += 1;
            entries.push(failed_entry(
                &entry.entry_id,
                KernelError::invalid_params("c09 can only quarantine files"),
            ));
            continue;
        }
        if stat.attrs.placeholder {
            entries.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Skipped,
                error: Some(KernelError::new(
                    "E2006",
                    "placeholder file skipped",
                    None,
                    true,
                )),
                detail: None,
            });
            continue;
        }

        if let Some(expected) = &entry.expected {
            if stat.size != expected.size {
                entries.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Skipped,
                    error: Some(KernelError::new(
                        "E2010",
                        "source size changed since plan preview",
                        None,
                        false,
                    )),
                    detail: None,
                });
                continue;
            }
            if let Some(expected_hash) = &expected.hash {
                let hashed = crate::retry::with_network_retry(network_retry, network, || {
                    fs.content_hash(&entry.source, &expected.algorithm)
                });
                match hashed {
                    Ok(Some(actual)) if &actual != expected_hash => {
                        entries.push(ApplyEntryReport {
                            entry_id: entry.entry_id.clone(),
                            status: ApplyStatus::Skipped,
                            error: Some(KernelError::new(
                                "E2010",
                                "source content changed since plan preview",
                                None,
                                false,
                            )),
                            detail: None,
                        });
                        continue;
                    }
                    Ok(_) => {}
                    Err(failure) if failure.abort => return Err(failure.error),
                    Err(failure) => {
                        failed += 1;
                        entries.push(failed_entry(&entry.entry_id, failure.error));
                        continue;
                    }
                }
            }
        }

        if let Err(error) = context.license.consume(C09, 1) {
            failed += 1;
            entries.push(failed_entry(&entry.entry_id, error));
            continue;
        }

        let file_name = match std::path::Path::new(&entry.source)
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
        {
            Some(name) => name,
            None => {
                context.license.release(C09, 1);
                failed += 1;
                entries.push(failed_entry(
                    &entry.entry_id,
                    KernelError::new("E1003", "source path has no file name", None, false),
                ));
                continue;
            }
        };
        let quarantine = match context.fs.quarantine_target_path(&file_name) {
            Ok(path) => path,
            Err(error) => {
                context.license.release(C09, 1);
                failed += 1;
                entries.push(failed_entry(&entry.entry_id, error));
                continue;
            }
        };

        let forward = FsOp::Quarantine {
            from: entry.source.clone(),
            to: quarantine.clone(),
        };
        let reverse = forward.inverse().ok_or_else(|| {
            KernelError::internal(
                "quarantine journal reverse is not expressible",
                "FsOp::inverse returned None for quarantine",
            )
        })?;
        // D5：先落"待确认"条目，再移动，最后确认 Applied；崩溃在移动与确认之间时，
        // 下一次打开 Journal 会按隔离路径是否存在把 Pending 收敛为 Applied/丢弃。
        let pending = match context.journal.append_pending_with_kind(
            &plan.plan_id,
            &entry.entry_id,
            &context.now_rfc3339,
            &plan.kind,
            forward,
            reverse.clone(),
        ) {
            Ok(entry) => entry,
            Err(error) => {
                context.license.release(C09, 1);
                failed += 1;
                entries.push(failed_entry(&entry.entry_id, error));
                continue;
            }
        };

        // D1：网络路径（UNC 或映射盘）断线时对隔离移动做 3 次退避重试；耗尽后由壳层暂停任务。
        let moved = crate::retry::with_network_retry(
            context.network_retry,
            context.fs.is_network_path(&entry.source),
            || {
                context
                    .fs
                    .rename(&entry.source, &quarantine, ConflictPolicy::Skip)
            },
        );
        let moved = match moved {
            Ok(outcome) if outcome.outcome != PathOutcomeKind::Skipped => outcome,
            Ok(_) => {
                let _ = context.journal.discard_pending(&plan.plan_id, pending.seq);
                context.license.release(C09, 1);
                failed += 1;
                entries.push(failed_entry(
                    &entry.entry_id,
                    KernelError::new("E2007", "quarantine target already exists", None, false),
                ));
                continue;
            }
            Err(failure) if failure.abort => return Err(failure.error),
            Err(failure) => {
                let _ = context.journal.discard_pending(&plan.plan_id, pending.seq);
                context.license.release(C09, 1);
                failed += 1;
                entries.push(failed_entry(&entry.entry_id, failure.error));
                continue;
            }
        };
        let _ = moved;
        if let Err(error) = context.journal.mark_applied(&plan.plan_id, pending.seq) {
            return Err(KernelError::internal(
                "quarantine move succeeded but journal confirmation failed",
                format!("{}: {}", error.code, error.message),
            ));
        }
        applied.push(AppliedOp {
            entry_index: index,
            seq: pending.seq,
            reverse,
        });

        ok += 1;
        entries.push(ApplyEntryReport {
            entry_id: entry.entry_id.clone(),
            status: ApplyStatus::Ok,
            error: None,
            detail: None,
        });
    }

    if cancelled {
        // 方案 §7.7 的两种处置：调用方已问过用户（keepApplied）就保留并留给撤销；
        // 没有交互面（缺省 rollbackApplied）就立即按 Journal 反向回滚。
        let keep_applied = options.cancel_policy == CancelPolicy::KeepApplied;
        let rollback_total = applied.len() as u64;
        let mut rollback_failed = false;
        if !keep_applied {
            report_progress(
                context,
                ProgressPhase::Journaling,
                0,
                rollback_total,
                Some("rolling back applied entries"),
            );
            for (done, applied_op) in applied.iter().rev().enumerate() {
                let result = match &applied_op.reverse {
                    FsOp::Rename { from, to } => {
                        match context.fs.rename(from, to, ConflictPolicy::Skip) {
                            Ok(outcome) if outcome.outcome != PathOutcomeKind::Skipped => Ok(()),
                            Ok(_) => Err(KernelError::new(
                                "E2007",
                                "cancel rollback target already exists",
                                None,
                                false,
                            )),
                            Err(error) => Err(error),
                        }
                    }
                    _ => Err(KernelError::internal(
                        "cancel rollback reverse is not supported",
                        format!("{:?}", applied_op.reverse),
                    )),
                };
                match result {
                    Ok(()) => {
                        if let Err(error) = context
                            .journal
                            .mark_reverted(&plan.plan_id, &[applied_op.seq])
                        {
                            rollback_failed = true;
                            mark_entry_error(&mut entries, applied_op.entry_index, error);
                        }
                    }
                    Err(error) => {
                        rollback_failed = true;
                        mark_entry_error(&mut entries, applied_op.entry_index, error);
                    }
                }
                report_progress(
                    context,
                    ProgressPhase::Journaling,
                    (done + 1) as u64,
                    rollback_total,
                    None,
                );
            }
        }
        for entry in &plan.entries[stopped_at..] {
            entries.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Skipped,
                error: None,
                detail: None,
            });
        }
        report_progress(context, ProgressPhase::Finalizing, 1, 1, None);
        return Ok(ApplyOutcome {
            state: ApplyState::Cancelled,
            report: ApplyReport {
                plan_id: plan.plan_id.clone(),
                task_id: context.task_id.clone(),
                entries,
                undone_by: if keep_applied || rollback_failed {
                    None
                } else {
                    Some(context.task_id.clone())
                },
            },
        });
    }

    report_progress(context, ProgressPhase::Executing, total, total, None);
    report_progress(context, ProgressPhase::Finalizing, 1, 1, None);
    let state = if failed > 0 && ok > 0 {
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
            entries,
            undone_by: None,
        },
    })
}

struct AppliedOp {
    entry_index: usize,
    seq: u64,
    reverse: FsOp,
}

fn is_cancelled(context: &ApplyContext<'_>) -> bool {
    context
        .cancel
        .map(|token| token.is_cancelled())
        .unwrap_or(false)
}

fn report_progress(
    context: &ApplyContext<'_>,
    phase: ProgressPhase,
    done: u64,
    total: u64,
    message: Option<&str>,
) {
    if let Some(sink) = context.progress {
        sink.report(ProgressEvent {
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

fn mark_entry_error(entries: &mut [ApplyEntryReport], index: usize, error: KernelError) {
    if let Some(report) = entries.get_mut(index) {
        report.status = ApplyStatus::Failed;
        report.error = Some(error);
    }
}

fn failed_entry(entry_id: &str, error: KernelError) -> ApplyEntryReport {
    ApplyEntryReport {
        entry_id: entry_id.to_string(),
        status: ApplyStatus::Failed,
        error: Some(error),
        detail: None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoFailure {
    pub entry_id: String,
    pub error: KernelError,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoOutcome {
    pub reverted: usize,
    pub failed: Vec<UndoFailure>,
    pub entries: Vec<ApplyEntryReport>,
}

#[derive(Default)]
pub struct UndoPorts<'a> {
    pub task_id: Option<&'a str>,
    pub progress: Option<&'a dyn ProgressSink>,
    pub cancel: Option<&'a dyn CancelToken>,
    /// D1：撤销路径的断线重试端口（与 apply 共用同一套退避/暂停语义）。
    pub network_retry: Option<&'a dyn crate::retry::NetworkRetry>,
}

pub fn undo_plan(
    plan_id: &str,
    fs: &dyn FsAdapter,
    license: &dyn LicenseService,
    journal: &mut dyn JournalStore,
) -> Result<UndoOutcome, KernelError> {
    undo_plan_with_ports(plan_id, fs, license, journal, UndoPorts::default())
}

/// C06 回滚预演（方案 §7.8"回滚前重新做冲突检测，目标被占用则逐条跳过并报告"）：
/// 按 Journal 逆序检查每条 applied 记录的 reverse 落点，只读 stat、零副作用。
/// 恢复目标已存在 → conflict（执行时按策略跳过并报告）；源/备份缺失 → missing；
/// 其余 → ok。预演结果不写 Journal、不改文件状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UndoDryRunStatus {
    Ok,
    Conflict,
    Missing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoDryRunEntry {
    pub entry_id: String,
    pub status: UndoDryRunStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoDryRun {
    pub plan_id: String,
    pub entries: Vec<UndoDryRunEntry>,
    pub conflict_count: usize,
    pub missing_count: usize,
}

pub fn undo_dry_run(
    plan_id: &str,
    journal: &mut dyn JournalStore,
    fs: &dyn FsAdapter,
) -> Result<UndoDryRun, KernelError> {
    undo_dry_run_with_retry(plan_id, journal, fs, None)
}

/// C06 回滚预演的带端口版本：网络路径的 `stat`/`list_dir` 走与 apply/undo 同一套
/// D1 重试；宿主没有暂停交互面时（CLI/预览命令），重试耗尽即返回 E2005，
/// 不把断线伪装成 `missing`（旧行为把任何 stat 错误都记成源已缺失）。
pub fn undo_dry_run_with_retry(
    plan_id: &str,
    journal: &mut dyn JournalStore,
    fs: &dyn FsAdapter,
    network_retry: Option<&dyn crate::retry::NetworkRetry>,
) -> Result<UndoDryRun, KernelError> {
    let entries = journal.entries_for_plan(plan_id)?;
    let mut preview = Vec::new();
    let mut conflict_count = 0;
    let mut missing_count = 0;
    for entry in entries
        .iter()
        .rev()
        .filter(|entry| entry.status == JournalStatus::Applied)
    {
        // 与执行期判定同一口径：逆向操作任一侧是网络路径，整条检查进重试族。
        let network = crate::retry::fs_op_touches_network(fs, &entry.reverse);
        let stat_exists = |path: &str| -> Result<bool, KernelError> {
            match crate::retry::with_network_retry(network_retry, network, || fs.stat(path)) {
                Ok(_) => Ok(true),
                // 断线重试耗尽/宿主取消：把 E2005 上抛，不能降级成「不存在」。
                Err(failure) if failure.error.code == "E2005" => Err(failure.error),
                Err(_) => Ok(false),
            }
        };
        let (status, code, detail) = match &entry.reverse {
            // rename 还原与"产物进隔离目录"（reverse=Quarantine）都是两点位移：
            // 落点被占 → conflict；源已缺失 → missing。
            FsOp::Rename { from, to } | FsOp::Quarantine { from, to } => {
                if stat_exists(to)? {
                    conflict_count += 1;
                    (
                        UndoDryRunStatus::Conflict,
                        Some("E2007".to_string()),
                        Some(format!("restore target already exists: {to}")),
                    )
                } else if !stat_exists(from)? {
                    missing_count += 1;
                    (
                        UndoDryRunStatus::Missing,
                        Some("E2001".to_string()),
                        Some(format!("source no longer exists: {from}")),
                    )
                } else {
                    (UndoDryRunStatus::Ok, None, None)
                }
            }
            // 覆盖写还原：备份在即可还原；目标缺失时还原会重建（提示预演事实）。
            FsOp::WriteAtomic {
                path,
                data_ref: Some(backup),
            } => {
                if !stat_exists(backup)? {
                    missing_count += 1;
                    (
                        UndoDryRunStatus::Missing,
                        Some("E2001".to_string()),
                        Some(format!("backup no longer exists: {backup}")),
                    )
                } else if !stat_exists(path)? {
                    (
                        UndoDryRunStatus::Ok,
                        None,
                        Some(format!("target missing; restore will recreate: {path}")),
                    )
                } else {
                    (UndoDryRunStatus::Ok, None, None)
                }
            }
            FsOp::WriteAtomic { data_ref: None, .. } => (
                UndoDryRunStatus::Missing,
                Some("E9001".to_string()),
                Some("write_atomic reverse has no backup dataRef".to_string()),
            ),
            FsOp::Copy { .. } => (
                UndoDryRunStatus::Missing,
                Some("E9001".to_string()),
                Some("copy is not a reversible journal op".to_string()),
            ),
            // CreateDir 的逆是"删空目录"：目录在且为空 → ok；目录已不在 → missing；
            // 目录非空 → conflict（撤销只删空目录，F11 语义）。
            FsOp::RemoveDir { path } => {
                if !stat_exists(path)? {
                    missing_count += 1;
                    (
                        UndoDryRunStatus::Missing,
                        Some("E2001".to_string()),
                        Some(format!("directory no longer exists: {path}")),
                    )
                } else {
                    let listed =
                        match crate::retry::with_network_retry(network_retry, network, || {
                            fs.list_dir(path, &ScanFilter::default())
                        }) {
                            Ok(entries) => Ok(entries),
                            Err(failure) if failure.error.code == "E2005" => {
                                return Err(failure.error)
                            }
                            Err(failure) => Err(failure.error),
                        };
                    match listed {
                        Ok(entries) if entries.is_empty() => (UndoDryRunStatus::Ok, None, None),
                        Ok(_) => (
                            UndoDryRunStatus::Conflict,
                            Some("E2007".to_string()),
                            Some(format!("directory is not empty: {path}")),
                        ),
                        Err(error) => (
                            UndoDryRunStatus::Missing,
                            Some(error.code),
                            Some(error.message),
                        ),
                    }
                }
            }
            // reverse 集中不会出现 CreateDir（它只是 RemoveDir 的逆表达）。
            FsOp::CreateDir { .. } => (
                UndoDryRunStatus::Missing,
                Some("E9001".to_string()),
                Some("create_dir is not a reversible journal op".to_string()),
            ),
        };
        preview.push(UndoDryRunEntry {
            entry_id: entry.entry_id.clone(),
            status,
            code,
            detail,
        });
    }
    Ok(UndoDryRun {
        plan_id: plan_id.to_string(),
        entries: preview,
        conflict_count,
        missing_count,
    })
}

pub fn undo_plan_with_ports(
    plan_id: &str,
    fs: &dyn FsAdapter,
    license: &dyn LicenseService,
    journal: &mut dyn JournalStore,
    ports: UndoPorts<'_>,
) -> Result<UndoOutcome, KernelError> {
    if plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    let mut journal_entries = journal.entries_for_plan(plan_id)?;
    // 只回滚已确认落盘的条目；Pending 是 D5 的中间态，既不能撤（动作可能没发生）
    // 也不能留（由 Journal 打开时按隔离目录事实收敛）。
    journal_entries.retain(|entry| entry.status == JournalStatus::Applied);
    journal_entries.sort_by_key(|entry| std::cmp::Reverse(entry.seq));
    let total = journal_entries.len() as u64;

    let mut reverted = 0usize;
    let mut failed = Vec::new();
    let mut reports = Vec::with_capacity(journal_entries.len());
    report_undo_progress(&ports, ProgressPhase::Executing, 0, total);
    for (index, entry) in journal_entries.iter().enumerate() {
        report_undo_progress(&ports, ProgressPhase::Executing, index as u64, total);
        if ports
            .cancel
            .map(|token| token.is_cancelled())
            .unwrap_or(false)
        {
            break;
        }
        // D1 补全：撤销的逆向操作（改名/隔离/写回/建删目录）同样走断线重试。
        // 判定经适配层能力（UNC 或映射盘）；Abort 表示宿主无暂停交互面，整任务中止。
        let network = crate::retry::fs_op_touches_network(fs, &entry.reverse);
        let result = match crate::retry::with_network_retry(ports.network_retry, network, || {
            execute_reverse(fs, &entry.reverse)
        }) {
            Ok(()) => Ok(()),
            Err(failure) if failure.abort => return Err(failure.error),
            Err(failure) => Err(failure.error),
        };
        match result {
            Ok(()) => {
                license.release(entry.plan_kind.as_deref().unwrap_or(C09), 1);
                journal.mark_reverted(plan_id, &[entry.seq])?;
                reverted += 1;
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Ok,
                    error: None,
                    detail: None,
                });
            }
            Err(error) => {
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Failed,
                    error: Some(error.clone()),
                    detail: None,
                });
                failed.push(UndoFailure {
                    entry_id: entry.entry_id.clone(),
                    error,
                });
            }
        }
    }
    if total > 0 {
        report_undo_progress(&ports, ProgressPhase::Finalizing, 1, 1);
    }
    Ok(UndoOutcome {
        reverted,
        failed,
        entries: reports,
    })
}

/// 按 Journal 的 `reverse` 字面执行一条逆向操作（设计 01 §8：回滚 = 逆序执行
/// reverse）。供撤销任务与执行中取消回滚共用；执行器不得"现场发明"reverse，
/// 只执行 Journal 里已记录的操作。
pub(crate) fn execute_reverse(fs: &dyn FsAdapter, reverse: &FsOp) -> Result<(), KernelError> {
    match reverse {
        // 隔离还原：quarantine 的逆是改名回原位。
        FsOp::Rename { from, to } => match fs.rename(from, to, ConflictPolicy::Skip) {
            Ok(outcome) if outcome.outcome != PathOutcomeKind::Skipped => Ok(()),
            Ok(_) => Err(KernelError::new(
                "E2007",
                "restore target already exists",
                None,
                false,
            )),
            Err(error) => Err(error),
        },
        // 新建产物的撤销（设计 01 §8）：产物移入隔离目录，不在原地删除。
        FsOp::Quarantine { from, to } => match fs.rename(from, to, ConflictPolicy::Skip) {
            Ok(outcome) if outcome.outcome != PathOutcomeKind::Skipped => Ok(()),
            Ok(_) => Err(KernelError::new(
                "E2007",
                "quarantine target already exists",
                None,
                false,
            )),
            Err(error) => Err(error),
        },
        // 覆盖写的撤销：用备份内容原子写回原路径（dataRef 语义）。
        FsOp::WriteAtomic {
            path,
            data_ref: Some(backup),
        } => {
            let data = fs.read_bytes(backup)?;
            fs.write_atomic(path, &data)
        }
        FsOp::WriteAtomic { data_ref: None, .. } => Err(KernelError::invalid_params(
            "write_atomic reverse requires a backup dataRef",
        )),
        FsOp::CreateDir { path } => fs.create_dir(path),
        FsOp::RemoveDir { path } => fs.remove_dir_if_empty(path),
        FsOp::Copy { .. } => Err(KernelError::invalid_params(
            "copy is not a reversible journal op; reverses are rename/quarantine/write_atomic",
        )),
    }
}

fn report_undo_progress(ports: &UndoPorts<'_>, phase: ProgressPhase, done: u64, total: u64) {
    if let Some(sink) = ports.progress {
        sink.report(ProgressEvent {
            task_id: ports.task_id.unwrap_or_default().to_string(),
            phase,
            done,
            total,
            message: None,
            entry_done: None,
            entry_total: None,
        });
    }
}

#[cfg(test)]
mod tests {

    /// 方案 §5.12「超大文件（>2 GB）自动把并发降为 1–2」——判定与上限。
    #[test]
    fn huge_entries_are_detected_and_capped() {
        use crate::apply::{
            adaptive_concurrency, huge_entry_cap, plan_has_huge_entry, WorkClass,
            HUGE_FILE_ENTRY_BYTES,
        };
        use crate::fs::MediumKind;
        use crate::plan::{PlanAction, PlanEntry, PlanExpected};

        fn transcode_entry() -> PlanEntry {
            PlanEntry {
                entry_id: "e1".to_string(),
                source: "/s/a".to_string(),
                target: None,
                action: PlanAction::Transcode,
                relation: None,
                reason: String::new(),
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

        fn entry_with_size(size: u64) -> PlanEntry {
            let mut entry = transcode_entry();
            entry.expected = Some(PlanExpected {
                size,
                algorithm: "sha256".to_string(),
                hash: None,
            });
            entry
        }

        let mut document = plan(vec![entry_with_size(HUGE_FILE_ENTRY_BYTES)]);
        assert!(
            !plan_has_huge_entry(&document),
            "恰好 2 GiB 不算「>2 GB」——边界是严格大于"
        );
        document.entries[0].expected.as_mut().unwrap().size = HUGE_FILE_ENTRY_BYTES + 1;
        assert!(plan_has_huge_entry(&document));
        document.entries.push(entry_with_size(1024));
        assert!(
            plan_has_huge_entry(&document),
            "只要有一条超大就降并发，不因其他条目小就放行"
        );

        // 上限按介质分档，且必须真的把自适应值压下来
        assert_eq!(huge_entry_cap(MediumKind::SsdNvme), 2);
        assert_eq!(huge_entry_cap(MediumKind::Hdd), 1);
        assert_eq!(huge_entry_cap(MediumKind::Network), 1);
        assert_eq!(huge_entry_cap(MediumKind::Unknown), 1);
        let normal = adaptive_concurrency(MediumKind::SsdNvme, WorkClass::IoBound, 8);
        assert_eq!(normal, 4);
        assert_eq!(normal.min(huge_entry_cap(MediumKind::SsdNvme)), 2);
        let normal = adaptive_concurrency(MediumKind::Hdd, WorkClass::IoBound, 8);
        assert_eq!(normal, 2);
        assert_eq!(normal.min(huge_entry_cap(MediumKind::Hdd)), 1);
    }

    /// 方案 §5.12「并发的默认值（决策）：按介质自适应」——纯函数全档断言。
    #[test]
    fn adaptive_concurrency_follows_medium_and_work_class() {
        use crate::apply::{adaptive_concurrency, WorkClass};
        use crate::fs::MediumKind;

        // I/O 密集：SSD/NVMe 4，机械盘／网络盘／判别不了 2
        assert_eq!(
            adaptive_concurrency(MediumKind::SsdNvme, WorkClass::IoBound, 8),
            4
        );
        assert_eq!(
            adaptive_concurrency(MediumKind::Hdd, WorkClass::IoBound, 8),
            2
        );
        assert_eq!(
            adaptive_concurrency(MediumKind::Network, WorkClass::IoBound, 8),
            2
        );
        assert_eq!(
            adaptive_concurrency(MediumKind::Unknown, WorkClass::IoBound, 8),
            2,
            "判别不了必须保守取低档，不得按 SSD 放行"
        );

        // CPU 密集：核数减一，夹在 1–8
        assert_eq!(
            adaptive_concurrency(MediumKind::SsdNvme, WorkClass::CpuBound, 8),
            7
        );
        assert_eq!(
            adaptive_concurrency(MediumKind::Hdd, WorkClass::CpuBound, 2),
            1
        );
        assert_eq!(
            adaptive_concurrency(MediumKind::Hdd, WorkClass::CpuBound, 1),
            1
        );
        assert_eq!(
            adaptive_concurrency(MediumKind::Hdd, WorkClass::CpuBound, 64),
            8,
            "CPU 密集上限 8，不随核数无限涨"
        );
        // 介质对 CPU 密集不产生影响
        assert_eq!(
            adaptive_concurrency(MediumKind::Network, WorkClass::CpuBound, 8),
            7
        );
    }

    /// 工作类别判定：哈希／编码检测／压缩归 CPU，复制／移动／解压归 I/O。
    #[test]
    fn plan_work_class_separates_cpu_from_io() {
        use crate::apply::{plan_work_class, WorkClass};
        use crate::plan::{PlanAction, PlanEntry};

        fn entry(action: PlanAction) -> PlanEntry {
            PlanEntry {
                entry_id: "e1".to_string(),
                source: "/s/a".to_string(),
                target: None,
                action,
                relation: None,
                reason: String::new(),
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

        for (kind, expected) in [
            (crate::plan::C01_KIND, WorkClass::CpuBound),
            (crate::line_endings::C02_KIND, WorkClass::CpuBound),
            (crate::replace_content::C03_KIND, WorkClass::CpuBound),
            (crate::plan::C09_KIND, WorkClass::CpuBound),
            (crate::csv_dedup::C11_KIND, WorkClass::CpuBound),
        ] {
            let mut document = plan(vec![entry(PlanAction::Transcode)]);
            document.kind = kind.to_string();
            assert_eq!(plan_work_class(&document), expected, "{kind}");
        }

        // c10：压缩＝CPU，解压＝I/O
        let mut compress = plan(vec![entry(PlanAction::Archive)]);
        compress.kind = crate::archive::C10_KIND.to_string();
        assert_eq!(plan_work_class(&compress), WorkClass::CpuBound);
        let mut extract = plan(vec![entry(PlanAction::Extract)]);
        extract.kind = crate::archive::C10_KIND.to_string();
        assert_eq!(plan_work_class(&extract), WorkClass::IoBound);

        // 复制／移动／改名／分片／合并／清单都是 I/O 主导
        for (kind, action) in [
            (crate::c07::C07_KIND, PlanAction::Move),
            (crate::name_replace::C04_KIND, PlanAction::Rename),
        ] {
            let mut document = plan(vec![entry(action)]);
            document.kind = kind.to_string();
            assert_eq!(plan_work_class(&document), WorkClass::IoBound, "{kind}");
        }
        let mut split = plan(vec![entry(PlanAction::Split)]);
        split.kind = crate::split::F01_KIND.to_string();
        assert_eq!(plan_work_class(&split), WorkClass::IoBound);
        let mut merge = plan(vec![entry(PlanAction::Merge)]);
        merge.kind = crate::merge::F04_KIND.to_string();
        assert_eq!(plan_work_class(&merge), WorkClass::IoBound);
    }

    use super::*;
    use crate::fs::{CaseSense, ConflictPolicy, DirEntryInfo, EntryAttrs, ScanFilter, StatInfo};
    use crate::journal::{FsOp, JournalEntry, JournalSink, JournalStatus, JournalStore};
    use crate::license::Capability;
    use crate::plan::{PlanEntry, PlanExpected, PlanStats, PlanWarning};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const PLAN_ID: &str = "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8";

    struct TestFs {
        files: Mutex<BTreeMap<String, Vec<u8>>>,
        next: Mutex<u64>,
        /// D5 崩溃注入：移动完成后 panic，模拟"移动已落盘、Journal 确认未写"。
        panic_after_move: AtomicBool,
        /// D1 故障注入：源路径 →（错误码, retryable, 剩余可失败次数）。
        rename_failures: Mutex<BTreeMap<String, (String, bool, u64)>>,
        /// D1 补全：stat 的网络断线注入（undo 预演的只读检查复用同一重试端口）。
        stat_failures: Mutex<BTreeMap<String, (String, bool, u64)>>,
        /// D1 实机补全：执行前哈希复核的网络断线注入（C09 在断线时曾在此逐条失败）。
        hash_failures: Mutex<BTreeMap<String, (String, bool, u64)>>,
        rename_calls: std::sync::atomic::AtomicU64,
        /// D1 补全：模拟盘符映射网络盘（`Z:`），验证执行器读适配层能力。
        mapped_network: AtomicBool,
    }

    impl TestFs {
        fn with_files(paths: &[&str]) -> Self {
            let mut files = BTreeMap::new();
            for path in paths {
                files.insert(path.to_string(), b"content".to_vec());
            }
            Self {
                files: Mutex::new(files),
                next: Mutex::new(0),
                panic_after_move: AtomicBool::new(false),
                rename_failures: Mutex::new(BTreeMap::new()),
                stat_failures: Mutex::new(BTreeMap::new()),
                hash_failures: Mutex::new(BTreeMap::new()),
                rename_calls: std::sync::atomic::AtomicU64::new(0),
                mapped_network: AtomicBool::new(false),
            }
        }

        fn set_mapped_network(&self, value: bool) {
            self.mapped_network.store(value, Ordering::SeqCst);
        }

        fn set_panic_after_move(&self, value: bool) {
            self.panic_after_move.store(value, Ordering::SeqCst);
        }

        fn set_rename_failure(&self, path: &str, code: &str, retryable: bool, times: u64) {
            self.rename_failures
                .lock()
                .unwrap()
                .insert(path.to_string(), (code.to_string(), retryable, times));
        }

        fn rename_error(&self, path: &str) -> Option<KernelError> {
            let mut failures = self.rename_failures.lock().unwrap();
            let (code, retryable, remaining) = failures.get_mut(path)?;
            if *remaining == 0 {
                return None;
            }
            *remaining -= 1;
            Some(KernelError::new(
                code.clone(),
                "injected rename failure",
                None,
                *retryable,
            ))
        }

        fn set_stat_failure(&self, path: &str, code: &str, retryable: bool, times: u64) {
            self.stat_failures
                .lock()
                .unwrap()
                .insert(path.to_string(), (code.to_string(), retryable, times));
        }

        fn stat_error(&self, path: &str) -> Option<KernelError> {
            let mut failures = self.stat_failures.lock().unwrap();
            let (code, retryable, remaining) = failures.get_mut(path)?;
            if *remaining == 0 {
                return None;
            }
            *remaining -= 1;
            Some(KernelError::new(
                code.clone(),
                "injected stat failure",
                None,
                *retryable,
            ))
        }

        fn exists(&self, path: &str) -> bool {
            self.files.lock().unwrap().contains_key(path)
        }

        fn set_hash_failure(&self, path: &str, code: &str, retryable: bool, times: u64) {
            self.hash_failures
                .lock()
                .unwrap()
                .insert(path.to_string(), (code.to_string(), retryable, times));
        }

        fn hash_error(&self, path: &str) -> Option<KernelError> {
            let mut failures = self.hash_failures.lock().unwrap();
            let (code, retryable, remaining) = failures.get_mut(path)?;
            if *remaining == 0 {
                return None;
            }
            *remaining -= 1;
            Some(KernelError::new(
                code.clone(),
                "injected hash failure",
                None,
                *retryable,
            ))
        }
    }

    fn hex(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::new();
        for byte in bytes {
            output.push(HEX[(byte >> 4) as usize] as char);
            output.push(HEX[(byte & 0x0f) as usize] as char);
        }
        output
    }

    #[derive(Default)]
    struct RecordingRetry {
        waits: Mutex<Vec<Duration>>,
        pauses: AtomicBool,
    }

    impl crate::retry::NetworkRetry for RecordingRetry {
        fn wait(&self, delay: Duration) -> bool {
            self.waits.lock().unwrap().push(delay);
            true
        }

        fn pause(&self, _error: &KernelError) -> crate::retry::PauseAction {
            self.pauses.store(true, Ordering::SeqCst);
            crate::retry::PauseAction::Resumed
        }
    }

    impl FsAdapter for TestFs {
        fn is_network_path(&self, path: &str) -> bool {
            (self.mapped_network.load(Ordering::SeqCst) && path.starts_with("Z:"))
                || crate::retry::is_network_path(path)
        }

        fn read_bytes(&self, path: &str) -> Result<Vec<u8>, KernelError> {
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
            Ok(Box::new(std::io::Cursor::new(data)))
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
        ) -> Result<crate::fs::PathOutcome, KernelError> {
            self.rename_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Some(error) = self.rename_error(from) {
                return Err(error);
            }
            let mut files = self.files.lock().unwrap();
            if !files.contains_key(from) {
                return Err(KernelError::new("E2001", "not found", None, false));
            }
            if files.contains_key(to) && policy == ConflictPolicy::Skip {
                return Ok(crate::fs::PathOutcome {
                    outcome: crate::fs::PathOutcomeKind::Skipped,
                    final_path: to.to_string(),
                });
            }
            let data = files.remove(from).unwrap();
            files.insert(to.to_string(), data);
            drop(files);
            if self.panic_after_move.load(Ordering::SeqCst) {
                panic!("simulated crash after quarantine move");
            }
            Ok(crate::fs::PathOutcome {
                outcome: crate::fs::PathOutcomeKind::Applied,
                final_path: to.to_string(),
            })
        }

        fn copy(
            &self,
            from: &str,
            to: &str,
            policy: ConflictPolicy,
        ) -> Result<crate::fs::PathOutcome, KernelError> {
            let files = self.files.lock().unwrap();
            let data = files
                .get(from)
                .cloned()
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))?;
            if files.contains_key(to) && policy == ConflictPolicy::Skip {
                return Ok(crate::fs::PathOutcome {
                    outcome: crate::fs::PathOutcomeKind::Skipped,
                    final_path: to.to_string(),
                });
            }
            drop(files);
            self.files.lock().unwrap().insert(to.to_string(), data);
            Ok(crate::fs::PathOutcome {
                outcome: crate::fs::PathOutcomeKind::Applied,
                final_path: to.to_string(),
            })
        }

        fn move_path(
            &self,
            from: &str,
            to: &str,
            policy: ConflictPolicy,
        ) -> Result<crate::fs::PathOutcome, KernelError> {
            self.rename(from, to, policy)
        }

        fn quarantine_target_path(&self, _file_name: &str) -> Result<String, KernelError> {
            let mut next = self.next.lock().unwrap();
            *next += 1;
            Ok(format!("/quarantine/{next}"))
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
            drop(files);
            drop(next);
            if self.panic_after_move.load(Ordering::SeqCst) {
                panic!("simulated crash after quarantine move");
            }
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
            if let Some(error) = self.stat_error(path) {
                return Err(error);
            }
            let files = self.files.lock().unwrap();
            if !files.contains_key(path) {
                return Err(KernelError::new("E2001", "not found", None, false));
            }
            Ok(StatInfo {
                kind: EntryKind::File,
                size: files[path].len() as u64,
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

        // 测试替身模拟 POSIX 临时目录：与 `Sensitive` 同一个前提，不是"随便填"。
        fn name_rules(&self) -> crate::conflict::NameRules {
            crate::conflict::NameRules::posix()
        }

        fn content_hash(&self, path: &str, algorithm: &str) -> Result<Option<String>, KernelError> {
            if let Some(error) = self.hash_error(path) {
                return Err(error);
            }
            if algorithm != "blake3" {
                return Ok(None);
            }
            let files = self.files.lock().unwrap();
            let data = files
                .get(path)
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))?;
            Ok(Some(hex(data)))
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
            for entry in &mut self.entries {
                if entry.plan_id == plan_id && seqs.contains(&entry.seq) {
                    entry.status = JournalStatus::Reverted;
                }
            }
            Ok(())
        }
    }

    struct DenyLicense;

    impl LicenseService for DenyLicense {
        fn capability(&self, _op: &str) -> Capability {
            Capability::Deny {
                reason: "test denial".to_string(),
            }
        }

        fn consume(&self, _op: &str, _n: u32) -> Result<(), KernelError> {
            Ok(())
        }

        fn release(&self, _op: &str, _n: u32) {}
    }

    fn delete_entry(entry_id: &str, source: &str) -> PlanEntry {
        PlanEntry {
            entry_id: entry_id.to_string(),
            source: source.to_string(),
            target: None,
            action: PlanAction::Delete,
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

    fn plan(entries: Vec<PlanEntry>) -> PlanDocument {
        let mut params = BTreeMap::new();
        params.insert(
            "mode".to_string(),
            PlanParamValue::String("full".to_string()),
        );
        PlanDocument {
            plan_id: PLAN_ID.to_string(),
            kind: C09.to_string(),
            params,
            entries,
            stats: PlanStats::default(),
            warnings: Vec::<PlanWarning>::new(),
        }
    }

    fn context<'a>(
        fs: &'a TestFs,
        license: &'a dyn LicenseService,
        journal: &'a mut TestJournal,
    ) -> ApplyContext<'a> {
        ApplyContext {
            fs,
            license,
            journal,
            task_id: "task-1".to_string(),
            now_rfc3339: "2026-10-29T00:00:00Z".to_string(),
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            name_rules: crate::conflict::NameRules::windows(),
            progress: None,
            cancel: None,
            network_retry: None,
        }
    }

    #[test]
    fn apply_moves_files_and_writes_journal() {
        let fs = TestFs::with_files(&["a"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_plan(
            &plan(vec![delete_entry("e1", "a")]),
            &ApplyOptions::default(),
            &mut context(&fs, &license, &mut journal),
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);
        assert!(!fs.exists("a"));
        assert_eq!(journal.entries.len(), 1);
        assert_eq!(journal.entries[0].status, JournalStatus::Applied);
    }

    #[test]
    fn c09_crash_after_quarantine_move_leaves_a_journal_entry() {
        // D5：旧实现"先移动、后写 Journal"——移动完成后立刻崩溃，Journal 里没有
        // 任何条目，隔离目录里的文件成为撤销看不见的孤儿。移动前必须先落意图。
        let fs = TestFs::with_files(&["/data/a.txt"]);
        fs.set_panic_after_move(true);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut context = context(&fs, &license, &mut journal);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = apply_plan(
                &plan(vec![delete_entry("e1", "/data/a.txt")]),
                &ApplyOptions::default(),
                &mut context,
            );
        }));
        assert!(result.is_err(), "故障注入必须真的在移动后中断");
        drop(context);
        assert_eq!(
            journal.entries.len(),
            1,
            "移动前必须先落 Journal 意图；否则崩溃后隔离文件无账可查（D5）"
        );
        assert!(!fs.exists("/data/a.txt"), "文件应已移动到隔离目录");
    }

    /// D1：c09 的隔离移动同样走 3 次退避重试；耗尽后暂停，恢复后重试。
    #[test]
    fn c09_network_disconnect_is_retried_three_times_then_pauses() {
        let fs = TestFs::with_files(&["//server/share/a.txt"]);
        fs.set_rename_failure("//server/share/a.txt", "E2005", true, 4);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let retry = RecordingRetry::default();
        let mut context = context(&fs, &license, &mut journal);
        context.network_retry = Some(&retry);
        let outcome = apply_plan(
            &plan(vec![delete_entry("e1", "//server/share/a.txt")]),
            &ApplyOptions::default(),
            &mut context,
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(
            fs.rename_calls.load(Ordering::SeqCst),
            5,
            "首次＋3 次退避重试＋暂停恢复后 1 次"
        );
        assert_eq!(
            *retry.waits.lock().unwrap(),
            vec![
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8)
            ]
        );
        assert!(retry.pauses.load(Ordering::SeqCst));
        assert!(!fs.exists("//server/share/a.txt"));
    }

    /// D1 实机补全（2026-11-08）：C09 的执行前 stat 在断线时同样自动暂停，
    /// 而不是逐条立即失败。此前只有隔离移动包了重试，实机 5200 条计划在
    /// 断线后 59 秒内跑完 5134 条 E2005 失败，界面从未进入「已暂停」。
    #[test]
    fn c09_network_stat_failure_is_retried_three_times_then_pauses() {
        let fs = TestFs::with_files(&["//server/share/a.txt"]);
        fs.set_stat_failure("//server/share/a.txt", "E2005", true, 4);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let retry = RecordingRetry::default();
        let mut context = context(&fs, &license, &mut journal);
        context.network_retry = Some(&retry);
        let outcome = apply_plan(
            &plan(vec![delete_entry("e1", "//server/share/a.txt")]),
            &ApplyOptions::default(),
            &mut context,
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(
            *retry.waits.lock().unwrap(),
            vec![
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8)
            ]
        );
        assert!(retry.pauses.load(Ordering::SeqCst));
        assert!(!fs.exists("//server/share/a.txt"));
    }

    /// D1 实机补全：执行前哈希复核（content_hash）同样走 3 次退避与暂停，
    /// 断线时不得把 5000+ 条计划逐条记失败跑完。
    #[test]
    fn c09_network_content_hash_failure_is_retried_three_times_then_pauses() {
        let fs = TestFs::with_files(&["//server/share/a.txt"]);
        fs.set_hash_failure("//server/share/a.txt", "E2005", true, 4);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let retry = RecordingRetry::default();
        let mut entry = delete_entry("e1", "//server/share/a.txt");
        entry.expected = Some(crate::plan::PlanExpected {
            size: 7,
            algorithm: "blake3".to_string(),
            hash: Some(hex(b"content")),
        });
        let mut context = context(&fs, &license, &mut journal);
        context.network_retry = Some(&retry);
        let outcome =
            apply_plan(&plan(vec![entry]), &ApplyOptions::default(), &mut context).unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(
            *retry.waits.lock().unwrap(),
            vec![
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8)
            ]
        );
        assert!(retry.pauses.load(Ordering::SeqCst));
        assert!(!fs.exists("//server/share/a.txt"));
    }

    /// D1 补全：undo 的逆向改名同样走 3 次退避重试；耗尽后暂停，恢复后完成撤销。
    #[test]
    fn undo_network_disconnect_is_retried_three_times_then_pauses() {
        let fs = TestFs::with_files(&["//server/share/a.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        {
            let mut context = context(&fs, &license, &mut journal);
            let outcome = apply_plan(
                &plan(vec![delete_entry("e1", "//server/share/a.txt")]),
                &ApplyOptions::default(),
                &mut context,
            )
            .unwrap();
            assert_eq!(outcome.state, ApplyState::Completed);
        }
        fs.set_rename_failure("/quarantine/1", "E2005", true, 4);
        let retry = RecordingRetry::default();
        let before = fs.rename_calls.load(Ordering::SeqCst);
        let outcome = undo_plan_with_ports(
            PLAN_ID,
            &fs,
            &license,
            &mut journal,
            UndoPorts {
                network_retry: Some(&retry),
                ..UndoPorts::default()
            },
        )
        .unwrap();
        assert_eq!(outcome.reverted, 1, "重试耗尽并恢复后必须完成撤销");
        assert_eq!(
            fs.rename_calls.load(Ordering::SeqCst) - before,
            5,
            "首次＋3 次退避重试＋暂停恢复后 1 次"
        );
        assert_eq!(
            *retry.waits.lock().unwrap(),
            vec![
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8)
            ]
        );
        assert!(retry.pauses.load(Ordering::SeqCst));
        assert!(fs.exists("//server/share/a.txt"), "撤销后源文件回到原位");
    }

    /// D1 补全：盘符映射网络盘（`Z:`）的判定走适配层能力，撤销与执行器都不再
    /// 只看字符串前缀。适配层把 `Z:` 标成网络盘后，apply 与 undo 都进入重试族。
    #[test]
    fn mapped_network_drive_apply_and_undo_are_retried() {
        let fs = TestFs::with_files(&["Z:\\data\\a.txt"]);
        fs.set_mapped_network(true);
        let entry = delete_entry("e1", "Z:\\data\\a.txt");
        assert!(
            crate::retry::entry_touches_network(&fs, &entry),
            "适配层判定 Z: 是网络盘时，条目必须进重试族"
        );
        assert!(
            !crate::retry::is_network_path("Z:\\data\\a.txt"),
            "纯字符串判定识别不了映射盘，必须由适配层补足"
        );
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        fs.set_rename_failure("Z:\\data\\a.txt", "E2005", true, 4);
        let apply_retry = RecordingRetry::default();
        {
            let mut context = context(&fs, &license, &mut journal);
            context.network_retry = Some(&apply_retry);
            let outcome =
                apply_plan(&plan(vec![entry]), &ApplyOptions::default(), &mut context).unwrap();
            assert_eq!(outcome.state, ApplyState::Completed, "映射盘断线后重试成功");
        }
        assert_eq!(
            fs.rename_calls.load(Ordering::SeqCst),
            5,
            "apply：首次＋3 次退避重试＋暂停恢复后 1 次"
        );
        assert!(apply_retry.pauses.load(Ordering::SeqCst));

        fs.set_rename_failure("/quarantine/1", "E2005", true, 4);
        let undo_retry = RecordingRetry::default();
        let before = fs.rename_calls.load(Ordering::SeqCst);
        let outcome = undo_plan_with_ports(
            PLAN_ID,
            &fs,
            &license,
            &mut journal,
            UndoPorts {
                network_retry: Some(&undo_retry),
                ..UndoPorts::default()
            },
        )
        .unwrap();
        assert_eq!(outcome.reverted, 1);
        assert_eq!(
            fs.rename_calls.load(Ordering::SeqCst) - before,
            5,
            "undo：映射盘断线后同样 5 次调用"
        );
        assert!(fs.exists("Z:\\data\\a.txt"));
    }

    /// D1 补全：无暂停交互面的宿主（CLI）在重试耗尽后按 Abort 中止整个撤销，
    /// 而不是把它降级成「条目失败、继续下一条」。
    #[test]
    fn undo_network_disconnect_aborts_the_whole_task_without_a_pause_host() {
        struct AbortRetry;
        impl crate::retry::NetworkRetry for AbortRetry {
            fn wait(&self, _delay: Duration) -> bool {
                true
            }
            fn pause(&self, _error: &KernelError) -> crate::retry::PauseAction {
                crate::retry::PauseAction::Abort
            }
        }

        let fs = TestFs::with_files(&["//server/share/a.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        {
            let mut context = context(&fs, &license, &mut journal);
            let _ = apply_plan(
                &plan(vec![delete_entry("e1", "//server/share/a.txt")]),
                &ApplyOptions::default(),
                &mut context,
            )
            .unwrap();
        }
        fs.set_rename_failure("/quarantine/1", "E2005", true, 4);
        let error = undo_plan_with_ports(
            PLAN_ID,
            &fs,
            &license,
            &mut journal,
            UndoPorts {
                network_retry: Some(&AbortRetry),
                ..UndoPorts::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "E2005");
    }

    #[test]
    fn denied_license_does_not_touch_the_file() {
        let fs = TestFs::with_files(&["a"]);
        let license = DenyLicense;
        let mut journal = TestJournal::default();
        let outcome = apply_plan(
            &plan(vec![delete_entry("e1", "a")]),
            &ApplyOptions::default(),
            &mut context(&fs, &license, &mut journal),
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Failed);
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Failed);
        assert_eq!(
            outcome.report.entries[0].error.as_ref().unwrap().code,
            "E4001"
        );
        assert!(fs.exists("a"));
        assert!(journal.entries.is_empty());
    }

    #[test]
    fn protected_source_is_skipped_even_without_plan_block() {
        let fs = TestFs::with_files(&["/protected/a"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut ctx = context(&fs, &license, &mut journal);
        ctx.protected_dirs = vec![ProtectedDir::new("/protected")];
        let outcome = apply_plan(
            &plan(vec![delete_entry("e1", "/protected/a")]),
            &ApplyOptions::default(),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Skipped);
        assert_eq!(
            outcome.report.entries[0].error.as_ref().unwrap().code,
            "E2009"
        );
        assert!(fs.exists("/protected/a"));
        assert!(journal.entries.is_empty());
    }

    #[test]
    fn blocked_entry_is_skipped_without_journal() {
        let fs = TestFs::with_files(&["a"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut entry = delete_entry("e1", "a");
        entry.blocked = Some(crate::plan::PlanBlocked {
            code: "E2009".to_string(),
            reason: "protected".to_string(),
            hint: None,
        });
        let outcome = apply_plan(
            &plan(vec![entry]),
            &ApplyOptions::default(),
            &mut context(&fs, &license, &mut journal),
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Skipped);
        assert!(fs.exists("a"));
        assert!(journal.entries.is_empty());
    }

    #[test]
    fn undo_replays_reverse_rename_and_marks_reverted() {
        let fs = TestFs::with_files(&["a"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        apply_plan(
            &plan(vec![delete_entry("e1", "a")]),
            &ApplyOptions::default(),
            &mut context(&fs, &license, &mut journal),
        )
        .unwrap();
        assert!(!fs.exists("a"));

        let outcome = undo_plan(PLAN_ID, &fs, &license, &mut journal).unwrap();
        assert_eq!(outcome.reverted, 1);
        assert!(outcome.failed.is_empty());
        assert!(fs.exists("a"));
        assert_eq!(journal.entries[0].status, JournalStatus::Reverted);
    }

    #[test]
    fn fast_mode_plans_are_rejected_before_apply() {
        let fs = TestFs::with_files(&["a"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut plan = plan(vec![delete_entry("e1", "a")]);
        plan.params.insert(
            "mode".to_string(),
            PlanParamValue::String("fast".to_string()),
        );
        let error = apply_plan(
            &plan,
            &ApplyOptions::default(),
            &mut context(&fs, &license, &mut journal),
        )
        .unwrap_err();
        assert_eq!(error.code, "E1002");
        assert!(fs.exists("a"));
    }

    #[test]
    fn matching_fingerprint_allows_apply() {
        let fs = TestFs::with_files(&["a"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut entry = delete_entry("e1", "a");
        entry.expected = Some(PlanExpected {
            size: 7,
            algorithm: "blake3".to_string(),
            hash: Some(hex(b"content")),
        });
        let outcome = apply_plan(
            &plan(vec![entry]),
            &ApplyOptions::default(),
            &mut context(&fs, &license, &mut journal),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);
        assert!(!fs.exists("a"));
    }

    #[test]
    fn changed_content_is_skipped_before_quarantine() {
        let fs = TestFs::with_files(&["a"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut entry = delete_entry("e1", "a");
        entry.expected = Some(PlanExpected {
            size: 7,
            algorithm: "blake3".to_string(),
            hash: Some("00".to_string()),
        });
        let outcome = apply_plan(
            &plan(vec![entry]),
            &ApplyOptions::default(),
            &mut context(&fs, &license, &mut journal),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Skipped);
        assert_eq!(
            outcome.report.entries[0].error.as_ref().unwrap().code,
            "E2010"
        );
        assert!(fs.exists("a"));
        assert!(journal.entries.is_empty());
    }

    #[test]
    fn changed_size_is_skipped_before_quarantine() {
        let fs = TestFs::with_files(&["a"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut entry = delete_entry("e1", "a");
        entry.expected = Some(PlanExpected {
            size: 8,
            algorithm: "blake3".to_string(),
            hash: Some(hex(b"content")),
        });
        let outcome = apply_plan(
            &plan(vec![entry]),
            &ApplyOptions::default(),
            &mut context(&fs, &license, &mut journal),
        )
        .unwrap();
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Skipped);
        assert_eq!(
            outcome.report.entries[0].error.as_ref().unwrap().code,
            "E2010"
        );
        assert!(fs.exists("a"));
    }

    #[test]
    fn non_delete_entries_are_rejected_before_apply() {
        let fs = TestFs::with_files(&["a"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let mut entry = delete_entry("e1", "a");
        entry.action = PlanAction::Move;
        let error = apply_plan(
            &plan(vec![entry]),
            &ApplyOptions::default(),
            &mut context(&fs, &license, &mut journal),
        )
        .unwrap_err();
        assert_eq!(error.code, "E1002");
        assert!(fs.exists("a"));
    }

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<ProgressEvent>>,
    }

    impl ProgressSink for RecordingSink {
        fn report(&self, event: ProgressEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    struct CancelAfterFirst {
        flag: Arc<AtomicBool>,
    }

    impl ProgressSink for CancelAfterFirst {
        fn report(&self, event: ProgressEvent) {
            if event.phase == ProgressPhase::Executing && event.done >= 1 {
                self.flag.store(true, Ordering::SeqCst);
            }
        }
    }

    struct FlagToken(Arc<AtomicBool>);

    impl CancelToken for FlagToken {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn apply_reports_entry_progress_and_finalizing() {
        let fs = TestFs::with_files(&["a", "b", "c"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let sink = RecordingSink::default();
        let mut ctx = context(&fs, &license, &mut journal);
        ctx.progress = Some(&sink);
        let outcome = apply_plan(
            &plan(vec![
                delete_entry("e1", "a"),
                delete_entry("e2", "b"),
                delete_entry("e3", "c"),
            ]),
            &ApplyOptions::default(),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Completed);
        let events = sink.events.lock().unwrap();
        let first = events.first().unwrap();
        assert_eq!(first.phase, ProgressPhase::Executing);
        assert_eq!((first.done, first.total), (0, 3));
        assert_eq!(events.last().unwrap().phase, ProgressPhase::Finalizing);
        let max_done = events
            .iter()
            .filter(|event| event.phase == ProgressPhase::Executing)
            .map(|event| event.done)
            .max();
        assert_eq!(max_done, Some(3));
    }

    #[test]
    fn cancel_rolls_back_applied_entries_and_marks_journal_reverted() {
        let fs = TestFs::with_files(&["a", "b", "c"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let flag = Arc::new(AtomicBool::new(false));
        let sink = CancelAfterFirst { flag: flag.clone() };
        let token = FlagToken(flag);
        let mut ctx = context(&fs, &license, &mut journal);
        ctx.progress = Some(&sink);
        ctx.cancel = Some(&token);
        let outcome = apply_plan(
            &plan(vec![
                delete_entry("e1", "a"),
                delete_entry("e2", "b"),
                delete_entry("e3", "c"),
            ]),
            &ApplyOptions::default(),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Cancelled);
        assert_eq!(outcome.report.undone_by.as_deref(), Some("task-1"));
        assert!(fs.exists("a"));
        assert!(fs.exists("b"));
        assert!(fs.exists("c"));
        assert_eq!(journal.entries.len(), 1);
        assert_eq!(journal.entries[0].status, JournalStatus::Reverted);
        assert_eq!(outcome.report.entries.len(), 3);
        assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);
        assert_eq!(outcome.report.entries[1].status, ApplyStatus::Skipped);
        assert_eq!(outcome.report.entries[2].status, ApplyStatus::Skipped);
    }

    #[test]
    fn cancel_keeps_applied_entries_when_the_caller_asks_the_user_first() {
        // 方案 §7.7：桌面壳传 keepApplied——取消只停止派发，已完成条目留在 Journal 里，
        // 等用户答复「是否一并撤销」；答复"撤销"时走同一条 undo_plan。
        let fs = TestFs::with_files(&["a", "b", "c"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let flag = Arc::new(AtomicBool::new(false));
        let sink = CancelAfterFirst { flag: flag.clone() };
        let token = FlagToken(flag);
        let mut ctx = context(&fs, &license, &mut journal);
        ctx.progress = Some(&sink);
        ctx.cancel = Some(&token);
        let outcome = apply_plan(
            &plan(vec![
                delete_entry("e1", "a"),
                delete_entry("e2", "b"),
                delete_entry("e3", "c"),
            ]),
            &ApplyOptions {
                cancel_policy: CancelPolicy::KeepApplied,
                ..ApplyOptions::default()
            },
            &mut ctx,
        )
        .unwrap();
        assert_eq!(outcome.state, ApplyState::Cancelled);
        assert_eq!(outcome.report.undone_by, None, "保留处置不得声称已撤销");
        assert!(!fs.exists("a"), "已完成的删除保留在隔离目录");
        assert!(fs.exists("b") && fs.exists("c"), "未派发条目原地不动");
        assert_eq!(
            journal.entries[0].status,
            JournalStatus::Applied,
            "保留的条目必须仍处于可撤销状态"
        );
        let undo = undo_plan(PLAN_ID, &fs, &license, &mut journal).unwrap();
        assert_eq!(undo.reverted, 1, "答复撤销后按 Journal 还原已完成的 1 条");
        assert!(fs.exists("a"));
        assert_eq!(journal.entries[0].status, JournalStatus::Reverted);
    }

    #[test]
    fn undo_reports_progress_and_cancels_between_entries() {
        let fs = TestFs::with_files(&["a", "b"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        apply_plan(
            &plan(vec![delete_entry("e1", "a"), delete_entry("e2", "b")]),
            &ApplyOptions::default(),
            &mut context(&fs, &license, &mut journal),
        )
        .unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        let sink = CancelAfterFirst { flag: flag.clone() };
        let token = FlagToken(flag);
        let outcome = undo_plan_with_ports(
            PLAN_ID,
            &fs,
            &license,
            &mut journal,
            UndoPorts {
                task_id: Some("task-2"),
                progress: Some(&sink),
                cancel: Some(&token),
                network_retry: None,
            },
        )
        .unwrap();
        assert_eq!(outcome.reverted, 1);
        assert_eq!(outcome.entries.len(), 1);
        assert_eq!(outcome.entries[0].status, ApplyStatus::Ok);
        assert_eq!(
            journal
                .entries
                .iter()
                .filter(|entry| entry.status == JournalStatus::Reverted)
                .count(),
            1
        );
        assert!(fs.exists("b"));
        assert!(!fs.exists("a"));
    }

    #[test]
    fn undo_dry_run_flags_conflicts_and_missing_without_touching_files() {
        // 已应用的 rename a→b（直接构造 Journal），文件态 = {b}：
        // ① 恢复点 a 空闲、源 b 在 → Ok；② 重建 a → Conflict；③ 源缺失 → Missing。
        let mut journal = TestJournal::default();
        journal
            .append(
                PLAN_ID,
                "e1",
                "2026-10-30T00:00:00Z",
                FsOp::Rename {
                    from: "a".to_string(),
                    to: "b".to_string(),
                },
                FsOp::Rename {
                    from: "b".to_string(),
                    to: "a".to_string(),
                },
            )
            .unwrap();

        let fs = TestFs::with_files(&["b"]);
        let preview = undo_dry_run(PLAN_ID, &mut journal, &fs).unwrap();
        assert_eq!(preview.entries.len(), 1);
        assert_eq!(preview.entries[0].status, UndoDryRunStatus::Ok);
        assert_eq!((preview.conflict_count, preview.missing_count), (0, 0));
        assert!(fs.exists("b"));
        assert!(!fs.exists("a"), "dry run must not touch files");

        let fs = TestFs::with_files(&["a", "b"]);
        let preview = undo_dry_run(PLAN_ID, &mut journal, &fs).unwrap();
        assert_eq!(preview.entries[0].status, UndoDryRunStatus::Conflict);
        assert_eq!(preview.entries[0].code.as_deref(), Some("E2007"));
        assert_eq!(preview.conflict_count, 1);

        let fs = TestFs::with_files(&[]);
        let preview = undo_dry_run(PLAN_ID, &mut journal, &fs).unwrap();
        assert_eq!(preview.entries[0].status, UndoDryRunStatus::Missing);
        assert_eq!(preview.entries[0].code.as_deref(), Some("E2001"));
        assert_eq!(preview.missing_count, 1);
    }

    #[test]
    fn undo_dry_run_checks_quarantine_reverses_and_empty_dirs() {
        // reverse=Quarantine（新建产物撤销）：产物在、隔离落点空闲 → Ok。
        let mut journal = TestJournal::default();
        journal
            .append(
                PLAN_ID,
                "e1",
                "2026-10-30T00:00:00Z",
                FsOp::WriteAtomic {
                    path: "/out/part-1.csv".to_string(),
                    data_ref: None,
                },
                FsOp::Quarantine {
                    from: "/out/part-1.csv".to_string(),
                    to: "/quarantine/part-1.csv".to_string(),
                },
            )
            .unwrap();
        let fs = TestFs::with_files(&["/out/part-1.csv"]);
        let preview = undo_dry_run(PLAN_ID, &mut journal, &fs).unwrap();
        assert_eq!(preview.entries[0].status, UndoDryRunStatus::Ok);

        // 隔离落点被占 → Conflict。
        let fs = TestFs::with_files(&["/out/part-1.csv", "/quarantine/part-1.csv"]);
        let preview = undo_dry_run(PLAN_ID, &mut journal, &fs).unwrap();
        assert_eq!(preview.entries[0].status, UndoDryRunStatus::Conflict);

        // RemoveDir 逆：目录非空 → Conflict（撤销只删空目录）。
        let mut journal2 = TestJournal::default();
        // F11 的 forward=CreateDir、reverse=RemoveDir（撤销删空目录）。
        journal2
            .append(
                PLAN_ID,
                "e2",
                "2026-10-30T00:00:00Z",
                FsOp::CreateDir {
                    path: "/data/created".to_string(),
                },
                FsOp::RemoveDir {
                    path: "/data/created".to_string(),
                },
            )
            .unwrap();
        let fs = TestFs::with_files(&["/data/created"]);
        let preview = undo_dry_run(PLAN_ID, &mut journal2, &fs).unwrap();
        assert_eq!(preview.entries[0].status, UndoDryRunStatus::Ok);
    }

    /// D1 补全：undo 预演的只读 stat 也走断线重试；网络错误不能被伪装成 missing。
    #[test]
    fn undo_dry_run_retries_network_stat_and_does_not_report_missing() {
        let mut journal = TestJournal::default();
        journal
            .append(
                PLAN_ID,
                "e1",
                "2026-10-30T00:00:00Z",
                FsOp::Rename {
                    from: "//server/share/a.txt".to_string(),
                    to: "/quarantine/1".to_string(),
                },
                // undo 执行的是 reverse：从隔离目录还原回 UNC 源。
                FsOp::Rename {
                    from: "/quarantine/1".to_string(),
                    to: "//server/share/a.txt".to_string(),
                },
            )
            .unwrap();
        let fs = TestFs::with_files(&["/quarantine/1"]);
        // 前 4 枪失败（首次＋3 次重试），暂停恢复后的第 5 枪成功。
        fs.set_stat_failure("/quarantine/1", "E2005", true, 4);
        let retry = RecordingRetry::default();
        let preview = undo_dry_run_with_retry(PLAN_ID, &mut journal, &fs, Some(&retry)).unwrap();
        assert_eq!(preview.entries[0].status, UndoDryRunStatus::Ok);
        assert_eq!(preview.missing_count, 0, "断线不能被记成源已缺失");
        assert_eq!(
            *retry.waits.lock().unwrap(),
            vec![
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8)
            ]
        );
        assert!(retry.pauses.load(Ordering::SeqCst));
        assert!(fs.exists("/quarantine/1"), "预演必须零副作用");
    }

    /// 无暂停交互面的宿主（CLI/预览命令）在重试耗尽后把 E2005 上抛，
    /// 而不是给出一个假的 `missing` 结论。
    #[test]
    fn undo_dry_run_surfaces_e2005_when_the_host_cannot_pause() {
        struct AbortRetry;
        impl crate::retry::NetworkRetry for AbortRetry {
            fn wait(&self, _delay: Duration) -> bool {
                true
            }
            fn pause(&self, _error: &KernelError) -> crate::retry::PauseAction {
                crate::retry::PauseAction::Abort
            }
        }

        let mut journal = TestJournal::default();
        journal
            .append(
                PLAN_ID,
                "e1",
                "2026-10-30T00:00:00Z",
                FsOp::Rename {
                    from: "//server/share/a.txt".to_string(),
                    to: "/quarantine/1".to_string(),
                },
                FsOp::Rename {
                    from: "/quarantine/1".to_string(),
                    to: "//server/share/a.txt".to_string(),
                },
            )
            .unwrap();
        let fs = TestFs::with_files(&["/quarantine/1"]);
        fs.set_stat_failure("/quarantine/1", "E2005", true, 4);
        let error =
            undo_dry_run_with_retry(PLAN_ID, &mut journal, &fs, Some(&AbortRetry)).unwrap_err();
        assert_eq!(error.code, "E2005");
    }
}
