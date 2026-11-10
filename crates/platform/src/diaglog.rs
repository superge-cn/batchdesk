//!
//! 诊断日志（运行轨，设计 10 §1／§2／§3；ADR-0091）。
//!
//! 载体是 `<app_data>/logs/diagnostic.log`（JSONL，每行一条 `DiagnosticLogRecord`）：
//! 单文件写满 `max_bytes` 触发轮转（改名 `diagnostic-<ts>.log` 后新开）；轮转文件按
//! mtime 清理（`retention_days`，缺省 14 天＝方案 §9.6）；全部诊断文件总大小超过
//! `max_total_bytes` 时从最旧开始删，删到保留窗口内则补一条 `warn`。
//!
//! 诊断轨**尽力而为**：写失败静默丢弃，绝不阻断业务——Journal 是撤销依据、操作日志是
//! 审计依据，诊断轨不是依据（设计 10 §1）。kernel 不取时钟也不做 I/O，所以本模块在
//! platform（共享壳层适配层），时钟经 `crate::fs`。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::fs::now_rfc3339;

/// 当前文件名（模块头已说明载体路径；仅测试按它拼路径）。
#[cfg(test)]
const CURRENT_FILE: &str = "diagnostic.log";
/// 轮转产物为 `diagnostic-<ts>.log`。
const ROTATED_PREFIX: &str = "diagnostic-";
const ROTATED_SUFFIX: &str = ".log";

/// 级别（设计 10 §2 的 `level`）。顺序即严重度，便于与缺省级别比较。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }

    /// 环境变量 `BATCHDESK_DIAG_LEVEL` 的解析（大小写不敏感；认不出返回 `None`）。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" | "warning" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

/// 轮转/保留参数（ADR-0091 的**工程默认**，非产品阈值）。
///
/// 保留期 14 天引用方案 §9.6「诊断日志 14 天」；`logging.retentionDays` 只管操作日志
/// （设计 03 §4.1），本轨不复用。大小与兜底上限是工程默认，可经环境变量调试期覆盖。
#[derive(Clone, Debug)]
pub struct DiagnosticOpts {
    pub level: Level,
    pub max_bytes: u64,
    pub max_total_bytes: u64,
    pub retention_days: u64,
}

impl Default for DiagnosticOpts {
    fn default() -> Self {
        Self {
            level: Level::Info,
            max_bytes: 8 * 1024 * 1024,
            max_total_bytes: 64 * 1024 * 1024,
            retention_days: 14,
        }
    }
}

/// 读 `BATCHDESK_DIAG_*` 覆盖工程默认（调试期用；认不出的值原样保留）。
pub fn opts_from_env() -> DiagnosticOpts {
    let mut opts = DiagnosticOpts::default();
    if let Ok(raw) = std::env::var("BATCHDESK_DIAG_LEVEL") {
        if let Some(level) = Level::parse(&raw) {
            opts.level = level;
        }
    }
    if let Ok(raw) = std::env::var("BATCHDESK_DIAG_MAX_BYTES") {
        if let Ok(value) = raw.trim().parse::<u64>() {
            if value > 0 {
                opts.max_bytes = value;
            }
        }
    }
    if let Ok(raw) = std::env::var("BATCHDESK_DIAG_MAX_TOTAL_BYTES") {
        if let Ok(value) = raw.trim().parse::<u64>() {
            if value > 0 {
                opts.max_total_bytes = value;
            }
        }
    }
    opts
}

/// 诊断记录（设计 10 §2 的工程投影；serde 命名对齐线协议）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticLogRecord {
    /// RFC 3339 UTC（经 Clock 语义的系统时钟；kernel 不取时钟）。
    pub ts: String,
    pub level: String,
    /// 模块/组件。
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// 技术消息，不含用户内容（脱敏口径见设计 10 §3）。
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Map<String, Value>>,
}

/// 一条诊断事件的上下文（`code`／`taskId`／`traceId`／`fields` 都可选）。
pub struct DiagnosticEvent<'a> {
    pub code: Option<&'a str>,
    pub task_id: Option<&'a str>,
    pub trace_id: Option<&'a str>,
    pub message: &'a str,
    pub fields: Option<&'a Map<String, Value>>,
}

