//! C05 执行前预览与冲突检测——重命名族的预览聚合纯函数（方案 §5.13、§7.4）。
//!
//! 输入是已生成的重命名计划（f12/f13/c04，§7.4"完整对照表按需生成"），
//! 输出聚合预览报告：逐条"旧名→新名"对照行（§5.5 状态标记集）＋
//! 四类固定计数（将处理/跳过/冲突/异常）＋高危项汇总。
//! 两阶段改名（环/仅大小写）在对照表中折叠为一行"旧名→最终新名"。
//! 本模块只读计划结构，不读盘、不取时钟（不变量 1）。

use serde::{Deserialize, Serialize};

use crate::error::KernelError;
use crate::plan::{PlanDocument, RiskLevel};

pub const C05_PREVIEW_KINDS: [&str; 3] = [
    crate::rename::F12_KIND,
    crate::regex_rename::F13_KIND,
    crate::name_replace::C04_KIND,
];

/// 对照行状态标记（§5.5：正常/冲突/非法/超长/无变化；`protected` 为工程投影，
/// 对应 §5.13 保护目录计划期阻断 E2009）。"无变化"条目在计划期即被丢弃并转入
/// 计划警告，因此不出现在对照行中。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PreviewStatus {
    Normal,
    Conflict,
    Invalid,
    TooLong,
    Protected,
}

