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
}
