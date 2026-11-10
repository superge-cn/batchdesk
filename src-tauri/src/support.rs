//! 支持包（设计 10 §5；方案 §9.7／§12.5）：用户主动导出，先预览后落盘。
//!
//! 内容限于**版本／构建类型／OS／架构／生成时间／最近诊断日志／错误码统计／复现步骤**；
//! 不含文件内容、许可证载荷/签名、设备指纹原文（方案 §4.10／§9.7），完整路径的取舍
//! 已在诊断轨写入时按 `logging.logFullPaths` 定好（读回即已定，本模块不再加工）。
//!
//! **落点**：`<app_data>/exports/batchdesk-support-<ts>.json`。刻意不引入 Tauri 文件对话框
//! 插件——capabilities 保持最小（ADR-0060），用户从导出目录自行取用（方案 §12.5 的
//! 「自行导出发送」）。写盘走文件系统适配层的原子写（不变量 8）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kernel::fs::FsAdapter;
use platform::diaglog::{DiagnosticLog, DiagnosticLogRecord};
use platform::fs::{now_rfc3339, CliFsAdapter};
use serde::{Deserialize, Serialize};

use crate::IpcError;

/// 预览/导出时默认带上的诊断日志条数上限（**工程默认，非产品阈值**：与设计 10 §4
/// 的轮转参数同性质，由 ADR 记录选型；调用方可显式覆盖）。
pub const DEFAULT_MAX_LOG_LINES: u32 = 200;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportPackageInput {
    /// 用户填写的复现步骤（方案 §12.5 的「问题反馈」正文，原样进包）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repro_steps: Option<String>,
    /// 诊断日志条数上限；缺省 [`DEFAULT_MAX_LOG_LINES`]。
    #[serde(default)]
    pub max_log_lines: Option<u32>,
    /// 是否带最近诊断日志；缺省带。
    #[serde(default)]
    pub include_diagnostic_log: Option<bool>,
    /// 是否带错误码统计；缺省带（未带日志时统计自然为空）。
    #[serde(default)]
    pub include_error_code_stats: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportCodeCount {
    pub code: String,
    pub count: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportPackage {
    pub version: String,
    /// `"dev"`／`"release"`。方案 §12.5 的 B/C 通道尚未实现，只如实标构建类型，
    /// 不假造通道字段（缺口见设计 10 §5 的剩余项）。
    pub build: String,
    pub os: String,
    pub arch: String,
    pub generated_at: String,
    pub diagnostic_log: Vec<DiagnosticLogRecord>,
    pub error_code_stats: Vec<SupportCodeCount>,
    pub repro_steps: String,
}

/// 组装支持包（纯读取，不落盘）。`diag` 与任务队列同一条轨（ADR-0091）。
pub fn build(diag: &DiagnosticLog, input: &SupportPackageInput) -> SupportPackage {
    let max_log_lines = input.max_log_lines.unwrap_or(DEFAULT_MAX_LOG_LINES) as usize;
    let include_log = input.include_diagnostic_log.unwrap_or(true);
    let include_stats = input.include_error_code_stats.unwrap_or(true);
    let diagnostic_log = if include_log {
        diag.read_recent(max_log_lines)
    } else {
        Vec::new()
    };
    let error_code_stats = if include_stats {
        code_stats(&diagnostic_log)
    } else {
        Vec::new()
    };
    SupportPackage {
        version: env!("CARGO_PKG_VERSION").to_string(),
        build: if cfg!(debug_assertions) {
            "dev"
        } else {
            "release"
        }
        .to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        generated_at: now_rfc3339(),
        diagnostic_log,
        error_code_stats,
        repro_steps: input.repro_steps.clone().unwrap_or_default(),
    }
}

/// 错误码统计取自**诊断轨**（设计 10 §5 的「错误码统计」）：按 `code` 计数、次数降序。
///
/// 操作日志的逐条错误码需要另建查询口（本批未做），所以这里的覆盖面是"诊断轨已记的
/// 码"——任务失败码已自 2026-11-10 起写进诊断轨（见 `tasks::Inner::finish`）。
fn code_stats(records: &[DiagnosticLogRecord]) -> Vec<SupportCodeCount> {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for record in records {
        if let Some(code) = &record.code {
            *counts.entry(code.clone()).or_insert(0) += 1;
        }
    }
    let mut stats: Vec<SupportCodeCount> = counts
        .into_iter()
        .map(|(code, count)| SupportCodeCount { code, count })
        .collect();
    stats.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.code.cmp(&right.code))
    });
    stats
}

