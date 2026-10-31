//!
//! 纯函数层：表头规范化、列名/列序号解析与计划条目全部由壳层准备好的输入
//! （每个文件的编码、分隔符、表头字段与列宽）决定；本模块不读盘、不取时钟。
//!
//! 口径（方案 §5.15、§5.4 表头规范化的异常处理）：
//! - 有表头时按**规范化后的表头名**或 1-based 列序号选择；输出保留用户给出的
//!   选择顺序，数据行同样按该顺序抽取，原行序不变；
//! - 表头名重复时按名称选择只取**首次出现**并告警；要选另一列请改用列序号；
//! - 无表头时必须用列序号选择，名称选择按 E3004 阻断（不猜测）；
//! - 选中的列不存在（名称未命中或序号越界）按 E3004 阻断，不静默补空；
//! - 同一列被重复选择属于非法参数（E1002）；
//! - 数据行字段数不足时该位置输出空字段，多余字段忽略，并计数上报；
//! - 本切片按整文件读取后解析；大文件流式抽取待补（F07 台账缺口）。

use std::collections::BTreeSet;

use crate::csv::Delimiter;
use crate::csv_merge::{normalize_header_name, normalized_columns, source_block, target_block};
use crate::encoding::{InvalidSequencePolicy, SourceEncoding, TargetEncoding};
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::{try_normalize, ProtectedDir};
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanConflict, PlanCsvExtract, PlanCsvExtractColumn,
    PlanDocument, PlanEntry, PlanExpected, PlanParamValue, PlanResolution, PlanStats, PlanWarning,
    RiskLevel,
};

pub const F07_KIND: &str = "f07";

/// 用户选择的列：表头名（规范化后比对）或 1-based 列序号。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColumnPick {
    Name(String),
    Index(u64),
}

