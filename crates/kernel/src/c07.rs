//! 纯函数层：C07 清单驱动批量移动/复制计划（方案 §5.8）。
//!
//! 口径（ADR-0036）：
//! - 清单两列：`名称或路径`（相对 source_root）＋`目标子目录`（相对 target_root，
//!   允许空＝根）；壳层负责清单的编码感知解析与逐行事实采集（存在性/大小/占用）；
//! - **路径安全**：名称与子目录都必须是相对路径且不含 `..` 段（E1003），
//!   解析后的 source 必须位于 source_root 之内、target 位于 target_root 之内
//!   （`is_under` 双重防线，防止误伤系统目录）；
//! - 缺失项处理：`skip`＝条目标记 E2001 并在执行期跳过；`error`＝整单 E2001 拒绝；
//! - 目标与源相同 → E1002 阻断（无操作语义）；同名目标 → C19 冲突（默认跳过）；
//! - 移动的跨盘校验在执行器（`generic_apply` Move 臂：复制→校验→删源），
//!   Journal 一律记 `Rename`（回滚＝反向移动，跨盘还原由适配层 move 兜底）；
//!   复制的回滚＝目标进隔离目录（既有 Copy reverse）；
//! - 目标子目录不存在时执行期 `create_dir_all` 自动创建（§5.8"按配置自动创建"，
//!   目录创建为实现细节、不单独记 Journal；空目录残留可接受）；
//!   "目标在源目录内部"的自我递归风险在文件粒度退化为 target==source 阻断
//!   （目录级归档属 C08，见 ADR-0036）；
//! - 本模块不读盘、不取时钟（不变量 1）；文件事实由壳层采集。

use crate::csv_merge::{source_block, target_block};
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::{is_under, normalize, try_normalize, ProtectedDir};
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanConflict, PlanDocument, PlanEntry, PlanExpected,
    PlanParamValue, PlanResolution, PlanStats, PlanWarning, RiskLevel,
};

pub const C07_KIND: &str = "c07";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum C07Operation {
    Move,
    Copy,
}

impl C07Operation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Copy => "copy",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingPolicy {
    Skip,
    Error,
}

impl MissingPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Skip => "skip",
            Self::Error => "error",
        }
    }
}

/// 清单行：名称或路径（相对 source_root）＋目标子目录（相对 target_root）。
#[derive(Clone, Debug)]
pub struct PlanC07Row {
    pub name: String,
    pub sub_dir: String,
}

/// 壳层采集的逐行事实（行序与 rows 一致）。
#[derive(Clone, Copy, Debug)]
pub struct PlanC07Fact {
    /// 源文件存在（shell stat）。
    pub exists: bool,
    pub size: u64,
    pub target_occupied: bool,
}

#[derive(Clone, Debug)]
pub struct PlanC07Input {
    pub plan_id: String,
    pub operation: C07Operation,
    pub missing: MissingPolicy,
    pub source_root: String,
    pub target_root: String,
    pub rows: Vec<PlanC07Row>,
    pub facts: Vec<PlanC07Fact>,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub long_path_supported: bool,
}

/// 清单字段的安全检查：拒绝空段、绝对路径与 `..` 逃逸（方案 §5.8"路径安全解析"）。
pub(crate) fn validate_relative_segment(value: &str, kind: &str) -> Result<String, KernelError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(KernelError::new(
            "E1003",
            format!("manifest {kind} must not be empty"),
            None,
            false,
        ));
    }
    if trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return Err(KernelError::new(
            "E1003",
            format!("manifest {kind} must be relative: {trimmed}"),
            None,
            false,
        ));
    }
    let normalized = normalize(trimmed);
    if normalized.split('/').any(|component| component == "..") {
        return Err(KernelError::new(
            "E1003",
            format!("manifest {kind} must not escape its root: {trimmed}"),
            None,
            false,
        ));
    }
    try_normalize(trimmed)?;
    Ok(normalized)
}

