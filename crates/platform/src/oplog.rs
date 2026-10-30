//! C17a 最小操作日志（审计轨；设计 10 §1/§2，ADR-0013）。
//!
//! 载体为桌面 SQLite（契约既定），schema 与迁移策略见 ADR-0013：
//! 单表 `operation_log`、`PRAGMA user_version` 只增不改迁移、损坏留副本重建
//! （E5009）、高版本只读拒绝（E1007）。本模块只存取记录，不做审计判定；
//! Journal（怎么撤）与操作日志（做了什么）并存，不得互相替代。
//!
//! 审计轨是尽力而为：调用方在写失败时降级诊断输出并继续执行，不阻断批处理。

use std::path::{Path, PathBuf};

use kernel::error::KernelError;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::fs::now_rfc3339;

/// 当前 schema 版本（`PRAGMA user_version`）。只增不改：新版本只能追加列/表。
const SCHEMA_VERSION: i64 = 1;

/// 操作日志记录（设计 10 §2 的工程字段投影；serde 命名对齐线协议）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationLogRecord {
    pub id: String,
    /// RFC 3339 UTC（经 Clock 语义的系统时钟；kernel 不取时钟）。
    pub ts: String,
    pub task_id: String,
    pub plan_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
    pub op_kind: String,
    /// `start` / `entry` / `retry` / `rollback` / `finish`（设计 10 §2）。
    pub phase: String,
    /// `ok` / `skipped` / `failed` / `overwritten` / `reverted`（设计 10 §2）。
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// 方案 §10.2"所在目录"：恒写，撤销/反查依赖它与文件名三列拼回。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir_name: Option<String>,
    /// 方案 §10.2"旧名"：恒写。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_name: Option<String>,
    /// 方案 §10.2"新名"：恒写。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_name: Option<String>,
    /// 完整路径受 `logging.logFullPaths` 控制（03 §4.1），默认 NULL（§9.6/§9.7）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_path: Option<String>,
    /// 方案 §10.2"是否整体完成"。
    pub overall_completed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

pub struct SqliteOperationLog {
    path: PathBuf,
    connection: Connection,
}

impl SqliteOperationLog {
    /// 打开（必要时创建）操作日志库并执行迁移链。
    ///
    /// 返回值的第二项是打开期告警（当前仅 E5009：文件损坏 → 留 `.corrupt.<ts>`
    /// 副本、重建空库，设计 10 §4"不得静默清空"，对齐 `config_store` 语义）；
    /// 调用方应记录告警但不得因此阻断批处理。高版本库 → E1007。
    pub fn open(path: PathBuf) -> Result<(Self, Option<KernelError>), KernelError> {
        match Self::open_inner(path.clone()) {
            Ok(log) => Ok((log, None)),
            Err(error) if error.code == "E5009" => {
                let backup = PathBuf::from(format!(
                    "{}.corrupt.{}",
                    path.display(),
                    now_rfc3339().replace(':', "-")
                ));
                let _ = std::fs::rename(&path, &backup);
                let log = Self::open_inner(path.clone())?;
                Ok((
                    log,
                    Some(KernelError::new(
                        "E5009",
                        "operation log is corrupt; a backup was kept and the store was rebuilt",
                        Some(format!(
                            "{}: {}; backup: {}",
                            path.display(),
                            error.message,
                            backup.display()
                        )),
                        false,
                    )),
                ))
            }
            Err(error) => Err(error),
        }
    }