/// 写盘（JSON，UTF-8，原子写）。返回落盘路径。
///
/// 同秒重复导出不会互相覆盖：文件名冲突时追加 `-1`／`-2`……
pub fn write(package: &SupportPackage, exports_dir: &Path) -> Result<PathBuf, IpcError> {
    let json = serde_json::to_string_pretty(package)
        .map_err(|_| IpcError::new("E9001", "cannot serialize the support package"))?;
    let stamp = now_rfc3339().replace(':', "-");
    let mut path = exports_dir.join(format!("batchdesk-support-{stamp}.json"));
    let mut counter = 1u32;
    while path.exists() {
        path = exports_dir.join(format!("batchdesk-support-{stamp}-{counter}.json"));
        counter += 1;
    }
    CliFsAdapter::new(None).write_atomic(&path.to_string_lossy(), json.as_bytes())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::diaglog::{DiagnosticEvent, Level};
    use std::path::PathBuf;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let base = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("test-tmp")
                .join(format!(
                    "batchdesk-support-{}-{sequence}",
                    std::process::id()
                ));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap();
            Self(base)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn diag_with_records(temp: &TempDir) -> DiagnosticLog {
        let diag = DiagnosticLog::open(temp.0.join("logs/diagnostic.log"), Default::default());
        diag.log_event(
            Level::Warn,
            "oplog",
            DiagnosticEvent {
                code: Some("E1007"),
                task_id: None,
                trace_id: None,
                message: "operation log was written by a newer version",
                fields: None,
            },
        );
        diag.log(Level::Info, "task-queue", "task started");
        for task_id in ["t-1", "t-2"] {
            diag.log_event(
                Level::Warn,
                "task-queue",
                DiagnosticEvent {
                    code: Some("E2002"),
                    task_id: Some(task_id),
                    trace_id: None,
                    message: "task failed",
                    fields: None,
                },
            );
        }
        diag
    }

    #[test]
    fn build_reports_environment_and_counts_codes_by_frequency() {
        let temp = TempDir::new();
        let diag = diag_with_records(&temp);
        let package = build(&diag, &SupportPackageInput::default());
        assert!(!package.version.is_empty());
        assert!(!package.os.is_empty());
        assert!(!package.arch.is_empty());
        assert!(
            package.generated_at.contains('T'),
            "生成时间必须是 RFC 3339"
        );
        assert_eq!(package.diagnostic_log.len(), 4);
        // 次数降序：E2002 ×2 在 E1007 ×1 之前；没有 code 的记录不参与统计。
        let stats = package.error_code_stats;
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].code, "E2002");
        assert_eq!(stats[0].count, 2);
        assert_eq!(stats[1].code, "E1007");
        assert_eq!(stats[1].count, 1);
    }

    #[test]
    fn build_honours_section_toggles_and_repro_steps() {
        let temp = TempDir::new();
        let diag = diag_with_records(&temp);
        let input = SupportPackageInput {
            repro_steps: Some("rename a folder twice".to_string()),
            max_log_lines: None,
            include_diagnostic_log: Some(false),
            include_error_code_stats: Some(false),
        };
        let package = build(&diag, &input);
        assert!(package.diagnostic_log.is_empty(), "关掉段落就不该带日志");
        assert!(package.error_code_stats.is_empty());
        assert_eq!(package.repro_steps, "rename a folder twice");
    }

    #[test]
    fn write_emits_pretty_json_and_avoids_same_second_collisions() {
        let temp = TempDir::new();
        let diag = diag_with_records(&temp);
        let exports = temp.0.join("exports");
        let package = build(&diag, &SupportPackageInput::default());
        let first = write(&package, &exports).unwrap();
        let second = write(&package, &exports).unwrap();
        assert_ne!(first, second, "同秒重复导出不得互相覆盖");
        let text = std::fs::read_to_string(&first).unwrap();
        let parsed: SupportPackage = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.version, package.version);
        assert_eq!(parsed.error_code_stats.len(), 2);
        assert!(
            text.contains('\n') && text.starts_with('{'),
            "落盘应为可读的 pretty JSON"
        );
    }
}
