//! File-backed JSONL Journal implementation (design 01 §8).

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use kernel::error::KernelError;
use kernel::journal::{FsOp, JournalEntry, JournalSink, JournalStatus, JournalStore};

/// 追加式 Journal 的读取合并：同一 `(plan_id, seq)` 只保留最后一条状态记录，
/// `Discarded` 墓碑行在合并后移除。旧日志（每 seq 只有一行）原样通过。
fn compact_entries(raw: Vec<JournalEntry>) -> Vec<JournalEntry> {
    let mut out: Vec<JournalEntry> = Vec::new();
    let mut positions: HashMap<(String, u64), usize> = HashMap::new();
    for entry in raw {
        let key = (entry.plan_id.clone(), entry.seq);
        if let Some(&index) = positions.get(&key) {
            out[index] = entry;
        } else {
            positions.insert(key, out.len());
            out.push(entry);
        }
    }
    out.retain(|entry| entry.status != JournalStatus::Discarded);
    out
}

pub struct FileJournal {
    path: PathBuf,
    entries: Vec<JournalEntry>,
}

impl FileJournal {
    pub fn open(path: PathBuf) -> Result<Self, KernelError> {
        let entries = if path.exists() {
            let contents = fs::read_to_string(&path).map_err(|error| {
                KernelError::new(
                    "E9001",
                    "journal file is not readable",
                    Some(format!("{}: {error}", path.display())),
                    false,
                )
            })?;
            let mut entries = Vec::new();
            for (index, line) in contents.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let entry = serde_json::from_str::<JournalEntry>(line).map_err(|error| {
                    KernelError::new(
                        "E9001",
                        "journal file is corrupt",
                        Some(format!("{}:{}: {error}", path.display(), index + 1)),
                        false,
                    )
                })?;
                entries.push(entry);
            }
            entries
        } else {
            Vec::new()
        };
        let (entries, changed) = reconcile_pending(compact_entries(entries));
        let journal = Self { path, entries };
        if changed {
            journal.flush()?;
        }
        Ok(journal)
    }

    pub fn entries(&self) -> &[JournalEntry] {
        &self.entries
    }

    /// 追加一行状态记录。相比每次重写整个文件，追加写把路径信息落盘的成本从
    /// O(总条目数) 降到 O(1)，并避开 Windows 上"删除+改名"的占用窗口。
    fn append_line(&self, entry: &JournalEntry) -> Result<(), KernelError> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|error| {
                    KernelError::new(
                        "E9001",
                        "journal directory is not writable",
                        Some(format!("{}: {error}", parent.display())),
                        false,
                    )
                })?;
            }
        }
        let line = serde_json::to_string(entry).map_err(|error| {
            KernelError::new(
                "E9001",
                "journal entry is not serializable",
                Some(error.to_string()),
                false,
            )
        })?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| {
                KernelError::new(
                    "E9001",
                    "journal file is not appendable",
                    Some(format!("{}: {error}", self.path.display())),
                    false,
                )
            })?;
        file.write_all(line.as_bytes())
            .and_then(|_| file.write_all(b"\n"))
            .map_err(|error| {
                KernelError::new(
                    "E9001",
                    "journal append failed",
                    Some(format!("{}: {error}", self.path.display())),
                    false,
                )
            })
    }

    pub fn remove_entries(&mut self, seqs: &[u64]) -> Result<(), KernelError> {
        self.entries.retain(|entry| !seqs.contains(&entry.seq));
        self.flush()
    }

    fn flush(&self) -> Result<(), KernelError> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|error| {
                    KernelError::new(
                        "E9001",
                        "journal directory is not writable",
                        Some(format!("{}: {error}", parent.display())),
                        false,
                    )
                })?;
            }
        }
        let mut contents = String::new();
        for entry in &self.entries {
            let line = serde_json::to_string(entry).map_err(|error| {
                KernelError::new(
                    "E9001",
                    "journal entry is not serializable",
                    Some(error.to_string()),
                    false,
                )
            })?;
            contents.push_str(&line);
            contents.push('\n');
        }
        let temp = self
            .path
            .with_extension(format!("tmp-{}", std::process::id()));
        fs::write(&temp, contents).map_err(|error| {
            KernelError::new(
                "E9001",
                "journal temp file is not writable",
                Some(format!("{}: {error}", temp.display())),
                false,
            )
        })?;
        // `fs::rename` 在 Windows 上以 MOVEFILE_REPLACE_EXISTING 替换目标；
        // 不再先删目标——删除成功后改名失败会丢失整个 Journal。
        fs::rename(&temp, &self.path).map_err(|error| {
            let _ = fs::remove_file(&temp);
            KernelError::new(
                "E9001",
                "journal file is not writable",
                Some(format!("{}: {error}", self.path.display())),
                false,
            )
        })
    }
}