#[derive(Default)]
struct Inner {
    file: Option<File>,
    /// 当前文件已写字节（用于判断轮转时机）。
    written: u64,
}

/// 诊断日志句柄：可克隆共享（壳层与任务队列同一条轨）。
#[derive(Clone)]
pub struct DiagnosticLog {
    path: PathBuf,
    opts: DiagnosticOpts,
    full_paths: Arc<AtomicBool>,
    inner: Arc<Mutex<Inner>>,
}

impl std::fmt::Debug for DiagnosticLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiagnosticLog")
            .field("path", &self.path)
            .field("opts", &self.opts)
            .finish_non_exhaustive()
    }
}

impl DiagnosticLog {
    /// 打开（**不失败**）：父目录建不了、文件打不开都留到写时处理，写失败即丢弃。
    pub fn open(path: impl Into<PathBuf>, opts: DiagnosticOpts) -> Self {
        let log = Self {
            path: path.into(),
            opts,
            full_paths: Arc::new(AtomicBool::new(false)),
            inner: Arc::new(Mutex::new(Inner::default())),
        };
        if let Some(parent) = log.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // 启动即清理：保留期不依赖"有没有新日志写下来"。
        let deleted = log.cleanup();
        if deleted > 0 {
            log.warn_cap_cleanup(deleted);
        }
        log
    }

    /// `logging.logFullPaths`（03 §4.1，默认 false）由装配层注入。
    pub fn set_full_paths(&self, flag: bool) {
        self.full_paths.store(flag, Ordering::Relaxed);
    }

    pub fn full_paths(&self) -> bool {
        self.full_paths.load(Ordering::Relaxed)
    }

