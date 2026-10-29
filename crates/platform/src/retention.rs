//! Journal/quarantine retention cleanup (baseline §7.8 / 11.6-14).
//!
//! The default 7 days and 1–30 range come from the baseline; this module does
//! not define new product thresholds.

use std::path::Path;

use kernel::error::KernelError;
use kernel::fs::FsAdapter;
use kernel::journal::{FsOp, JournalStatus};
use kernel::path::{is_under, try_normalize};
use serde::{Deserialize, Serialize};

use crate::fs::{rfc3339_days_ago, CliFsAdapter};
use crate::journal::FileJournal;

pub const DEFAULT_RETENTION_DAYS: u64 = 7;
pub const MIN_RETENTION_DAYS: u64 = 1;
pub const MAX_RETENTION_DAYS: u64 = 30;

pub fn validate_retention_days(days: u64) -> Result<(), KernelError> {
    if (MIN_RETENTION_DAYS..=MAX_RETENTION_DAYS).contains(&days) {
        Ok(())
    } else {
        Err(KernelError::invalid_params(format!(
            "retention days must be {MIN_RETENTION_DAYS}..={MAX_RETENTION_DAYS}"
        )))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupFailure {
    pub entry_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupOutcome {
    pub cutoff: String,
    pub retention_days: u64,
    pub dry_run: bool,
    pub expired_entries: usize,
    pub expired_quarantine_files: usize,
    pub removed_entries: usize,
    pub removed_files: usize,
    pub failed: Vec<CleanupFailure>,
}

pub fn cleanup(
    journal: &mut FileJournal,
    quarantine_dir: &Path,
    retention_days: u64,
    apply: bool,
) -> Result<CleanupOutcome, KernelError> {
    validate_retention_days(retention_days)?;
    let quarantine_dir = crate::protected::make_absolute(quarantine_dir);
    let quarantine_text = quarantine_dir.to_string_lossy();
    let quarantine_norm = try_normalize(&quarantine_text)?;
    let sense = CliFsAdapter::new(Some(quarantine_dir)).case_sensitivity();
    let cutoff = rfc3339_days_ago(retention_days);
    let expired: Vec<_> = journal
        .entries()
        .iter()
        .filter(|entry| entry.ts <= cutoff)
        .cloned()
        .collect();

    let mut failed = Vec::new();
    let mut removable = Vec::new();
    let mut expired_quarantine_files = 0usize;
    let mut removed_files = 0usize;
    for entry in &expired {
        let mut can_remove = true;
        if entry.status == JournalStatus::Applied {
            if let FsOp::Quarantine { to, .. } = &entry.forward {
                let target = Path::new(to);
                if target.exists() {
                    if !target.is_file() {
                        failed.push(CleanupFailure {
                            entry_id: entry.entry_id.clone(),
                            path: Some(to.clone()),
                            message: "quarantine path is not a file".to_string(),
                        });
                        can_remove = false;
                    } else {
                        match try_normalize(to) {
                            Ok(target_norm) => {
                                if !is_under(&target_norm, &quarantine_norm, sense) {
                                    failed.push(CleanupFailure {
                                        entry_id: entry.entry_id.clone(),
                                        path: Some(to.clone()),
                                        message: "quarantine path is outside --quarantine-dir"
                                            .to_string(),
                                    });
                                    can_remove = false;
                                } else {
                                    expired_quarantine_files += 1;
                                    if apply {
                                        match std::fs::remove_file(target) {
                                            Ok(()) => removed_files += 1,
                                            Err(error) => {
                                                failed.push(CleanupFailure {
                                                    entry_id: entry.entry_id.clone(),
                                                    path: Some(to.clone()),
                                                    message: error.to_string(),
                                                });
                                                can_remove = false;
                                            }
                                        }
                                    }
                                }
                            }
                            Err(error) => {
                                failed.push(CleanupFailure {
                                    entry_id: entry.entry_id.clone(),
                                    path: Some(to.clone()),
                                    message: error.message,
                                });
                                can_remove = false;
                            }
                        }
                    }
                }
            }
        }
        if can_remove {
            removable.push(entry.seq);
        }
    }

    let removed_entries = if apply {
        if !removable.is_empty() {
            journal.remove_entries(&removable)?;
        }
        removable.len()
    } else {
        0
    };

    Ok(CleanupOutcome {
        cutoff,
        retention_days,
        dry_run: !apply,
        expired_entries: expired.len(),
        expired_quarantine_files,
        removed_entries,
        removed_files,
        failed,
    })
}