/// D5：打开 Journal 时收敛待确认条目。
///
/// 两阶段写入的顺序是「先落 Pending → 移动 → 标记 Applied」。崩溃窗口只可能：
/// - 移动已完成、Applied 未落盘：隔离路径存在 → 补成 Applied（撤销可见）；
/// - 移动未发生：隔离路径不存在 → 丢弃 Pending（不留下假记录）。
///
/// 只处理隔离移动（`FsOp::Quarantine`）；其他 forward 的 Pending 视为未完成丢弃。
fn reconcile_pending(mut entries: Vec<JournalEntry>) -> (Vec<JournalEntry>, bool) {
    let before = entries.len();
    let mut changed = false;
    for entry in &mut entries {
        if entry.status != JournalStatus::Pending {
            continue;
        }
        if let FsOp::Quarantine { to, .. } = &entry.forward {
            if crate::fs::fs_path(std::path::Path::new(to)).exists() {
                entry.status = JournalStatus::Applied;
                changed = true;
            }
        }
    }
    entries.retain(|entry| entry.status != JournalStatus::Pending);
    changed |= entries.len() != before;
    (entries, changed)
}

impl JournalSink for FileJournal {
    fn append(
        &mut self,
        plan_id: &str,
        entry_id: &str,
        ts: &str,
        forward: FsOp,
        reverse: FsOp,
    ) -> Result<JournalEntry, KernelError> {
        let seq = self.entries.last().map(|entry| entry.seq).unwrap_or(0) + 1;
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
        if let Err(error) = self.append_line(&entry) {
            self.entries.pop();
            return Err(error);
        }
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
        let seq = self.entries.last().map(|entry| entry.seq).unwrap_or(0) + 1;
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
        if let Err(error) = self.append_line(&entry) {
            self.entries.pop();
            return Err(error);
        }
        Ok(entry)
    }

    fn append_pending_with_kind(
        &mut self,
        plan_id: &str,
        entry_id: &str,
        ts: &str,
        plan_kind: &str,
        forward: FsOp,
        reverse: FsOp,
    ) -> Result<JournalEntry, KernelError> {
        let seq = self.entries.last().map(|entry| entry.seq).unwrap_or(0) + 1;
        let entry = JournalEntry {
            seq,
            plan_id: plan_id.to_string(),
            entry_id: entry_id.to_string(),
            ts: ts.to_string(),
            plan_kind: Some(plan_kind.to_string()),
            forward,
            reverse,
            status: JournalStatus::Pending,
        };
        self.entries.push(entry.clone());
        if let Err(error) = self.append_line(&entry) {
            self.entries.pop();
            return Err(error);
        }
        Ok(entry)
    }

    fn mark_applied(&mut self, plan_id: &str, seq: u64) -> Result<(), KernelError> {
        let mut updated = None;
        for entry in &mut self.entries {
            if entry.plan_id == plan_id && entry.seq == seq {
                entry.status = JournalStatus::Applied;
                updated = Some(entry.clone());
            }
        }
        match updated {
            Some(entry) => self.append_line(&entry),
            None => Ok(()),
        }
    }

    fn discard_pending(&mut self, plan_id: &str, seq: u64) -> Result<(), KernelError> {
        let mut discarded = None;
        self.entries.retain(|entry| {
            if entry.plan_id == plan_id
                && entry.seq == seq
                && entry.status == JournalStatus::Pending
            {
                discarded = Some(entry.clone());
                false
            } else {
                true
            }
        });
        match discarded {
            Some(mut entry) => {
                entry.status = JournalStatus::Discarded;
                self.append_line(&entry)
            }
            None => Ok(()),
        }
    }
}

impl JournalStore for FileJournal {
    fn entries_for_plan(&self, plan_id: &str) -> Result<Vec<JournalEntry>, KernelError> {
        Ok(self
            .entries
            .iter()
            .filter(|entry| entry.plan_id == plan_id)
            .cloned()
            .collect())
    }

    fn mark_reverted(&mut self, plan_id: &str, seqs: &[u64]) -> Result<(), KernelError> {
        let mut reverted = Vec::new();
        for entry in &mut self.entries {
            if entry.plan_id == plan_id && seqs.contains(&entry.seq) {
                entry.status = JournalStatus::Reverted;
                reverted.push(entry.clone());
            }
        }
        for entry in &reverted {
            self.append_line(entry)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("batchdesk-journal-{nanos}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn rename_op(from: &str, to: &str) -> FsOp {
        FsOp::Rename {
            from: from.to_string(),
            to: to.to_string(),
        }
    }

    #[test]
    fn append_persists_and_mark_reverted_updates_status() {
        let dir = TempDir::new();
        let path = dir.0.join("journal.jsonl");
        let mut journal = FileJournal::open(path.clone()).unwrap();
        journal
            .append(
                "plan-1",
                "entry-1",
                "2026-10-29T00:00:00Z",
                rename_op("a", "b"),
                rename_op("b", "a"),
            )
            .unwrap();
        journal
            .append(
                "plan-1",
                "entry-2",
                "2026-10-29T00:00:01Z",
                rename_op("c", "d"),
                rename_op("d", "c"),
            )
            .unwrap();

        let reopened = FileJournal::open(path.clone()).unwrap();
        let entries = reopened.entries_for_plan("plan-1").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].seq, 1);
        assert_eq!(entries[1].seq, 2);
        assert_eq!(entries[0].status, JournalStatus::Applied);

        let mut journal = FileJournal::open(path.clone()).unwrap();
        journal.mark_reverted("plan-1", &[2]).unwrap();
        let reopened = FileJournal::open(path).unwrap();
        let entries = reopened.entries_for_plan("plan-1").unwrap();
        assert_eq!(entries[0].status, JournalStatus::Applied);
        assert_eq!(entries[1].status, JournalStatus::Reverted);
    }

