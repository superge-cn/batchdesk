//!
//! C04 名称查找替换计划（方案 §5.15；复用 F12 的预览与执行引擎）。
//!
//! 把"规则"换成字符串替换：plain 模式按字面（非重叠左到右），regex 模式与 F13
//! 同引擎（捕获组引用校验同规则）；冲突扫描/保留名/长路径分级/两阶段编排全部继承
//! `plan_rename`。本模块不读盘；壳层提供目录项与现存目标。

use std::collections::BTreeMap;

use crate::regex_rename::{apply_regex_rule, compile_regex, validate_replacement};
use crate::rename::{plan_rename, PlanF12Item, RenamePlanContext, RenameScope};
use crate::{error::KernelError, plan::PlanDocument, plan::PlanParamValue};

pub const C04_KIND: &str = "c04";

/// 替换模式（方案 §5.15 C04 功能表：支持普通与正则）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum C04Mode {
    Plain,
    Regex,
}

impl C04Mode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "plain" => Some(Self::Plain),
            "regex" => Some(Self::Regex),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Regex => "regex",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlanC04Rule {
    pub mode: C04Mode,
    /// plain 模式为字面查找文本；regex 模式为正则模式串。
    pub find: String,
    pub replace: String,
    pub scope: RenameScope,
}

#[derive(Clone, Debug)]
pub struct PlanC04Input {
    pub plan_id: String,
    pub items: Vec<PlanF12Item>,
    /// 壳层探测到的现存目录项；计划会排除本批次将移走的源路径。
    pub existing_targets: Vec<String>,
    pub protected_dirs: Vec<crate::path::ProtectedDir>,
    pub case_sense: crate::fs::CaseSense,
    /// 壳层探测到的系统长路径支持开关（方案 §5.13）；计划层不读注册表。
    pub long_path_supported: bool,
    pub rule: PlanC04Rule,
}

/// 生成 C04 计划；查找文本为空/非法正则/替换模板引用缺失在计划期返回 E1002。
pub fn plan_c04(input: PlanC04Input) -> Result<PlanDocument, KernelError> {
    if input.rule.find.is_empty() {
        return Err(KernelError::invalid_params("find text must not be empty"));
    }
    let PlanC04Input {
        plan_id,
        items,
        existing_targets,
        protected_dirs,
        case_sense,
        long_path_supported,
        rule,
    } = input;
    let mut params = BTreeMap::new();
    params.insert(
        "mode".to_string(),
        PlanParamValue::String(rule.mode.as_str().to_string()),
    );
    params.insert(
        "scope".to_string(),
        PlanParamValue::String(rule.scope.as_str().to_string()),
    );
    params.insert(
        "find".to_string(),
        PlanParamValue::String(rule.find.clone()),
    );
    params.insert(
        "replace".to_string(),
        PlanParamValue::String(rule.replace.clone()),
    );
    match rule.mode {
        C04Mode::Plain => plan_rename(
            RenamePlanContext {
                plan_id,
                items,
                existing_targets,
                protected_dirs,
                case_sense,
                long_path_supported,
            },
            C04_KIND,
            params,
            |name| {
                crate::rename::transform_name(name, rule.scope, |segment| {
                    segment.replace(&rule.find, &rule.replace)
                })
            },
        ),
        C04Mode::Regex => {
            let regex = compile_regex(&rule.find)?;
            validate_replacement(&rule.replace, &regex)?;
            plan_rename(
                RenamePlanContext {
                    plan_id,
                    items,
                    existing_targets,
                    protected_dirs,
                    case_sense,
                    long_path_supported,
                },
                C04_KIND,
                params,
                |name| apply_regex_rule(name, &regex, &rule.replace, rule.scope),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::CaseSense;
    use crate::path::ProtectedDir;
    use crate::plan::{PlanAction, PlanEntry};

    fn item(path: &str, name: &str) -> PlanF12Item {
        PlanF12Item {
            path: path.to_string(),
            name: name.to_string(),
        }
    }

    fn input(rule: PlanC04Rule, items: Vec<PlanF12Item>) -> PlanC04Input {
        PlanC04Input {
            plan_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            items,
            existing_targets: Vec::new(),
            protected_dirs: Vec::<ProtectedDir>::new(),
            case_sense: CaseSense::Sensitive,
            long_path_supported: true,
            rule,
        }
    }

    fn plain(find: &str, replace: &str, scope: RenameScope) -> PlanC04Rule {
        PlanC04Rule {
            mode: C04Mode::Plain,
            find: find.to_string(),
            replace: replace.to_string(),
            scope,
        }
    }

    fn regex(pattern: &str, replace: &str, scope: RenameScope) -> PlanC04Rule {
        PlanC04Rule {
            mode: C04Mode::Regex,
            find: pattern.to_string(),
            replace: replace.to_string(),
            scope,
        }
    }

    fn rename_targets(plan: &PlanDocument) -> Vec<(String, String)> {
        plan.entries
            .iter()
            .filter(|entry: &&PlanEntry| entry.action == PlanAction::Rename)
            .map(|entry| (entry.source.clone(), entry.target.clone().unwrap()))
            .collect()
    }

    #[test]
    fn plain_replaces_first_level_scoped_segments() {
        let plan = plan_c04(input(
            plain("report", "memo", RenameScope::Full),
            vec![
                item("/data/report-2024.txt", "report-2024.txt"),
                item("/data/notes.txt", "notes.txt"),
            ],
        ))
        .unwrap();
        assert_eq!(plan.kind, C04_KIND);
        let targets = rename_targets(&plan);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].1, "/data/memo-2024.txt");
    }

    #[test]
    fn plain_scope_stem_only_touches_the_stem() {
        let plan = plan_c04(input(
            plain("txt", "md", RenameScope::Stem),
            vec![item("/data/a.txt.bak", "a.txt.bak")],
        ))
        .unwrap();
        // stem = "a.txt"，ext = "bak"：stem 内的 "txt" 被替换、ext 不动。
        let targets = rename_targets(&plan);
        assert_eq!(targets[0].1, "/data/a.md.bak");
    }

    #[test]
    fn regex_capture_groups_rebuild_the_name() {
        let plan = plan_c04(input(
            regex(
                r"^(\d{4})-(\d{2})-(.*)$",
                r"${2}_${1}_${3}",
                RenameScope::Stem,
            ),
            vec![item("/data/2024-05-report.txt", "2024-05-report.txt")],
        ))
        .unwrap();
        let targets = rename_targets(&plan);
        assert_eq!(targets[0].1, "/data/05_2024_report.txt");
    }

    #[test]
    fn empty_find_is_rejected_in_both_modes() {
        let error = plan_c04(input(plain("", "x", RenameScope::Full), Vec::new())).unwrap_err();
        assert_eq!(error.code, "E1002");
        let error = plan_c04(input(regex("", "x", RenameScope::Full), Vec::new())).unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn invalid_regex_and_missing_capture_are_rejected() {
        let error = plan_c04(input(regex("([a", "x", RenameScope::Full), Vec::new())).unwrap_err();
        assert_eq!(error.code, "E1002");
        let error = plan_c04(input(
            regex("^(\\d+)$", "$2", RenameScope::Full),
            Vec::new(),
        ))
        .unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn no_match_produces_no_entries() {
        let plan = plan_c04(input(
            plain("zzz", "x", RenameScope::Full),
            vec![item("/data/a.txt", "a.txt")],
        ))
        .unwrap();
        assert!(plan.entries.is_empty());
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "no-change"));
    }
}