fn join_relative(root: &str, relative: &str) -> String {
    let root = root.trim_end_matches(['/', '\\']);
    if relative.is_empty() {
        root.to_string()
    } else {
        format!("{root}/{relative}")
    }
}

/// 生成 C07 计划：每个清单行一条条目（1:1，动作按 operation）。
pub fn plan_c07(input: PlanC07Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.rows.is_empty() {
        return Err(KernelError::missing_param(
            "manifest plans require at least one row",
        ));
    }
    if input.rows.len() != input.facts.len() {
        return Err(KernelError::invalid_params(
            "manifest rows and facts must have the same length",
        ));
    }
    let source_root = try_normalize(&input.source_root)?;
    let target_root = try_normalize(&input.target_root)?;

    let mut entries = Vec::with_capacity(input.rows.len());
    let mut missing = Vec::new();
    for (row, fact) in input.rows.iter().zip(input.facts.iter()) {
        let name = validate_relative_segment(&row.name, "file name")
            .map_err(|error| error_with_row(error, &row.name))?;
        let sub_dir = if row.sub_dir.trim().is_empty() || row.sub_dir.trim() == "." {
            String::new()
        } else {
            validate_relative_segment(&row.sub_dir, "target sub-directory")
                .map_err(|error| error_with_row(error, &row.name))?
        };
        let source = join_relative(&source_root, &name);
        if !is_under(&source, &source_root, input.case_sense) {
            return Err(KernelError::new(
                "E1003",
                format!("manifest path escapes the source root: {name}"),
                Some(row.name.clone()),
                false,
            ));
        }
        let target = join_relative(&target_root, &sub_dir);
        let target = if target.ends_with('/') {
            format!("{target}{}", crate::conflict::file_name_of(&name))
        } else {
            format!("{target}/{}", crate::conflict::file_name_of(&name))
        };
        if !is_under(&target, &target_root, input.case_sense) {
            return Err(KernelError::new(
                "E1003",
                format!("manifest target escapes the target root: {}", row.sub_dir),
                Some(row.name.clone()),
                false,
            ));
        }

        let mut blocked = target_block(&target, &input.protected_dirs, input.case_sense);
        if blocked.is_none() {
            blocked = source_block(&source, None, &input.protected_dirs, input.case_sense);
        }
        if blocked.is_none() && normalize(&source) == normalize(&target) {
            blocked = Some(PlanBlocked {
                code: "E1002".to_string(),
                reason: "target equals source; nothing to move or copy".to_string(),
                hint: Some("check the manifest sub-directory".to_string()),
            });
        }

        let missing_entry = !fact.exists;
        if missing_entry && input.missing == MissingPolicy::Error {
            return Err(KernelError::new(
                "E2001",
                "manifest source is missing (missing policy is error)",
                Some(source.clone()),
                false,
            ));
        }

        let conflict = fact.target_occupied.then(|| PlanConflict {
            kind: "existing-target".to_string(),
            risk_level: RiskLevel::Danger,
        });
        let resolution = if missing_entry {
            Some(PlanResolution::Skip)
        } else {
            conflict.as_ref().map(|_| PlanResolution::Skip)
        };
        let reason = if missing_entry {
            "manifest source is missing; entry will be skipped (E2001)".to_string()
        } else {
            format!(
                "manifest {} {} into {}",
                input.operation.as_str(),
                source,
                target
            )
        };
        let mut entry = PlanEntry {
            entry_id: format!("{}-{:04}", input.plan_id, entries.len() + 1),
            source: source.clone(),
            target: Some(target.clone()),
            action: match input.operation {
                C07Operation::Move => PlanAction::Move,
                C07Operation::Copy => PlanAction::Copy,
            },
            relation: None,
            reason,
            conflict: conflict.clone(),
            resolution,
            blocked: blocked.clone(),
            expected: Some(PlanExpected {
                size: fact.size,
                algorithm: "size".to_string(),
                hash: None,
            }),
            range: None,
            merge: None,
            csv: None,
            csv_merge: None,
            csv_extract: None,
            csv_rows: None,
            csv_dedup: None,
            csv_filter: None,
            csv_columns: None,
            csv_sort: None,
        };
        if missing_entry && blocked.is_none() {
            entry.blocked = Some(PlanBlocked {
                code: "E2001".to_string(),
                reason: "manifest source is missing".to_string(),
                hint: Some("fix the manifest or the source directory".to_string()),
            });
        }
        if crate::preflight::path_length_level(&target, input.long_path_supported).0
            == crate::preflight::PreflightLevel::Warn
        {
            entry.blocked.get_or_insert_with(|| PlanBlocked {
                code: "E1002".to_string(),
                reason: "target path is in the long-path caution range".to_string(),
                hint: Some("shorten the target sub-directory or enable long paths".to_string()),
            });
        }
        if missing_entry {
            missing.push(source);
        }
        entries.push(entry);
    }

    let mut warnings = Vec::new();
    if !missing.is_empty() {
        warnings.push(PlanWarning {
            kind: "manifest-missing".to_string(),
            suggestion: Some(format!(
                "manifest sources are missing and will be skipped: {}",
                missing.join(", ")
            )),
            risk_level: RiskLevel::Caution,
        });
    }
    if entries.iter().any(|entry| entry.conflict.is_some()) {
        warnings.push(PlanWarning {
            kind: "existing-target".to_string(),
            suggestion: Some(
                "occupied targets default to skip; overwrite requires explicit confirmation (方案 §7.7)"
                    .to_string(),
            ),
            risk_level: RiskLevel::Caution,
        });
    }

    let mut params = std::collections::BTreeMap::new();
    params.insert(
        "operation".to_string(),
        PlanParamValue::String(input.operation.as_str().to_string()),
    );
    params.insert(
        "missing".to_string(),
        PlanParamValue::String(input.missing.as_str().to_string()),
    );
    params.insert(
        "rowCount".to_string(),
        PlanParamValue::Number(input.rows.len() as u64),
    );

    let stats = PlanStats {
        total: entries.len(),
        conflict: entries
            .iter()
            .filter(|entry| entry.conflict.is_some())
            .count(),
        warning: warnings.len(),
        blocked: entries
            .iter()
            .filter(|entry| entry.blocked.is_some())
            .count(),
    };
    Ok(PlanDocument {
        plan_id: input.plan_id,
        kind: C07_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

fn error_with_row(mut error: KernelError, row_name: &str) -> KernelError {
    error.detail = Some(match error.detail {
        Some(detail) => format!("{detail} (manifest row: {row_name})"),
        None => format!("manifest row: {row_name}"),
    });
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(exists: bool, occupied: bool) -> PlanC07Fact {
        PlanC07Fact {
            exists,
            size: 10,
            target_occupied: occupied,
        }
    }

    fn input(rows: Vec<PlanC07Row>, facts: Vec<PlanC07Fact>) -> PlanC07Input {
        PlanC07Input {
            plan_id: "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8".to_string(),
            operation: C07Operation::Move,
            missing: MissingPolicy::Skip,
            source_root: "/data/src".to_string(),
            target_root: "/data/dst".to_string(),
            rows,
            facts,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            long_path_supported: true,
        }
    }

    #[test]
    fn plan_resolves_relative_paths_into_targets() {
        let document = plan_c07(input(
            vec![
                PlanC07Row {
                    name: "a.txt".to_string(),
                    sub_dir: "archive/2026".to_string(),
                },
                PlanC07Row {
                    name: "b.txt".to_string(),
                    sub_dir: String::new(),
                },
            ],
            vec![fact(true, false), fact(true, false)],
        ))
        .unwrap();
        let entry = &document.entries[0];
        assert_eq!(entry.action, PlanAction::Move);
        assert_eq!(entry.source, "/data/src/a.txt");
        assert_eq!(
            entry.target.as_deref(),
            Some("/data/dst/archive/2026/a.txt")
        );
        assert_eq!(
            document.entries[1].target.as_deref(),
            Some("/data/dst/b.txt")
        );
        assert_eq!(document.stats.total, 2);
    }

    #[test]
    fn escaping_or_absolute_segments_are_rejected_with_e1003() {
        let error = plan_c07(input(
            vec![PlanC07Row {
                name: "../secret.txt".to_string(),
                sub_dir: "x".to_string(),
            }],
            vec![fact(true, false)],
        ))
        .unwrap_err();
        assert_eq!(error.code, "E1003");

        let error = plan_c07(input(
            vec![PlanC07Row {
                name: "a.txt".to_string(),
                sub_dir: "/etc".to_string(),
            }],
            vec![fact(true, false)],
        ))
        .unwrap_err();
        assert_eq!(error.code, "E1003");

        let error = plan_c07(input(
            vec![PlanC07Row {
                name: "a.txt".to_string(),
                sub_dir: "x/../../y".to_string(),
            }],
            vec![fact(true, false)],
        ))
        .unwrap_err();
        assert_eq!(error.code, "E1003");
    }

    #[test]
    fn missing_sources_skip_or_error_by_policy() {
        let document = plan_c07(input(
            vec![
                PlanC07Row {
                    name: "a.txt".to_string(),
                    sub_dir: "x".to_string(),
                },
                PlanC07Row {
                    name: "gone.txt".to_string(),
                    sub_dir: "x".to_string(),
                },
            ],
            vec![fact(true, false), fact(false, false)],
        ))
        .unwrap();
        assert_eq!(document.entries[1].resolution, Some(PlanResolution::Skip));
        assert_eq!(document.entries[1].blocked.as_ref().unwrap().code, "E2001");
        assert!(document
            .warnings
            .iter()
            .any(|warning| warning.kind == "manifest-missing"));

        let mut error_input = input(
            vec![PlanC07Row {
                name: "gone.txt".to_string(),
                sub_dir: "x".to_string(),
            }],
            vec![fact(false, false)],
        );
        error_input.missing = MissingPolicy::Error;
        let error = plan_c07(error_input).unwrap_err();
        assert_eq!(error.code, "E2001");
    }

    #[test]
    fn occupied_targets_conflict_and_self_targets_are_blocked() {
        let document = plan_c07(input(
            vec![PlanC07Row {
                name: "a.txt".to_string(),
                sub_dir: "x".to_string(),
            }],
            vec![fact(true, true)],
        ))
        .unwrap();
        assert_eq!(
            document.entries[0].conflict.as_ref().unwrap().kind,
            "existing-target"
        );
        assert_eq!(document.entries[0].resolution, Some(PlanResolution::Skip));

        // 自指向：target_root 与 source_root 相同且子目录为空 → target == source。
        let mut self_input = input(
            vec![PlanC07Row {
                name: "same.txt".to_string(),
                sub_dir: String::new(),
            }],
            vec![fact(true, false)],
        );
        self_input.target_root = "/data/src".to_string();
        let error = plan_c07(self_input).unwrap();
        let entry = &error.entries[0];
        assert_eq!(entry.blocked.as_ref().unwrap().code, "E1002");
    }

    #[test]
    fn copy_operation_produces_copy_actions() {
        let mut document_input = input(
            vec![PlanC07Row {
                name: "a.txt".to_string(),
                sub_dir: "x".to_string(),
            }],
            vec![fact(true, false)],
        );
        document_input.operation = C07Operation::Copy;
        let document = plan_c07(document_input).unwrap();
        assert_eq!(document.entries[0].action, PlanAction::Copy);
    }
}
