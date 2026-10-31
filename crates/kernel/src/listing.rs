//!
//! 纯函数层：目录清单（F10）计划与流式清单写出端口。
//!
//! 口径（方案 §5.15、§4.8；TC F10）：
//! - 只遍历元数据，不递归读取文件内容；
//! - 输出 CSV（UTF-8 BOM），字段为相对路径/名称/类型/大小/修改时间/层级；
//! - 根目录的子项层级为 1，逐级递增；目录大小记为 0；
//! - 占位文件跳过并计数（E02），符号链接/不可读目录跳过并计数；
//! - 目标文件不得位于被列出的根目录内，避免清单自包含；
//! - 计划期只保存摘要计数；apply 通过 `DirectoryLister` 端口流式遍历写出。

use crate::csv_extract::ensure_unique_targets;
use crate::csv_merge::{source_block, target_block};
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::{is_under, try_normalize, ProtectedDir};
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanConflict, PlanDocument, PlanEntry, PlanParamValue,
    PlanResolution, PlanStats, PlanWarning, RiskLevel,
};

pub const F10_KIND: &str = "f10";
pub const LISTING_HEADER: [&str; 6] = ["相对路径", "名称", "类型", "大小", "修改时间", "层级"];

/// 清单写出选项（当前固定包含文件与目录，保留端口便于后续扩展）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListingOptions {
    pub include_files: bool,
    pub include_dirs: bool,
}

impl Default for ListingOptions {
    fn default() -> Self {
        Self {
            include_files: true,
            include_dirs: true,
        }
    }
}

/// 目录遍历摘要；计划期用于预览，apply 期用于执行统计。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListingStats {
    pub files: u64,
    pub dirs: u64,
    pub placeholders: u64,
    pub skipped: u64,
    pub max_depth: u32,
}

/// 流式清单写出端口；实现方只做 I/O，不持有目录内容。
pub trait DirectoryLister: Send + Sync {
    fn write_listing(
        &self,
        root: &str,
        options: &ListingOptions,
        out: &mut dyn std::io::Write,
    ) -> Result<ListingStats, KernelError>;
}

/// F10 的单个输入；摘要计数由壳层流式扫描得到。
#[derive(Clone, Debug)]
pub struct PlanF10File {
    pub root: String,
    pub canonical_root: Option<String>,
    pub target: String,
    pub target_occupied: bool,
    pub files: u64,
    pub dirs: u64,
    pub placeholders: u64,
    pub skipped: u64,
    pub max_depth: u32,
}

#[derive(Clone, Debug)]
pub struct PlanF10Input {
    pub plan_id: String,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub files: Vec<PlanF10File>,
}