    /// 路径脱敏（设计 10 §3）：开关关闭时只留文件名，取不到文件名则给占位符。
    pub fn redact_path(&self, path: &str) -> String {
        if self.full_paths() {
            return path.to_string();
        }
        Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_string())
            .unwrap_or_else(|| "<path>".to_string())
    }

    /// 便捷入口：只有消息的事件。
    pub fn log(&self, level: Level, target: &str, message: &str) {
        self.log_event(
            level,
            target,
            DiagnosticEvent {
                code: None,
                task_id: None,
                trace_id: None,
                message,
                fields: None,
            },
        );
    }

    pub fn log_event(&self, level: Level, target: &str, event: DiagnosticEvent<'_>) {
        if level < self.opts.level {
            return;
        }
        let record = DiagnosticLogRecord {
            ts: now_rfc3339(),
            level: level.as_str().to_string(),
            target: target.to_string(),
            code: event.code.map(str::to_string),
            task_id: event.task_id.map(str::to_string),
            trace_id: event.trace_id.map(str::to_string),
            message: event.message.to_string(),
            fields: event.fields.cloned(),
        };
        let line = match serde_json::to_string(&record) {
            Ok(mut json) => {
                json.push('\n');
                json
            }
            Err(_) => return,
        };
        let deleted = {
            let mut inner = self.lock();
            if inner.file.is_none() && !self.open_current(&mut inner) {
                return;
            }
            let over = inner.written.saturating_add(line.len() as u64) > self.opts.max_bytes;
            let mut deleted = 0usize;
            if over {
                deleted = self.rotate(&mut inner);
                // 轮转失败（改名不了）也继续往原文件写：宁可超限也不丢日志。
                if inner.file.is_none() && !self.open_current(&mut inner) {
                    return;
                }
            }
            let written = inner
                .file
                .as_mut()
                .map(|file| file.write_all(line.as_bytes()).is_ok())
                .unwrap_or(false);
            if written {
                inner.written = inner.written.saturating_add(line.len() as u64);
            } else {
                // 写失败：丢掉句柄下次重开，本轮丢弃该行（尽力而为）。
                inner.file = None;
                inner.written = 0;
            }
            deleted
        };
        if deleted > 0 {
            self.warn_cap_cleanup(deleted);
        }
    }

    /// 兜底清理删到了保留窗口内的文件：记一条 warn（不静默），消息不含路径原文。
    fn warn_cap_cleanup(&self, deleted: usize) {
        self.log(
            Level::Warn,
            "diaglog",
            &format!(
                "诊断日志磁盘上限 {} 字节触发清理，保留窗口内删除 {deleted} 份轮转文件",
                self.opts.max_total_bytes
            ),
        );
    }

    /// 读最近 `max_lines` 条（供设计 10 §5 的支持包使用）。
    ///
    /// 从最新文件往回读（当前文件＋轮转文件按 mtime 倒序），够数即停；坏行跳过。
    pub fn read_recent(&self, max_lines: usize) -> Vec<DiagnosticLogRecord> {
        if max_lines == 0 {
            return Vec::new();
        }
        let mut files = self.rotated_files();
        files.insert(0, self.path.clone());
        let mut collected: Vec<DiagnosticLogRecord> = Vec::new();
        for path in files {
            let content = match std::fs::read_to_string(&path) {
                Ok(content) => content,
                Err(_) => continue,
            };
            let mut lines: Vec<DiagnosticLogRecord> = content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| serde_json::from_str::<DiagnosticLogRecord>(line).ok())
                .collect();
            // 该文件内只取最后 max_lines 行（更晚的），再拼在已收（更新）的前面。
            lines.reverse();
            lines.truncate(max_lines);
            lines.reverse();
            // 该文件（更新）在前、已收（更旧）在后；只保留最新 max_lines 条。
            let mut merged = lines;
            merged.extend(collected);
            if merged.len() > max_lines {
                let excess = merged.len() - max_lines;
                merged.drain(0..excess);
            }
            collected = merged;
            if collected.len() >= max_lines {
                return collected;
            }
        }
        collected
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 打开当前文件（追加）；返回是否成功。
    fn open_current(&self, inner: &mut Inner) -> bool {
        if let Some(parent) = self.path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return false;
            }
        }
        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(file) => {
                let written = file.metadata().map(|meta| meta.len()).unwrap_or(0);
                inner.file = Some(file);
                inner.written = written;
                true
            }
            Err(_) => false,
        }
    }

    /// 轮转：关当前文件 → 改名 → 清理。返回"在保留窗口内被兜底删掉"的份数（用于补 warn）。
    fn rotate(&self, inner: &mut Inner) -> usize {
        inner.file = None;
        let stamp = now_rfc3339().replace(':', "-");
        let mut target = self.rotated_path(&stamp);
        let mut counter = 1;
        while target.exists() {
            target = self.rotated_path(&format!("{stamp}-{counter}"));
            counter += 1;
        }
        if std::fs::rename(&self.path, &target).is_err() {
            return 0;
        }
        inner.written = 0;
        self.cleanup()
    }

    fn rotated_path(&self, stamp: &str) -> PathBuf {
        self.path
            .with_file_name(format!("{ROTATED_PREFIX}{stamp}{ROTATED_SUFFIX}"))
    }

    /// 同目录的轮转文件（新→旧）。
    fn rotated_files(&self) -> Vec<PathBuf> {
        let dir = match self.path.parent() {
            Some(dir) => dir,
            None => return Vec::new(),
        };
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };
        let mut files: Vec<(SystemTime, PathBuf)> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                let name = path.file_name().and_then(|name| name.to_str());
                match name {
                    Some(name) => {
                        name.starts_with(ROTATED_PREFIX) && name.ends_with(ROTATED_SUFFIX)
                    }
                    None => false,
                }
            })
            .filter_map(|path| {
                let meta = std::fs::metadata(&path).ok()?;
                Some((meta.modified().unwrap_or_else(|_| SystemTime::now()), path))
            })
            .collect();
        files.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
        files.into_iter().map(|(_, path)| path).collect()
    }

    /// 清理：① 删掉超过保留期的轮转文件；② 总大小超兜底上限时从最旧开始删。
    /// 返回"在保留窗口内被②删掉"的份数（调用方补一条 warn，不静默）。
    fn cleanup(&self) -> usize {
        let cutoff = SystemTime::now()
            .checked_sub(Duration::from_secs(
                self.opts.retention_days.saturating_mul(86_400),
            ))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let mut kept: Vec<(SystemTime, PathBuf, u64)> = Vec::new();
        for (mtime, path) in self.rotated_files_newest_last() {
            let len = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            if mtime < cutoff {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            kept.push((mtime, path, len));
        }
        let current = std::fs::metadata(&self.path)
            .map(|meta| meta.len())
            .unwrap_or(0);
        let mut total = current + kept.iter().map(|(_, _, len)| *len).sum::<u64>();
        if total <= self.opts.max_total_bytes {
            return 0;
        }
        kept.sort_by_key(|(mtime, _, _)| *mtime);
        let mut deleted = 0usize;
        for (mtime, path, len) in kept {
            if total <= self.opts.max_total_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
                if mtime >= cutoff {
                    deleted += 1;
                }
            }
        }
        deleted
    }

    /// 同目录轮转文件（旧→新），供清理按时间判定。
    fn rotated_files_newest_last(&self) -> Vec<(SystemTime, PathBuf)> {
        let mut files = self.rotated_files();
        files.reverse();
        files
            .into_iter()
            .filter_map(|path| {
                let meta = std::fs::metadata(&path).ok()?;
                Some((meta.modified().unwrap_or_else(|_| SystemTime::now()), path))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::AtomicUsize;

    fn temp_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "batchdesk-diaglog-{label}-{}-{seq}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read_current(dir: &Path) -> Vec<DiagnosticLogRecord> {
        let content = fs::read_to_string(dir.join(CURRENT_FILE)).unwrap_or_default();
        content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str::<DiagnosticLogRecord>(line).unwrap())
            .collect()
    }

    fn rotated_count(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                name.starts_with(ROTATED_PREFIX) && name.ends_with(ROTATED_SUFFIX)
            })
            .count()
    }

    fn write_rotated(dir: &Path, name: &str, body: &str, age_secs: u64) {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(age_secs))
            .unwrap();
        drop(file);
    }

    #[test]
    fn writes_contract_shaped_lines_and_reads_them_back() {
        let dir = temp_dir("basic");
        let log = DiagnosticLog::open(dir.join(CURRENT_FILE), DiagnosticOpts::default());
        log.log(Level::Info, "tasks", "任务开始");
        log.log(Level::Warn, "tasks", "任务失败");

        let lines = read_current(&dir);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].level, "info");
        assert_eq!(lines[0].target, "tasks");
        assert_eq!(lines[0].message, "任务开始");
        assert!(lines[0].ts.ends_with('Z'), "ts 应为 RFC3339 UTC");
        assert_eq!(lines[1].level, "warn");

        let recent = log.read_recent(10);
        let messages: Vec<&str> = recent.iter().map(|r| r.message.as_str()).collect();
        assert_eq!(messages, vec!["任务开始", "任务失败"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn level_filters_below_threshold() {
        let dir = temp_dir("level");
        let opts = DiagnosticOpts {
            level: Level::Warn,
            ..Default::default()
        };
        let log = DiagnosticLog::open(dir.join(CURRENT_FILE), opts);
        log.log(Level::Debug, "t", "debug");
        log.log(Level::Info, "t", "info");
        log.log(Level::Warn, "t", "warn");
        log.log(Level::Error, "t", "error");

        let records = read_current(&dir);
        let levels: Vec<&str> = records.iter().map(|r| r.level.as_str()).collect();
        assert_eq!(levels, vec!["warn", "error"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_level_the_way_env_var_will_use_it() {
        assert_eq!(Level::parse("DEBUG"), Some(Level::Debug));
        assert_eq!(Level::parse(" info "), Some(Level::Info));
        assert_eq!(Level::parse("warning"), Some(Level::Warn));
        assert_eq!(Level::parse("Error"), Some(Level::Error));
        assert_eq!(Level::parse("trace"), None);
    }

    #[test]
    fn redact_path_follows_full_paths_flag() {
        let dir = temp_dir("redact");
        let log = DiagnosticLog::open(dir.join(CURRENT_FILE), DiagnosticOpts::default());
        assert_eq!(log.redact_path("C:\\data\\a.txt"), "a.txt");
        assert_eq!(log.redact_path("/home/u/secret.bin"), "secret.bin");
        assert_eq!(log.redact_path("C:\\"), "<path>");
        log.set_full_paths(true);
        assert_eq!(log.redact_path("C:\\data\\a.txt"), "C:\\data\\a.txt");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotates_on_size_and_read_recent_spans_rotations_in_order() {
        let dir = temp_dir("rotate");
        let opts = DiagnosticOpts {
            max_bytes: 200,
            max_total_bytes: 10_000,
            ..Default::default()
        };
        let log = DiagnosticLog::open(dir.join(CURRENT_FILE), opts);
        for index in 0..20 {
            log.log(Level::Info, "bench", &format!("line {index}"));
        }

        assert!(!read_current(&dir).is_empty(), "当前文件应有内容");
        assert!(rotated_count(&dir) >= 1, "应至少轮转出一份文件");

        let recent = log.read_recent(5);
        let messages: Vec<&str> = recent.iter().map(|r| r.message.as_str()).collect();
        assert_eq!(
            messages,
            vec!["line 15", "line 16", "line 17", "line 18", "line 19"],
            "跨轮转按时间序返回最新 N 条"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_removes_rotated_files_older_than_retention() {
        let dir = temp_dir("retention");
        write_rotated(
            &dir,
            "diagnostic-2020-01-01T00-00-00Z.log",
            "{\"ts\":\"old\"}\n",
            400 * 86_400,
        );
        write_rotated(
            &dir,
            "diagnostic-2026-01-01T00-00-00Z.log",
            "{\"ts\":\"new\"}\n",
            60,
        );

        let log = DiagnosticLog::open(dir.join(CURRENT_FILE), DiagnosticOpts::default());
        assert!(!dir.join("diagnostic-2020-01-01T00-00-00Z.log").exists());
        assert!(dir.join("diagnostic-2026-01-01T00-00-00Z.log").exists());
        let _ = log;
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn total_cap_deletes_oldest_and_leaves_a_warn() {
        let dir = temp_dir("cap");
        let body = format!(
            "{}\n",
            "{\"ts\":\"x\",\"level\":\"info\",\"target\":\"t\",\"message\":\"old\"}".repeat(3)
        );
        write_rotated(
            &dir,
            "diagnostic-2026-11-01T00-00-00Z.log",
            &body,
            9 * 86_400,
        );
        write_rotated(
            &dir,
            "diagnostic-2026-11-05T00-00-00Z.log",
            &body,
            5 * 86_400,
        );
        write_rotated(&dir, "diagnostic-2026-11-09T00-00-00Z.log", &body, 86_400);

        let opts = DiagnosticOpts {
            max_bytes: 8 * 1024 * 1024,
            max_total_bytes: 400,
            ..Default::default()
        };
        let _log = DiagnosticLog::open(dir.join(CURRENT_FILE), opts);
        assert!(
            rotated_count(&dir) <= 2,
            "兜底上限应从最旧的开始删（剩 {} 份）",
            rotated_count(&dir)
        );
        assert!(
            read_current(&dir)
                .iter()
                .any(|r| r.level == "warn" && r.target == "diaglog"),
            "删到保留窗口内要留一条 warn"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_failure_is_swallowed_and_track_stays_usable() {
        let dir = temp_dir("failure");
        // 父路径被一个文件占住：建不了目录、打不开文件——写必须静默丢弃。
        let blocker = dir.join("blocker");
        fs::write(&blocker, "not a directory").unwrap();
        let log = DiagnosticLog::open(blocker.join(CURRENT_FILE), DiagnosticOpts::default());
        log.log(Level::Info, "tasks", "这条会失败");
        assert!(log.read_recent(5).is_empty());

        // 换到正常目录后同一条轨仍可用（句柄是路径级的，不受上次失败影响）。
        let good_dir = temp_dir("failure-recovered");
        let good = DiagnosticLog::open(good_dir.join(CURRENT_FILE), DiagnosticOpts::default());
        good.log(Level::Info, "tasks", "这条能写");
        assert_eq!(good.read_recent(5).len(), 1);
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&good_dir);
    }
}
