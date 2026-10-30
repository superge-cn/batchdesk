//!
//! 纯函数层：F12 批量重命名计划（方案 §5.13）。
//!
//! 规则集为本切片的基础规则：前缀、后缀、字面查找替换，作用范围
//! `full`/`stem`/`ext`。计划期完成名称合法性、批内/现存冲突扫描；
//! 有环或仅大小写变更时展开为“临时名→目标名”两阶段条目，无环场景按拓扑序直接改名。
//! 本模块不读盘、不取时钟。

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use crate::conflict::{scan_conflicts, validate_target_name, ConflictKind};
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::ProtectedDir;
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanConflict, PlanDocument, PlanEntry, PlanParamValue,
    PlanResolution, PlanStats, PlanWarning, RiskLevel,
};

pub const F12_KIND: &str = "f12";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameScope {
    Full,
    Stem,
    Ext,
}

impl RenameScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Stem => "stem",
            Self::Ext => "ext",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "full" => Some(Self::Full),
            "stem" => Some(Self::Stem),
            "ext" => Some(Self::Ext),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlanF12Rule {
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub find: Option<String>,
    pub replace: String,
    pub scope: RenameScope,
}

impl PlanF12Rule {
    fn validate(&self) -> Result<(), KernelError> {
        if self.find.as_deref() == Some("") {
            return Err(KernelError::invalid_params("find text must not be empty"));
        }
        if self.prefix.as_deref() == Some("") {
            return Err(KernelError::invalid_params("prefix must not be empty"));
        }
        if self.suffix.as_deref() == Some("") {
            return Err(KernelError::invalid_params("suffix must not be empty"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct PlanF12Item {
    pub path: String,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct PlanF12Input {
    pub plan_id: String,
    pub items: Vec<PlanF12Item>,
    /// 壳层探测到的现存目录项；计划会排除本批次将移走的源路径。
    pub existing_targets: Vec<String>,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    /// 壳层探测到的系统长路径支持开关（方案 §5.13）；计划层不读注册表。
    pub long_path_supported: bool,
    pub rule: PlanF12Rule,
}

#[derive(Clone, Debug)]
struct Mapping {
    source: String,
    target: String,
    source_key: String,
    target_key: String,
}

/// 生成 F12 计划；每个输入最多一条目标条目，必要时追加两阶段临时条目。
pub fn plan_f12(input: PlanF12Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    input.rule.validate()?;
    if input.items.is_empty() {
        return Err(KernelError::missing_param(
            "rename plans require at least one item",
        ));
    }

    let mut entries = Vec::new();
    let mut mappings = Vec::new();
    let mut blocked_items = Vec::new();
    let mut no_change = Vec::new();
    let mut long_path_caution = Vec::new();
    let mut long_path_skips = Vec::new();

    for item in &input.items {
        let source_key = conflict_key(&item.path, input.case_sense);
        let target_name = match apply_rule(&item.name, &input.rule) {
            Ok(name) => name,
            Err(reason) => {
                blocked_items.push((item.clone(), reason));
                continue;
            }
        };
        if target_name == item.name {
            no_change.push(item.path.clone());
            continue;
        }
        let target = target_path(&item.path, &target_name);
        let target_key = conflict_key(&target, input.case_sense);
        let target_warnings = validate_target_name(&target_name);
        let mut blocked = None;
        if let Some(warning) = target_warnings.first() {
            blocked = Some(PlanBlocked {
                code: "E1002".to_string(),
                reason: format!("target name is not allowed: {}", warning.kind),
                hint: warning.suggestion.clone(),
            });
        }
        if blocked.is_none() {
            blocked = crate::csv_merge::source_block(
                &item.path,
                None,
                &input.protected_dirs,
                input.case_sense,
            )
            .or_else(|| {
                crate::csv_merge::target_block(&target, &input.protected_dirs, input.case_sense)
            });
        }
        if let Some(blocked) = blocked {
            entries.push(blocked_entry(
                &input.plan_id,
                item,
                &target,
                blocked,
                entries.len(),
            ));
            continue;
        }
        match crate::preflight::path_length_level(&target, input.long_path_supported).0 {
            crate::preflight::PreflightLevel::Pass => {}
            crate::preflight::PreflightLevel::Warn => {
                long_path_caution.push(target.clone());
            }
            crate::preflight::PreflightLevel::Block => {
                let mut entry = plain_entry(
                    &input.plan_id,
                    item,
                    &target,
                    entries.len(),
                    format!(
                        "target path exceeds {} characters and long-path support is disabled",
                        crate::preflight::PATH_WARN_MAX_CHARS
                    ),
                );
                entry.resolution = Some(PlanResolution::Skip);
                entries.push(entry);
                long_path_skips.push(target.clone());
                continue;
            }
        }
        mappings.push(Mapping {
            source: item.path.clone(),
            target,
            source_key,
            target_key,
        });
    }

    // 本批次将移走的源路径不算外部占用；其余现存项参与冲突扫描。
    let moving_sources: BTreeSet<String> = mappings.iter().map(|m| m.source_key.clone()).collect();
    let mut existing_effective: BTreeSet<String> = input
        .existing_targets
        .iter()
        .map(|path| crate::path::normalize(path))
        .collect();
    existing_effective
        .retain(|path| !moving_sources.contains(&conflict_key(path, input.case_sense)));
    for item in &input.items {
        let key = conflict_key(&item.path, input.case_sense);
        if !moving_sources.contains(&key) {
            existing_effective.insert(crate::path::normalize(&item.path));
        }
    }
    let pairs: Vec<(String, String)> = mappings
        .iter()
        .map(|mapping| {
            (
                crate::path::normalize(&mapping.source),
                crate::path::normalize(&mapping.target),
            )
        })
        .collect();
    let scan = scan_conflicts(&pairs, &existing_effective, input.case_sense);

    let mut valid = Vec::new();
    for (index, mapping) in mappings.into_iter().enumerate() {
        if let Some(kind) = scan.conflicts.get(&index).copied() {
            let conflict = PlanConflict {
                kind: conflict_label(kind).to_string(),
                risk_level: kind.risk_level(),
            };
            let item = PlanF12Item {
                path: mapping.source.clone(),
                name: crate::conflict::file_name_of(&mapping.source).to_string(),
            };
            let mut entry = plain_entry(
                &input.plan_id,
                &item,
                &mapping.target,
                entries.len(),
                format!("rename target conflicts: {}", conflict.kind),
            );
            entry.conflict = Some(conflict.clone());
            entry.resolution = Some(PlanResolution::Skip);
            entries.push(entry);
            continue;
        }
        valid.push(mapping);
    }

    let two_phase = needs_two_phase(&valid, input.case_sense);
    if two_phase {
        append_two_phase(&input.plan_id, &valid, &existing_effective, &mut entries);
    } else {
        for index in topological_order(&valid, input.case_sense) {
            let mapping = &valid[index];
            let item = PlanF12Item {
                path: mapping.source.clone(),
                name: crate::conflict::file_name_of(&mapping.source).to_string(),
            };
            entries.push(plain_entry(
                &input.plan_id,
                &item,
                &mapping.target,
                entries.len(),
                "rename to target".to_string(),
            ));
        }
    }

    for (item, reason) in blocked_items {
        let target = target_path(&item.path, &item.name);
        entries.push(blocked_entry(
            &input.plan_id,
            &item,
            &target,
            PlanBlocked {
                code: "E1002".to_string(),
                reason,
                hint: None,
            },
            entries.len(),
        ));
    }

    let mut warnings = Vec::new();
    if !no_change.is_empty() {
        warnings.push(PlanWarning {
            kind: "no-change".to_string(),
            suggestion: Some(format!(
                "rule leaves these names unchanged: {}",
                no_change.join(", ")
            )),
            risk_level: RiskLevel::Safe,
        });
    }
    if !long_path_caution.is_empty() {
        warnings.push(PlanWarning {
            kind: "long-path-caution".to_string(),
            suggestion: Some(format!(
                "target paths are in the {}-{} character caution range: {}",
                crate::preflight::PATH_SAFE_MAX_CHARS + 1,
                crate::preflight::PATH_WARN_MAX_CHARS,
                long_path_caution.join(", ")
            )),
            risk_level: RiskLevel::Caution,
        });
    }
    if !long_path_skips.is_empty() {
        warnings.push(PlanWarning {
            kind: "long-path-disabled".to_string(),
            suggestion: Some(format!(
                "target paths exceed {} characters and long-path support is disabled; \
                 enable LongPathsEnabled or shorten the target path: {}",
                crate::preflight::PATH_WARN_MAX_CHARS,
                long_path_skips.join(", ")
            )),
            risk_level: RiskLevel::Danger,
        });
    }
    if entries.iter().any(|entry| entry.blocked.is_some()) {
        warnings.push(PlanWarning {
            kind: "invalid-target-names".to_string(),
            suggestion: Some(
                "invalid target names are blocked; review the blocked entries".to_string(),
            ),
            risk_level: RiskLevel::Danger,
        });
    }
    if two_phase {
        warnings.push(PlanWarning {
            kind: "two-phase-rename".to_string(),
            suggestion: Some(
                "cycle or case-only rename uses temporary names; preview shows both phases"
                    .to_string(),
            ),
            risk_level: RiskLevel::Caution,
        });
    }

    let mut params = BTreeMap::new();
    params.insert(
        "scope".to_string(),
        PlanParamValue::String(input.rule.scope.as_str().to_string()),
    );
    params.insert(
        "prefix".to_string(),
        PlanParamValue::String(input.rule.prefix.clone().unwrap_or_default()),
    );
    params.insert(
        "suffix".to_string(),
        PlanParamValue::String(input.rule.suffix.clone().unwrap_or_default()),
    );
    params.insert(
        "find".to_string(),
        PlanParamValue::String(input.rule.find.clone().unwrap_or_default()),
    );
    params.insert(
        "replace".to_string(),
        PlanParamValue::String(input.rule.replace.clone()),
    );
    params.insert(
        "itemCount".to_string(),
        PlanParamValue::Number(input.items.len() as u64),
    );
    params.insert(
        "validCount".to_string(),
        PlanParamValue::Number(valid.len() as u64),
    );
    params.insert("twoPhase".to_string(), PlanParamValue::Bool(two_phase));
    params.insert(
        "longPathSupported".to_string(),
        PlanParamValue::Bool(input.long_path_supported),
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
        kind: F12_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

/// 应用基础规则；返回新名称或不可执行原因。
pub fn apply_rule(name: &str, rule: &PlanF12Rule) -> Result<String, String> {
    if name.is_empty() {
        return Err("source name is empty".to_string());
    }
    let (stem, extension) = split_extension(name);
    let segment = match rule.scope {
        RenameScope::Full => name.to_string(),
        RenameScope::Stem => stem.to_string(),
        RenameScope::Ext => extension.unwrap_or("").to_string(),
    };
    if rule.scope == RenameScope::Ext && extension.is_none() {
        return Ok(name.to_string());
    }
    let mut changed = match &rule.find {
        Some(find) => segment.replace(find, &rule.replace),
        None => segment,
    };
    if let Some(prefix) = &rule.prefix {
        changed = format!("{prefix}{changed}");
    }
    if let Some(suffix) = &rule.suffix {
        changed.push_str(suffix);
    }
    let result = match rule.scope {
        RenameScope::Full => changed,
        RenameScope::Stem => match extension {
            Some(extension) => format!("{changed}.{extension}"),
            None => changed,
        },
        RenameScope::Ext => format!("{stem}.{changed}"),
    };
    if result.contains('/') || result.contains('\\') {
        return Err("target name must not contain path separators".to_string());
    }
    if result == "." || result == ".." {
        return Err("target name must not be . or ..".to_string());
    }
    Ok(result)
}

fn split_extension(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (name, None),
    }
}

fn target_path(source: &str, name: &str) -> String {
    match source.rfind(['/', '\\']) {
        Some(index) => format!("{}{name}", &source[..=index]),
        None => name.to_string(),
    }
}

fn plain_entry(
    plan_id: &str,
    item: &PlanF12Item,
    target: &str,
    index: usize,
    reason: String,
) -> PlanEntry {
    PlanEntry {
        entry_id: format!("{plan_id}-{:04}", index + 1),
        source: item.path.clone(),
        target: Some(target.to_string()),
        action: PlanAction::Rename,
        relation: None,
        reason,
        conflict: None,
        resolution: None,
        blocked: None,
        expected: None,
        range: None,
        merge: None,
        csv: None,
        csv_merge: None,
        csv_extract: None,
        csv_rows: None,
    }
}

fn blocked_entry(
    plan_id: &str,
    item: &PlanF12Item,
    target: &str,
    blocked: PlanBlocked,
    index: usize,
) -> PlanEntry {
    let mut entry = plain_entry(
        plan_id,
        item,
        target,
        index,
        format!("rename blocked: {}", blocked.reason),
    );
    entry.blocked = Some(blocked);
    entry
}

fn conflict_label(kind: ConflictKind) -> &'static str {
    match kind {
        ConflictKind::Existing => "existing-target",
        ConflictKind::WithinBatch => "within-batch",
        ConflictKind::CaseOnly => "case-only",
    }
}

fn conflict_key(path: &str, sense: CaseSense) -> String {
    let normalized = crate::path::normalize(path);
    match sense {
        CaseSense::Sensitive => normalized,
        CaseSense::CaseInsensitive => normalized.to_lowercase(),
    }
}

fn needs_two_phase(mappings: &[Mapping], sense: CaseSense) -> bool {
    mappings
        .iter()
        .any(|mapping| mapping.target_key == mapping.source_key && mapping.target != mapping.source)
        || topological_order(mappings, sense).len() != mappings.len()
}

fn topological_order(mappings: &[Mapping], _sense: CaseSense) -> Vec<usize> {
    let mut index_by_source: HashMap<&str, usize> = HashMap::new();
    for (index, mapping) in mappings.iter().enumerate() {
        index_by_source.insert(mapping.source_key.as_str(), index);
    }
    let mut indegree = vec![0usize; mappings.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); mappings.len()];
    for (index, mapping) in mappings.iter().enumerate() {
        if let Some(&dependency) = index_by_source.get(mapping.target_key.as_str()) {
            if dependency != index {
                indegree[index] += 1;
                dependents[dependency].push(index);
            }
        }
    }
    let mut queue: VecDeque<usize> = indegree
        .iter()
        .enumerate()
        .filter(|(_, degree)| **degree == 0)
        .map(|(index, _)| index)
        .collect();
    let mut order = Vec::with_capacity(mappings.len());
    while let Some(current) = queue.pop_front() {
        order.push(current);
        for dependent in &dependents[current] {
            indegree[*dependent] -= 1;
            if indegree[*dependent] == 0 {
                queue.push_back(*dependent);
            }
        }
    }
    order
}

fn append_two_phase(
    plan_id: &str,
    mappings: &[Mapping],
    existing: &BTreeSet<String>,
    entries: &mut Vec<PlanEntry>,
) {
    let mut occupied = existing.clone();
    for mapping in mappings {
        occupied.insert(mapping.source_key.clone());
        occupied.insert(mapping.target_key.clone());
    }
    let mut temporary = Vec::with_capacity(mappings.len());
    for (index, mapping) in mappings.iter().enumerate() {
        let mut counter = 0u64;
        let temp = loop {
            let name = format!(".batchdesk-rename-{plan_id}-{index}-{counter}");
            let candidate = target_path(&mapping.source, &name);
            let key = crate::path::normalize(&candidate);
            if !occupied.contains(&key) {
                occupied.insert(key);
                break candidate;
            }
            counter += 1;
        };
        temporary.push(temp);
    }
    for (index, mapping) in mappings.iter().enumerate() {
        let item = PlanF12Item {
            path: mapping.source.clone(),
            name: crate::conflict::file_name_of(&mapping.source).to_string(),
        };
        entries.push(plain_entry(
            plan_id,
            &item,
            &temporary[index],
            entries.len(),
            format!("phase 1 temporary rename for {}", mapping.target),
        ));
    }
    for (index, mapping) in mappings.iter().enumerate() {
        let item = PlanF12Item {
            path: temporary[index].clone(),
            name: crate::conflict::file_name_of(&temporary[index]).to_string(),
        };
        entries.push(plain_entry(
            plan_id,
            &item,
            &mapping.target,
            entries.len(),
            "phase 2 rename to target".to_string(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    fn item(name: &str) -> PlanF12Item {
        PlanF12Item {
            path: format!("/data/{name}"),
            name: name.to_string(),
        }
    }

    fn input(items: Vec<PlanF12Item>, rule: PlanF12Rule) -> PlanF12Input {
        PlanF12Input {
            plan_id: PLAN_ID.to_string(),
            items,
            existing_targets: Vec::new(),
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            long_path_supported: true,
            rule,
        }
    }

    fn prefix_rule(prefix: &str) -> PlanF12Rule {
        PlanF12Rule {
            prefix: Some(prefix.to_string()),
            suffix: None,
            find: None,
            replace: String::new(),
            scope: RenameScope::Stem,
        }
    }

    #[test]
    fn prefix_rule_builds_direct_rename_entries() {
        let plan = plan_f12(input(
            vec![item("a.txt"), item("b.txt")],
            prefix_rule("new_"),
        ))
        .unwrap();
        assert_eq!(plan.kind, F12_KIND);
        assert_eq!(plan.entries.len(), 2);
        assert_eq!(plan.entries[0].target.as_deref(), Some("/data/new_a.txt"));
        assert_eq!(plan.params["twoPhase"], PlanParamValue::Bool(false));
    }

    #[test]
    fn existing_target_conflict_is_detected_on_case_insensitive_hosts() {
        let plan = plan_f12(PlanF12Input {
            case_sense: CaseSense::CaseInsensitive,
            existing_targets: vec!["/data/a.txt".to_string(), "/data/x.txt".to_string()],
            ..input(
                vec![item("a.txt"), item("x.txt")],
                PlanF12Rule {
                    prefix: None,
                    suffix: None,
                    find: Some("a".to_string()),
                    replace: "x".to_string(),
                    scope: RenameScope::Full,
                },
            )
        })
        .unwrap();
        assert!(plan.entries.iter().any(|entry| {
            entry
                .conflict
                .as_ref()
                .map(|conflict| conflict.kind.as_str())
                == Some("existing-target")
        }));
    }

    #[test]
    fn windows_style_targets_still_match_existing_conflicts() {
        let plan = plan_f12(PlanF12Input {
            case_sense: CaseSense::CaseInsensitive,
            existing_targets: vec![r"C:\data\a.txt".to_string(), r"C:\data\x.txt".to_string()],
            ..input(
                vec![
                    PlanF12Item {
                        path: r"C:\data\a.txt".to_string(),
                        name: "a.txt".to_string(),
                    },
                    PlanF12Item {
                        path: r"C:\data\x.txt".to_string(),
                        name: "x.txt".to_string(),
                    },
                ],
                PlanF12Rule {
                    prefix: None,
                    suffix: None,
                    find: Some("a".to_string()),
                    replace: "x".to_string(),
                    scope: RenameScope::Full,
                },
            )
        })
        .unwrap();
        assert!(plan.entries.iter().any(|entry| entry
            .conflict
            .as_ref()
            .map(|conflict| conflict.kind.as_str())
            == Some("existing-target")));
    }

    #[test]
    fn source_target_chain_is_not_an_existing_conflict() {
        let plan = plan_f12(input(
            vec![item("a.txt"), item("new_a.txt")],
            prefix_rule("new_"),
        ))
        .unwrap();
        assert_eq!(plan.entries.len(), 2);
        assert!(plan
            .entries
            .iter()
            .all(|entry| entry.conflict.is_none() && entry.blocked.is_none()));
        assert_eq!(plan.entries[0].source, "/data/new_a.txt");
        assert_eq!(plan.entries[1].source, "/data/a.txt");
    }

    #[test]
    fn chain_order_is_topological_and_direct() {
        let mapping = |source: &str, target: &str| Mapping {
            source: source.to_string(),
            target: target.to_string(),
            source_key: source.to_string(),
            target_key: target.to_string(),
        };
        let mappings = vec![mapping("/data/a", "/data/b"), mapping("/data/b", "/data/c")];
        assert!(!needs_two_phase(&mappings, CaseSense::Sensitive));
        assert_eq!(
            topological_order(&mappings, CaseSense::Sensitive),
            vec![1, 0],
            "b must move to c before a moves to b"
        );
    }

    #[test]
    fn cycle_uses_two_phase_temporary_entries() {
        let mapping = |source: &str, target: &str| Mapping {
            source: source.to_string(),
            target: target.to_string(),
            source_key: source.to_string(),
            target_key: target.to_string(),
        };
        let mappings = vec![mapping("/data/a", "/data/b"), mapping("/data/b", "/data/a")];
        assert!(needs_two_phase(&mappings, CaseSense::Sensitive));
        let mut entries = Vec::new();
        append_two_phase(PLAN_ID, &mappings, &BTreeSet::new(), &mut entries);
        assert_eq!(entries.len(), 4);
        assert!(entries[0]
            .target
            .as_deref()
            .unwrap()
            .contains(".batchdesk-rename-"));
        assert_eq!(entries[2].target.as_deref(), Some("/data/b"));
        assert_eq!(entries[3].target.as_deref(), Some("/data/a"));
    }

    #[test]
    fn case_only_rename_uses_two_phase() {
        let plan = plan_f12(PlanF12Input {
            case_sense: CaseSense::CaseInsensitive,
            ..input(
                vec![item("a.txt")],
                PlanF12Rule {
                    prefix: None,
                    suffix: None,
                    find: Some("a".to_string()),
                    replace: "A".to_string(),
                    scope: RenameScope::Full,
                },
            )
        })
        .unwrap();
        assert_eq!(plan.params["twoPhase"], PlanParamValue::Bool(true));
        assert_eq!(plan.entries.len(), 2);
        assert!(plan.entries[0]
            .target
            .as_deref()
            .unwrap()
            .contains(".batchdesk-rename-"));
    }

    #[test]
    fn invalid_names_are_blocked() {
        let plan = plan_f12(input(
            vec![item("a.txt")],
            PlanF12Rule {
                prefix: None,
                suffix: None,
                find: Some("a".to_string()),
                replace: "CON".to_string(),
                scope: RenameScope::Full,
            },
        ))
        .unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E1002");
    }

    #[test]
    fn scope_stem_and_ext_are_applied_separately() {
        let stem = apply_rule(
            "a.txt",
            &PlanF12Rule {
                prefix: Some("x_".to_string()),
                suffix: None,
                find: None,
                replace: String::new(),
                scope: RenameScope::Stem,
            },
        )
        .unwrap();
        assert_eq!(stem, "x_a.txt");
        let ext = apply_rule(
            "a.txt",
            &PlanF12Rule {
                prefix: Some("x".to_string()),
                suffix: None,
                find: None,
                replace: String::new(),
                scope: RenameScope::Ext,
            },
        )
        .unwrap();
        assert_eq!(ext, "a.xtxt");
    }

    fn long_path_case(chars: usize) -> (PlanF12Item, PlanF12Rule) {
        let name_len = chars - "/data/".len();
        let target_name = format!("{}.txt", "a".repeat(name_len - 4));
        (
            PlanF12Item {
                path: "/data/x.txt".to_string(),
                name: "x.txt".to_string(),
            },
            PlanF12Rule {
                prefix: None,
                suffix: None,
                find: Some("x.txt".to_string()),
                replace: target_name,
                scope: RenameScope::Full,
            },
        )
    }

    #[test]
    fn long_path_tiers_warn_and_skip_by_support() {
        let (item200, rule200) = long_path_case(200);
        let plan = plan_f12(input(vec![item200], rule200)).unwrap();
        assert!(!plan
            .warnings
            .iter()
            .any(|warning| warning.kind.starts_with("long-path")));
        assert_eq!(plan.entries[0].resolution, None);
        assert_eq!(
            plan.entries[0].target.as_deref().unwrap().chars().count(),
            200
        );

        let (item259, rule259) = long_path_case(259);
        let plan = plan_f12(input(vec![item259], rule259)).unwrap();
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "long-path-caution"));
        assert_eq!(plan.entries[0].resolution, None);

        let (item260, rule260) = long_path_case(260);
        let plan = plan_f12(PlanF12Input {
            long_path_supported: false,
            ..input(vec![item260.clone()], rule260.clone())
        })
        .unwrap();
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "long-path-disabled"));
        assert_eq!(plan.entries[0].resolution, Some(PlanResolution::Skip));
        assert_eq!(
            plan.params["longPathSupported"],
            PlanParamValue::Bool(false)
        );

        let plan = plan_f12(PlanF12Input {
            long_path_supported: true,
            ..input(vec![item260], rule260)
        })
        .unwrap();
        assert!(!plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "long-path-disabled"));
        assert_eq!(plan.entries[0].resolution, None);
        assert_eq!(plan.params["longPathSupported"], PlanParamValue::Bool(true));
    }
}
