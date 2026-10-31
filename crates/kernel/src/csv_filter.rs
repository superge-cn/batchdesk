//!
//! C12 CSV 过滤计划与纯函数（方案 §5.15）。
//!
//! 条件（包含/不包含/数值比较）编译为判定器，逐行求值；表头单独保留，
//! 脏行与非数值单元格只计数告警、不静默修正。本模块不读盘；壳层提供解析后的记录。

use std::collections::BTreeMap;

use crate::csv_extract::ColumnPick;
use crate::csv_merge::normalize_header_name;
use crate::encoding::{InvalidSequencePolicy, SourceEncoding, TargetEncoding};
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::ProtectedDir;
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanConflict, PlanCsvFilter, PlanCsvFilterColumn,
    PlanCsvFilterCondition, PlanDocument, PlanEntry, PlanExpected, PlanParamValue, PlanResolution,
    PlanStats, PlanWarning, RiskLevel,
};

pub const C12_KIND: &str = "c12";

/// 判定算子（方案 §5.15 C12 功能表：包含、不包含、数值比较）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterOp {
    Contains,
    NotContains,
    NumEq,
    NumNe,
    NumGt,
    NumGe,
    NumLt,
    NumLe,
}

impl FilterOp {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "contains" => Some(Self::Contains),
            "not-contains" => Some(Self::NotContains),
            "eq" => Some(Self::NumEq),
            "ne" => Some(Self::NumNe),
            "gt" => Some(Self::NumGt),
            "ge" => Some(Self::NumGe),
            "lt" => Some(Self::NumLt),
            "le" => Some(Self::NumLe),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Contains => "contains",
            Self::NotContains => "not-contains",
            Self::NumEq => "eq",
            Self::NumNe => "ne",
            Self::NumGt => "gt",
            Self::NumGe => "ge",
            Self::NumLt => "lt",
            Self::NumLe => "le",
        }
    }

    fn is_numeric(self) -> bool {
        !matches!(self, Self::Contains | Self::NotContains)
    }
}

#[derive(Clone, Debug)]
pub struct PlanC12File {
    pub path: String,
    pub canonical_path: Option<String>,
    pub output: String,
    pub file_size: u64,
    pub source_encoding: Option<SourceEncoding>,
    pub delimiter: crate::csv::Delimiter,
    pub has_header: bool,
    /// 原始表头字段（无表头为空）。
    pub columns: Vec<String>,
    pub column_count: u64,
    pub dirty_rows: u64,
    /// 解析后的全部记录（有表头时含表头）。
    pub records: Vec<Vec<String>>,
    pub target_occupied: bool,
}

#[derive(Clone, Debug)]
pub struct PlanC12Input {
    pub plan_id: String,
    pub target: TargetEncoding,
    pub invalid: InvalidSequencePolicy,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    /// `"keep"`（保留匹配行）或 `"drop"`（剔除匹配行）。
    pub mode_keep: bool,
    pub condition_column: ColumnPick,
    pub condition_op: FilterOp,
    pub condition_value: String,
    pub files: Vec<PlanC12File>,
}

fn resolve_condition_column(
    file: &PlanC12File,
    pick: &ColumnPick,
) -> Result<PlanCsvFilterColumn, PlanBlocked> {
    if !file.has_header && matches!(pick, ColumnPick::Name(_)) {
        return Err(PlanBlocked {
            code: "E3004".to_string(),
            reason: "column-name conditions require a header row".to_string(),
            hint: Some("use --column-index or mark the input as headerless".to_string()),
        });
    }
    let (index, name) = match pick {
        ColumnPick::Name(name) => {
            let name = normalize_header_name(name);
            let normalized: Vec<String> = file
                .columns
                .iter()
                .map(|column| normalize_header_name(column))
                .collect();
            let Some(index) = normalized.iter().position(|column| column == &name) else {
                return Err(PlanBlocked {
                    code: "E3004".to_string(),
                    reason: format!("condition column not found: {name}"),
                    hint: Some("check the header or use a 1-based index".to_string()),
                });
            };
            (index as u64, Some(name))
        }
        ColumnPick::Index(index) => {
            if *index == 0 || *index > file.column_count {
                return Err(PlanBlocked {
                    code: "E3004".to_string(),
                    reason: format!("condition column index out of range: {index}"),
                    hint: Some(format!("index must be between 1 and {}", file.column_count)),
                });
            }
            (
                index.saturating_sub(1),
                normalized_column_name(file, *index as usize - 1),
            )
        }
    };
    Ok(PlanCsvFilterColumn { index, name })
}