    fn open_inner(path: PathBuf) -> Result<Self, KernelError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    KernelError::new(
                        "E5011",
                        "operation log directory is not writable",
                        Some(format!("{}: {error}", parent.display())),
                        false,
                    )
                })?;
            }
        }
        let connection = Connection::open(&path).map_err(|error| {
            KernelError::new(
                "E5011",
                "operation log is not openable",
                Some(format!("{}: {error}", path.display())),
                false,
            )
        })?;
        let version = connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .map_err(|error| corrupt_error(&path, &error.to_string()))?;
        if version > SCHEMA_VERSION {
            return Err(KernelError::new(
                "E1007",
                "operation log was written by a newer version; audit writing is disabled",
                Some(format!("{}: user_version={version}", path.display())),
                false,
            ));
        }
        migrate(&connection, version).map_err(|error| corrupt_error(&path, &error.to_string()))?;
        Ok(Self { path, connection })
    }

    /// 单事务批量写入（ADR-0013：任务收尾一次性落批次与逐条记录）。
    pub fn append_batch(&mut self, records: &[OperationLogRecord]) -> Result<(), KernelError> {
        if records.is_empty() {
            return Ok(());
        }
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| write_error(&self.path, &error.to_string()))?;
        {
            let mut statement = transaction
                .prepare(
                    "INSERT INTO operation_log (
                        id, ts, task_id, plan_id, entry_id, op_kind, phase, status, code,
                        dir_name, old_name, new_name, before_path, after_path,
                        overall_completed, detail
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                )
                .map_err(|error| write_error(&self.path, &error.to_string()))?;
            for record in records {
                statement
                    .execute(rusqlite::params![
                        record.id,
                        record.ts,
                        record.task_id,
                        record.plan_id,
                        record.entry_id,
                        record.op_kind,
                        record.phase,
                        record.status,
                        record.code,
                        record.dir_name,
                        record.old_name,
                        record.new_name,
                        record.before_path,
                        record.after_path,
                        record.overall_completed,
                        record.detail,
                    ])
                    .map_err(|error| write_error(&self.path, &error.to_string()))?;
            }
        }
        transaction
            .commit()
            .map_err(|error| write_error(&self.path, &error.to_string()))
    }

    /// C06 反查：按计划取全部记录（`plan_id` 索引）。
    pub fn entries_for_plan(&self, plan_id: &str) -> Result<Vec<OperationLogRecord>, KernelError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, ts, task_id, plan_id, entry_id, op_kind, phase, status, code,
                        dir_name, old_name, new_name, before_path, after_path,
                        overall_completed, detail
                 FROM operation_log WHERE plan_id = ?1 ORDER BY ts, id",
            )
            .map_err(|error| write_error(&self.path, &error.to_string()))?;
        let rows = statement
            .query_map([plan_id], row_to_record)
            .map_err(|error| write_error(&self.path, &error.to_string()))?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row.map_err(|error| write_error(&self.path, &error.to_string()))?);
        }
        Ok(records)
    }

    /// 保留期清理：删除 `ts <= cutoff` 的记录，返回删除条数（`ts` 索引）。
    pub fn purge_before(&mut self, cutoff: &str) -> Result<usize, KernelError> {
        self.connection
            .execute("DELETE FROM operation_log WHERE ts <= ?1", [cutoff])
            .map_err(|error| write_error(&self.path, &error.to_string()))
    }
}

