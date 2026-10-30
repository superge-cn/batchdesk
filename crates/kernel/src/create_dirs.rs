//!
//! 纯函数层：F11 按清单批量创建文件夹的计划生成；本模块不读盘、不取时钟。
//!
//! 口径（方案 §5.15、§5.8）：
//! - 清单行只接受相对路径；绝对路径、`..` 逃逸、非法/保留名按无效行集中报告；
//! - 每条有效路径按层级展开为 `create-dir` 条目，父目录先于子目录；
//! - 目标已存在的条目按 `existing-target` + skip 处理；受保护目标按 E2009 阻断；
//! - 空行忽略；重复路径只创建一次并告警；
//! - apply 逐条创建单级目录，撤销按 Journal 逆序删除空目录。

use std::collections::BTreeSet;

use crate::conflict::validate_target_name;
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::ProtectedDir;
use crate::plan::{
    is_ulid, PlanAction, PlanConflict, PlanDocument, PlanEntry, PlanParamValue, PlanResolution,
    PlanStats, PlanWarning, RiskLevel,
};

pub const F11_KIND: &str = "f11";

/// 清单中的一行；`line` 用于无效行报告。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanF11Row {
    pub line: u64,
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct PlanF11Input {
    pub plan_id: String,
    pub root: String,
    pub canonical_root: Option<String>,
    pub rows: Vec<PlanF11Row>,
    /// 壳层探测到的现存目标路径（原样字符串；计划只做比较标记）。
    pub existing_targets: Vec<String>,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
}

