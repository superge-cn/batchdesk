//! File-backed JSONL Journal implementation (design 01 §8).

use std::fs;
use std::path::PathBuf;

use kernel::error::KernelError;
use kernel::journal::{FsOp, JournalEntry, JournalSink, JournalStatus, JournalStore};

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
        Ok(Self { path, entries })
    }

    pub fn entries(&self) -> &[JournalEntry] {
        &self.entries
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
        #[cfg(windows)]
        if self.path.exists() {
            fs::remove_file(&self.path).map_err(|error| {
                KernelError::new(
                    "E9001",
                    "journal file is not replaceable",
                    Some(format!("{}: {error}", self.path.display())),
                    false,
                )
            })?;
        }
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
            forward,
            reverse,
            status: JournalStatus::Applied,
        };
        self.entries.push(entry.clone());
        if let Err(error) = self.flush() {
            self.entries.pop();
            return Err(error);
        }
        Ok(entry)
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
        for entry in &mut self.entries {
            if entry.plan_id == plan_id && seqs.contains(&entry.seq) {
                entry.status = JournalStatus::Reverted;
            }
        }
        self.flush()
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
}