fn migrate(connection: &Connection, version: i64) -> Result<(), rusqlite::Error> {
    if version < 1 {
        connection.execute_batch(
            "BEGIN;
             CREATE TABLE IF NOT EXISTS operation_log (
                 id TEXT PRIMARY KEY,
                 ts TEXT NOT NULL,
                 task_id TEXT NOT NULL,
                 plan_id TEXT NOT NULL,
                 entry_id TEXT,
                 op_kind TEXT NOT NULL,
                 phase TEXT NOT NULL,
                 status TEXT NOT NULL,
                 code TEXT,
                 dir_name TEXT,
                 old_name TEXT,
                 new_name TEXT,
                 before_path TEXT,
                 after_path TEXT,
                 overall_completed INTEGER NOT NULL,
                 detail TEXT
             );
             CREATE INDEX IF NOT EXISTS idx_oplog_plan ON operation_log (plan_id);
             CREATE INDEX IF NOT EXISTS idx_oplog_task ON operation_log (task_id);
             CREATE INDEX IF NOT EXISTS idx_oplog_ts ON operation_log (ts);
             PRAGMA user_version = 1;
             COMMIT;",
        )?;
    }
    Ok(())
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<OperationLogRecord> {
    Ok(OperationLogRecord {
        id: row.get(0)?,
        ts: row.get(1)?,
        task_id: row.get(2)?,
        plan_id: row.get(3)?,
        entry_id: row.get(4)?,
        op_kind: row.get(5)?,
        phase: row.get(6)?,
        status: row.get(7)?,
        code: row.get(8)?,
        dir_name: row.get(9)?,
        old_name: row.get(10)?,
        new_name: row.get(11)?,
        before_path: row.get(12)?,
        after_path: row.get(13)?,
        overall_completed: row.get::<_, i64>(14)? != 0,
        detail: row.get(15)?,
    })
}

fn corrupt_error(path: &Path, detail: &str) -> KernelError {
    KernelError::new(
        "E5009",
        "operation log is corrupt; a backup was kept and the store was rebuilt",
        Some(format!("{}: {detail}", path.display())),
        false,
    )
}

fn write_error(path: &Path, detail: &str) -> KernelError {
    KernelError::new(
        "E5011",
        "operation log is not writable",
        Some(format!("{}: {detail}", path.display())),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("batchdesk-oplog-{nanos}"));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn record(id: &str, plan_id: &str, ts: &str, status: &str, phase: &str) -> OperationLogRecord {
        OperationLogRecord {
            id: id.to_string(),
            ts: ts.to_string(),
            task_id: format!("task-{id}"),
            plan_id: plan_id.to_string(),
            entry_id: Some(format!("{plan_id}-e1")),
            op_kind: "c09".to_string(),
            phase: phase.to_string(),
            status: status.to_string(),
            code: None,
            dir_name: Some("/data".to_string()),
            old_name: Some("a.txt".to_string()),
            new_name: Some("a.txt.1".to_string()),
            before_path: None,
            after_path: None,
            overall_completed: true,
            detail: None,
        }
    }

    #[test]
    fn schema_migrates_once_and_round_trips_records() {
        let dir = TempDir::new();
        let path = dir.0.join("oplog.sqlite3");
        let (mut log, warning) = SqliteOperationLog::open(path.clone()).unwrap();
        assert!(warning.is_none());
        let version: i64 = log
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);

        let batch = vec![
            record("01A", "plan-1", "2026-10-30T00:00:01Z", "ok", "start"),
            record("01B", "plan-1", "2026-10-30T00:00:02Z", "ok", "entry"),
            record("01C", "plan-1", "2026-10-30T00:00:03Z", "failed", "finish"),
        ];
        log.append_batch(&batch).unwrap();
        log.append_batch(&[]).unwrap();

        let mut other = record("02A", "plan-2", "2026-10-30T00:00:04Z", "ok", "entry");
        other.overall_completed = false;
        log.append_batch(std::slice::from_ref(&other)).unwrap();

        let entries = log.entries_for_plan("plan-1").unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].id, "01A");
        assert_eq!(entries[2].status, "failed");
        assert_eq!(entries[0].dir_name.as_deref(), Some("/data"));
        assert_eq!(log.entries_for_plan("plan-2").unwrap().len(), 1);
        assert_eq!(log.entries_for_plan("plan-missing").unwrap(), Vec::new());
    }

    #[test]
    fn full_paths_follow_the_log_full_paths_switch() {
        let dir = TempDir::new();
        let (mut log, _) = SqliteOperationLog::open(dir.0.join("oplog.sqlite3")).unwrap();
        let mut entry = record("01A", "plan-1", "2026-10-30T00:00:01Z", "ok", "entry");
        entry.before_path = Some("/data/a.txt".to_string());
        entry.after_path = Some("/quarantine/a.txt.1".to_string());
        log.append_batch(std::slice::from_ref(&entry)).unwrap();
        let loaded = log.entries_for_plan("plan-1").unwrap();
        assert_eq!(loaded[0].before_path.as_deref(), Some("/data/a.txt"));
        assert_eq!(loaded[0].after_path.as_deref(), Some("/quarantine/a.txt.1"));
    }

    #[test]
    fn purge_before_removes_only_expired_records() {
        let dir = TempDir::new();
        let (mut log, _) = SqliteOperationLog::open(dir.0.join("oplog.sqlite3")).unwrap();
        let expired = record("01A", "plan-1", "2026-10-20T00:00:00Z", "ok", "entry");
        let fresh = record("02A", "plan-2", "2026-10-30T00:00:00Z", "ok", "entry");
        log.append_batch(&[expired, fresh]).unwrap();
        let removed = log.purge_before("2026-10-29T00:00:00Z").unwrap();
        assert_eq!(removed, 1);
        assert!(log.entries_for_plan("plan-1").unwrap().is_empty());
        assert_eq!(log.entries_for_plan("plan-2").unwrap().len(), 1);
    }

    #[test]
    fn corrupt_database_is_backed_up_and_rebuilt() {
        let dir = TempDir::new();
        let path = dir.0.join("oplog.sqlite3");
        std::fs::write(&path, b"definitely not sqlite").unwrap();
        let (log, warning) = SqliteOperationLog::open(path.clone()).unwrap();
        let warning = warning.expect("corrupt store must surface a warning");
        assert_eq!(warning.code, "E5009");

        let version: i64 = log
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
        assert!(log.entries_for_plan("plan-1").unwrap().is_empty());
        let backups: Vec<_> = std::fs::read_dir(&dir.0)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("oplog.sqlite3.corrupt.")
            })
            .collect();
        assert_eq!(backups.len(), 1);
    }

    #[test]
    fn newer_schema_version_is_rejected_read_only() {
        let dir = TempDir::new();
        let path = dir.0.join("oplog.sqlite3");
        let (log, _) = SqliteOperationLog::open(path.clone()).unwrap();
        log.connection
            .execute_batch("PRAGMA user_version = 99")
            .unwrap();
        drop(log);
        let error = match SqliteOperationLog::open(path) {
            Err(error) => error,
            Ok(_) => panic!("a newer schema version must be rejected"),
        };
        assert_eq!(error.code, "E1007");
    }
}
