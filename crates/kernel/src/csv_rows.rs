//!
//! 纯函数层：CSV 数量行窗口（前 N / 后 N）的计划生成；本模块不读盘、不取时钟。
//!
//! 口径（方案 §5.15）：
//! - "前 N"取数据行窗口的前 N 行；"后 N"取末 N 行；数据行不含表头；
//! - N=0 合法：有表头时只输出表头，无表头时输出空文件；
//! - N 大于数据行数时输出全部数据行并告警（不报错）；
//! - 数据行字段数与表头不一致时原样输出并计数上报，不静默修正（T04）；
//! - 有表头与无表头混用、编码未确认、目标重名等按既有错误码阻断；
//! - 本切片按整文件读取后解析；"后 N"的环形缓冲流式实现仍为缺口（F08 台账）。

use crate::csv::Delimiter;
use crate::csv_extract::ensure_unique_targets;
use crate::csv_merge::{normalized_columns, source_block, target_block};
use crate::encoding::{InvalidSequencePolicy, SourceEncoding, TargetEncoding};
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::ProtectedDir;
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanConflict, PlanCsvRows, PlanDocument, PlanEntry,
    PlanExpected, PlanParamValue, PlanResolution, PlanStats, PlanWarning, RiskLevel,
};

pub const F08_KIND: &str = "f08";

/// 行窗口方向：前 N、后 N 或 F09 的 X–Y 位置区间。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowWindow {
    First,
    Last,
    Range,
}

impl RowWindow {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Last => "last",
            Self::Range => "range",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "first" => Some(Self::First),
            "last" => Some(Self::Last),
            "range" => Some(Self::Range),
            _ => None,
        }
    }
}

/// F08 的单个输入；表头字段为壳层解码后的**原始**文本（计划层再规范化）。
#[derive(Clone, Debug)]
pub struct PlanF08File {
    pub path: String,
    pub canonical_path: Option<String>,
    pub file_size: u64,
    pub source_encoding: Option<SourceEncoding>,
    pub delimiter: Delimiter,
    pub has_header: bool,
    pub columns: Vec<String>,
    pub column_count: u64,
    pub data_rows: u64,
    /// 数据行字段数与列宽不一致的记录数。
    pub dirty_rows: u64,
    pub target: String,
    pub target_occupied: bool,
}

#[derive(Clone, Debug)]
pub struct PlanF08Input {
    pub plan_id: String,
    pub target_encoding: TargetEncoding,
    pub invalid: InvalidSequencePolicy,
    pub window: RowWindow,
    pub count: u64,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub files: Vec<PlanF08File>,
}