/// F07 的单个输入；表头字段为壳层解码后的**原始**文本（计划层再规范化）。
#[derive(Clone, Debug)]
pub struct PlanF07File {
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
pub struct PlanF07Input {
    pub plan_id: String,
    pub target_encoding: TargetEncoding,
    pub invalid: InvalidSequencePolicy,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub selections: Vec<ColumnPick>,
    pub files: Vec<PlanF07File>,
}

/// 生成 F07 计划：每个输入文件一条 `csv-extract` 条目（1:1）。
#[allow(clippy::too_many_lines)]
pub fn plan_f07(input: PlanF07Input) -> Result<PlanDocument, KernelError> {
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
            "extract plans require at least one file",
        ));
    }
    if input.selections.is_empty() {
        return Err(KernelError::missing_param(
            "extract plans require at least one selected column",
        ));
    }
    for selection in &input.selections {
        if let ColumnPick::Name(name) = selection {
            if normalize_header_name(name).is_empty() {
                return Err(KernelError::invalid_params(
                    "selected column names must not be empty",
                ));
            }
        }
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

        let mut selections = Vec::with_capacity(input.selections.len());
        if blocked.is_none() {
            match resolve_selections(
                &input.selections,
                &file.path,
                has_header,
                &normalized,
                file.column_count,
            ) {
                Ok((resolved, duplicate_warning)) => {
                    selections = resolved;
                    if let Some(warning) = duplicate_warning {
                        warnings.push(warning);
                    }
                }
                Err(ResolutionError::Blocked(blocked_entry)) => blocked = Some(blocked_entry),
                Err(ResolutionError::Invalid(error)) => return Err(error),
            }
        }
        if file.dirty_rows > 0 {
            dirty_files.push(format!("{} ({} row(s))", file.path, file.dirty_rows));
        }

        let conflict = file.target_occupied.then(|| PlanConflict {
            kind: "existing-target".to_string(),
            risk_level: RiskLevel::Danger,
        });
        let output_columns = selections.len() as u64;
        let target_name = crate::conflict::file_name_of(&file.target);
        entries.push(PlanEntry {
            entry_id: format!("{}-{:04}", input.plan_id, entries.len() + 1),
            source: file.path.clone(),
            target: Some(file.target.clone()),
            action: PlanAction::CsvExtract,
            relation: None,
            reason: format!(
                "extract {} column(s) from {} into {} as {}",
                output_columns,
                file.path,
                target_name,
                input.target_encoding.as_str()
            ),
            conflict: conflict.clone(),
            resolution: conflict.as_ref().map(|_| PlanResolution::Skip),
            blocked: blocked.clone(),
            expected: Some(PlanExpected {
                size: file.file_size,
                algorithm: "size".to_string(),
                hash: None,
            }),
            range: None,
            merge: None,
            csv: None,
            csv_merge: None,
            csv_extract: Some(PlanCsvExtract {
                encoding: file.source_encoding,
                size: file.file_size,
                delimiter: file.delimiter.as_str().to_string(),
                has_header,
                columns: if has_header {
                    normalized.clone()
                } else {
                    Vec::new()
                },
                column_count: file.column_count,
                selections,
                target_encoding: input.target_encoding.as_str().to_string(),
                invalid: input.invalid.as_str().to_string(),
                dirty_rows: file.dirty_rows,
            }),
            csv_rows: None,
            csv_dedup: None,
        });
    }

    if !dirty_files.is_empty() {
        warnings.push(PlanWarning {
            kind: "csv-row-width-mismatch".to_string(),
            suggestion: Some(format!(
                "records with a different field count (missing selected fields are written empty): {}",
                dirty_files.join(", ")
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
        "columns".to_string(),
        PlanParamValue::String(display_selections(&input.selections)),
    );
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
        kind: F07_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

enum ResolutionError {
    Blocked(PlanBlocked),
    Invalid(KernelError),
}

fn resolve_selections(
    picks: &[ColumnPick],
    path: &str,
    has_header: bool,
    normalized: &[String],
    column_count: u64,
) -> Result<(Vec<PlanCsvExtractColumn>, Option<PlanWarning>), ResolutionError> {
    let width = if has_header {
        normalized.len() as u64
    } else {
        column_count
    };
    let mut selections = Vec::with_capacity(picks.len());
    let mut seen = BTreeSet::new();
    let mut duplicate_warning: Option<PlanWarning> = None;

    for pick in picks {
        let (index, name) = match pick {
            ColumnPick::Name(name) => {
                if !has_header {
                    return Err(ResolutionError::Blocked(PlanBlocked {
                        code: "E3004".to_string(),
                        reason: format!(
                            "{} has no header row; select columns by index instead of name '{name}'",
                            path
                        ),
                        hint: Some("drop --no-header or select --column-index".to_string()),
                    }));
                }
                let key = normalize_header_name(name);
                let matches: Vec<usize> = normalized
                    .iter()
                    .enumerate()
                    .filter(|(_, value)| *value == &key)
                    .map(|(index, _)| index)
                    .collect();
                let Some(first) = matches.first().copied() else {
                    return Err(ResolutionError::Blocked(PlanBlocked {
                        code: "E3004".to_string(),
                        reason: format!("{} has no column named '{key}'", path),
                        hint: Some(
                            "check the header names or select by --column-index".to_string(),
                        ),
                    }));
                };
                if matches.len() > 1 && duplicate_warning.is_none() {
                    duplicate_warning = Some(PlanWarning {
                        kind: "duplicate-column-name".to_string(),
                        suggestion: Some(format!(
                            "{} has {} columns named '{key}'; the first occurrence is used",
                            path,
                            matches.len()
                        )),
                        risk_level: RiskLevel::Caution,
                    });
                }
                (first as u64, Some(key))
            }
            ColumnPick::Index(value) => {
                let zero_based = value.saturating_sub(1);
                if *value == 0 || zero_based >= width {
                    return Err(ResolutionError::Blocked(PlanBlocked {
                        code: "E3004".to_string(),
                        reason: format!(
                            "{} has {} column(s); column index {} is out of range",
                            path, width, value
                        ),
                        hint: Some(
                            "use a 1-based column index within the header width".to_string(),
                        ),
                    }));
                }
                (
                    zero_based,
                    has_header.then(|| normalized[zero_based as usize].clone()),
                )
            }
        };
        if !seen.insert(index) {
            return Err(ResolutionError::Invalid(KernelError::invalid_params(
                format!("column {} is selected more than once", index + 1),
            )));
        }
        selections.push(PlanCsvExtractColumn { index, name });
    }
    Ok((selections, duplicate_warning))
}

pub(crate) fn ensure_unique_targets<'a>(
    targets: impl IntoIterator<Item = &'a str>,
    sense: CaseSense,
) -> Result<(), KernelError> {
    let mut seen = BTreeSet::new();
    for target in targets {
        let normalized = try_normalize(target)?;
        let key = match sense {
            CaseSense::Sensitive => normalized,
            CaseSense::CaseInsensitive => normalized.to_lowercase(),
        };
        if !seen.insert(key) {
            return Err(KernelError::invalid_params(format!(
                "multiple inputs map to the same target: {target}"
            )));
        }
    }
    Ok(())
}

fn display_selections(picks: &[ColumnPick]) -> String {
    picks
        .iter()
        .map(|pick| match pick {
            ColumnPick::Name(name) => normalize_header_name(name),
            ColumnPick::Index(index) => format!("#{index}"),
        })
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    fn file(path: &str, columns: &[&str]) -> PlanF07File {
        PlanF07File {
            path: path.to_string(),
            canonical_path: None,
            file_size: 10,
            source_encoding: Some(SourceEncoding::Utf8),
            delimiter: Delimiter::Comma,
            has_header: true,
            columns: columns.iter().map(|value| value.to_string()).collect(),
            column_count: columns.len() as u64,
            data_rows: 1,
            dirty_rows: 0,
            target: format!("{path}.out"),
            target_occupied: false,
        }
    }

    fn input(files: Vec<PlanF07File>, selections: Vec<ColumnPick>) -> PlanF07Input {
        PlanF07Input {
            plan_id: PLAN_ID.to_string(),
            target_encoding: TargetEncoding::Utf8 { bom: false },
            invalid: InvalidSequencePolicy::ReplaceAndCount,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            selections,
            files,
        }
    }

    #[test]
    fn selects_columns_by_name_in_given_order() {
        let plan = plan_f07(input(
            vec![file("a.csv", &[" id ", "name", "age"])],
            vec![
                ColumnPick::Name("age".to_string()),
                ColumnPick::Name("id".to_string()),
            ],
        ))
        .unwrap();
        assert_eq!(plan.kind, F07_KIND);
        assert_eq!(plan.entries.len(), 1);
        let entry = &plan.entries[0];
        assert_eq!(entry.action, PlanAction::CsvExtract);
        let payload = entry.csv_extract.as_ref().unwrap();
        assert_eq!(
            payload.selections,
            vec![
                PlanCsvExtractColumn {
                    index: 2,
                    name: Some("age".to_string())
                },
                PlanCsvExtractColumn {
                    index: 0,
                    name: Some("id".to_string())
                }
            ]
        );
        assert_eq!(
            plan.params["columns"],
            PlanParamValue::String("age,id".into())
        );
    }

    #[test]
    fn selects_columns_by_one_based_index() {
        let plan = plan_f07(input(
            vec![file("a.csv", &["id", "name", "age"])],
            vec![ColumnPick::Index(3), ColumnPick::Index(1)],
        ))
        .unwrap();
        let payload = plan.entries[0].csv_extract.as_ref().unwrap();
        assert_eq!(payload.selections[0].index, 2);
        assert_eq!(payload.selections[0].name.as_deref(), Some("age"));
        assert_eq!(payload.selections[1].index, 0);
    }

    #[test]
    fn duplicate_header_names_use_first_occurrence_and_warn() {
        let plan = plan_f07(input(
            vec![file("a.csv", &["name", "name", "id"])],
            vec![ColumnPick::Name("name".to_string())],
        ))
        .unwrap();
        let payload = plan.entries[0].csv_extract.as_ref().unwrap();
        assert_eq!(payload.selections[0].index, 0);
        assert_eq!(plan.warnings[0].kind, "duplicate-column-name".to_string());
        assert_eq!(plan.stats.warning, 1);
    }

    #[test]
    fn missing_column_name_blocks_with_e3004() {
        let plan = plan_f07(input(
            vec![file("a.csv", &["id", "name"])],
            vec![ColumnPick::Name("age".to_string())],
        ))
        .unwrap();
        let blocked = plan.entries[0].blocked.as_ref().unwrap();
        assert_eq!(blocked.code, "E3004");
        assert_eq!(plan.stats.blocked, 1);
    }

    #[test]
    fn out_of_range_index_blocks_with_e3004() {
        let plan = plan_f07(input(
            vec![file("a.csv", &["id", "name"])],
            vec![ColumnPick::Index(3)],
        ))
        .unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3004");
    }

    #[test]
    fn selecting_the_same_column_twice_is_rejected() {
        let error = plan_f07(input(
            vec![file("a.csv", &["id", "name"])],
            vec![ColumnPick::Name("id".to_string()), ColumnPick::Index(1)],
        ))
        .unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn headerless_files_require_index_selection() {
        let mut headerless = file("a.csv", &[]);
        headerless.has_header = false;
        headerless.column_count = 3;
        let plan = plan_f07(input(
            vec![headerless],
            vec![ColumnPick::Name("id".to_string())],
        ))
        .unwrap();
        let blocked = plan.entries[0].blocked.as_ref().unwrap();
        assert_eq!(blocked.code, "E3004");
        assert!(blocked.reason.contains("no header row"));
    }

    #[test]
    fn headerless_files_select_by_position() {
        let mut headerless = file("a.csv", &[]);
        headerless.has_header = false;
        headerless.column_count = 3;
        let plan = plan_f07(input(vec![headerless], vec![ColumnPick::Index(2)])).unwrap();
        let payload = plan.entries[0].csv_extract.as_ref().unwrap();
        assert_eq!(payload.selections[0].index, 1);
        assert!(payload.selections[0].name.is_none());
        assert!(payload.columns.is_empty());
    }

    #[test]
    fn ambiguous_encoding_blocks_with_e3001() {
        let mut ambiguous = file("a.csv", &["id"]);
        ambiguous.source_encoding = None;
        let plan = plan_f07(input(vec![ambiguous], vec![ColumnPick::Index(1)])).unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3001");
    }

    #[test]
    fn occupied_target_is_marked_as_skip_conflict() {
        let mut occupied = file("a.csv", &["id"]);
        occupied.target_occupied = true;
        let plan = plan_f07(input(vec![occupied], vec![ColumnPick::Index(1)])).unwrap();
        let entry = &plan.entries[0];
        assert_eq!(entry.conflict.as_ref().unwrap().kind, "existing-target");
        assert_eq!(entry.resolution, Some(PlanResolution::Skip));
        assert_eq!(plan.stats.conflict, 1);
    }

    #[test]
    fn duplicate_targets_are_rejected() {
        let mut first = file("a.csv", &["id"]);
        first.target = "/out/data.csv".to_string();
        let mut second = file("b.csv", &["id"]);
        second.target = "/out/data.csv".to_string();
        let error = plan_f07(input(vec![first, second], vec![ColumnPick::Index(1)])).unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn mixed_header_modes_are_rejected() {
        let mut headerless = file("b.csv", &[]);
        headerless.has_header = false;
        headerless.column_count = 2;
        let error = plan_f07(input(
            vec![file("a.csv", &["id"]), headerless],
            vec![ColumnPick::Index(1)],
        ))
        .unwrap_err();
        assert_eq!(error.code, "E1002");
    }
}
