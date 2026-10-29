//! S01 任务预检（纯函数；方案 §3.3、§4.9、§5.13、§9.1）。
//!
//! 预检把"执行到一半失败"前移为"执行前阻断"。kernel 只做判定：磁盘余量、
//! 只读/占用/占位、路径长度分级、UNC 可达性与保护目录命中都来自调用方采集的
//! 事实（kernel 不读盘、不探测、不读时钟，不变量 1）。
//!
//! 判定式（方案 §9.1、11.6-12）：剩余空间 ≥ 需用空间 ＋ 安全余量 ＋ 临时峰值；
//! 安全余量 ＝ max(下限, 预估输出总量 × 比例)，两个常数由调用方传入，kernel
//! 不内置产品数值。路径长度分级（方案 §5.13）：≤200 安全、200–259 警告但允许、
//! ≥260 需系统长路径支持，否则阻断该项。

use serde::{Deserialize, Serialize};

use crate::fs::CaseSense;
use crate::path::{protected_by, try_normalize, ProtectedDir};

/// 路径长度安全上限（方案 §5.13：≤200 安全）。
pub const PATH_SAFE_MAX_CHARS: usize = 200;
/// 路径长度警告上限（方案 §5.13：200–259 警告但允许）。
pub const PATH_WARN_MAX_CHARS: usize = 259;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreflightLevel {
    Pass,
    Warn,
    Block,
}

