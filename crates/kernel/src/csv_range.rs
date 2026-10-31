//!
//! 纯函数层：CSV 位置行区间（F09）的计划生成；本模块不读盘、不取时钟。
//!
//! 口径（方案 §5.15、TC F09）：
//! - X–Y 为 1-based **数据记录**（不含表头）；引号内换行仍算同一条记录；
//! - `endRow < startRow` 或 `startRow=0` 为非法区间（E1002）；
//! - 计划期扫描到第 Y 条数据记录即停；只有 Y 超出实际行数（或 X 超出）时才扫到 EOF，
//!   用于给出 `range-exceeds-rows` 告警；
//! - 表头与选中记录均记录字节区间，apply 只读取这些区间，不整读源文件；
//! - UTF-16 源与多行表头按既有错误码阻断（E3001/E3004），不猜测；
//! - 数据行字段数与表头不一致时只计数上报，不静默修正（T04）。

use crate::csv::Delimiter;
use crate::csv_extract::ensure_unique_targets;
use crate::csv_merge::{normalized_columns, source_block, target_block};
use crate::encoding::{InvalidSequencePolicy, SourceEncoding, TargetEncoding};
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::ProtectedDir;
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanConflict, PlanCsvRows, PlanDocument, PlanEntry,
    PlanExpected, PlanParamValue, PlanRange, PlanResolution, PlanStats, PlanWarning, RiskLevel,
};

pub const F09_KIND: &str = "f09";

/// F09 的单个输入；表头字段为壳层解码后的**原始**文本（计划层再规范化）。
#[derive(Clone, Debug)]
pub struct PlanF09File {
    pub path: String,
    pub canonical_path: Option<String>,
    pub file_size: u64,
    pub source_encoding: Option<SourceEncoding>,
    pub delimiter: Delimiter,
    pub delimiter_detected: bool,
    pub has_header: bool,
    pub columns: Vec<String>,
    pub column_count: u64,
    /// 计划期观察到的数据记录数（提前终止时等于 endRow 或实际总数）。
    pub data_rows: u64,
    /// 是否已扫描到 EOF；false 表示在 endRow 处提前终止。
    pub total_known: bool,
    pub selected_rows: u64,
    pub header_range: Option<(u64, u64)>,
    pub data_range: Option<(u64, u64)>,
    pub dirty_rows: u64,
    pub multi_line_header: bool,
    pub unclosed_quote: bool,
    pub target: String,
    pub target_occupied: bool,
}

#[derive(Clone, Debug)]
pub struct PlanF09Input {
    pub plan_id: String,
    pub target_encoding: TargetEncoding,
    pub invalid: InvalidSequencePolicy,
    pub start_row: u64,
    pub end_row: u64,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub files: Vec<PlanF09File>,
}

