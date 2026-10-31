//!
//! C11 CSV 去重计划与纯函数（方案 §5.15）。
//!
//! 整行或指定列拼键，按首次出现保留；表头单独保留，脏行只上报不修正。
//! 本模块不读盘；壳层提供解析后的记录。

use std::collections::{BTreeMap, HashSet};

use crate::csv_extract::ColumnPick;
use crate::csv_merge::normalize_header_name;
use crate::encoding::{InvalidSequencePolicy, SourceEncoding, TargetEncoding};
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::ProtectedDir;
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanConflict, PlanCsvDedup, PlanCsvDedupColumn, PlanDocument,
    PlanEntry, PlanExpected, PlanParamValue, PlanResolution, PlanStats, PlanWarning, RiskLevel,
};

pub const C11_KIND: &str = "c11";

#[derive(Clone, Debug)]
pub struct PlanC11File {
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
pub struct PlanC11Input {
    pub plan_id: String,
    pub target: TargetEncoding,
    pub invalid: InvalidSequencePolicy,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub key: Vec<ColumnPick>,
    pub files: Vec<PlanC11File>,
}

fn resolve_key_columns(
    file: &PlanC11File,
    picks: &[ColumnPick],
) -> Result<Vec<PlanCsvDedupColumn>, PlanBlocked> {
    if picks.is_empty() {
        return Ok(Vec::new());
    }
    if !file.has_header && picks.iter().any(|pick| matches!(pick, ColumnPick::Name(_))) {
        return Err(PlanBlocked {
            code: "E3004".to_string(),
            reason: "column-name keys require a header row".to_string(),
            hint: Some("use --key-column-index or mark the input as headerless".to_string()),
        });
    }
    let normalized: Vec<String> = file
        .columns
        .iter()
        .map(|column| normalize_header_name(column))
        .collect();
    let mut resolved = Vec::with_capacity(picks.len());
    for pick in picks {
        let (index, name) = match pick {
            ColumnPick::Name(name) => {
                let name = normalize_header_name(name);
                let Some(index) = normalized.iter().position(|column| column == &name) else {
                    return Err(PlanBlocked {
                        code: "E3004".to_string(),
                        reason: format!("key column not found: {name}"),
                        hint: Some("check the header or use a 1-based index".to_string()),
                    });
                };
                (index as u64, Some(name))
            }
            ColumnPick::Index(index) => {
                if *index == 0 || *index > file.column_count {
                    return Err(PlanBlocked {
                        code: "E3004".to_string(),
                        reason: format!("key column index out of range: {index}"),
                        hint: Some(format!("index must be between 1 and {}", file.column_count)),
                    });
                }
                (
                    index.saturating_sub(1),
                    normalized.get(*index as usize - 1).cloned(),
                )
            }
        };
        if resolved
            .iter()
            .any(|column: &PlanCsvDedupColumn| column.index == index)
        {
            return Err(PlanBlocked {
                code: "E1002".to_string(),
                reason: "duplicate key column selection".to_string(),
                hint: Some("select each key column once".to_string()),
            });
        }
        resolved.push(PlanCsvDedupColumn { index, name });
    }
    Ok(resolved)
}

fn key_of(record: &[String], key_columns: &[PlanCsvDedupColumn]) -> String {
    if key_columns.is_empty() {
        serde_json::to_string(record).unwrap_or_default()
    } else {
        let values: Vec<&str> = key_columns
            .iter()
            .map(|column| {
                record
                    .get(column.index as usize)
                    .map(String::as_str)
                    .unwrap_or("")
            })
            .collect();
        serde_json::to_string(&values).unwrap_or_default()
    }
}

/// 返回（唯一数据行，重复行数）。
pub fn dedup_records(
    records: &[Vec<String>],
    has_header: bool,
    key_columns: &[PlanCsvDedupColumn],
) -> (Vec<Vec<String>>, u64) {
    let data: &[Vec<String>] = if has_header {
        records.get(1..).unwrap_or(&[])
    } else {
        records
    };
    let mut seen = HashSet::new();
    let mut unique = Vec::new();
    let mut duplicates = 0u64;
    for record in data {
        if seen.insert(key_of(record, key_columns)) {
            unique.push(record.clone());
        } else {
            duplicates += 1;
        }
    }
    (unique, duplicates)
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

pub fn plan_c11(input: PlanC11Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.files.is_empty() {
        return Err(KernelError::missing_param("c11 requires at least one file"));
    }
    let mut targets = BTreeMap::new();
    for file in &input.files {
        let key = crate::path::normalize(&file.output);
        if targets.insert(key, &file.path).is_some() {
            return Err(KernelError::invalid_params(
                "duplicate output targets in one dedup plan",
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
        "key".to_string(),
        PlanParamValue::String(if input.key.is_empty() {
            "row".to_string()
        } else {
            "columns".to_string()
        }),
    );
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let mut duplicate_total = 0u64;
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
        let key_columns = if blocked.is_none() {
            match resolve_key_columns(file, &input.key) {
                Ok(columns) => columns,
                Err(error) => {
                    blocked = Some(error);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        if blocked.is_none() && file.records.is_empty() {
            blocked = Some(PlanBlocked {
                code: "E1002".to_string(),
                reason: "empty file has no rows".to_string(),
                hint: None,
            });
        }
        let (unique, duplicates) = if blocked.is_none() {
            dedup_records(&file.records, file.has_header, &key_columns)
        } else {
            (Vec::new(), 0)
        };
        duplicate_total += duplicates;
        let data_rows = file
            .records
            .len()
            .saturating_sub(usize::from(file.has_header)) as u64;
        let payload = PlanCsvDedup {
            encoding: file.source_encoding,
            size: file.file_size,
            delimiter: file.delimiter.as_str().to_string(),
            has_header: file.has_header,
            columns: file.columns.clone(),
            column_count: file.column_count,
            key: if key_columns.is_empty() {
                "row".to_string()
            } else {
                "columns".to_string()
            },
            key_columns: key_columns.clone(),
            target_encoding: input.target.as_str().to_string(),
            invalid: input.invalid.as_str().to_string(),
            data_rows,
            unique_rows: unique.len() as u64,
            duplicate_rows: duplicates,
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
                "dedup by {}: {} data rows -> {} unique, {} duplicates",
                if key_columns.is_empty() {
                    "row"
                } else {
                    "key columns"
                },
                data_rows,
                unique.len(),
                duplicates
            ),
        };
        if duplicates > 0 {
            warnings.push(PlanWarning {
                kind: "duplicates-removed".to_string(),
                suggestion: Some(format!(
                    "{} duplicate rows will be removed from {}",
                    duplicates, file.path
                )),
                risk_level: RiskLevel::Caution,
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
            action: PlanAction::CsvDedup,
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
            csv_dedup: Some(payload),
        });
    }
    if duplicate_total == 0 {
        warnings.push(PlanWarning {
            kind: "no-duplicates".to_string(),
            suggestion: Some("no duplicate rows were found".to_string()),
            risk_level: RiskLevel::Safe,
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
        kind: C11_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csv::Delimiter;

    fn file(records: Vec<Vec<String>>, header: bool, occupied: bool) -> PlanC11File {
        let columns = if header {
            records.first().cloned().unwrap_or_default()
        } else {
            Vec::new()
        };
        PlanC11File {
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

    fn input(files: Vec<PlanC11File>, key: Vec<ColumnPick>) -> PlanC11Input {
        PlanC11Input {
            plan_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            target: TargetEncoding::Utf8 { bom: false },
            invalid: InvalidSequencePolicy::ReplaceAndCount,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            key,
            files,
        }
    }

    #[test]
    fn full_row_dedup_keeps_first_occurrence() {
        let records = vec![
            vec!["id".into(), "name".into()],
            vec!["1".into(), "a".into()],
            vec!["2".into(), "b".into()],
            vec!["1".into(), "a".into()],
        ];
        let plan = plan_c11(input(vec![file(records, true, false)], Vec::new())).unwrap();
        assert_eq!(plan.entries[0].payload_csv_dedup().unique_rows, 2);
        assert_eq!(plan.entries[0].payload_csv_dedup().duplicate_rows, 1);
    }

    #[test]
    fn selected_columns_build_the_key() {
        let records = vec![
            vec!["id".into(), "name".into()],
            vec!["1".into(), "a".into()],
            vec!["1".into(), "other".into()],
            vec!["2".into(), "c".into()],
        ];
        let plan = plan_c11(input(
            vec![file(records, true, false)],
            vec![ColumnPick::Name("id".into())],
        ))
        .unwrap();
        let payload = plan.entries[0].payload_csv_dedup();
        assert_eq!(payload.key, "columns");
        assert_eq!(payload.key_columns[0].index, 0);
        assert_eq!(payload.unique_rows, 2);
        assert_eq!(payload.duplicate_rows, 1);
    }

    #[test]
    fn missing_key_column_blocks_the_entry() {
        let records = vec![
            vec!["id".into(), "name".into()],
            vec!["1".into(), "a".into()],
        ];
        let plan = plan_c11(input(
            vec![file(records, true, false)],
            vec![ColumnPick::Name("missing".into())],
        ))
        .unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3004");
    }

    #[test]
    fn duplicate_output_targets_are_rejected() {
        let records = vec![vec!["id".into()], vec!["1".into()]];
        let mut first = file(records.clone(), true, false);
        let mut second = file(records, true, false);
        second.path = "/data/b.csv".to_string();
        first.output = "/out/a.csv".to_string();
        second.output = "/out/a.csv".to_string();
        let error = plan_c11(input(vec![first, second], Vec::new())).unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn existing_target_is_marked_skip() {
        let records = vec![vec!["id".into()], vec!["1".into()]];
        let plan = plan_c11(input(vec![file(records, true, true)], Vec::new())).unwrap();
        assert_eq!(
            plan.entries[0].conflict.as_ref().unwrap().kind,
            "existing-target"
        );
        assert_eq!(plan.entries[0].resolution, Some(PlanResolution::Skip));
    }

    trait PayloadExt {
        fn payload_csv_dedup(&self) -> &PlanCsvDedup;
    }
    impl PayloadExt for PlanEntry {
        fn payload_csv_dedup(&self) -> &PlanCsvDedup {
            self.csv_dedup.as_ref().unwrap()
        }
    }
}
