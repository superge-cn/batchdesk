//! 纯函数层：C08 按规则归档计划（方案 §5.9）。
//!
//! 口径（ADR-0037）：
//! - 规则集为**判定器链**，按声明顺序求值、**首个命中生效**（"全条件求值"模式
//!   待补，见台账）；条件四类：扩展名集合（大小写折叠）、修改日期区间（ISO
//!   日期字典序比较，来源＝文件 mtime）、大小区间（字节，含边界）、关键字
//!   （文件名子串，大小写折叠）；内容关键字匹配默认关闭（性能代价需明示，
//!   本切片未实现，见台账）；
//! - 命中 → `move` 条目（1:1，target=target_root/规则子目录/文件名），执行
//!   复用 C07 的移动引擎（同盘改名/跨盘校验删源/子目录自动创建）与回滚；
//! - 未命中项**默认留在原位**（不进计划），可配落入"未分类"子目录（此时生成
//!   move 条目）；
//! - 高危规则：单规则命中率超过 §5.9 示意阈值 50% 时 Danger 级警告（三重
//!   防护的"高危规则二次确认"由 UI 层承载）；
//! - 路径安全与 C07 同规：规则子目录相对且无 `..` 段（E1003）、target==source
//!   E1002、保护名单 E2009；本模块不读盘、不取时钟（不变量 1），文件事实
//!   （相对路径/大小/修改日期）由壳层采集。

use crate::c07::validate_relative_segment;
use crate::csv_merge::{source_block, target_block};
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::{normalize, try_normalize, ProtectedDir};
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanDocument, PlanEntry, PlanExpected, PlanParamValue,
    PlanStats, PlanWarning, RiskLevel,
};

pub const C08_KIND: &str = "c08";

/// §5.9 示意阈值（"命中超过阈值（如 50%）的高危规则"）。
pub const HIGH_HIT_RATIO: f64 = 0.5;

/// 单条归档规则：条件＋目标子目录（按声明顺序求值，首中生效）。
#[derive(Clone, Debug)]
pub struct ArchiveRule {
    pub sub_dir: String,
    pub condition: ArchiveCondition,
}

/// 条件类型（§5.9 四类；内容关键字未实现，见台账）。
#[derive(Clone, Debug)]
pub enum ArchiveCondition {
    /// 扩展名集合（不含点，大小写折叠比对）。
    Extensions(Vec<String>),
    /// 修改日期区间（`yyyy-mm-dd`，字典序比较；`None`＝该侧不设界）。
    ModifiedDateRange {
        from: Option<String>,
        to: Option<String>,
    },
    /// 大小区间（字节，含边界；`None`＝该侧不设界）。
    SizeRange { min: Option<u64>, max: Option<u64> },
    /// 文件名关键字（大小写折叠子串匹配）。
    NameKeyword(String),
}

/// 未命中项去向（§5.9：默认留原位，可配落入"未分类"目录）。
#[derive(Clone, Debug)]
pub enum UnmatchedPolicy {
    Keep,
    MoveTo(String),
}

/// C08 的单个文件事实（壳层经 `platform::listing::collect_tree_files` 采集）。
#[derive(Clone, Debug)]
pub struct PlanC08File {
    /// 相对 source_root 的路径（正斜杠，来自目录遍历）。
    pub rel_path: String,
    pub size: u64,
    /// 修改日期（`yyyy-mm-dd`，取自 RFC 3339 mtime 前 10 字符）。
    pub modified_date: Option<String>,
    pub exists: bool,
}

#[derive(Clone, Debug)]
pub struct PlanC08Input {
    pub plan_id: String,
    pub source_root: String,
    pub target_root: String,
    pub rules: Vec<ArchiveRule>,
    pub unmatched: UnmatchedPolicy,
    pub files: Vec<PlanC08File>,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub long_path_supported: bool,
}