fn normalized_column_name(file: &PlanC12File, index: usize) -> Option<String> {
    file.columns
        .get(index)
        .map(|column| normalize_header_name(column))
}

fn numeric_matches(number: f64, limit: f64, op: FilterOp) -> bool {
    match op {
        FilterOp::NumEq => number == limit,
        FilterOp::NumNe => number != limit,
        FilterOp::NumGt => number > limit,
        FilterOp::NumGe => number >= limit,
        FilterOp::NumLt => number < limit,
        FilterOp::NumLe => number <= limit,
        FilterOp::Contains | FilterOp::NotContains => false,
    }
}

/// 返回（保留的记录，匹配行数，非数值单元格计数）。
/// 非数值单元格对任何数值算子都不满足判定（含 `ne`，避免意外保留/剔除）。
pub fn filter_records(
    records: &[Vec<String>],
    has_header: bool,
    column_index: usize,
    op: FilterOp,
    value: &str,
    mode_keep: bool,
) -> (Vec<Vec<String>>, u64, u64) {
    let data: &[Vec<String>] = if has_header {
        records.get(1..).unwrap_or(&[])
    } else {
        records
    };
    let limit = if op.is_numeric() {
        value.parse::<f64>().ok()
    } else {
        None
    };
    let mut kept = Vec::new();
    let mut matched = 0u64;
    let mut non_numeric = 0u64;
    for record in data {
        let cell = record
            .get(column_index)
            .map(String::as_str)
            .unwrap_or_default();
        let is_match = match op {
            FilterOp::Contains => cell.contains(value),
            FilterOp::NotContains => !cell.contains(value),
            FilterOp::NumEq
            | FilterOp::NumNe
            | FilterOp::NumGt
            | FilterOp::NumGe
            | FilterOp::NumLt
            | FilterOp::NumLe => match cell.parse::<f64>() {
                Ok(number) => match limit {
                    Some(limit) => numeric_matches(number, limit, op),
                    None => false,
                },
                Err(_) => {
                    non_numeric += 1;
                    false
                }
            },
        };
        if is_match {
            matched += 1;
        }
        if is_match == mode_keep {
            kept.push(record.clone());
        }
    }
    (kept, matched, non_numeric)
}

pub fn render_records(header: Option<&Vec<String>>, rows: &[Vec<String>], delimiter: u8) -> String {
    let mut out = String::new();
    if let Some(header) = header {
        crate::csv_merge::write_record(header, delimiter, &mut out);
    }
    for row in rows {
        crate::csv_merge::write_record(row, delimiter, &mut out);
    }
    out
}

