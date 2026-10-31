//!
//! F13 正则重命名计划（方案 §5.5）。
//!
//! 复用 [`crate::rename::plan_rename`] 的冲突扫描、长路径分级与两阶段编排；
//! 本模块只负责正则编译、捕获组替换与替换模板引用校验。纯函数层：不读盘、不取时钟。

use std::collections::BTreeMap;

use regex::Regex;

use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::ProtectedDir;
use crate::plan::{PlanDocument, PlanParamValue};
use crate::rename::{plan_rename, PlanF12Item, RenamePlanContext, RenameScope};

pub const F13_KIND: &str = "f13";

/// F13 复用 F12 的重命名项（路径＋当前名），仅变换器不同。
pub type PlanF13Item = PlanF12Item;

#[derive(Clone, Debug)]
pub struct PlanF13Rule {
    pub pattern: String,
    pub replacement: String,
    pub scope: RenameScope,
}

#[derive(Clone, Debug)]
pub struct PlanF13Input {
    pub plan_id: String,
    pub items: Vec<PlanF13Item>,
    /// 壳层探测到的现存目录项；计划会排除本批次将移走的源路径。
    pub existing_targets: Vec<String>,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    /// 壳层探测到的系统长路径支持开关（方案 §5.13）；计划层不读注册表。
    pub long_path_supported: bool,
    pub rule: PlanF13Rule,
}

/// 生成 F13 计划；非法正则/替换模板引用在计划期直接返回 E1002。
pub fn plan_f13(input: PlanF13Input) -> Result<PlanDocument, KernelError> {
    let regex = compile_regex(&input.rule.pattern)?;
    validate_replacement(&input.rule.replacement, &regex)?;
    let PlanF13Input {
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
        "scope".to_string(),
        PlanParamValue::String(rule.scope.as_str().to_string()),
    );
    params.insert(
        "pattern".to_string(),
        PlanParamValue::String(rule.pattern.clone()),
    );
    params.insert(
        "replacement".to_string(),
        PlanParamValue::String(rule.replacement.clone()),
    );
    plan_rename(
        RenamePlanContext {
            plan_id,
            items,
            existing_targets,
            protected_dirs,
            case_sense,
            long_path_supported,
        },
        F13_KIND,
        params,
        |name| apply_regex_rule(name, &regex, &rule.replacement, rule.scope),
    )
}

/// 对作用范围应用正则替换；无匹配时返回原名（由计划层转为 no-change）。
pub fn apply_regex_rule(
    name: &str,
    regex: &Regex,
    replacement: &str,
    scope: RenameScope,
) -> Result<String, String> {
    if name.is_empty() {
        return Err("source name is empty".to_string());
    }
    crate::rename::transform_name(name, scope, |segment| {
        regex.replace_all(segment, replacement).into_owned()
    })
}

pub(crate) fn compile_regex(pattern: &str) -> Result<Regex, KernelError> {
    if pattern.is_empty() {
        return Err(KernelError::invalid_params(
            "regex pattern must not be empty",
        ));
    }
    Regex::new(pattern)
        .map_err(|error| KernelError::invalid_params(format!("invalid regex pattern: {error}")))
}

enum CaptureRef<'a> {
    Number(usize),
    Named(&'a str),
}

/// 校验替换模板中的 `$N`/`$name`/`${name}` 均指向现有捕获组。
///
/// regex crate 对不存在的组展开为空串（可能静默删掉名字片段），因此计划期
/// 先做引用校验；`$$` 与不构成引用的 `$` 按引擎语义视为字面量。
pub(crate) fn validate_replacement(replacement: &str, regex: &Regex) -> Result<(), KernelError> {
    let mut rest = replacement;
    while let Some(index) = rest.find('$') {
        rest = &rest[index..];
        if rest.as_bytes().get(1) == Some(&b'$') {
            rest = &rest[2..];
            continue;
        }
        match find_capture_ref(rest) {
            Some((reference, length)) => {
                let valid = match reference {
                    CaptureRef::Number(index) => index < regex.captures_len(),
                    CaptureRef::Named(name) => regex
                        .capture_names()
                        .flatten()
                        .any(|candidate| candidate == name),
                };
                if !valid {
                    return Err(KernelError::invalid_params(format!(
                        "replacement references missing capture group: {}",
                        &rest[..length]
                    )));
                }
                rest = &rest[length..];
            }
            None => {
                rest = &rest[1..];
            }
        }
    }
    Ok(())
}

fn find_capture_ref(input: &str) -> Option<(CaptureRef<'_>, usize)> {
    let bytes = input.as_bytes();
    if bytes.len() <= 1 || bytes[0] != b'$' {
        return None;
    }
    if bytes[1] == b'{' {
        let start = 2;
        let end = input[start..].find('}')? + start;
        let name = &input[start..end];
        return Some((capture_ref(name), end + 1));
    }
    let start = 1;
    let mut end = start;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    if end == start {
        return None;
    }
    Some((capture_ref(&input[start..end]), end))
}