impl PreflightLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Block => "block",
        }
    }

    fn max(self, other: Self) -> Self {
        match (self, other) {
            (Self::Block, _) | (_, Self::Block) => Self::Block,
            (Self::Warn, _) | (_, Self::Warn) => Self::Warn,
            _ => Self::Pass,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightItem {
    /// 预检项类别：disk-space / protected / readonly / locked / placeholder /
    /// long-path / unc / path。
    pub kind: String,
    pub level: PreflightLevel,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightReport {
    pub items: Vec<PreflightItem>,
    pub level: PreflightLevel,
    pub pass: usize,
    pub warn: usize,
    pub block: usize,
    /// 整任务阻断（当前仅磁盘空间不足）：执行前必须停下，零副作用；
    /// 条目级阻断（保护目录/长路径）由计划逐条 skipped，不阻断其余条目。
    pub task_blocked: bool,
}

impl PreflightReport {
    pub fn is_task_blocked(&self) -> bool {
        self.task_blocked
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiskSpaceCheck {
    pub required_bytes: u64,
    pub available_bytes: u64,
    pub temp_peak_bytes: u64,
    pub margin_floor_bytes: u64,
    pub margin_ratio: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathCheck {
    pub path: String,
    pub canonical_path: Option<String>,
    pub readonly: bool,
    pub locked: bool,
    pub placeholder: bool,
    /// Windows 长路径支持开关（方案 §5.13）；非 Windows 恒为 true。
    pub long_path_supported: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UncCheck {
    pub path: String,
    /// `Some(true)` 可达；`Some(false)` 明确不可达；`None` 探测超时（暂不可达）。
    pub reachable: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct PreflightInput {
    pub disk: Option<DiskSpaceCheck>,
    pub paths: Vec<PathCheck>,
    pub unc: Vec<UncCheck>,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
}

/// 安全余量（方案 §9.1）：max(下限, 预估输出总量 × 比例)，比例非法按 0 处理。
pub fn disk_margin_bytes(required_bytes: u64, margin_floor_bytes: u64, margin_ratio: f64) -> u64 {
    let ratio = if margin_ratio.is_finite() && margin_ratio > 0.0 {
        margin_ratio
    } else {
        0.0
    };
    let proportional = (required_bytes as f64 * ratio).ceil() as u64;
    proportional.max(margin_floor_bytes)
}

/// 判定路径长度分级（方案 §5.13）；返回预检级别与说明。
pub fn path_length_level(path: &str, long_path_supported: bool) -> (PreflightLevel, String) {
    let length = path.chars().count();
    if length <= PATH_SAFE_MAX_CHARS {
        return (
            PreflightLevel::Pass,
            format!("path length {length} is safe"),
        );
    }
    if length <= PATH_WARN_MAX_CHARS {
        return (
            PreflightLevel::Warn,
            format!("path length {length} is in the 200-259 caution range"),
        );
    }
    if long_path_supported {
        (
            PreflightLevel::Pass,
            format!("path length {length} needs long-path support (enabled)"),
        )
    } else {
        (
            PreflightLevel::Block,
            format!("path length {length} exceeds 259 and long-path support is disabled"),
        )
    }
}

pub fn run_preflight(input: &PreflightInput) -> PreflightReport {
    let mut items = Vec::new();
    let mut task_blocked = false;

    if let Some(disk) = input.disk {
        let margin = disk_margin_bytes(
            disk.required_bytes,
            disk.margin_floor_bytes,
            disk.margin_ratio,
        );
        let need = disk
            .required_bytes
            .saturating_add(margin)
            .saturating_add(disk.temp_peak_bytes);
        let (level, message, hint) = if disk.available_bytes >= need {
            (
                PreflightLevel::Pass,
                format!(
                    "available {} bytes >= required {} + margin {} + temp peak {}",
                    disk.available_bytes, disk.required_bytes, margin, disk.temp_peak_bytes
                ),
                None,
            )
        } else {
            task_blocked = true;
            (
                PreflightLevel::Block,
                format!(
                    "insufficient space: available {} bytes < required {} + margin {} + temp peak {}",
                    disk.available_bytes, disk.required_bytes, margin, disk.temp_peak_bytes
                ),
                Some("free disk space or choose a smaller output".to_string()),
            )
        };
        items.push(item(
            "disk-space",
            level,
            message,
            None,
            Some("E2004"),
            hint,
        ));
    }

    let mut protected_hits = 0usize;
    let mut readonly_hits = 0usize;
    let mut locked_hits = 0usize;
    let mut placeholder_hits = 0usize;
    let mut long_path_items = 0usize;

    for check in &input.paths {
        let normalized = match try_normalize(&check.path) {
            Ok(normalized) => normalized,
            Err(error) => {
                items.push(item(
                    "path",
                    PreflightLevel::Block,
                    error.message,
                    Some(check.path.clone()),
                    Some(&error.code),
                    None,
                ));
                continue;
            }
        };
        if let Some(hit) = protected_by(
            &normalized,
            check.canonical_path.as_deref(),
            &input.protected_dirs,
            input.case_sense,
        ) {
            protected_hits += 1;
            items.push(item(
                "protected",
                PreflightLevel::Block,
                format!("path is under protected directory {hit}"),
                Some(check.path.clone()),
                Some("E2009"),
                Some("remove it from protectedDirs or choose another source".to_string()),
            ));
        }
        if check.readonly {
            readonly_hits += 1;
            items.push(item(
                "readonly",
                PreflightLevel::Warn,
                "target is read-only; execution retries once and then skips".to_string(),
                Some(check.path.clone()),
                Some("E2002"),
                None,
            ));
        }
        if check.locked {
            locked_hits += 1;
            items.push(item(
                "locked",
                PreflightLevel::Warn,
                "target is in use; execution retries once and then skips".to_string(),
                Some(check.path.clone()),
                Some("E2003"),
                None,
            ));
        }
        if check.placeholder {
            placeholder_hits += 1;
            items.push(item(
                "placeholder",
                PreflightLevel::Warn,
                "placeholder file is skipped by the default policy".to_string(),
                Some(check.path.clone()),
                Some("E2006"),
                None,
            ));
        }
        let (level, message) = path_length_level(&check.path, check.long_path_supported);
        if level != PreflightLevel::Pass {
            long_path_items += 1;
            items.push(item(
                "long-path",
                level,
                message,
                Some(check.path.clone()),
                None,
                if level == PreflightLevel::Block {
                    Some("enable long-path support or shorten the target path".to_string())
                } else {
                    None
                },
            ));
        }
    }

    if input.paths.is_empty() {
        items.push(item(
            "path",
            PreflightLevel::Pass,
            "no target paths to check".to_string(),
            None,
            None,
            None,
        ));
    }
    if protected_hits == 0 {
        items.push(item(
            "protected",
            PreflightLevel::Pass,
            "no protected-directory hits".to_string(),
            None,
            None,
            None,
        ));
    }
    if readonly_hits == 0 {
        items.push(item(
            "readonly",
            PreflightLevel::Pass,
            "no read-only targets".to_string(),
            None,
            None,
            None,
        ));
    }
    if locked_hits == 0 {
        items.push(item(
            "locked",
            PreflightLevel::Pass,
            "no locked targets".to_string(),
            None,
            None,
            None,
        ));
    }
    if placeholder_hits == 0 {
        items.push(item(
            "placeholder",
            PreflightLevel::Pass,
            "no placeholder targets".to_string(),
            None,
            None,
            None,
        ));
    }
    if long_path_items == 0 {
        items.push(item(
            "long-path",
            PreflightLevel::Pass,
            "all target paths are within the safe length".to_string(),
            None,
            None,
            None,
        ));
    }

    if input.unc.is_empty() {
        items.push(item(
            "unc",
            PreflightLevel::Pass,
            "no network paths to probe".to_string(),
            None,
            None,
            None,
        ));
    } else {
        for check in &input.unc {
            match check.reachable {
                Some(true) => items.push(item(
                    "unc",
                    PreflightLevel::Pass,
                    "network path is reachable".to_string(),
                    Some(check.path.clone()),
                    None,
                    None,
                )),
                Some(false) | None => {
                    let timed_out = check.reachable.is_none();
                    items.push(item(
                        "unc",
                        PreflightLevel::Warn,
                        if timed_out {
                            "network path probe timed out; treated as temporarily unreachable"
                                .to_string()
                        } else {
                            "network path is temporarily unreachable".to_string()
                        },
                        Some(check.path.clone()),
                        Some("E2005"),
                        Some(
                            "continue waiting or skip this path; the task is not blocked"
                                .to_string(),
                        ),
                    ))
                }
            }
        }
    }

    let level = items
        .iter()
        .fold(PreflightLevel::Pass, |level, item| level.max(item.level));
    let pass = items
        .iter()
        .filter(|item| item.level == PreflightLevel::Pass)
        .count();
    let warn = items
        .iter()
        .filter(|item| item.level == PreflightLevel::Warn)
        .count();
    let block = items
        .iter()
        .filter(|item| item.level == PreflightLevel::Block)
        .count();

    PreflightReport {
        items,
        level,
        pass,
        warn,
        block,
        task_blocked,
    }
}

fn item(
    kind: &str,
    level: PreflightLevel,
    message: String,
    path: Option<String>,
    code: Option<&str>,
    hint: Option<String>,
) -> PreflightItem {
    PreflightItem {
        kind: kind.to_string(),
        level,
        message,
        path,
        code: code.map(str::to_string),
        hint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(required: u64, available: u64) -> DiskSpaceCheck {
        DiskSpaceCheck {
            required_bytes: required,
            available_bytes: available,
            temp_peak_bytes: 0,
            margin_floor_bytes: 10,
            margin_ratio: 0.10,
        }
    }

    fn check(path: &str) -> PathCheck {
        PathCheck {
            path: path.to_string(),
            canonical_path: None,
            readonly: false,
            locked: false,
            placeholder: false,
            long_path_supported: false,
        }
    }

    fn find<'a>(report: &'a PreflightReport, kind: &str) -> Vec<&'a PreflightItem> {
        report
            .items
            .iter()
            .filter(|item| item.kind == kind)
            .collect()
    }

    #[test]
    fn disk_margin_uses_floor_and_ratio() {
        assert_eq!(disk_margin_bytes(100, 10, 0.10), 10);
        assert_eq!(disk_margin_bytes(1000, 10, 0.10), 100);
        assert_eq!(disk_margin_bytes(1000, 200, 0.10), 200);
        assert_eq!(disk_margin_bytes(100, 10, f64::NAN), 10);
    }

    #[test]
    fn disk_pass_at_boundary_and_block_one_byte_below() {
        let input = PreflightInput {
            disk: Some(disk(100, 110)),
            ..Default::default()
        };
        let report = run_preflight(&input);
        assert_eq!(report.level, PreflightLevel::Pass);
        assert!(!report.task_blocked);

        let input = PreflightInput {
            disk: Some(disk(100, 109)),
            ..Default::default()
        };
        let report = run_preflight(&input);
        assert_eq!(report.level, PreflightLevel::Block);
        assert!(report.task_blocked);
        let disks = find(&report, "disk-space");
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].code.as_deref(), Some("E2004"));
    }

    #[test]
    fn temp_peak_is_part_of_the_requirement() {
        let input = PreflightInput {
            disk: Some(DiskSpaceCheck {
                required_bytes: 100,
                available_bytes: 115,
                temp_peak_bytes: 5,
                margin_floor_bytes: 10,
                margin_ratio: 0.0,
            }),
            ..Default::default()
        };
        let report = run_preflight(&input);
        assert_eq!(report.level, PreflightLevel::Pass);
        assert!(!report.task_blocked);

        let input = PreflightInput {
            disk: Some(DiskSpaceCheck {
                required_bytes: 100,
                available_bytes: 114,
                temp_peak_bytes: 5,
                margin_floor_bytes: 10,
                margin_ratio: 0.0,
            }),
            ..Default::default()
        };
        let report = run_preflight(&input);
        assert!(report.task_blocked);
    }

    #[test]
    fn path_length_grades_at_200_259_and_260() {
        let path = "a".repeat(200);
        assert_eq!(path_length_level(&path, false).0, PreflightLevel::Pass);
        let path = "a".repeat(201);
        assert_eq!(path_length_level(&path, false).0, PreflightLevel::Warn);
        let path = "a".repeat(259);
        assert_eq!(path_length_level(&path, false).0, PreflightLevel::Warn);
        let path = "a".repeat(260);
        assert_eq!(path_length_level(&path, false).0, PreflightLevel::Block);
        assert_eq!(path_length_level(&path, true).0, PreflightLevel::Pass);

        let input = PreflightInput {
            paths: vec![check(&path)],
            ..Default::default()
        };
        let report = run_preflight(&input);
        let long = find(&report, "long-path");
        assert_eq!(long.len(), 1);
        assert_eq!(long[0].level, PreflightLevel::Block);
        assert!(
            !report.task_blocked,
            "entry blocks do not block the whole task"
        );
    }

    #[test]
    fn protected_literal_and_canonical_hits_block_the_entry() {
        let direct = PathCheck {
            long_path_supported: true,
            ..check("/protected/file")
        };
        let linked = PathCheck {
            canonical_path: Some("/real/protected/file".to_string()),
            long_path_supported: true,
            ..check("/link/file")
        };
        let input = PreflightInput {
            paths: vec![direct, linked],
            protected_dirs: vec![ProtectedDir::with_canonical(
                "/protected",
                Some("/real/protected".to_string()),
            )],
            ..Default::default()
        };
        let report = run_preflight(&input);
        let hits = find(&report, "protected");
        assert!(hits.iter().any(
            |item| item.level == PreflightLevel::Block && item.code.as_deref() == Some("E2009")
        ));
        assert!(!report.task_blocked);
    }

    #[test]
    fn readonly_locked_and_placeholder_are_warnings_not_task_blocks() {
        let target = PathCheck {
            readonly: true,
            locked: true,
            placeholder: true,
            ..check("/data/a.txt")
        };
        let input = PreflightInput {
            paths: vec![target],
            ..Default::default()
        };
        let report = run_preflight(&input);
        for kind in ["readonly", "locked", "placeholder"] {
            let items = find(&report, kind);
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].level, PreflightLevel::Warn);
        }
        assert_eq!(report.level, PreflightLevel::Warn);
        assert!(!report.task_blocked);
    }

    #[test]
    fn unc_timeout_and_unreachable_warn_but_do_not_block() {
        let input = PreflightInput {
            unc: vec![
                UncCheck {
                    path: r"\\server\share".to_string(),
                    reachable: None,
                },
                UncCheck {
                    path: r"\\server\share2".to_string(),
                    reachable: Some(false),
                },
                UncCheck {
                    path: r"\\server\share3".to_string(),
                    reachable: Some(true),
                },
            ],
            ..Default::default()
        };
        let report = run_preflight(&input);
        let unc = find(&report, "unc");
        assert_eq!(unc.len(), 3);
        assert_eq!(
            unc.iter()
                .filter(|item| item.level == PreflightLevel::Warn)
                .count(),
            2
        );
        assert_eq!(
            unc.iter()
                .filter(|item| item.level == PreflightLevel::Pass)
                .count(),
            1
        );
        assert!(!report.task_blocked);
        assert_eq!(report.level, PreflightLevel::Warn);
    }

    #[test]
    fn empty_input_reports_each_category_as_pass() {
        let report = run_preflight(&PreflightInput::default());
        assert_eq!(report.level, PreflightLevel::Pass);
        assert!(!report.task_blocked);
        for kind in [
            "path",
            "protected",
            "readonly",
            "locked",
            "placeholder",
            "long-path",
            "unc",
        ] {
            assert_eq!(find(&report, kind).len(), 1, "category {kind}");
        }
    }

    #[test]
    fn invalid_path_blocks_that_entry() {
        let target = PathCheck {
            long_path_supported: true,
            ..check("/../escape")
        };
        let input = PreflightInput {
            paths: vec![target],
            ..Default::default()
        };
        let report = run_preflight(&input);
        let items = find(&report, "path");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].level, PreflightLevel::Block);
        assert_eq!(items[0].code.as_deref(), Some("E1003"));
        assert!(!report.task_blocked);
    }
}
