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
        /// 备份文件路径（隔离目录内，经 FsAdapter 取得；设计 01 §8）。
        /// 覆盖已有文件时指向原内容备份；新建产物无备份，`dataRef` 省略。
        #[serde(rename = "dataRef")]
        #[serde(skip_serializing_if = "Option::is_none")]
        data_ref: Option<String>,
    },
    Quarantine {
        from: String,
        to: String,
    },
    /// 新建单级目录（F11）。
    CreateDir {
        path: String,
    },
    /// 删除空目录（CreateDir 的逆；非空目录必须失败）。
    RemoveDir {
        path: String,
    },
}

impl FsOp {
    /// Canonical reverse for operations whose inverse is expressible with the
    /// frozen FsOp set.
    ///
    /// - `Rename` 的逆是逆改名；`Quarantine` 的逆是改名还原；
    /// - `WriteAtomic` 携带备份（`dataRef`）时，逆是"用备份内容原子写回原路径"
    ///   （方案 §7.8、设计 01 §8 的备份语义）；新建产物（无备份）的逆是
    ///   "产物进隔离目录"，由执行器按 §8 规则在 apply 期显式给出 reverse，
    ///   不在此凭空推导（隔离目录路径只有适配层知道）；
    /// - `Copy` 的逆依赖目标是否新建/覆盖，同样由执行器显式给出。
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
            Self::WriteAtomic {
                path,
                data_ref: Some(backup),
            } => Some(Self::WriteAtomic {
                path: path.clone(),
                data_ref: Some(backup.clone()),
            }),
            Self::CreateDir { path } => Some(Self::RemoveDir { path: path.clone() }),
            Self::RemoveDir { path } => Some(Self::CreateDir { path: path.clone() }),
            Self::Copy { .. } | Self::WriteAtomic { data_ref: None, .. } => None,
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
    fn write_atomic_inverse_restores_from_backup_when_present() {
        // 覆盖写：reverse = 用备份内容原子写回原路径（设计 01 §8 dataRef 语义）。
        let forward = FsOp::WriteAtomic {
            path: "/data/a.txt".to_string(),
            data_ref: Some("/quarantine/a.backup".to_string()),
        };
        assert_eq!(forward.inverse(), Some(forward.clone()));
        // 新建产物（无备份）：逆不可凭空推导，执行器按 §8 显式给 Quarantine。
        let created = FsOp::WriteAtomic {
            path: "/out/part-1.csv".to_string(),
            data_ref: None,
        };
        assert_eq!(created.inverse(), None);
    }

    #[test]
    fn write_atomic_data_ref_is_omitted_when_absent() {
        // 契约（设计 01 §8）：无备份需求时 dataRef 省略。
        let json = serde_json::to_string(&FsOp::WriteAtomic {
            path: "/out/x".to_string(),
            data_ref: None,
        })
        .unwrap();
        assert_eq!(json, r#"{"op":"write_atomic","path":"/out/x"}"#);
        let with_ref = serde_json::to_string(&FsOp::WriteAtomic {
            path: "/out/x".to_string(),
            data_ref: Some("/q/x.bak".to_string()),
        })
        .unwrap();
        assert_eq!(
            with_ref,
            r#"{"op":"write_atomic","path":"/out/x","dataRef":"/q/x.bak"}"#
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
