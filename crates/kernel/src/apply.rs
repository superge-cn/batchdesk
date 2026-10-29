//! Apply and undo orchestration for C09.
//!
//! The kernel only drives ports: `FsAdapter` performs path operations,
//! `LicenseService` is checked per entry, and `JournalSink` records the
//! forward/reverse operation before the next entry. No clock, network or
//! environment access happens here; the caller supplies task/plan IDs and the
//! RFC 3339 timestamp.

use serde::{Deserialize, Serialize};

use crate::error::KernelError;
use crate::fs::{CaseSense, ConflictPolicy, EntryKind, FsAdapter, PathOutcomeKind};
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOptions {
    #[serde(default)]
    pub on_conflict: OnConflict,
    #[serde(default)]
    pub overwrite_confirmed: bool,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            on_conflict: OnConflict::Skip,
            overwrite_confirmed: false,
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
    pub progress: Option<&'a dyn ProgressSink>,
    pub cancel: Option<&'a dyn CancelToken>,
}

pub fn apply_plan(
    plan: &PlanDocument,
    _options: &ApplyOptions,
    context: &mut ApplyContext<'_>,
) -> Result<ApplyOutcome, KernelError> {
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
            });
            continue;
        }
        if entry.resolution == Some(PlanResolution::Skip) {
            entries.push(ApplyEntryReport {
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

        let stat = match context.fs.stat(&entry.source) {
            Ok(stat) => stat,
            Err(error) => {
                failed += 1;
                entries.push(failed_entry(&entry.entry_id, error));
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
                });
                continue;
            }
            if let Some(expected_hash) = &expected.hash {
                match context.fs.content_hash(&entry.source, &expected.algorithm) {
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
                        });
                        continue;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        failed += 1;
                        entries.push(failed_entry(&entry.entry_id, error));
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

        let quarantine = match context.fs.remove_to_quarantine(&entry.source) {
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
        let journal_entry = match context.journal.append_with_kind(
            &plan.plan_id,
            &entry.entry_id,
            &context.now_rfc3339,
            &plan.kind,
            forward,
            reverse.clone(),
        ) {
            Ok(journal_entry) => journal_entry,
            Err(error) => {
                context.license.release(C09, 1);
                match context
                    .fs
                    .rename(&quarantine, &entry.source, ConflictPolicy::Skip)
                {
                    Ok(outcome) if outcome.outcome != PathOutcomeKind::Skipped => {
                        failed += 1;
                        entries.push(failed_entry(&entry.entry_id, error));
                    }
                    Ok(_) => {
                        return Err(KernelError::internal(
                            "journal write failed and quarantine could not be restored",
                            format!("{}: {}", error.code, error.message),
                        ));
                    }
                    Err(restore_error) => {
                        return Err(KernelError::internal(
                            "journal write failed and quarantine restore failed",
                            format!("{}; restore: {}", error.message, restore_error.message),
                        ));
                    }
                }
                continue;
            }
        };
        applied.push(AppliedOp {
            entry_index: index,
            seq: journal_entry.seq,
            reverse,
        });

        ok += 1;
        entries.push(ApplyEntryReport {
            entry_id: entry.entry_id.clone(),
            status: ApplyStatus::Ok,
            error: None,
        });
    }

    if cancelled {
        let rollback_total = applied.len() as u64;
        report_progress(
            context,
            ProgressPhase::Journaling,
            0,
            rollback_total,
            Some("rolling back applied entries"),
        );
        let mut rollback_failed = false;
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
        for entry in &plan.entries[stopped_at..] {
            entries.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Skipped,
                error: None,
            });
        }
        report_progress(context, ProgressPhase::Finalizing, 1, 1, None);
        return Ok(ApplyOutcome {
            state: ApplyState::Cancelled,
            report: ApplyReport {
                plan_id: plan.plan_id.clone(),
                task_id: context.task_id.clone(),
                entries,
                undone_by: if rollback_failed {
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
}

pub fn undo_plan(
    plan_id: &str,
    fs: &dyn FsAdapter,
    license: &dyn LicenseService,
    journal: &mut dyn JournalStore,
) -> Result<UndoOutcome, KernelError> {
    undo_plan_with_ports(plan_id, fs, license, journal, UndoPorts::default())
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
    journal_entries.retain(|entry| entry.status != JournalStatus::Reverted);
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
        let result = match &entry.reverse {
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
            _ => Err(KernelError::invalid_params(
                "undo supports only quarantine rename reverses in this slice",
            )),
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
                });
            }
            Err(error) => {
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Failed,
                    error: Some(error.clone()),
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

fn report_undo_progress(ports: &UndoPorts<'_>, phase: ProgressPhase, done: u64, total: u64) {
    if let Some(sink) = ports.progress {
        sink.report(ProgressEvent {
            task_id: ports.task_id.unwrap_or_default().to_string(),
            phase,
            done,
            total,
            message: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::{CaseSense, ConflictPolicy, DirEntryInfo, EntryAttrs, ScanFilter, StatInfo};
    use crate::journal::{FsOp, JournalEntry, JournalSink, JournalStatus, JournalStore};
    use crate::license::Capability;
    use crate::plan::{PlanEntry, PlanExpected, PlanStats, PlanWarning};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    const PLAN_ID: &str = "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8";

    struct TestFs {
        files: Mutex<BTreeMap<String, Vec<u8>>>,
        next: Mutex<u64>,
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
            }
        }

        fn exists(&self, path: &str) -> bool {
            self.files.lock().unwrap().contains_key(path)
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
        ) -> Result<crate::fs::PathOutcome, KernelError> {
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

        fn content_hash(&self, path: &str, algorithm: &str) -> Result<Option<String>, KernelError> {
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
            progress: None,
            cancel: None,
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
}