    #[test]
    fn append_with_kind_persists_plan_kind() {
        let dir = TempDir::new();
        let path = dir.0.join("journal.jsonl");
        let mut journal = FileJournal::open(path.clone()).unwrap();
        journal
            .append_with_kind(
                "plan-2",
                "entry-1",
                "2026-10-29T00:00:00Z",
                "c05",
                rename_op("a", "b"),
                rename_op("b", "a"),
            )
            .unwrap();
        let reopened = FileJournal::open(path).unwrap();
        let entries = reopened.entries_for_plan("plan-2").unwrap();
        assert_eq!(entries[0].plan_kind.as_deref(), Some("c05"));
    }

    #[test]
    fn pending_quarantine_entry_recovers_applied_when_the_file_exists() {
        // D5：移动已完成、Applied 确认未落盘（崩溃窗口）——重开 Journal 必须补成
        // Applied，撤销才看得见隔离目录里的文件。
        let dir = TempDir::new();
        let path = dir.0.join("journal.jsonl");
        let source = dir.0.join("source.txt");
        let quarantine = dir.0.join("quarantine.txt");
        fs::write(&quarantine, b"moved").unwrap();
        let mut journal = FileJournal::open(path.clone()).unwrap();
        journal
            .append_pending_with_kind(
                "plan-3",
                "entry-1",
                "2026-11-08T00:00:00Z",
                "c09",
                FsOp::Quarantine {
                    from: source.to_string_lossy().into_owned(),
                    to: quarantine.to_string_lossy().into_owned(),
                },
                rename_op(&quarantine.to_string_lossy(), &source.to_string_lossy()),
            )
            .unwrap();
        drop(journal);

        let reopened = FileJournal::open(path).unwrap();
        let entries = reopened.entries_for_plan("plan-3").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, JournalStatus::Applied);
    }

    #[test]
    fn pending_entry_is_dropped_when_the_move_never_happened() {
        // D5：意图落盘后、移动前崩溃——隔离路径不存在，重开 Journal 应丢弃该条，
        // 不能在撤销列表里留下指向不存在文件的假记录。
        let dir = TempDir::new();
        let path = dir.0.join("journal.jsonl");
        let source = dir.0.join("source.txt");
        let quarantine = dir.0.join("missing.txt");
        let mut journal = FileJournal::open(path.clone()).unwrap();
        journal
            .append_pending_with_kind(
                "plan-4",
                "entry-1",
                "2026-11-08T00:00:00Z",
                "c09",
                FsOp::Quarantine {
                    from: source.to_string_lossy().into_owned(),
                    to: quarantine.to_string_lossy().into_owned(),
                },
                rename_op(&quarantine.to_string_lossy(), &source.to_string_lossy()),
            )
            .unwrap();
        drop(journal);

        let reopened = FileJournal::open(path).unwrap();
        assert!(reopened.entries_for_plan("plan-4").unwrap().is_empty());
    }

    #[test]
    fn pending_then_applied_appends_and_merges_on_reopen() {
        // 实机缺陷 2026-11-08：mark_applied 每次重写整个 Journal，在 2798 条后
        // 因 Windows 文件替换失败中止任务（E9001）。追加式写入应只增加一行，
        // 并在重开时合并为最新状态。
        let dir = TempDir::new();
        let path = dir.0.join("journal.jsonl");
        let mut journal = FileJournal::open(path.clone()).unwrap();
        let pending = journal
            .append_pending_with_kind(
                "plan-5",
                "entry-1",
                "2026-11-08T00:00:00Z",
                "c09",
                FsOp::Quarantine {
                    from: "a".to_string(),
                    to: "q".to_string(),
                },
                rename_op("q", "a"),
            )
            .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 1);
        journal.mark_applied("plan-5", pending.seq).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 2);
        drop(journal);

        let reopened = FileJournal::open(path).unwrap();
        let entries = reopened.entries_for_plan("plan-5").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, JournalStatus::Applied);
    }

    #[test]
    fn discarded_pending_is_tombstoned_and_removed_on_reopen() {
        let dir = TempDir::new();
        let path = dir.0.join("journal.jsonl");
        let mut journal = FileJournal::open(path.clone()).unwrap();
        let pending = journal
            .append_pending_with_kind(
                "plan-6",
                "entry-1",
                "2026-11-08T00:00:00Z",
                "c09",
                FsOp::Quarantine {
                    from: "a".to_string(),
                    to: "q".to_string(),
                },
                rename_op("q", "a"),
            )
            .unwrap();
        journal.discard_pending("plan-6", pending.seq).unwrap();
        drop(journal);

        let reopened = FileJournal::open(path).unwrap();
        assert!(reopened.entries_for_plan("plan-6").unwrap().is_empty());
    }
}