/// 生成 F10 计划：每个根目录一条 `listing` 条目（1:1，每个根目录一个清单文件）。
pub fn plan_f10(input: PlanF10Input) -> Result<PlanDocument, KernelError> {
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
            "listing plans require at least one root directory",
        ));
    }
    ensure_unique_targets(
        input.files.iter().map(|file| file.target.as_str()),
        input.case_sense,
    )?;

    let mut warnings = Vec::new();
    let mut empty_roots = Vec::new();
    let mut placeholders = Vec::new();
    let mut skipped = Vec::new();
    let mut entries = Vec::with_capacity(input.files.len());

    for file in &input.files {
        let mut blocked = target_block(&file.target, &input.protected_dirs, input.case_sense);
        if blocked.is_none() {
            blocked = source_block(
                &file.root,
                file.canonical_root.as_deref(),
                &input.protected_dirs,
                input.case_sense,
            );
        }
        if blocked.is_none() {
            if let (Ok(root), Ok(target)) = (try_normalize(&file.root), try_normalize(&file.target))
            {
                if is_under(&target, &root, input.case_sense) {
                    blocked = Some(PlanBlocked {
                        code: "E1002".to_string(),
                        reason: "listing target must be outside the listed root".to_string(),
                        hint: Some(
                            "choose an output file outside the directory being listed".to_string(),
                        ),
                    });
                }
            }
        }
        if file.files + file.dirs == 0 {
            empty_roots.push(file.root.clone());
        }
        if file.placeholders > 0 {
            placeholders.push(format!("{} ({})", file.root, file.placeholders));
        }
        if file.skipped > 0 {
            skipped.push(format!("{} ({})", file.root, file.skipped));
        }

        let conflict = file.target_occupied.then(|| PlanConflict {
            kind: "existing-target".to_string(),
            risk_level: RiskLevel::Danger,
        });
        let target_name = crate::conflict::file_name_of(&file.target);
        entries.push(PlanEntry {
            entry_id: format!("{}-{:04}", input.plan_id, entries.len() + 1),
            source: file.root.clone(),
            target: Some(file.target.clone()),
            action: PlanAction::Listing,
            relation: None,
            reason: format!(
                "list {} file(s) and {} dir(s) from {} into {} as CSV (UTF-8 BOM)",
                file.files, file.dirs, file.root, target_name
            ),
            conflict: conflict.clone(),
            resolution: conflict.as_ref().map(|_| PlanResolution::Skip),
            blocked,
            expected: None,
            range: None,
            merge: None,
            csv: None,
            csv_merge: None,
            csv_extract: None,
            csv_rows: None,
            csv_dedup: None,
        });
    }

    if !empty_roots.is_empty() {
        warnings.push(PlanWarning {
            kind: "empty-directory".to_string(),
            suggestion: Some(format!(
                "directories contain no entries; the listing has only the header: {}",
                empty_roots.join(", ")
            )),
            risk_level: RiskLevel::Safe,
        });
    }
    if !placeholders.is_empty() {
        warnings.push(PlanWarning {
            kind: "placeholders-skipped".to_string(),
            suggestion: Some(format!(
                "placeholder files are skipped and counted separately: {}",
                placeholders.join(", ")
            )),
            risk_level: RiskLevel::Caution,
        });
    }
    if !skipped.is_empty() {
        warnings.push(PlanWarning {
            kind: "unreadable-or-symlink-skipped".to_string(),
            suggestion: Some(format!(
                "unreadable entries or symbolic links are skipped: {}",
                skipped.join(", ")
            )),
            risk_level: RiskLevel::Caution,
        });
    }

    let mut params = std::collections::BTreeMap::new();
    params.insert(
        "format".to_string(),
        PlanParamValue::String("csv".to_string()),
    );
    params.insert("includeFiles".to_string(), PlanParamValue::Bool(true));
    params.insert("includeDirs".to_string(), PlanParamValue::Bool(true));
    params.insert(
        "targetEncoding".to_string(),
        PlanParamValue::String("utf-8-bom".to_string()),
    );
    params.insert(
        "rootCount".to_string(),
        PlanParamValue::Number(input.files.len() as u64),
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
        kind: F10_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    fn file(root: &str, files: u64, dirs: u64) -> PlanF10File {
        PlanF10File {
            root: root.to_string(),
            canonical_root: None,
            target: format!("{root}.csv"),
            target_occupied: false,
            files,
            dirs,
            placeholders: 0,
            skipped: 0,
            max_depth: 1,
        }
    }

    fn input(files: Vec<PlanF10File>) -> PlanF10Input {
        PlanF10Input {
            plan_id: PLAN_ID.to_string(),
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            files,
        }
    }

    #[test]
    fn builds_listing_entries_with_summary_counts() {
        let plan = plan_f10(input(vec![file("/data/tree", 3, 2)])).unwrap();
        assert_eq!(plan.kind, F10_KIND);
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].action, PlanAction::Listing);
        assert_eq!(plan.entries[0].source, "/data/tree");
        assert_eq!(plan.entries[0].expected, None);
        assert!(plan.params.contains_key("format"));
        assert_eq!(plan.params["includeFiles"], PlanParamValue::Bool(true));
    }

    #[test]
    fn empty_directory_warns() {
        let plan = plan_f10(input(vec![file("/data/empty", 0, 0)])).unwrap();
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "empty-directory"));
    }

    #[test]
    fn placeholder_and_skipped_entries_warn() {
        let mut root = file("/data/tree", 2, 1);
        root.placeholders = 1;
        root.skipped = 2;
        let plan = plan_f10(input(vec![root])).unwrap();
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "placeholders-skipped"));
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "unreadable-or-symlink-skipped"));
    }

    #[test]
    fn target_inside_root_is_blocked() {
        let mut root = file("/data/tree", 1, 1);
        root.target = "/data/tree/list.csv".to_string();
        let plan = plan_f10(input(vec![root])).unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E1002");
    }

    #[test]
    fn occupied_target_is_a_skip_conflict() {
        let mut root = file("/data/tree", 1, 1);
        root.target_occupied = true;
        let plan = plan_f10(input(vec![root])).unwrap();
        assert_eq!(
            plan.entries[0].conflict.as_ref().unwrap().kind,
            "existing-target"
        );
        assert_eq!(plan.entries[0].resolution, Some(PlanResolution::Skip));
    }

    #[test]
    fn duplicate_targets_are_rejected() {
        let mut first = file("/data/a", 1, 0);
        first.target = "/out/list.csv".to_string();
        let mut second = file("/data/b", 1, 0);
        second.target = "/out/list.csv".to_string();
        assert_eq!(
            plan_f10(input(vec![first, second])).unwrap_err().code,
            "E1002"
        );
    }
}