fn evaluate(condition: &ArchiveCondition, file: &PlanC08File) -> bool {
    match condition {
        ArchiveCondition::Extensions(set) => {
            let name = crate::conflict::file_name_of(&file.rel_path);
            match name.rsplit_once('.') {
                Some((_, extension)) => set.iter().any(|wanted| {
                    wanted
                        .trim_start_matches('.')
                        .eq_ignore_ascii_case(extension)
                }),
                None => false,
            }
        }
        ArchiveCondition::ModifiedDateRange { from, to } => {
            let Some(date) = &file.modified_date else {
                return false;
            };
            from.as_deref().is_none_or(|bound| date.as_str() >= bound)
                && to.as_deref().is_none_or(|bound| date.as_str() <= bound)
        }
        ArchiveCondition::SizeRange { min, max } => {
            min.is_none_or(|bound| file.size >= bound) && max.is_none_or(|bound| file.size <= bound)
        }
        ArchiveCondition::NameKeyword(keyword) => crate::conflict::file_name_of(&file.rel_path)
            .to_lowercase()
            .contains(&keyword.to_lowercase()),
    }
}

/// 生成 C08 计划：命中（与未分类去向）文件各一条 `move` 条目（1:1）。
pub fn plan_c08(input: PlanC08Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.files.is_empty() {
        return Err(KernelError::missing_param(
            "archive plans require at least one file fact",
        ));
    }
    if input.rules.is_empty() {
        return Err(KernelError::missing_param(
            "archive plans require at least one rule",
        ));
    }
    for rule in &input.rules {
        validate_relative_segment(&rule.sub_dir, "rule target sub-directory")?;
    }
    if let UnmatchedPolicy::MoveTo(dir) = &input.unmatched {
        validate_relative_segment(dir, "unclassified sub-directory")?;
    }
    let source_root = try_normalize(&input.source_root)?;
    let target_root = try_normalize(&input.target_root)?;

    // 规则命中统计（§5.9 预览：每条规则命中数与样例条目）。
    let mut hits: Vec<(usize, u64, Vec<String>)> = (0..input.rules.len())
        .map(|index| (index, 0, Vec::new()))
        .collect();
    let mut entries = Vec::new();
    let mut unmatched_kept = 0u64;
    let mut missing = Vec::new();

    for file in &input.files {
        let source = join_root(&source_root, &file.rel_path);
        if !file.exists {
            missing.push(source);
            continue;
        }
        let rule_index = input
            .rules
            .iter()
            .position(|rule| evaluate(&rule.condition, file));
        let sub_dir = match (rule_index, &input.unmatched) {
            (Some(index), _) => {
                let hit = &mut hits[index];
                hit.1 += 1;
                if hit.2.len() < 3 {
                    hit.2.push(file.rel_path.clone());
                }
                input.rules[index].sub_dir.clone()
            }
            (None, UnmatchedPolicy::Keep) => {
                unmatched_kept += 1;
                continue;
            }
            (None, UnmatchedPolicy::MoveTo(dir)) => dir.clone(),
        };
        let name = crate::conflict::file_name_of(&file.rel_path);
        let target = join_root(&join_root(&target_root, &sub_dir), name);

        let mut blocked = target_block(&target, &input.protected_dirs, input.case_sense);
        if blocked.is_none() {
            blocked = source_block(&source, None, &input.protected_dirs, input.case_sense);
        }
        if blocked.is_none() && normalize(&source) == normalize(&target) {
            blocked = Some(PlanBlocked {
                code: "E1002".to_string(),
                reason: "target equals source; nothing to archive".to_string(),
                hint: Some("the file already sits in its rule target".to_string()),
            });
        }
        if blocked.is_none()
            && crate::preflight::path_length_level(&target, input.long_path_supported).0
                == crate::preflight::PreflightLevel::Warn
        {
            blocked = Some(PlanBlocked {
                code: "E1002".to_string(),
                reason: "target path is in the long-path caution range".to_string(),
                hint: Some("shorten the rule sub-directory or enable long paths".to_string()),
            });
        }

        // 同名目标冲突在执行期按 C19 裁决（目标由规则求值决定，计划期不可知；
        // 占位检测留给 apply 的 resolve_target，与 F11 同口径）。
        let entry = PlanEntry {
            entry_id: format!("{}-{:04}", input.plan_id, entries.len() + 1),
            source,
            target: Some(target),
            action: PlanAction::Move,
            relation: None,
            reason: match rule_index {
                Some(index) => format!("archive rule {} matched {}", index + 1, file.rel_path),
                None => format!("unclassified archive of {}", file.rel_path),
            },
            conflict: None,
            resolution: None,
            blocked,
            expected: Some(PlanExpected {
                size: file.size,
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
        };
        entries.push(entry);
    }

    let mut warnings = Vec::new();
    let total_existing = input.files.iter().filter(|file| file.exists).count() as f64;
    for (index, count, samples) in &hits {
        let ratio = if total_existing > 0.0 {
            *count as f64 / total_existing
        } else {
            0.0
        };
        if ratio > HIGH_HIT_RATIO && *count > 0 {
            warnings.push(PlanWarning {
                kind: "high-hit-rule".to_string(),
                suggestion: Some(format!(
                    "rule {} matches {} of {} file(s) ({:.0}%); review before applying (方案 §5.9 高危规则)",
                    index + 1,
                    count,
                    total_existing as u64,
                    ratio * 100.0
                )),
                risk_level: RiskLevel::Danger,
            });
        }
        warnings.push(PlanWarning {
            kind: "rule-hits".to_string(),
            suggestion: Some(format!(
                "rule {} matched {}: samples: {}",
                index + 1,
                count,
                if samples.is_empty() {
                    "—".to_string()
                } else {
                    samples.join(", ")
                }
            )),
            risk_level: RiskLevel::Safe,
        });
    }
    if unmatched_kept > 0 {
        warnings.push(PlanWarning {
            kind: "unmatched-kept".to_string(),
            suggestion: Some(format!(
                "{unmatched_kept} file(s) matched no rule and stay in place"
            )),
            risk_level: RiskLevel::Safe,
        });
    }
    if !missing.is_empty() {
        warnings.push(PlanWarning {
            kind: "walk-missing".to_string(),
            suggestion: Some(format!(
                "files disappeared between listing and planning: {}",
                missing.join(", ")
            )),
            risk_level: RiskLevel::Caution,
        });
    }

    let mut params = std::collections::BTreeMap::new();
    params.insert(
        "ruleCount".to_string(),
        PlanParamValue::Number(input.rules.len() as u64),
    );
    params.insert(
        "fileCount".to_string(),
        PlanParamValue::Number(input.files.len() as u64),
    );
    params.insert(
        "unmatchedKept".to_string(),
        PlanParamValue::Number(unmatched_kept),
    );

    let stats = PlanStats {
        total: entries.len(),
        conflict: 0,
        warning: warnings.len(),
        blocked: entries
            .iter()
            .filter(|entry| entry.blocked.is_some())
            .count(),
    };
    Ok(PlanDocument {
        plan_id: input.plan_id,
        kind: C08_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

fn join_root(root: &str, relative: &str) -> String {
    let root = root.trim_end_matches(['/', '\\']);
    if relative.is_empty() {
        root.to_string()
    } else {
        format!("{root}/{relative}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(rel_path: &str, size: u64, date: Option<&str>) -> PlanC08File {
        PlanC08File {
            rel_path: rel_path.to_string(),
            size,
            modified_date: date.map(String::from),
            exists: true,
        }
    }

    fn input(rules: Vec<ArchiveRule>, files: Vec<PlanC08File>) -> PlanC08Input {
        PlanC08Input {
            plan_id: "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8".to_string(),
            source_root: "/data/src".to_string(),
            target_root: "/data/dst".to_string(),
            rules,
            unmatched: UnmatchedPolicy::Keep,
            files,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            long_path_supported: true,
        }
    }

    #[test]
    fn first_matching_rule_wins_and_new_columns_follow() {
        let document = plan_c08(input(
            vec![
                ArchiveRule {
                    sub_dir: "photos".to_string(),
                    condition: ArchiveCondition::Extensions(vec!["jpg".to_string()]),
                },
                ArchiveRule {
                    sub_dir: "big".to_string(),
                    condition: ArchiveCondition::SizeRange {
                        min: Some(100),
                        max: None,
                    },
                },
            ],
            vec![
                file("a.jpg", 500, Some("2026-01-01")),
                file("b.png", 500, Some("2026-01-01")),
                file("c.txt", 500, Some("2026-01-01")),
            ],
        ))
        .unwrap();
        assert_eq!(document.entries.len(), 3, "jpg + big(c.txt) archived");
        assert_eq!(
            document.entries[0].target.as_deref(),
            Some("/data/dst/photos/a.jpg")
        );
        // 首中生效：b.png 未命中 photos（jpg），命中 big。
        assert_eq!(
            document.entries[1].target.as_deref(),
            Some("/data/dst/big/b.png")
        );
        assert_eq!(
            document.entries[2].target.as_deref(),
            Some("/data/dst/big/c.txt")
        );
        // 命中统计进警告（§5.9 每条规则命中数）。
        assert!(document
            .warnings
            .iter()
            .any(|warning| warning.kind == "rule-hits"
                && warning
                    .suggestion
                    .as_deref()
                    .unwrap_or("")
                    .contains("rule 1 matched 1")));
    }

    #[test]
    fn extension_matching_folds_case_and_requires_a_match() {
        let document = plan_c08(input(
            vec![ArchiveRule {
                sub_dir: "photos".to_string(),
                condition: ArchiveCondition::Extensions(vec!["JPG".to_string()]),
            }],
            vec![file("a.JPg", 1, None), file("b.png", 1, None)],
        ))
        .unwrap();
        assert_eq!(document.entries.len(), 1);
        assert_eq!(document.entries[0].source, "/data/src/a.JPg");
    }

    #[test]
    fn date_and_size_ranges_use_inclusive_lexicographic_bounds() {
        let document = plan_c08(input(
            vec![ArchiveRule {
                sub_dir: "recent".to_string(),
                condition: ArchiveCondition::ModifiedDateRange {
                    from: Some("2026-01-01".to_string()),
                    to: Some("2026-12-31".to_string()),
                },
            }],
            vec![
                file("in-range.txt", 1, Some("2026-06-15")),
                file("before.txt", 1, Some("2025-12-31")),
                file("no-date.txt", 1, None),
            ],
        ))
        .unwrap();
        assert_eq!(document.entries.len(), 1);
        assert_eq!(document.entries[0].source, "/data/src/in-range.txt");
    }

    #[test]
    fn high_hit_rules_surface_a_danger_warning() {
        let document = plan_c08(input(
            vec![ArchiveRule {
                sub_dir: "all".to_string(),
                condition: ArchiveCondition::NameKeyword("a".to_string()),
            }],
            vec![
                file("a1", 1, None),
                file("a2", 1, None),
                file("zzz", 1, None),
            ],
        ))
        .unwrap();
        // 2/3 = 67% > 50% → Danger（§5.9 高危规则）。
        assert!(document
            .warnings
            .iter()
            .any(|warning| warning.kind == "high-hit-rule"
                && warning.risk_level == RiskLevel::Danger));
    }

    #[test]
    fn unmatched_files_keep_or_move_by_policy() {
        let rules = vec![ArchiveRule {
            sub_dir: "photos".to_string(),
            condition: ArchiveCondition::Extensions(vec!["jpg".to_string()]),
        }];
        let files = vec![file("a.jpg", 1, None), file("b.txt", 1, None)];
        // 默认留原位：未命中不进计划。
        let kept = plan_c08(input(rules.clone(), files.clone())).unwrap();
        assert_eq!(kept.entries.len(), 1);
        assert!(kept
            .warnings
            .iter()
            .any(|warning| warning.kind == "unmatched-kept"));
        // 未分类目录：未命中生成 move 条目。
        let mut moved_input = input(rules, files);
        moved_input.unmatched = UnmatchedPolicy::MoveTo("_未分类".to_string());
        let moved = plan_c08(moved_input).unwrap();
        assert_eq!(moved.entries.len(), 2);
        assert_eq!(
            moved.entries[1].target.as_deref(),
            Some("/data/dst/_未分类/b.txt")
        );
        assert_eq!(moved.entries[1].reason, "unclassified archive of b.txt");
    }

    #[test]
    fn rule_sub_dirs_must_be_safe() {
        let error = plan_c08(input(
            vec![ArchiveRule {
                sub_dir: "../escape".to_string(),
                condition: ArchiveCondition::NameKeyword("a".to_string()),
            }],
            vec![file("a.jpg", 1, None)],
        ))
        .unwrap_err();
        assert_eq!(error.code, "E1003");
    }
}