pub fn plan_c12(input: PlanC12Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.files.is_empty() {
        return Err(KernelError::missing_param("c12 requires at least one file"));
    }
    if input.condition_op.is_numeric() && input.condition_value.parse::<f64>().is_err() {
        return Err(KernelError::invalid_params(
            "c12 numeric operators require a numeric value",
        ));
    }
    let mut targets = BTreeMap::new();
    for file in &input.files {
        let key = crate::path::normalize(&file.output);
        if targets.insert(key, &file.path).is_some() {
            return Err(KernelError::invalid_params(
                "duplicate output targets in one filter plan",
            ));
        }
    }

    let mut params = BTreeMap::new();
    params.insert(
        "target".to_string(),
        PlanParamValue::String(input.target.as_str().to_string()),
    );
    params.insert(
        "invalid".to_string(),
        PlanParamValue::String(input.invalid.as_str().to_string()),
    );
    params.insert(
        "op".to_string(),
        PlanParamValue::String(input.condition_op.as_str().to_string()),
    );
    params.insert(
        "mode".to_string(),
        PlanParamValue::String(if input.mode_keep {
            "keep".to_string()
        } else {
            "drop".to_string()
        }),
    );
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    for file in &input.files {
        let mut blocked =
            crate::csv_merge::target_block(&file.output, &input.protected_dirs, input.case_sense);
        if blocked.is_none() {
            blocked = crate::csv_merge::source_block(
                &file.path,
                file.canonical_path.as_deref(),
                &input.protected_dirs,
                input.case_sense,
            );
        }
        if blocked.is_none() && file.source_encoding.is_none() {
            blocked = Some(PlanBlocked {
                code: "E3001".to_string(),
                reason: "source encoding requires manual confirmation".to_string(),
                hint: Some("confirm the source encoding or pass --source".to_string()),
            });
        }
        let column = if blocked.is_none() {
            match resolve_condition_column(file, &input.condition_column) {
                Ok(column) => Some(column),
                Err(error) => {
                    blocked = Some(error);
                    None
                }
            }
        } else {
            None
        };
        if blocked.is_none() && file.records.is_empty() {
            blocked = Some(PlanBlocked {
                code: "E1002".to_string(),
                reason: "empty file has no rows".to_string(),
                hint: None,
            });
        }
        let (kept, matched, non_numeric) = match (&blocked, column.as_ref()) {
            (None, Some(column)) => filter_records(
                &file.records,
                file.has_header,
                column.index as usize,
                input.condition_op,
                &input.condition_value,
                input.mode_keep,
            ),
            _ => (Vec::new(), 0, 0),
        };
        let data_rows = file
            .records
            .len()
            .saturating_sub(usize::from(file.has_header)) as u64;
        let payload = PlanCsvFilter {
            encoding: file.source_encoding,
            size: file.file_size,
            delimiter: file.delimiter.as_str().to_string(),
            has_header: file.has_header,
            columns: file.columns.clone(),
            column_count: file.column_count,
            condition: PlanCsvFilterCondition {
                column: column.clone().unwrap_or(PlanCsvFilterColumn {
                    index: 0,
                    name: None,
                }),
                op: input.condition_op.as_str().to_string(),
                value: input.condition_value.clone(),
            },
            mode: if input.mode_keep {
                "keep".to_string()
            } else {
                "drop".to_string()
            },
            target_encoding: input.target.as_str().to_string(),
            invalid: input.invalid.as_str().to_string(),
            data_rows,
            matched_rows: matched,
            kept_rows: kept.len() as u64,
            non_numeric_cells: non_numeric,
            dirty_rows: file.dirty_rows,
        };
        let mut conflict = None;
        let mut resolution = None;
        if blocked.is_none() && file.target_occupied {
            conflict = Some(PlanConflict {
                kind: "existing-target".to_string(),
                risk_level: RiskLevel::Danger,
            });
            resolution = Some(PlanResolution::Skip);
        }
        let reason = match &blocked {
            Some(block) => block.reason.clone(),
            None if file.target_occupied => "target exists; skipped by default".to_string(),
            None => format!(
                "filter {} {} {:?}: {} data rows -> {} matched, {} kept ({})",
                column
                    .as_ref()
                    .and_then(|column| column.name.clone())
                    .unwrap_or_else(|| format!("column#{}", input.condition_column_index())),
                input.condition_op.as_str(),
                input.condition_value,
                data_rows,
                matched,
                kept.len(),
                if input.mode_keep { "keep" } else { "drop" },
            ),
        };
        if non_numeric > 0 {
            warnings.push(PlanWarning {
                kind: "non-numeric-cells".to_string(),
                suggestion: Some(format!(
                    "{} cells in the condition column are not numeric and never match",
                    non_numeric
                )),
                risk_level: RiskLevel::Caution,
            });
        }
        if blocked.is_none() && matched == 0 {
            warnings.push(PlanWarning {
                kind: "no-matching-rows".to_string(),
                suggestion: Some("no rows satisfy the condition".to_string()),
                risk_level: RiskLevel::Safe,
            });
        }
        if file.dirty_rows > 0 {
            warnings.push(PlanWarning {
                kind: "column-drift".to_string(),
                suggestion: Some(format!(
                    "{} rows have a different column count in {}",
                    file.dirty_rows, file.path
                )),
                risk_level: RiskLevel::Caution,
            });
        }
        entries.push(PlanEntry {
            entry_id: format!("{}-{:04}", input.plan_id, entries.len() + 1),
            source: file.path.clone(),
            target: Some(file.output.clone()),
            action: PlanAction::CsvFilter,
            relation: None,
            reason,
            conflict,
            resolution,
            blocked,
            expected: Some(PlanExpected {
                size: file.file_size,
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
            csv_filter: Some(payload),
        });
    }
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
        kind: C12_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

impl PlanC12Input {
    fn condition_column_index(&self) -> u64 {
        match &self.condition_column {
            ColumnPick::Index(index) => *index,
            ColumnPick::Name(_) => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csv::Delimiter;

    fn file(records: Vec<Vec<String>>, header: bool, occupied: bool) -> PlanC12File {
        let columns = if header {
            records.first().cloned().unwrap_or_default()
        } else {
            Vec::new()
        };
        PlanC12File {
            path: "/data/a.csv".to_string(),
            canonical_path: None,
            output: "/out/a.csv".to_string(),
            file_size: 100,
            source_encoding: Some(SourceEncoding::Utf8),
            delimiter: Delimiter::Comma,
            has_header: header,
            columns,
            column_count: records.first().map_or(0, |row| row.len() as u64),
            dirty_rows: 0,
            records,
            target_occupied: occupied,
        }
    }

    fn input(
        files: Vec<PlanC12File>,
        column: ColumnPick,
        op: FilterOp,
        value: &str,
        mode_keep: bool,
    ) -> PlanC12Input {
        PlanC12Input {
            plan_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            target: TargetEncoding::Utf8 { bom: false },
            invalid: InvalidSequencePolicy::ReplaceAndCount,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            mode_keep,
            condition_column: column,
            condition_op: op,
            condition_value: value.to_string(),
            files,
        }
    }

    fn records() -> Vec<Vec<String>> {
        vec![
            vec!["id".into(), "amount".into()],
            vec!["1".into(), "10".into()],
            vec!["2".into(), "5".into()],
            vec!["3".into(), "20".into()],
        ]
    }

    #[test]
    fn contains_keep_mode_keeps_only_matching_rows() {
        let plan = plan_c12(input(
            vec![file(
                vec![
                    vec!["id".into(), "name".into()],
                    vec!["1".into(), "alice".into()],
                    vec!["2".into(), "bob".into()],
                ],
                true,
                false,
            )],
            ColumnPick::Name("name".into()),
            FilterOp::Contains,
            "ali",
            true,
        ))
        .unwrap();
        let payload = plan.entries[0].payload_csv_filter();
        assert_eq!(payload.matched_rows, 1);
        assert_eq!(payload.kept_rows, 1);
    }

    #[test]
    fn drop_mode_removes_matching_rows() {
        let plan = plan_c12(input(
            vec![file(records(), true, false)],
            ColumnPick::Index(2),
            FilterOp::NumGt,
            "5",
            false,
        ))
        .unwrap();
        let payload = plan.entries[0].payload_csv_filter();
        assert_eq!(payload.matched_rows, 2);
        assert_eq!(payload.kept_rows, 1);
        assert_eq!(payload.mode, "drop");
    }

    #[test]
    fn numeric_comparison_keeps_header_and_counts_non_numeric() {
        let mut dirty = records();
        dirty.push(vec!["x".into(), "n/a".into()]);
        let plan = plan_c12(input(
            vec![file(dirty, true, false)],
            ColumnPick::Name("amount".into()),
            FilterOp::NumGe,
            "10",
            true,
        ))
        .unwrap();
        let payload = plan.entries[0].payload_csv_filter();
        assert_eq!(payload.matched_rows, 2);
        assert_eq!(payload.kept_rows, 2);
        assert_eq!(payload.non_numeric_cells, 1);
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "non-numeric-cells"));
    }

    #[test]
    fn non_numeric_never_satisfies_numeric_ne() {
        let plan = plan_c12(input(
            vec![file(
                vec![vec!["id".into()], vec!["1".into()], vec!["n/a".into()]],
                true,
                false,
            )],
            ColumnPick::Index(1),
            FilterOp::NumNe,
            "2",
            true,
        ))
        .unwrap();
        let payload = plan.entries[0].payload_csv_filter();
        assert_eq!(payload.matched_rows, 1);
        assert_eq!(payload.non_numeric_cells, 1);
    }

    #[test]
    fn missing_column_blocks_the_entry() {
        let plan = plan_c12(input(
            vec![file(records(), true, false)],
            ColumnPick::Name("missing".into()),
            FilterOp::Contains,
            "x",
            true,
        ))
        .unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3004");
    }

    #[test]
    fn non_numeric_value_is_rejected_for_numeric_ops() {
        let error = plan_c12(input(
            vec![file(records(), true, false)],
            ColumnPick::Index(2),
            FilterOp::NumGt,
            "abc",
            true,
        ))
        .unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn existing_target_is_marked_skip() {
        let plan = plan_c12(input(
            vec![file(records(), true, true)],
            ColumnPick::Index(1),
            FilterOp::Contains,
            "1",
            true,
        ))
        .unwrap();
        assert_eq!(
            plan.entries[0].conflict.as_ref().unwrap().kind,
            "existing-target"
        );
        assert_eq!(plan.entries[0].resolution, Some(PlanResolution::Skip));
    }

    trait PayloadExt {
        fn payload_csv_filter(&self) -> &PlanCsvFilter;
    }
    impl PayloadExt for PlanEntry {
        fn payload_csv_filter(&self) -> &PlanCsvFilter {
            self.csv_filter.as_ref().unwrap()
        }
    }
}