/// 生成 F11 计划：每条有效清单路径按其层级展开为 `create-dir` 条目。
pub fn plan_f11(input: PlanF11Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.root.trim().is_empty() {
        return Err(KernelError::missing_param("root is required"));
    }

    let existing: BTreeSet<String> = input
        .existing_targets
        .iter()
        .map(|path| conflict_key(path, input.case_sense))
        .collect();
    let mut invalid_rows = Vec::new();
    let mut duplicate_rows = Vec::new();
    let mut valid_paths: BTreeSet<String> = BTreeSet::new();
    let mut valid_rows = 0u64;
    let mut entries = Vec::new();
    let mut planned: BTreeSet<String> = BTreeSet::new();

    for row in &input.rows {
        let raw = row.path.trim().trim_start_matches('\u{feff}');
        if raw.is_empty() {
            continue;
        }
        let normalized = match normalize_list_path(raw) {
            Ok(path) => path,
            Err(reason) => {
                invalid_rows.push(format!("line {}: {reason}", row.line));
                continue;
            }
        };
        let key = conflict_key(&normalized, input.case_sense);
        if !valid_paths.insert(key) {
            duplicate_rows.push(format!("line {}: {}", row.line, normalized));
            continue;
        }
        valid_rows += 1;
        let components: Vec<&str> = normalized.split('/').collect();
        let mut prefix = String::new();
        for component in components {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            let target = target_for_list_path(&input.root, &prefix);
            let target_key = conflict_key(&target, input.case_sense);
            if !planned.insert(target_key.clone()) {
                continue;
            }
            let conflict = existing.contains(&target_key).then(|| PlanConflict {
                kind: "existing-target".to_string(),
                risk_level: RiskLevel::Danger,
            });
            let target_block =
                crate::csv_merge::target_block(&target, &input.protected_dirs, input.case_sense);
            entries.push(PlanEntry {
                entry_id: format!("{}-{:04}", input.plan_id, entries.len() + 1),
                source: String::new(),
                target: Some(target.clone()),
                action: PlanAction::CreateDir,
                relation: None,
                reason: format!("create directory {target}"),
                conflict: conflict.clone(),
                resolution: conflict.as_ref().map(|_| PlanResolution::Skip),
                blocked: target_block,
                expected: None,
                range: None,
                merge: None,
                csv: None,
                csv_merge: None,
                csv_extract: None,
                csv_rows: None,
            });
        }
    }

    let mut warnings = Vec::new();
    if !invalid_rows.is_empty() {
        warnings.push(PlanWarning {
            kind: "invalid-rows".to_string(),
            suggestion: Some(invalid_rows.join("; ")),
            risk_level: RiskLevel::Caution,
        });
    }
    if !duplicate_rows.is_empty() {
        warnings.push(PlanWarning {
            kind: "duplicate-rows".to_string(),
            suggestion: Some(format!(
                "duplicate paths are created once: {}",
                duplicate_rows.join("; ")
            )),
            risk_level: RiskLevel::Caution,
        });
    }
    if entries.is_empty() {
        warnings.push(PlanWarning {
            kind: "empty-list".to_string(),
            suggestion: Some("the list contains no valid directory rows".to_string()),
            risk_level: RiskLevel::Safe,
        });
    }

    let mut params = std::collections::BTreeMap::new();
    params.insert(
        "root".to_string(),
        PlanParamValue::String(input.root.clone()),
    );
    params.insert(
        "rowCount".to_string(),
        PlanParamValue::Number(input.rows.len() as u64),
    );
    params.insert("validRows".to_string(), PlanParamValue::Number(valid_rows));
    params.insert(
        "targetCount".to_string(),
        PlanParamValue::Number(entries.len() as u64),
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
        kind: F11_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

/// 规范化清单相对路径；拒绝绝对路径、`..` 逃逸和非法/保留名。
pub fn normalize_list_path(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_start_matches('\u{feff}');
    if trimmed.is_empty() {
        return Err("empty path".to_string());
    }
    if looks_absolute(trimmed) {
        return Err("absolute paths are not allowed".to_string());
    }
    let normalized = crate::path::try_normalize(trimmed).map_err(|error| error.message)?;
    if normalized == "." || normalized.is_empty() {
        return Err("empty path".to_string());
    }
    if normalized
        .split('/')
        .any(|component| component == ".." || component.is_empty())
    {
        return Err("path escapes the target root".to_string());
    }
    let mut problems = Vec::new();
    for component in normalized.split('/') {
        for warning in validate_target_name(component) {
            problems.push(format!("{component}: {}", warning.kind));
        }
    }
    if !problems.is_empty() {
        return Err(problems.join(", "));
    }
    Ok(normalized)
}

fn looks_absolute(path: &str) -> bool {
    let slash = path.replace('\\', "/");
    slash.starts_with('/')
        || slash.starts_with("//")
        || (slash.len() >= 2
            && slash.as_bytes()[1] == b':'
            && slash.as_bytes()[0].is_ascii_alphabetic())
}

/// 把清单相对路径挂到目标根目录下（供计划与壳层探测复用）。
pub fn target_for_list_path(root: &str, relative: &str) -> String {
    let base = root.trim_end_matches(['/', '\\']);
    if base.is_empty() {
        format!("/{relative}")
    } else {
        format!("{base}/{relative}")
    }
}

fn conflict_key(path: &str, sense: CaseSense) -> String {
    let normalized = crate::path::normalize(path);
    match sense {
        CaseSense::Sensitive => normalized,
        CaseSense::CaseInsensitive => normalized.to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    fn row(line: u64, path: &str) -> PlanF11Row {
        PlanF11Row {
            line,
            path: path.to_string(),
        }
    }

    fn input(rows: Vec<PlanF11Row>) -> PlanF11Input {
        PlanF11Input {
            plan_id: PLAN_ID.to_string(),
            root: "/data/out".to_string(),
            canonical_root: None,
            rows,
            existing_targets: Vec::new(),
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
        }
    }

    #[test]
    fn expands_multi_level_paths_parents_first() {
        let plan = plan_f11(input(vec![row(2, "a/b/c")])).unwrap();
        assert_eq!(plan.kind, F11_KIND);
        assert_eq!(plan.entries.len(), 3);
        assert_eq!(plan.entries[0].target.as_deref(), Some("/data/out/a"));
        assert_eq!(plan.entries[1].target.as_deref(), Some("/data/out/a/b"));
        assert_eq!(plan.entries[2].target.as_deref(), Some("/data/out/a/b/c"));
        assert_eq!(plan.entries[0].action, PlanAction::CreateDir);
    }

    #[test]
    fn duplicate_rows_and_invalid_rows_are_reported() {
        let plan = plan_f11(input(vec![
            row(2, "a"),
            row(3, "a"),
            row(4, "../escape"),
            row(5, "/absolute"),
            row(6, "CON"),
        ]))
        .unwrap();
        assert_eq!(plan.entries.len(), 1);
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "invalid-rows"));
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "duplicate-rows"));
    }

    #[test]
    fn existing_target_is_a_skip_conflict() {
        let plan = plan_f11(PlanF11Input {
            existing_targets: vec!["/data/out/a/b".to_string()],
            ..input(vec![row(2, "a/b")])
        })
        .unwrap();
        let entry = plan
            .entries
            .iter()
            .find(|entry| entry.target.as_deref() == Some("/data/out/a/b"))
            .unwrap();
        assert_eq!(entry.conflict.as_ref().unwrap().kind, "existing-target");
        assert_eq!(entry.resolution, Some(PlanResolution::Skip));
    }

    #[test]
    fn protected_target_is_blocked() {
        let plan = plan_f11(PlanF11Input {
            protected_dirs: vec![ProtectedDir::new("/data/out/secret")],
            ..input(vec![row(2, "secret/a")])
        })
        .unwrap();
        assert!(plan.entries.iter().any(|entry| entry
            .blocked
            .as_ref()
            .map(|blocked| blocked.code.as_str())
            == Some("E2009")));
    }

    #[test]
    fn empty_or_only_invalid_rows_produce_an_empty_plan() {
        let plan = plan_f11(input(vec![row(1, ""), row(2, "../x")])).unwrap();
        assert!(plan.entries.is_empty());
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "empty-list"));
    }

    #[test]
    fn normalize_rejects_absolute_and_escape_paths() {
        assert!(normalize_list_path("../x").is_err());
        assert!(normalize_list_path("/x").is_err());
        assert!(normalize_list_path("C:/x").is_err());
        assert_eq!(normalize_list_path("a/./b").unwrap(), "a/b");
    }
}