/// 生成 F09 计划：每个输入文件一条 `csv-rows` 条目（1:1），复用行抽取动作。
pub fn plan_f09(input: PlanF09Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.start_row == 0 || input.end_row < input.start_row {
        return Err(KernelError::invalid_params(
            "startRow and endRow must be 1-based with endRow >= startRow",
        ));
    }
    if input.files.is_empty() {
        return Err(KernelError::missing_param(
            "row range plans require at least one file",
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
    let mut range_files = Vec::new();
    let mut unclosed_files = Vec::new();
    let mut delimiter_files = Vec::new();
    let mut empty_files = Vec::new();
    let mut header_only_files = Vec::new();
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
        if blocked.is_none() {
            blocked = match file.source_encoding {
                None => Some(PlanBlocked {
                    code: "E3001".to_string(),
                    reason: "source encoding requires manual confirmation".to_string(),
                    hint: Some(
                        "confirm the source encoding (or pass a global --source override)"
                            .to_string(),
                    ),
                }),
                Some(SourceEncoding::Utf16Le | SourceEncoding::Utf16Be) => Some(PlanBlocked {
                    code: "E3001".to_string(),
                    reason: "UTF-16 CSV must be transcoded before extracting a row range"
                        .to_string(),
                    hint: Some(
                        "run C01 (transcode to UTF-8/GB18030) first, then extract the range"
                            .to_string(),
                    ),
                }),
                _ => None,
            };
        }
        if blocked.is_none() && file.multi_line_header {
            blocked = Some(PlanBlocked {
                code: "E3004".to_string(),
                reason: "multi-line headers are not supported in v1".to_string(),
                hint: Some(
                    "keep a single header row before extracting a row range (方案 §5.3)"
                        .to_string(),
                ),
            });
        }
        if file.dirty_rows > 0 {
            dirty_files.push(format!("{} ({} row(s))", file.path, file.dirty_rows));
        }
        if file.total_known && input.end_row > file.data_rows {
            range_files.push(format!(
                "{} (requested {}-{}, available {})",
                file.path, input.start_row, input.end_row, file.data_rows
            ));
        }
        if file.unclosed_quote {
            unclosed_files.push(file.path.clone());
        }
        if !file.delimiter_detected {
            delimiter_files.push(file.path.clone());
        }
        if file.file_size == 0 {
            empty_files.push(file.path.clone());
            continue;
        }
        if has_header && file.data_rows == 0 {
            header_only_files.push(file.path.clone());
        }

        let conflict = file.target_occupied.then(|| PlanConflict {
            kind: "existing-target".to_string(),
            risk_level: RiskLevel::Danger,
        });
        let header_range = file
            .header_range
            .map(|(start, end)| PlanRange { start, end });
        let data_range = file.data_range.map(|(start, end)| PlanRange { start, end });
        let target_name = crate::conflict::file_name_of(&file.target);
        let reason = if file.selected_rows == 0 {
            format!(
                "take no data rows from {} into {} as {} (requested {}-{}, available {})",
                file.path,
                target_name,
                input.target_encoding.as_str(),
                input.start_row,
                input.end_row,
                file.data_rows
            )
        } else {
            format!(
                "take data rows {}-{} ({} selected) from {} into {} as {}",
                input.start_row,
                input.end_row,
                file.selected_rows,
                file.path,
                target_name,
                input.target_encoding.as_str()
            )
        };

        entries.push(PlanEntry {
            entry_id: format!("{}-{:04}", input.plan_id, entries.len() + 1),
            source: file.path.clone(),
            target: Some(file.target.clone()),
            action: PlanAction::CsvRows,
            relation: None,
            reason,
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
                window: "range".to_string(),
                count: file.selected_rows,
                start_row: Some(input.start_row),
                end_row: Some(input.end_row),
                header_range,
                data_range,
                total_known: file.total_known,
                target_encoding: input.target_encoding.as_str().to_string(),
                invalid: input.invalid.as_str().to_string(),
                dirty_rows: file.dirty_rows,
            }),
            csv_dedup: None,
        });
    }

    if !empty_files.is_empty() {
        warnings.push(PlanWarning {
            kind: "empty-file".to_string(),
            suggestion: Some(format!(
                "empty input files; no data rows can be selected: {}",
                empty_files.join(", ")
            )),
            risk_level: RiskLevel::Safe,
        });
    }
    if !header_only_files.is_empty() {
        warnings.push(PlanWarning {
            kind: "header-only".to_string(),
            suggestion: Some(format!(
                "input files have a header but no data rows: {}",
                header_only_files.join(", ")
            )),
            risk_level: RiskLevel::Safe,
        });
    }
    if !range_files.is_empty() {
        warnings.push(PlanWarning {
            kind: "range-exceeds-rows".to_string(),
            suggestion: Some(format!(
                "requested range exceeds available data rows; only rows within each file are written: {}",
                range_files.join(", ")
            )),
            risk_level: RiskLevel::Caution,
        });
    }
    if !dirty_files.is_empty() {
        warnings.push(PlanWarning {
            kind: "csv-row-width-mismatch".to_string(),
            suggestion: Some(format!(
                "selected records with a different field count are written unchanged: {}",
                dirty_files.join(", ")
            )),
            risk_level: RiskLevel::Caution,
        });
    }
    if !unclosed_files.is_empty() {
        warnings.push(PlanWarning {
            kind: "unclosed-quote".to_string(),
            suggestion: Some(format!(
                "unclosed quote; the remainder is kept as the last record: {}",
                unclosed_files.join(", ")
            )),
            risk_level: RiskLevel::Danger,
        });
    }
    if !delimiter_files.is_empty() {
        warnings.push(PlanWarning {
            kind: "delimiter-not-detected".to_string(),
            suggestion: Some(format!(
                "delimiter could not be detected; comma is assumed, specify --delimiter to override: {}",
                delimiter_files.join(", ")
            )),
            risk_level: RiskLevel::Caution,
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
        "startRow".to_string(),
        PlanParamValue::Number(input.start_row),
    );
    params.insert("endRow".to_string(), PlanParamValue::Number(input.end_row));
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
        kind: F09_KIND.to_string(),
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

    fn file(path: &str, columns: &[&str], data_rows: u64) -> PlanF09File {
        PlanF09File {
            path: path.to_string(),
            canonical_path: None,
            file_size: 100,
            source_encoding: Some(SourceEncoding::Utf8),
            delimiter: Delimiter::Comma,
            delimiter_detected: true,
            has_header: true,
            columns: columns.iter().map(|value| value.to_string()).collect(),
            column_count: columns.len() as u64,
            data_rows,
            total_known: true,
            selected_rows: 0,
            header_range: Some((0, 6)),
            data_range: None,
            dirty_rows: 0,
            multi_line_header: false,
            unclosed_quote: false,
            target: format!("{path}.out"),
            target_occupied: false,
        }
    }

    fn input(files: Vec<PlanF09File>, start_row: u64, end_row: u64) -> PlanF09Input {
        PlanF09Input {
            plan_id: PLAN_ID.to_string(),
            target_encoding: TargetEncoding::Utf8 { bom: false },
            invalid: InvalidSequencePolicy::ReplaceAndCount,
            start_row,
            end_row,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            files,
        }
    }

    #[test]
    fn builds_range_payload_with_header_and_data_offsets() {
        let mut ranged = file("a.csv", &["id", "name"], 5);
        ranged.selected_rows = 2;
        ranged.data_range = Some((20, 32));
        let plan = plan_f09(input(vec![ranged], 2, 3)).unwrap();
        assert_eq!(plan.kind, F09_KIND);
        assert_eq!(plan.entries.len(), 1);
        let payload = plan.entries[0].csv_rows.as_ref().unwrap();
        assert_eq!(payload.window, "range");
        assert_eq!(payload.start_row, Some(2));
        assert_eq!(payload.end_row, Some(3));
        assert_eq!(payload.count, 2);
        assert_eq!(payload.header_range, Some(PlanRange { start: 0, end: 6 }));
        assert_eq!(payload.data_range, Some(PlanRange { start: 20, end: 32 }));
        assert!(payload.total_known);
        assert_eq!(plan.params["startRow"], PlanParamValue::Number(2));
        assert_eq!(plan.params["endRow"], PlanParamValue::Number(3));
    }

    #[test]
    fn invalid_ranges_are_rejected() {
        let ranged = file("a.csv", &["id"], 5);
        assert_eq!(
            plan_f09(input(vec![ranged.clone()], 0, 1))
                .unwrap_err()
                .code,
            "E1002"
        );
        assert_eq!(
            plan_f09(input(vec![ranged], 4, 3)).unwrap_err().code,
            "E1002"
        );
    }

    #[test]
    fn range_beyond_available_rows_warns_and_marks_selected_rows() {
        let mut ranged = file("a.csv", &["id"], 2);
        ranged.selected_rows = 1;
        ranged.data_range = Some((10, 14));
        let plan = plan_f09(input(vec![ranged], 2, 5)).unwrap();
        assert_eq!(plan.entries[0].csv_rows.as_ref().unwrap().count, 1);
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "range-exceeds-rows"));
    }

    #[test]
    fn empty_file_is_reported_without_an_entry() {
        let mut empty = file("a.csv", &[], 0);
        empty.file_size = 0;
        empty.header_range = None;
        let plan = plan_f09(input(vec![empty], 1, 1)).unwrap();
        assert!(plan.entries.is_empty());
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "empty-file"));
    }

    #[test]
    fn encoding_and_multiline_header_block_the_entry() {
        let mut ambiguous = file("a.csv", &["id"], 1);
        ambiguous.source_encoding = None;
        let plan = plan_f09(input(vec![ambiguous], 1, 1)).unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3001");

        let mut multi = file("b.csv", &["id"], 1);
        multi.multi_line_header = true;
        let plan = plan_f09(input(vec![multi], 1, 1)).unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3004");
    }

    #[test]
    fn dirty_rows_are_reported_for_selected_range() {
        let mut dirty = file("a.csv", &["id"], 3);
        dirty.dirty_rows = 1;
        dirty.selected_rows = 1;
        dirty.data_range = Some((10, 14));
        let plan = plan_f09(input(vec![dirty], 1, 1)).unwrap();
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "csv-row-width-mismatch"));
    }

    #[test]
    fn headerless_payload_has_no_columns() {
        let mut headerless = file("a.csv", &[], 3);
        headerless.has_header = false;
        headerless.column_count = 3;
        headerless.header_range = None;
        headerless.selected_rows = 1;
        headerless.data_range = Some((0, 8));
        let plan = plan_f09(input(vec![headerless], 1, 1)).unwrap();
        let payload = plan.entries[0].csv_rows.as_ref().unwrap();
        assert!(!payload.has_header);
        assert!(payload.columns.is_empty());
        assert_eq!(payload.header_range, None);
    }

    #[test]
    fn occupied_target_is_a_skip_conflict() {
        let mut occupied = file("a.csv", &["id"], 1);
        occupied.target_occupied = true;
        occupied.selected_rows = 1;
        occupied.data_range = Some((10, 14));
        let plan = plan_f09(input(vec![occupied], 1, 1)).unwrap();
        let entry = &plan.entries[0];
        assert_eq!(entry.conflict.as_ref().unwrap().kind, "existing-target");
        assert_eq!(entry.resolution, Some(PlanResolution::Skip));
    }

    #[test]
    fn duplicate_targets_and_mixed_header_modes_are_rejected() {
        let mut first = file("a.csv", &["id"], 1);
        first.target = "/out/data.csv".to_string();
        first.selected_rows = 1;
        first.data_range = Some((10, 14));
        let mut second = file("b.csv", &["id"], 1);
        second.target = "/out/data.csv".to_string();
        second.selected_rows = 1;
        second.data_range = Some((10, 14));
        assert_eq!(
            plan_f09(input(vec![first, second], 1, 1)).unwrap_err().code,
            "E1002"
        );

        let mut headerless = file("b.csv", &[], 1);
        headerless.has_header = false;
        headerless.header_range = None;
        headerless.selected_rows = 1;
        headerless.data_range = Some((0, 8));
        let error = plan_f09(input(vec![file("a.csv", &["id"], 1), headerless], 1, 1)).unwrap_err();
        assert_eq!(error.code, "E1002");
    }
}