fn capture_ref(name: &str) -> CaptureRef<'_> {
    match name.parse::<usize>() {
        Ok(index) => CaptureRef::Number(index),
        Err(_) => CaptureRef::Named(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    fn item(name: &str) -> PlanF13Item {
        PlanF13Item {
            path: format!("/data/{name}"),
            name: name.to_string(),
        }
    }

    fn input(names: &[&str], pattern: &str, replacement: &str, scope: RenameScope) -> PlanF13Input {
        PlanF13Input {
            plan_id: PLAN_ID.to_string(),
            items: names.iter().map(|name| item(name)).collect(),
            existing_targets: Vec::new(),
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            long_path_supported: true,
            rule: PlanF13Rule {
                pattern: pattern.to_string(),
                replacement: replacement.to_string(),
                scope,
            },
        }
    }

    #[test]
    fn capture_groups_rewrite_full_names() {
        let plan = plan_f13(input(
            &["photo-001.jpg", "photo-002.jpg"],
            r"^photo-(\d+)\.jpg$",
            "img_$1.jpg",
            RenameScope::Full,
        ))
        .unwrap();
        assert_eq!(plan.kind, F13_KIND);
        assert_eq!(plan.entries.len(), 2);
        assert_eq!(plan.entries[0].target.as_deref(), Some("/data/img_001.jpg"));
        assert_eq!(plan.entries[1].target.as_deref(), Some("/data/img_002.jpg"));
        assert_eq!(
            plan.params["pattern"],
            PlanParamValue::String(r"^photo-(\d+)\.jpg$".to_string())
        );
    }

    #[test]
    fn named_captures_apply_to_stem_only() {
        let plan = plan_f13(input(
            &["photo-002.jpg"],
            r"^photo-(?P<num>\d+)$",
            "item_${num}",
            RenameScope::Stem,
        ))
        .unwrap();
        assert_eq!(
            plan.entries[0].target.as_deref(),
            Some("/data/item_002.jpg")
        );
    }

    #[test]
    fn ext_scope_keeps_stem_and_rejects_missing_extension_changes() {
        let plan = plan_f13(input(&["a.txt", "b"], "txt", "md", RenameScope::Ext)).unwrap();
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].target.as_deref(), Some("/data/a.md"));
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "no-change"));
    }

    #[test]
    fn invalid_regex_is_rejected_in_plan() {
        let error = plan_f13(input(&["a.txt"], "(", "x", RenameScope::Full)).unwrap_err();
        assert_eq!(error.code, "E1002");
        assert!(error.message.contains("invalid regex"));
    }

    #[test]
    fn empty_pattern_is_rejected() {
        let error = plan_f13(input(&["a.txt"], "", "x", RenameScope::Full)).unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn missing_capture_reference_is_rejected() {
        let error = plan_f13(input(
            &["a.txt"],
            r"^(\w+)\.txt$",
            "$2.txt",
            RenameScope::Full,
        ))
        .unwrap_err();
        assert_eq!(error.code, "E1002");
        assert!(error.message.contains("missing capture group"));
    }

    #[test]
    fn dollar_escape_is_literal_and_does_not_require_a_group() {
        let plan = plan_f13(input(&["a.txt"], "a", "$$", RenameScope::Stem)).unwrap();
        assert_eq!(plan.entries[0].target.as_deref(), Some("/data/$.txt"));
    }

    #[test]
    fn no_match_does_not_create_entries() {
        let plan = plan_f13(input(&["a.txt"], "zzz", "x", RenameScope::Full)).unwrap();
        assert!(plan.entries.is_empty());
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "no-change"));
    }

    #[test]
    fn reserved_target_names_are_blocked() {
        let plan = plan_f13(input(&["a.txt"], "a", "CON", RenameScope::Stem)).unwrap();
        let blocked = plan.entries[0].blocked.as_ref().unwrap();
        assert_eq!(blocked.code, "E1002");
        assert!(blocked.reason.contains("reserved-name"));
    }

    #[test]
    fn case_only_change_uses_two_phase() {
        let mut input = input(&["a.txt"], "a", "A", RenameScope::Stem);
        input.case_sense = CaseSense::CaseInsensitive;
        let plan = plan_f13(input).unwrap();
        assert_eq!(plan.params["twoPhase"], PlanParamValue::Bool(true));
        assert!(plan.entries[0]
            .target
            .as_deref()
            .unwrap()
            .contains(".batchdesk-rename-"));
    }

    #[test]
    fn pathological_backtracking_pattern_returns_without_hanging() {
        let name = format!("{}.txt", "a".repeat(200));
        let plan = plan_f13(input(&[&name], r"^(a+)+$", "$1", RenameScope::Stem)).unwrap();
        assert!(plan.entries.is_empty());
    }
}