/// 生成 F08 计划：每个输入文件一条 `csv-rows` 条目（1:1）。
pub fn plan_f08(input: PlanF08Input) -> Result<PlanDocument, KernelError> {
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
            "row extraction plans require at least one file",
        ));
    }
    let has_header = input.files[0].has_header;
    if input.files.iter().any(|file| file.has_header != has_header) {
        return Err(KernelError::invalid_params(
            "inputs mix files with and without a header row",
        ));
    }
    ensure_unique_targets(
        input.files.iter().map(|file| file.target.as_str()),
        input.case_sense,
    )?;

    let mut warnings = Vec::new();
    let mut dirty_files = Vec::new();
    let mut exceed_files = Vec::new();
    let mut entries = Vec::with_capacity(input.files.len());

    for file in &input.files {
        let normalized = normalized_columns(&file.columns);
        let mut blocked = target_block(&file.target, &input.protected_dirs, input.case_sense);
        if blocked.is_none() {
            blocked = source_block(
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
                hint: Some(
                    "confirm the source encoding (or pass a global --source override)".to_string(),
                ),
            });
        }
        if file.dirty_rows > 0 {
            dirty_files.push(format!("{} ({} row(s))", file.path, file.dirty_rows));
        }
        if input.count > file.data_rows {
            exceed_files.push(format!(
                "{} ({} requested, {} available)",
                file.path, input.count, file.data_rows
            ));
        }

        let conflict = file.target_occupied.then(|| PlanConflict {
            kind: "existing-target".to_string(),
            risk_level: RiskLevel::Danger,
        });
        let selected = input.count.min(file.data_rows);
        let target_name = crate::conflict::file_name_of(&file.target);
        entries.push(PlanEntry {
            entry_id: format!("{}-{:04}", input.plan_id, entries.len() + 1),
            source: file.path.clone(),
            target: Some(file.target.clone()),
            action: PlanAction::CsvRows,
            relation: None,
            reason: format!(
                "take {} {} data row(s) from {} into {} as {}",
                selected,
                input.window.as_str(),
                file.path,
                target_name,
                input.target_encoding.as_str()
            ),
            conflict: conflict.clone(),
            resolution: conflict.as_ref().map(|_| PlanResolution::Skip),
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
            csv_rows: Some(PlanCsvRows {
                encoding: file.source_encoding,
                size: file.file_size,
                delimiter: file.delimiter.as_str().to_string(),
                has_header,
                columns: if has_header { normalized } else { Vec::new() },
                column_count: file.column_count,
                data_rows: file.data_rows,
                window: input.window.as_str().to_string(),
                count: input.count,
                start_row: None,
                end_row: None,
                header_range: None,
                data_range: None,
                total_known: false,
                target_encoding: input.target_encoding.as_str().to_string(),
                invalid: input.invalid.as_str().to_string(),
                dirty_rows: file.dirty_rows,
            }),
        });
    }

    if !dirty_files.is_empty() {
        warnings.push(PlanWarning {
            kind: "csv-row-width-mismatch".to_string(),
            suggestion: Some(format!(
                "records with a different field count are written unchanged: {}",
                dirty_files.join(", ")
            )),
            risk_level: RiskLevel::Caution,
        });
    }
    if !exceed_files.is_empty() {
        warnings.push(PlanWarning {
            kind: "count-exceeds-rows".to_string(),
            suggestion: Some(format!(
                "requested row count exceeds available data rows; all rows are written: {}",
                exceed_files.join(", ")
            )),
            risk_level: RiskLevel::Safe,
        });
    }

    let mut params = std::collections::BTreeMap::new();
    params.insert(
        "targetEncoding".to_string(),
        PlanParamValue::String(input.target_encoding.as_str().to_string()),
    );
    params.insert(
        "invalid".to_string(),
        PlanParamValue::String(input.invalid.as_str().to_string()),
    );
    params.insert("hasHeader".to_string(), PlanParamValue::Bool(has_header));
    params.insert(
        "window".to_string(),
        PlanParamValue::String(input.window.as_str().to_string()),
    );
    params.insert("count".to_string(), PlanParamValue::Number(input.count));
    params.insert(
        "fileCount".to_string(),
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
        kind: F08_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

/// 在已解析的数据行上选择前/后 N 行；后 N 用有界 `VecDeque`，选择内存为 O(N)。
pub fn select_row_window(rows: &[Vec<String>], window: RowWindow, count: u64) -> Vec<Vec<String>> {
    let count = count as usize;
    if count == 0 || rows.is_empty() {
        return Vec::new();
    }
    match window {
        RowWindow::First => rows.iter().take(count).cloned().collect(),
        RowWindow::Last => {
            let mut ring = std::collections::VecDeque::with_capacity(count.min(rows.len()));
            for row in rows {
                if ring.len() == count.min(rows.len()) {
                    ring.pop_front();
                }
                ring.push_back(row.clone());
            }
            ring.into()
        }
        RowWindow::Range => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    fn file(path: &str, columns: &[&str], data_rows: u64) -> PlanF08File {
        PlanF08File {
            path: path.to_string(),
            canonical_path: None,
            file_size: 10,
            source_encoding: Some(SourceEncoding::Utf8),
            delimiter: Delimiter::Comma,
            has_header: true,
            columns: columns.iter().map(|value| value.to_string()).collect(),
            column_count: columns.len() as u64,
            data_rows,
            dirty_rows: 0,
            target: format!("{path}.out"),
            target_occupied: false,
        }
    }

    fn input(files: Vec<PlanF08File>, window: RowWindow, count: u64) -> PlanF08Input {
        PlanF08Input {
            plan_id: PLAN_ID.to_string(),
            target_encoding: TargetEncoding::Utf8 { bom: false },
            invalid: InvalidSequencePolicy::ReplaceAndCount,
            window,
            count,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            files,
        }
    }

    #[test]
    fn first_window_builds_one_entry_per_file() {
        let plan = plan_f08(input(
            vec![file("a.csv", &["id", "name"], 3), file("b.csv", &["id"], 5)],
            RowWindow::First,
            2,
        ))
        .unwrap();
        assert_eq!(plan.kind, F08_KIND);
        assert_eq!(plan.entries.len(), 2);
        assert_eq!(plan.entries[0].action, PlanAction::CsvRows);
        let payload = plan.entries[0].csv_rows.as_ref().unwrap();
        assert_eq!(payload.window, "first");
        assert_eq!(payload.count, 2);
        assert!(payload.columns.len() == 2);
        assert_eq!(
            plan.params["window"],
            PlanParamValue::String("first".into())
        );
    }

    #[test]
    fn last_window_is_recorded_in_the_payload() {
        let plan = plan_f08(input(vec![file("a.csv", &["id"], 4)], RowWindow::Last, 3)).unwrap();
        let payload = plan.entries[0].csv_rows.as_ref().unwrap();
        assert_eq!(payload.window, "last");
        assert_eq!(payload.count, 3);
        assert!(plan.warnings.is_empty());
    }

    #[test]
    fn zero_count_is_valid_and_keeps_the_header_payload() {
        let plan = plan_f08(input(
            vec![file("a.csv", &["id", "name"], 3)],
            RowWindow::First,
            0,
        ))
        .unwrap();
        let payload = plan.entries[0].csv_rows.as_ref().unwrap();
        assert_eq!(payload.count, 0);
        assert_eq!(payload.columns, vec!["id", "name"]);
        assert!(plan.warnings.is_empty());
    }

    #[test]
    fn count_exceeding_rows_warns_and_writes_all() {
        let plan = plan_f08(input(vec![file("a.csv", &["id"], 2)], RowWindow::First, 5)).unwrap();
        assert_eq!(plan.warnings[0].kind, "count-exceeds-rows".to_string());
        assert_eq!(plan.stats.warning, 1);
    }

    #[test]
    fn dirty_rows_produce_a_width_mismatch_warning() {
        let mut drifted = file("a.csv", &["id", "name"], 2);
        drifted.dirty_rows = 1;
        let plan = plan_f08(input(vec![drifted], RowWindow::First, 1)).unwrap();
        assert_eq!(plan.warnings[0].kind, "csv-row-width-mismatch".to_string());
    }

    #[test]
    fn headerless_files_have_no_header_columns() {
        let mut headerless = file("a.csv", &[], 3);
        headerless.has_header = false;
        headerless.column_count = 3;
        let plan = plan_f08(input(vec![headerless], RowWindow::Last, 1)).unwrap();
        let payload = plan.entries[0].csv_rows.as_ref().unwrap();
        assert!(payload.columns.is_empty());
        assert_eq!(payload.column_count, 3);
    }

    #[test]
    fn ambiguous_encoding_blocks_with_e3001() {
        let mut ambiguous = file("a.csv", &["id"], 1);
        ambiguous.source_encoding = None;
        let plan = plan_f08(input(vec![ambiguous], RowWindow::First, 1)).unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3001");
    }

    #[test]
    fn occupied_target_is_marked_as_skip_conflict() {
        let mut occupied = file("a.csv", &["id"], 1);
        occupied.target_occupied = true;
        let plan = plan_f08(input(vec![occupied], RowWindow::First, 1)).unwrap();
        let entry = &plan.entries[0];
        assert_eq!(entry.conflict.as_ref().unwrap().kind, "existing-target");
        assert_eq!(entry.resolution, Some(PlanResolution::Skip));
    }

    #[test]
    fn duplicate_targets_and_mixed_header_modes_are_rejected() {
        let mut first = file("a.csv", &["id"], 1);
        first.target = "/out/data.csv".to_string();
        let mut second = file("b.csv", &["id"], 1);
        second.target = "/out/data.csv".to_string();
        let error = plan_f08(input(vec![first, second], RowWindow::First, 1)).unwrap_err();
        assert_eq!(error.code, "E1002");

        let mut headerless = file("b.csv", &[], 1);
        headerless.has_header = false;
        let error = plan_f08(input(
            vec![file("a.csv", &["id"], 1), headerless],
            RowWindow::First,
            1,
        ))
        .unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn select_window_takes_the_expected_rows() {
        let rows = vec![
            vec!["1".to_string()],
            vec!["2".to_string()],
            vec!["3".to_string()],
        ];
        assert_eq!(select_row_window(&rows, RowWindow::First, 2), rows[..2]);
        assert_eq!(select_row_window(&rows, RowWindow::Last, 2), rows[1..]);
        assert!(select_row_window(&rows, RowWindow::First, 0).is_empty());
        assert!(select_row_window(&[], RowWindow::Last, 2).is_empty());
        assert_eq!(select_row_window(&rows, RowWindow::Last, 9), rows);
    }
}
