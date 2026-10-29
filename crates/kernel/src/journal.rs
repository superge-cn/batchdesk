//! Journal record contract (design 01 §8).
//!
//! The shell owns persistence; kernel writes entries through `JournalSink`.
//! Undo reads them through `JournalStore` and replays `reverse` in seq order.

use serde::{Deserialize, Serialize};

use crate::error::KernelError;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FsOp {
    Rename {
        from: String,
        to: String,
    },
    Copy {
        from: String,
        to: String,
    },
    WriteAtomic {
        path: String,
        #[serde(rename = "dataRef")]
        data_ref: String,
    },
    Quarantine {
        from: String,
        to: String,
    },
}

impl FsOp {
    /// Canonical reverse for operations whose inverse is expressible with the
    /// frozen FsOp set. Copy/WriteAtomic reverses need a quarantine delete and
    /// are intentionally not guessed here.
    pub fn inverse(&self) -> Option<Self> {
        match self {
            Self::Rename { from, to } => Some(Self::Rename {
                from: to.clone(),
                to: from.clone(),
            }),
            Self::Quarantine { from, to } => Some(Self::Rename {
                from: to.clone(),
                to: from.clone(),
            }),
            Self::Copy { .. } | Self::WriteAtomic { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JournalStatus {
    Applied,
    Reverted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntry {
    pub seq: u64,
    pub plan_id: String,
    pub entry_id: String,
    pub ts: String,
    /// 计划类别（如 "c05"）；撤销时据此向 LicenseService 退还配额。
    /// 旧日志缺省时按 c09 处理（设计 01 §8，追加式可选字段）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_kind: Option<String>,
    pub forward: FsOp,
    pub reverse: FsOp,
    pub status: JournalStatus,
}

pub trait JournalSink {
    fn append(
        &mut self,
        plan_id: &str,
        entry_id: &str,
        ts: &str,
        forward: FsOp,
        reverse: FsOp,
    ) -> Result<JournalEntry, KernelError>;

    /// 记录 plan kind 的追加式写法；默认实现忽略类别以保持旧实现兼容。
    fn append_with_kind(
        &mut self,
        plan_id: &str,
        entry_id: &str,
        ts: &str,
        plan_kind: &str,
        forward: FsOp,
        reverse: FsOp,
    ) -> Result<JournalEntry, KernelError> {
        let _ = plan_kind;
        self.append(plan_id, entry_id, ts, forward, reverse)
    }
}

pub trait JournalStore: JournalSink {
    fn entries_for_plan(&self, plan_id: &str) -> Result<Vec<JournalEntry>, KernelError>;
    fn mark_reverted(&mut self, plan_id: &str, seqs: &[u64]) -> Result<(), KernelError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarantine_inverse_is_a_rename_back() {
        let forward = FsOp::Quarantine {
            from: "/data/a".to_string(),
            to: "/quarantine/a".to_string(),
        };
        assert_eq!(
            forward.inverse(),
            Some(FsOp::Rename {
                from: "/quarantine/a".to_string(),
                to: "/data/a".to_string(),
            })
        );
        assert_eq!(
            FsOp::Copy {
                from: "a".to_string(),
                to: "b".to_string(),
            }
            .inverse(),
            None
        );
    }

    #[test]
    fn legacy_journal_lines_without_plan_kind_parse_as_none() {
        let legacy = r#"{"seq":1,"planId":"p1","entryId":"e1","ts":"2026-10-29T00:00:00Z","forward":{"op":"rename","from":"a","to":"b"},"reverse":{"op":"rename","from":"b","to":"a"},"status":"applied"}"#;
        let entry: JournalEntry = serde_json::from_str(legacy).unwrap();
        assert_eq!(entry.plan_kind, None);
    }

    #[test]
    fn plan_kind_round_trips_when_present() {
        let entry = JournalEntry {
            seq: 1,
            plan_id: "p1".to_string(),
            entry_id: "e1".to_string(),
            ts: "2026-10-29T00:00:00Z".to_string(),
            plan_kind: Some("c05".to_string()),
            forward: FsOp::Rename {
                from: "a".to_string(),
                to: "b".to_string(),
            },
            reverse: FsOp::Rename {
                from: "b".to_string(),
                to: "a".to_string(),
            },
            status: JournalStatus::Applied,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let decoded: JournalEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, entry);
    }
}
