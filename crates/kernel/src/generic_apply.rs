//! 通用文件操作执行器（C05/C06/C18 复用；方案 §5.13、设计 01 §5）。
//!
//! 只执行计划，不做规则生成：新增功能只需产出 `PlanDocument`（参数 → 条目），
//! 冲突二次裁决、Journal、进度、取消回滚、授权复核与指纹复核都在这里统一完成。
//!
//! 本轮覆盖 `rename`／`move`／`delete`；`copy` 与压缩/转码类需专用执行器或
//! Journal 的删除型逆向（契约评审后再接），遇到即 E1002 明确拒绝，不静默降级。

use crate::apply::{
    ApplyEntryReport, ApplyOptions, ApplyOutcome, ApplyReport, ApplyState, ApplyStatus, OnConflict,
};
use crate::conflict::sequence_target;
use crate::error::KernelError;
use crate::fs::{CaseSense, ConflictPolicy, EntryKind, FsAdapter, PathOutcomeKind};
use crate::journal::{FsOp, JournalStore};
use crate::license::{Capability, LicenseService};
use crate::path::{protected_by, try_normalize, ProtectedDir};
use crate::plan::{PlanAction, PlanDocument, PlanResolution};
use crate::progress::{CancelToken, ProgressPhase, ProgressSink};

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
            PlanAction::Rename | PlanAction::Move | PlanAction::Delete => {}
            PlanAction::Copy => {
                return Err(KernelError::invalid_params(
                    "copy needs a delete-type journal reverse; not supported by this executor yet",
                ))
            }
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
            PlanAction::Rename | PlanAction::Move if entry.target.is_none() => {
                return Err(KernelError::invalid_params(
                    "rename/move entries require a target",
                ))
            }
            _ => {}
        }
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

        let normalized = match try_normalize(&entry.source) {
            Ok(normalized) => normalized,
            Err(error) => {
                failed += 1;
                reports.push(failed_entry(entry.entry_id.clone(), error));
                continue;
            }
        };
        if let Some(error) = protected_hit(context, entry, &normalized) {
            reports.push(skipped_entry(entry.entry_id.clone(), error));
            continue;
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

        let stat = match context.fs.stat(&entry.source) {
            Ok(stat) => stat,
            Err(error) => {
                failed += 1;
                reports.push(failed_entry(entry.entry_id.clone(), error));
                continue;
            }
        };
        if entry.action != PlanAction::Delete && stat.kind != EntryKind::File {
            failed += 1;
            reports.push(failed_entry(
                entry.entry_id.clone(),
                KernelError::invalid_params("rename/move expects a file source"),
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

        if let Err(error) = context.license.consume(&plan.kind, 1) {
            failed += 1;
            reports.push(failed_entry(entry.entry_id.clone(), error));
            continue;
        }

        let outcome = match perform(context, entry, &target, overwrite) {
            Ok(Ok(outcome)) => outcome,
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

        if outcome.outcome == PathOutcomeKind::Skipped {
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

        let forward = forward_op(entry, &outcome.final_path);
        let reverse = match forward.inverse() {
            Some(reverse) => reverse,
            None => {
                context.license.release(&plan.kind, 1);
                return Err(KernelError::internal(
                    "generic apply journal reverse is not expressible",
                    format!("{forward:?}"),
                ));
            }
        };
        match context.journal.append(
            &plan.plan_id,
            &entry.entry_id,
            &context.now_rfc3339,
            forward,
            reverse,
        ) {
            Ok(journal_entry) => {
                applied.push((index, journal_entry.seq, journal_entry.reverse));
            }
            Err(error) => {
                context.license.release(&plan.kind, 1);
                // 落盘成功但 Journal 失败：尽力回退本次动作后再报错。
                if rollback_single(context, &outcome.final_path, entry) {
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
            status: if overwrite || outcome.outcome == PathOutcomeKind::Replaced {
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
            match &reverse {
                FsOp::Rename { from, to } => {
                    match context.fs.rename(from, to, ConflictPolicy::Skip) {
                        Ok(outcome) if outcome.outcome != PathOutcomeKind::Skipped => {
                            let _ = context.journal.mark_reverted(&plan.plan_id, &[*seq]);
                        }
                        Ok(_) => {
                            mark_entry_error(
                                &mut reports,
                                *index,
                                KernelError::new(
                                    "E2007",
                                    "cancel rollback target already exists",
                                    None,
                                    false,
                                ),
                            );
                        }
                        Err(error) => {
                            mark_entry_error(&mut reports, *index, error);
                        }
                    }
                }
                _ => {
                    mark_entry_error(
                        &mut reports,
                        *index,
                        KernelError::internal(
                            "cancel rollback reverse is not supported",
                            format!("{reverse:?}"),
                        ),
                    );
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

fn perform(
    context: &GenericApplyContext<'_>,
    entry: &crate::plan::PlanEntry,
    target: &str,
    overwrite: bool,
) -> Result<Result<crate::fs::PathOutcome, KernelError>, KernelError> {
    let policy = if overwrite {
        ConflictPolicy::Overwrite
    } else {
        ConflictPolicy::Skip
    };
    match entry.action {
        PlanAction::Rename => Ok(context.fs.rename(&entry.source, target, policy)),
        PlanAction::Move => Ok(context.fs.move_path(&entry.source, target, policy)),
        PlanAction::Delete => {
            Ok(context
                .fs
                .remove_to_quarantine(&entry.source)
                .map(|path| crate::fs::PathOutcome {
                    outcome: PathOutcomeKind::Applied,
                    final_path: path,
                }))
        }
        _ => Err(KernelError::invalid_params("unsupported action")),
    }
}

fn forward_op(entry: &crate::plan::PlanEntry, final_path: &str) -> FsOp {
    match entry.action {
        PlanAction::Delete => FsOp::Quarantine {
            from: entry.source.clone(),
            to: final_path.to_string(),
        },
        _ => FsOp::Rename {
            from: entry.source.clone(),
            to: final_path.to_string(),
        },
    }
}

fn rollback_single(
    context: &GenericApplyContext<'_>,
    final_path: &str,
    entry: &crate::plan::PlanEntry,
) -> bool {
    context
        .fs
        .rename(final_path, &entry.source, ConflictPolicy::Skip)
        .map(|outcome| outcome.outcome != PathOutcomeKind::Skipped)
        .unwrap_or(false)
}

fn fingerprint_mismatch(
    context: &GenericApplyContext<'_>,
    entry: &crate::plan::PlanEntry,
    size: u64,
) -> Option<KernelError> {
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

fn protected_hit(
    context: &GenericApplyContext<'_>,
    entry: &crate::plan::PlanEntry,
    normalized: &str,
) -> Option<KernelError> {
    if context.protected_dirs.is_empty() {
        return None;
    }
    let canonical = context.fs.canonicalize(&entry.source).ok();
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
    use crate::fs::{DirEntryInfo, EntryAttrs, PathOutcome, ScanFilter, StatInfo};
    use crate::journal::{JournalEntry, JournalSink, JournalStatus};
    use crate::plan::{PlanEntry, PlanExpected, PlanStats};
    use crate::progress::{CancelToken, ProgressEvent, ProgressSink};
    use std::collections::{BTreeMap, HashMap};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    const PLAN_ID: &str = "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8";

    #[derive(Default)]
    struct TestFs {
        files: Mutex<HashMap<String, Vec<u8>>>,
        next: Mutex<u64>,
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
    }

    impl FsAdapter for TestFs {
        fn read_bytes(&self, path: &str) -> Result<Vec<u8>, KernelError> {
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
            _policy: ConflictPolicy,
        ) -> Result<PathOutcome, KernelError> {
            let files = self.files.lock().unwrap();
            let data = files
                .get(from)
                .cloned()
                .ok_or_else(|| KernelError::new("E2001", "not found", None, false))?;
            drop(files);
            self.files.lock().unwrap().insert(to.to_string(), data);
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
        license: &'a crate::license::AlwaysAllowLicense,
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
    fn copy_is_rejected_before_touching_files() {
        let fs = TestFs::with_files(&["/data/a.txt"]);
        let license = crate::license::AlwaysAllowLicense;
        let mut journal = TestJournal::default();
        let error = apply_generic_plan(
            &plan(vec![action_entry(
                "e1",
                "/data/a.txt",
                Some("/data/copy.txt"),
                PlanAction::Copy,
            )]),
            &ApplyOptions::default(),
            &mut context(&fs, &mut journal, &license),
        )
        .unwrap_err();
        assert_eq!(error.code, "E1002");
        assert!(fs.exists("/data/a.txt"));
        assert!(!fs.exists("/data/copy.txt"));
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