impl PreviewStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Conflict => "conflict",
            Self::Invalid => "invalid",
            Self::TooLong => "too-long",
            Self::Protected => "protected",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenamePreviewRow {
    pub entry_id: String,
    /// 旧完整路径（两阶段行＝阶段一源路径）。
    pub old_path: String,
    /// 新完整路径（两阶段行＝阶段二最终目标）。
    pub new_path: String,
    pub status: PreviewStatus,
    /// 该行由"临时名→目标名"两阶段条目折叠而来（§5.5 实现要点）。
    pub two_phase: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// §7.4 固定四类计数：将处理/跳过/冲突/异常。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenamePreviewCounts {
    pub to_process: usize,
    pub skipped: usize,
    pub conflicts: usize,
    pub blocked: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenamePreview {
    pub plan_id: String,
    pub kind: String,
    pub counts: RenamePreviewCounts,
    pub rows: Vec<RenamePreviewRow>,
    /// 高危项汇总（§7.4）：阻断明细、冲突策略提示与 Danger 级计划警告。
    pub highlights: Vec<String>,
}

const LONG_PATH_SKIP_MARKER: &str = "long-path support is disabled";
/// rename.rs 两阶段临时名前缀（同 crate 内常量耦合，属同一模块族契约）。
const TWO_PHASE_TEMP_PREFIX: &str = ".batchdesk-rename-";

/// 从重命名计划聚合 C05 预览报告；非重命名族计划返回 E1002。
pub fn rename_preview(plan: &PlanDocument) -> Result<RenamePreview, KernelError> {
    if !C05_PREVIEW_KINDS.contains(&plan.kind.as_str()) {
        return Err(KernelError::invalid_params(format!(
            "preview supports rename-family plans ({:?}), got {}",
            C05_PREVIEW_KINDS, plan.kind
        )));
    }
    let temp_prefix = format!("{TWO_PHASE_TEMP_PREFIX}{}", plan.plan_id);

    // 阶段一条目（源→临时名）先入表；阶段二条目（临时名→最终目标）回填折叠。
    struct Draft {
        row: RenamePreviewRow,
    }
    let mut drafts: Vec<Draft> = Vec::new();
    let mut temp_sources: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for entry in &plan.entries {
        let is_temp_source = crate::conflict::file_name_of(&entry.source).starts_with(&temp_prefix);
        if is_temp_source {
            // 阶段二：把对应阶段一行的目标回填为最终目标。
            if let Some(&index) = temp_sources.get(entry.source.as_str()) {
                let row = &mut drafts[index].row;
                row.new_path = entry.target.clone().unwrap_or_default();
                row.two_phase = true;
            }
            continue;
        }
        let (status, detail) = if let Some(blocked) = &entry.blocked {
            let status = if blocked.code == "E2009" {
                PreviewStatus::Protected
            } else {
                PreviewStatus::Invalid
            };
            (
                status,
                Some(format!("{}: {}", blocked.code, blocked.reason)),
            )
        } else if entry.reason.contains(LONG_PATH_SKIP_MARKER) {
            (PreviewStatus::TooLong, Some(entry.reason.clone()))
        } else if entry.conflict.is_some() {
            let kind = entry
                .conflict
                .as_ref()
                .map(|conflict| conflict.kind.clone())
                .unwrap_or_default();
            (PreviewStatus::Conflict, Some(format!("conflict: {kind}")))
        } else {
            (PreviewStatus::Normal, None)
        };
        let row = RenamePreviewRow {
            entry_id: entry.entry_id.clone(),
            old_path: entry.source.clone(),
            new_path: entry.target.clone().unwrap_or_default(),
            status,
            two_phase: false,
            detail,
        };
        let temp_path = entry.target.clone().unwrap_or_default();
        temp_sources.insert(temp_path.clone(), drafts.len());
        drafts.push(Draft { row });
    }

    let rows: Vec<RenamePreviewRow> = drafts.into_iter().map(|draft| draft.row).collect();
    let counts = RenamePreviewCounts {
        to_process: rows
            .iter()
            .filter(|row| row.status == PreviewStatus::Normal)
            .count(),
        skipped: rows
            .iter()
            .filter(|row| row.status == PreviewStatus::TooLong)
            .count(),
        conflicts: rows
            .iter()
            .filter(|row| row.status == PreviewStatus::Conflict)
            .count(),
        blocked: rows
            .iter()
            .filter(|row| {
                matches!(
                    row.status,
                    PreviewStatus::Invalid | PreviewStatus::Protected
                )
            })
            .count(),
    };

    let mut highlights = Vec::new();
    for row in &rows {
        match row.status {
            PreviewStatus::Invalid | PreviewStatus::Protected => {
                if let Some(detail) = &row.detail {
                    highlights.push(format!("{}: {detail}", row.old_path));
                }
            }
            PreviewStatus::Conflict => {
                highlights.push(format!(
                    "{} → {} conflicts with an existing or in-batch name; default policy is skip",
                    row.old_path, row.new_path
                ));
            }
            PreviewStatus::TooLong => {
                highlights.push(format!(
                    "{} → {} exceeds the long-path limit and is skipped",
                    row.old_path, row.new_path
                ));
            }
            PreviewStatus::Normal => {}
        }
    }
    if counts.conflicts > 0 {
        highlights.push(
            "conflicting entries default to skip; overwrite requires explicit confirmation (方案 §5.13)"
                .to_string(),
        );
    }
    for warning in &plan.warnings {
        if warning.risk_level == RiskLevel::Danger {
            highlights.push(format!(
                "warning {}: {}",
                warning.kind,
                warning.suggestion.clone().unwrap_or_default()
            ));
        }
    }

    Ok(RenamePreview {
        plan_id: plan.plan_id.clone(),
        kind: plan.kind.clone(),
        counts,
        rows,
        highlights,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conflict::ConflictKind;
    use crate::plan::PlanResolution;
    use crate::plan::{PlanBlocked, PlanConflict, PlanEntry, PlanParamValue, PlanStats};
    use std::collections::BTreeMap;

    fn entry(entry_id: &str, source: &str, target: &str) -> PlanEntry {
        PlanEntry {
            entry_id: entry_id.to_string(),
            source: source.to_string(),
            target: Some(target.to_string()),
            action: crate::plan::PlanAction::Rename,
            relation: None,
            reason: "rename to target".to_string(),
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
            csv_dedup: None,
            csv_filter: None,
            csv_columns: None,
            csv_sort: None,
        }
    }

    fn plan(kind: &str, entries: Vec<PlanEntry>) -> PlanDocument {
        let mut params = BTreeMap::new();
        params.insert(
            "scope".to_string(),
            PlanParamValue::String("full".to_string()),
        );
        PlanDocument {
            plan_id: "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8".to_string(),
            kind: kind.to_string(),
            params,
            entries,
            stats: PlanStats::default(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn normal_rows_count_as_to_process() {
        let document = plan(
            crate::rename::F12_KIND,
            vec![
                entry("e1", "/data/a.txt", "/data/a-1.txt"),
                entry("e2", "/data/b.txt", "/data/b-1.txt"),
            ],
        );
        let preview = rename_preview(&document).unwrap();
        assert_eq!(preview.counts.to_process, 2);
        assert_eq!(preview.counts.conflicts, 0);
        assert_eq!(preview.rows[0].status, PreviewStatus::Normal);
        assert_eq!(preview.rows[0].old_path, "/data/a.txt");
        assert_eq!(preview.rows[0].new_path, "/data/a-1.txt");
        assert!(!preview.rows[0].two_phase);
        assert!(preview.highlights.is_empty());
    }

    #[test]
    fn conflict_blocked_and_long_path_rows_are_labelled() {
        let mut conflict_entry = entry("e1", "/data/a.txt", "/data/b.txt");
        conflict_entry.conflict = Some(PlanConflict {
            kind: ConflictKind::Existing.as_str().to_string(),
            risk_level: crate::plan::RiskLevel::Caution,
        });
        conflict_entry.resolution = Some(PlanResolution::Skip);
        let mut invalid_entry = entry("e2", "/data/c.txt", "/data/con.txt");
        invalid_entry.blocked = Some(PlanBlocked {
            code: "E1002".to_string(),
            reason: "target name is not allowed: reserved-name".to_string(),
            hint: None,
        });
        let mut protected_entry = entry("e3", "/data/d.txt", "/prot/d.txt");
        protected_entry.blocked = Some(PlanBlocked {
            code: "E2009".to_string(),
            reason: "target is under protected directory".to_string(),
            hint: None,
        });
        let mut long_entry = entry("e4", "/data/e.txt", "/data/e-1.txt");
        long_entry.reason =
            "target path exceeds 259 characters and long-path support is disabled".to_string();
        long_entry.resolution = Some(PlanResolution::Skip);

        let preview = rename_preview(&plan(
            crate::rename::F12_KIND,
            vec![conflict_entry, invalid_entry, protected_entry, long_entry],
        ))
        .unwrap();
        assert_eq!(preview.counts.conflicts, 1);
        assert_eq!(preview.counts.blocked, 2);
        assert_eq!(preview.counts.skipped, 1);
        assert_eq!(preview.counts.to_process, 0);
        assert_eq!(preview.rows[0].status, PreviewStatus::Conflict);
        assert_eq!(preview.rows[1].status, PreviewStatus::Invalid);
        assert_eq!(preview.rows[2].status, PreviewStatus::Protected);
        assert_eq!(preview.rows[3].status, PreviewStatus::TooLong);
        assert!(preview
            .highlights
            .iter()
            .any(|line| line.contains("default policy is skip")));
        assert!(preview
            .highlights
            .iter()
            .any(|line| line.contains("target name is not allowed")));
    }

    #[test]
    fn two_phase_entries_collapse_into_one_mapping_row() {
        // 环（A→B、B→A）展开为两阶段：源→临时、临时→目标；预览折叠回旧名→新名。
        let temp_a = "/data/.batchdesk-rename-01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8-0-0";
        let temp_b = "/data/.batchdesk-rename-01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8-1-0";
        let document = plan(
            crate::rename::F12_KIND,
            vec![
                entry("e1-phase1", "/data/a.txt", temp_a),
                entry("e1-phase2", temp_a, "/data/b.txt"),
                entry("e2-phase1", "/data/b.txt", temp_b),
                entry("e2-phase2", temp_b, "/data/a.txt"),
            ],
        );
        let preview = rename_preview(&document).unwrap();
        assert_eq!(
            preview.rows.len(),
            2,
            "phase entries collapse into mapping rows"
        );
        assert!(preview.rows.iter().all(|row| row.two_phase));
        let by_old: std::collections::HashMap<&str, &RenamePreviewRow> = preview
            .rows
            .iter()
            .map(|row| (row.old_path.as_str(), row))
            .collect();
        assert_eq!(by_old["/data/a.txt"].new_path, "/data/b.txt");
        assert_eq!(by_old["/data/b.txt"].new_path, "/data/a.txt");
        assert_eq!(preview.counts.to_process, 2);
    }

    #[test]
    fn non_rename_kinds_are_rejected_with_e1002() {
        let error = rename_preview(&plan(crate::plan::C09_KIND, vec![])).unwrap_err();
        assert_eq!(error.code, "E1002");
        let error = rename_preview(&plan(crate::replace_content::C03_KIND, vec![])).unwrap_err();
        assert_eq!(error.code, "E1002");
    }
}
