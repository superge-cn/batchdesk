//! F06 CSV 合并（方案 §5.4：表头名对齐、缺失补空、额外列追加末尾）。
//!
//! 纯函数层：列并集、表头规范化、来源列与计划条目全部由壳层准备好的输入
//! （每个文件的编码、分隔符、表头字段与列数）决定；本模块不读盘、不取时钟。
//! 壳层负责采样与编码判定（`csv_merge_inspect`），apply 负责逐文件解码、
//! 按列并集映射字段并在临时文件上原子改名（设计 01 §5/§8）。
//!
//! 口径（方案 §5.4）：
//! - 有表头时按**规范化后的表头名**对齐：先剥离 BOM 残留与前后空白再比对；
//!   缺失列补空，额外列按首次出现顺序追加到并集末尾；重复列名按"名称+出现序号"
//!   区分，不丢弃数据；
//! - 无表头时按**列序号**对齐（并集宽度取各文件最大列数），计划期明确提示；
//! - 有表头与无表头混合无法可靠对齐，计划期按 E3004 阻断，不猜测；
//! - 严格模式下任一文件列集合（含重复次数）与并集不一致即按 E3004 阻断；
//! - 数据行字段数与表头不一致时只计数上报；多余字段追加在行尾，缺失字段补空，
//!   不静默修正（T04/T07/T08）；
//! - 本切片按整文件读取后解析；大文件首行零成本与第二遍流式写出待补。

use std::collections::{BTreeMap, BTreeSet};

use crate::csv::Delimiter;
use crate::encoding::{InvalidSequencePolicy, SourceEncoding, TargetEncoding};
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::{protected_by, try_normalize, ProtectedDir};
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanConflict, PlanCsvMerge, PlanCsvMergeFile, PlanDocument,
    PlanEntry, PlanExpected, PlanParamValue, PlanResolution, PlanStats, PlanWarning, RiskLevel,
};

pub const F06_KIND: &str = "f06";

/// F06 的单个输入；表头字段为壳层解码后的**原始**文本（计划层再规范化）。
#[derive(Clone, Debug)]
pub struct PlanF06File {
    pub path: String,
    pub canonical_path: Option<String>,
    pub file_size: u64,
    pub source_encoding: Option<SourceEncoding>,
    pub delimiter: Delimiter,
    pub has_header: bool,
    /// 原始表头字段（有表头时按文件顺序；无表头时为空）。
    pub columns: Vec<String>,
    /// 该文件的列数（含表头或数据首行的字段数），用于无表头按列序号对齐。
    pub column_count: u64,
    pub data_rows: u64,
    /// 数据行字段数与表头列数不一致的记录数。
    pub dirty_rows: u64,
    /// 来源列取值（文件名或相对路径）；未启用来源列时为 `None`。
    pub source_label: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PlanF06Input {
    pub plan_id: String,
    pub target: String,
    pub target_encoding: TargetEncoding,
    pub invalid: InvalidSequencePolicy,
    /// 输出分隔符；`None` 表示跟随第一个输入文件。
    pub output_delimiter: Option<Delimiter>,
    pub add_source_column: bool,
    /// 来源列名（方案 §5.4 默认"来源文件"由壳层传入；空值报错）。
    pub source_column_name: String,
    pub strict: bool,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub target_occupied: bool,
    pub files: Vec<PlanF06File>,
}

/// 剥离 BOM 残留并去除前后空白（方案 §5.4 表头规范化）。
pub fn normalize_header_name(raw: &str) -> String {
    raw.trim_start_matches('\u{feff}').trim().to_string()
}

/// 表头字段的规范化结果（保留重复项与出现顺序）。
pub fn normalized_columns(columns: &[String]) -> Vec<String> {
    columns
        .iter()
        .map(|name| normalize_header_name(name))
        .collect()
}

/// 为规范化后的列名生成"名称+出现序号"键，重复列名按第 0/1/... 次出现区分。
pub fn column_keys(columns: &[String]) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    columns
        .iter()
        .map(|name| {
            let occurrence = counts.entry(name.clone()).or_insert(0);
            let key = (name.clone(), *occurrence);
            *occurrence += 1;
            key
        })
        .collect()
}

/// 按首次出现顺序构建列并集（后续文件的新列追加到末尾）。
pub fn build_union(files: &[Vec<String>]) -> Vec<(String, usize)> {
    let mut seen: BTreeSet<(String, usize)> = BTreeSet::new();
    let mut union = Vec::new();
    for columns in files {
        for key in column_keys(columns) {
            if seen.insert(key.clone()) {
                union.push(key);
            }
        }
    }
    union
}

/// 并集键对应的输出表头名（重复列名保持重复，不重新命名）。
pub fn union_names(union: &[(String, usize)]) -> Vec<String> {
    union.iter().map(|(name, _)| name.clone()).collect()
}

/// RFC 4180 解析：把整段 UTF-8 字节解析为记录序列（字段均为 UTF-8 文本）。
///
/// 引号内换行属于同一记录；`""` 为转义引号；`\r\n`/`\n` 结束记录；
/// 引号外单独的 `\r` 忽略。空行按一条单字段空记录处理（与 F05 扫描器一致）。
pub fn parse_records(data: &[u8], delimiter: u8) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field: Vec<u8> = Vec::new();
    let mut in_quotes = false;
    let mut after_quote = false;
    let mut field_start = true;
    let mut saw_field = false;

    for &byte in data {
        if in_quotes {
            if after_quote {
                if byte == b'"' {
                    field.push(b'"');
                    after_quote = false;
                } else {
                    in_quotes = false;
                    after_quote = false;
                    if byte == delimiter {
                        finish_field(&mut record, &mut field, &mut saw_field);
                        field_start = true;
                    } else if byte == b'\n' {
                        finish_field(&mut record, &mut field, &mut saw_field);
                        finish_record(&mut records, &mut record, &mut saw_field, &mut field_start);
                    } else if byte != b'\r' {
                        field.push(byte);
                        field_start = false;
                        saw_field = true;
                    }
                }
                continue;
            }
            if byte == b'"' {
                after_quote = true;
            } else {
                field.push(byte);
            }
            continue;
        }

        if field_start && byte == b'"' {
            in_quotes = true;
            field_start = false;
            saw_field = true;
        } else if byte == delimiter {
            finish_field(&mut record, &mut field, &mut saw_field);
            field_start = true;
        } else if byte == b'\n' {
            finish_field(&mut record, &mut field, &mut saw_field);
            finish_record(&mut records, &mut record, &mut saw_field, &mut field_start);
        } else if byte != b'\r' {
            field.push(byte);
            field_start = false;
            saw_field = true;
        }
    }

    if !field.is_empty() || saw_field || !record.is_empty() {
        finish_field(&mut record, &mut field, &mut saw_field);
        records.push(std::mem::take(&mut record));
    }
    records
}

fn finish_field(record: &mut Vec<String>, field: &mut Vec<u8>, saw_field: &mut bool) {
    record.push(String::from_utf8_lossy(field).into_owned());
    field.clear();
    *saw_field = true;
}

fn finish_record(
    records: &mut Vec<Vec<String>>,
    record: &mut Vec<String>,
    saw_field: &mut bool,
    field_start: &mut bool,
) {
    records.push(std::mem::take(record));
    *saw_field = false;
    *field_start = true;
}

/// 按 RFC 4180 写出单条记录（字段含分隔符/引号/换行时加引号并转义）。
pub fn write_record(fields: &[String], delimiter: u8, out: &mut String) {
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            out.push(delimiter as char);
        }
        let needs_quote = field
            .bytes()
            .any(|byte| byte == delimiter || byte == b'"' || byte == b'\n' || byte == b'\r');
        if needs_quote {
            out.push('"');
            for character in field.chars() {
                if character == '"' {
                    out.push('"');
                }
                out.push(character);
            }
            out.push('"');
        } else {
            out.push_str(field);
        }
    }
    out.push('\n');
}

/// 生成 F06 计划：一条 `merge` 条目按输入顺序汇总全部文件。
#[allow(clippy::too_many_lines)]
pub fn plan_f06(input: PlanF06Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.target.trim().is_empty() {
        return Err(KernelError::missing_param("target is required"));
    }
    if input.files.is_empty() {
        return Err(KernelError::missing_param(
            "merge plans require at least one file",
        ));
    }
    if input.add_source_column && input.source_column_name.trim().is_empty() {
        return Err(KernelError::missing_param(
            "sourceColumnName is required when the source column is enabled",
        ));
    }

    let mut warnings = Vec::new();
    let mut blocked = target_block(&input.target, &input.protected_dirs, input.case_sense);
    let mut ambiguous = Vec::new();
    let mut detected = BTreeSet::new();
    let mut normalized = Vec::with_capacity(input.files.len());
    for file in &input.files {
        if blocked.is_none() {
            blocked = source_block(
                &file.path,
                file.canonical_path.as_deref(),
                &input.protected_dirs,
                input.case_sense,
            );
        }
        if let Some(encoding) = file.source_encoding {
            detected.insert(encoding.as_str());
        } else {
            ambiguous.push(file.path.clone());
        }
        normalized.push(normalized_columns(&file.columns));
    }
    if blocked.is_none() && !ambiguous.is_empty() {
        blocked = Some(PlanBlocked {
            code: "E3001".to_string(),
            reason: format!(
                "source encoding requires manual confirmation: {}",
                ambiguous.join(", ")
            ),
            hint: Some(
                "confirm each source encoding (or pass a global --source override)".to_string(),
            ),
        });
    }

    let has_header = input.files[0].has_header;
    let mixed_headers = input.files.iter().any(|file| file.has_header != has_header);
    if blocked.is_none() && mixed_headers {
        blocked = Some(PlanBlocked {
            code: "E3004".to_string(),
            reason: "inputs mix files with and without a header row".to_string(),
            hint: Some(
                "use --no-header for all inputs or provide a header for every file".to_string(),
            ),
        });
    }
    if blocked.is_none() && !has_header && input.add_source_column {
        blocked = Some(PlanBlocked {
            code: "E3004".to_string(),
            reason: "a source column requires header mode".to_string(),
            hint: Some("drop --source-column or add header rows to every input".to_string()),
        });
    }
    if blocked.is_none() && input.add_source_column {
        if let Some(file) = input.files.iter().find(|file| file.source_label.is_none()) {
            blocked = Some(PlanBlocked {
                code: "E3004".to_string(),
                reason: format!("source column value is missing for {}", file.path),
                hint: Some("provide a file name or relative path for every input".to_string()),
            });
        }
    }

    let union = if has_header {
        build_union(&normalized)
    } else {
        Vec::new()
    };
    let union_names_value = union_names(&union);
    let positional_columns = if has_header {
        union.len() as u64
    } else {
        input
            .files
            .iter()
            .map(|file| file.column_count)
            .max()
            .unwrap_or(0)
    };

    if blocked.is_none() && input.strict {
        if has_header {
            for (file, columns) in input.files.iter().zip(normalized.iter()) {
                let keys: BTreeSet<(String, usize)> = column_keys(columns).into_iter().collect();
                let union_keys: BTreeSet<(String, usize)> = union.iter().cloned().collect();
                if keys != union_keys {
                    let missing: Vec<String> = union_keys
                        .difference(&keys)
                        .map(|(name, occurrence)| display_column(name, *occurrence))
                        .collect();
                    blocked = Some(PlanBlocked {
                        code: "E3004".to_string(),
                        reason: format!(
                            "strict mode: {} does not match the merged schema (missing: {})",
                            file.path,
                            missing.join(", ")
                        ),
                        hint: Some(
                            "drop --strict to fill missing columns with empty values".to_string(),
                        ),
                    });
                    break;
                }
            }
        } else if input
            .files
            .iter()
            .any(|file| file.column_count != positional_columns)
        {
            blocked = Some(PlanBlocked {
                code: "E3004".to_string(),
                reason: format!(
                    "strict mode: column counts differ (expected {positional_columns})"
                ),
                hint: Some(
                    "drop --strict to align by column index and fill missing positions".to_string(),
                ),
            });
        }
    }

    if blocked.is_none() && !has_header {
        warnings.push(PlanWarning {
            kind: "no-header-positional".to_string(),
            suggestion: Some(format!(
                "inputs have no header row; columns are aligned by 1-based position (max width {positional_columns}); verify column order before applying"
            )),
            risk_level: RiskLevel::Caution,
        });
    }
    if blocked.is_none() && has_header {
        for (file, columns) in input.files.iter().zip(normalized.iter()) {
            let changed: Vec<String> = file
                .columns
                .iter()
                .zip(columns.iter())
                .filter(|(raw, normalized)| raw.as_str() != normalized.as_str())
                .map(|(_, normalized)| normalized.clone())
                .collect();
            if !changed.is_empty() {
                warnings.push(PlanWarning {
                    kind: "header-normalized".to_string(),
                    suggestion: Some(format!(
                        "{}: header fields were normalized (BOM/trim) before matching: {}",
                        file.path,
                        changed.join(", ")
                    )),
                    risk_level: RiskLevel::Caution,
                });
            }
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            for name in columns {
                *counts.entry(name.as_str()).or_insert(0) += 1;
            }
            let duplicates: Vec<String> = counts
                .iter()
                .filter(|(_, count)| **count > 1)
                .map(|(name, count)| format!("{name} x{count}"))
                .collect();
            if !duplicates.is_empty() {
                warnings.push(PlanWarning {
                    kind: "duplicate-column-names".to_string(),
                    suggestion: Some(format!(
                        "{}: duplicate header names are kept as separate columns by occurrence: {}",
                        file.path,
                        duplicates.join(", ")
                    )),
                    risk_level: RiskLevel::Caution,
                });
            }
        }
    }

    if blocked.is_none() && has_header {
        let first_keys: BTreeSet<(String, usize)> =
            column_keys(&normalized[0]).into_iter().collect();
        for (file, columns) in input.files.iter().zip(normalized.iter()) {
            let keys: BTreeSet<(String, usize)> = column_keys(columns).into_iter().collect();
            let union_keys: BTreeSet<(String, usize)> = union.iter().cloned().collect();
            let missing: Vec<String> = union_keys
                .difference(&keys)
                .map(|(name, occurrence)| display_column(name, *occurrence))
                .collect();
            if !missing.is_empty() {
                warnings.push(PlanWarning {
                    kind: "schema-missing-columns".to_string(),
                    suggestion: Some(format!(
                        "{} lacks columns (filled with empty values): {}",
                        file.path,
                        missing.join(", ")
                    )),
                    risk_level: RiskLevel::Caution,
                });
            }
            let extra: Vec<String> = keys
                .difference(&first_keys)
                .map(|(name, occurrence)| display_column(name, *occurrence))
                .collect();
            if !extra.is_empty() {
                warnings.push(PlanWarning {
                    kind: "schema-extra-columns".to_string(),
                    suggestion: Some(format!(
                        "{} contributes columns appended at the end: {}",
                        file.path,
                        extra.join(", ")
                    )),
                    risk_level: RiskLevel::Safe,
                });
            }
        }
    }
    let dirty_files: Vec<String> = input
        .files
        .iter()
        .filter(|file| file.dirty_rows > 0)
        .map(|file| format!("{} ({} row(s))", file.path, file.dirty_rows))
        .collect();
    if !dirty_files.is_empty() {
        warnings.push(PlanWarning {
            kind: "csv-row-width-mismatch".to_string(),
            suggestion: Some(format!(
                "records with a different field count (extra fields kept at row end, missing filled): {}",
                dirty_files.join(", ")
            )),
            risk_level: RiskLevel::Caution,
        });
    }
    if input
        .files
        .iter()
        .map(|file| file.delimiter.as_str())
        .collect::<BTreeSet<_>>()
        .len()
        > 1
    {
        warnings.push(PlanWarning {
            kind: "mixed-delimiters".to_string(),
            suggestion: Some(format!(
                "inputs use different delimiters; each file is parsed with its own and the output uses {}",
                input
                    .output_delimiter
                    .unwrap_or(input.files[0].delimiter)
                    .as_str()
            )),
            risk_level: RiskLevel::Caution,
        });
    }
    if detected.len() > 1 {
        warnings.push(PlanWarning {
            kind: "mixed-source-encodings".to_string(),
            suggestion: Some(format!(
                "inputs use {} encodings ({}); each file is decoded and re-encoded to {}",
                detected.len(),
                detected.iter().copied().collect::<Vec<_>>().join(", "),
                input.target_encoding.as_str()
            )),
            risk_level: RiskLevel::Caution,
        });
    }

    let conflict = input.target_occupied.then(|| PlanConflict {
        kind: "existing-target".to_string(),
        risk_level: RiskLevel::Danger,
    });
    let output_delimiter = input.output_delimiter.unwrap_or(input.files[0].delimiter);
    let output_columns = if has_header {
        union_names_value.len() as u64
    } else {
        positional_columns
    };
    let target_name = crate::conflict::file_name_of(&input.target);
    let csv_files: Vec<PlanCsvMergeFile> = input
        .files
        .iter()
        .zip(normalized.iter())
        .map(|(file, columns)| PlanCsvMergeFile {
            path: file.path.clone(),
            encoding: file.source_encoding,
            size: file.file_size,
            delimiter: file.delimiter.as_str().to_string(),
            columns: if file.has_header {
                columns.clone()
            } else {
                Vec::new()
            },
            source_label: file.source_label.clone(),
            dirty_rows: file.dirty_rows,
        })
        .collect();
    let entry = PlanEntry {
        entry_id: format!("{}-0001", input.plan_id),
        source: input.files[0].path.clone(),
        target: Some(input.target.clone()),
        action: PlanAction::Merge,
        relation: None,
        reason: format!(
            "merge {} CSV file(s) into {} as {} ({} column(s){})",
            input.files.len(),
            target_name,
            input.target_encoding.as_str(),
            output_columns,
            if input.add_source_column {
                ", source column first"
            } else {
                ""
            }
        ),
        conflict: conflict.clone(),
        resolution: conflict.as_ref().map(|_| PlanResolution::Skip),
        blocked,
        expected: Some(PlanExpected {
            size: input.files[0].file_size,
            algorithm: "size".to_string(),
            hash: None,
        }),
        range: None,
        merge: None,
        csv: None,
        csv_merge: Some(PlanCsvMerge {
            files: csv_files,
            union: union_names_value,
            positional_columns,
            has_header,
            source_column: input
                .add_source_column
                .then(|| input.source_column_name.trim().to_string()),
            target_encoding: input.target_encoding.as_str().to_string(),
            invalid: input.invalid.as_str().to_string(),
            delimiter: output_delimiter.as_str().to_string(),
            strict: input.strict,
        }),
        csv_extract: None,
        csv_rows: None,
        csv_dedup: None,
    };
    let entries = vec![entry];

    let mut params = std::collections::BTreeMap::new();
    params.insert("target".to_string(), PlanParamValue::String(input.target));
    params.insert(
        "targetEncoding".to_string(),
        PlanParamValue::String(input.target_encoding.as_str().to_string()),
    );
    params.insert(
        "invalid".to_string(),
        PlanParamValue::String(input.invalid.as_str().to_string()),
    );
    params.insert(
        "delimiter".to_string(),
        PlanParamValue::String(output_delimiter.as_str().to_string()),
    );
    params.insert("hasHeader".to_string(), PlanParamValue::Bool(has_header));
    params.insert(
        "columns".to_string(),
        PlanParamValue::Number(output_columns),
    );
    params.insert("strict".to_string(), PlanParamValue::Bool(input.strict));
    params.insert(
        "sourceColumn".to_string(),
        if input.add_source_column {
            PlanParamValue::String(input.source_column_name.trim().to_string())
        } else {
            PlanParamValue::Null
        },
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
        kind: F06_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

fn display_column(name: &str, occurrence: usize) -> String {
    if occurrence == 0 {
        name.to_string()
    } else {
        format!("{name}#{}", occurrence + 1)
    }
}

pub(crate) fn source_block(
    path: &str,
    canonical: Option<&str>,
    protected: &[ProtectedDir],
    sense: CaseSense,
) -> Option<PlanBlocked> {
    match try_normalize(path) {
        Ok(normalized) => {
            protected_by(&normalized, canonical, protected, sense).map(|hit| PlanBlocked {
                code: "E2009".to_string(),
                reason: format!("path is under protected directory {hit}"),
                hint: Some("remove it from protectedDirs or choose another source".to_string()),
            })
        }
        Err(error) => Some(PlanBlocked {
            code: error.code,
            reason: error.message,
            hint: None,
        }),
    }
}

pub(crate) fn target_block(
    path: &str,
    protected: &[ProtectedDir],
    sense: CaseSense,
) -> Option<PlanBlocked> {
    match try_normalize(path) {
        Ok(normalized) => {
            protected_by(&normalized, None, protected, sense).map(|hit| PlanBlocked {
                code: "E2009".to_string(),
                reason: format!("target is under protected directory {hit}"),
                hint: Some("choose another output path".to_string()),
            })
        }
        Err(error) => Some(PlanBlocked {
            code: error.code,
            reason: error.message,
            hint: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    fn file(path: &str, columns: &[&str], label: Option<&str>) -> PlanF06File {
        PlanF06File {
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
            source_label: label.map(str::to_string),
        }
    }

    fn input(files: Vec<PlanF06File>) -> PlanF06Input {
        PlanF06Input {
            plan_id: PLAN_ID.to_string(),
            target: "/out/merged.csv".to_string(),
            target_encoding: TargetEncoding::Utf8 { bom: false },
            invalid: InvalidSequencePolicy::ReplaceAndCount,
            output_delimiter: None,
            add_source_column: false,
            source_column_name: "来源文件".to_string(),
            strict: false,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            target_occupied: false,
            files,
        }
    }

    #[test]
    fn normalize_strips_bom_and_whitespace() {
        assert_eq!(normalize_header_name("\u{feff} 名称 "), "名称");
        assert_eq!(normalize_header_name("  id"), "id");
    }

    #[test]
    fn union_appends_new_columns_and_keeps_duplicates() {
        let union = build_union(&[
            vec!["id".to_string(), "name".to_string()],
            vec!["name".to_string(), "extra".to_string()],
            vec!["id".to_string(), "id".to_string()],
        ]);
        assert_eq!(
            union,
            vec![
                ("id".to_string(), 0),
                ("name".to_string(), 0),
                ("extra".to_string(), 0),
                ("id".to_string(), 1)
            ]
        );
        assert_eq!(union_names(&union), vec!["id", "name", "extra", "id"]);
    }

    #[test]
    fn parse_records_handles_quotes_and_multiline_fields() {
        let records = parse_records(b"id,name\r\n1,\"a, b\"\r\n2,\"line\r\nbreak\"\r\n", b',');
        assert_eq!(records.len(), 3);
        assert_eq!(records[0], vec!["id", "name"]);
        assert_eq!(records[1], vec!["1", "a, b"]);
        assert_eq!(records[2], vec!["2", "line\r\nbreak"]);
    }

    #[test]
    fn parse_records_supports_escaped_quotes_and_empty_fields() {
        let records = parse_records(b"1,\"say \"\"hi\"\"\"\n2,,3", b',');
        assert_eq!(records.len(), 2);
        assert_eq!(records[0], vec!["1", "say \"hi\""]);
        assert_eq!(records[1], vec!["2", "", "3"]);
    }

    #[test]
    fn write_record_quotes_only_when_needed() {
        let mut out = String::new();
        write_record(
            &[
                "plain".to_string(),
                "a,b".to_string(),
                "say \"hi\"".to_string(),
            ],
            b',',
            &mut out,
        );
        assert_eq!(out, "plain,\"a,b\",\"say \"\"hi\"\"\"\n");
    }

    #[test]
    fn plan_aligns_by_header_and_reports_schema_differences() {
        let plan = plan_f06(input(vec![
            file("/data/a.csv", &["id", "name"], None),
            file("/data/b.csv", &["name", "extra"], None),
        ]))
        .unwrap();
        assert_eq!(plan.kind, F06_KIND);
        let entry = &plan.entries[0];
        assert_eq!(entry.action, PlanAction::Merge);
        assert!(entry.merge.is_none());
        let payload = entry.csv_merge.as_ref().unwrap();
        assert_eq!(payload.union, vec!["id", "name", "extra"]);
        assert_eq!(payload.positional_columns, 3);
        assert!(payload.has_header);
        assert_eq!(payload.delimiter, ",");
        assert_eq!(payload.files[0].columns, vec!["id", "name"]);
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "schema-missing-columns"));
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "schema-extra-columns"));
        assert!(!plan.warnings.iter().any(|warning| matches!(
            warning.kind.as_str(),
            "no-header-positional" | "header-normalized" | "duplicate-column-names"
        )));
    }

    #[test]
    fn plan_inserts_source_column_name() {
        let plan = plan_f06(PlanF06Input {
            add_source_column: true,
            source_column_name: "来源文件".to_string(),
            ..input(vec![
                file("/data/a.csv", &["id"], Some("a.csv")),
                file("/data/b.csv", &["id"], Some("b.csv")),
            ])
        })
        .unwrap();
        let payload = plan.entries[0].csv_merge.as_ref().unwrap();
        assert_eq!(payload.source_column.as_deref(), Some("来源文件"));
        assert_eq!(payload.files[0].source_label.as_deref(), Some("a.csv"));
    }

    #[test]
    fn plan_blocks_strict_mode_on_schema_mismatch() {
        let plan = plan_f06(PlanF06Input {
            strict: true,
            ..input(vec![
                file("/data/a.csv", &["id", "name"], None),
                file("/data/b.csv", &["name"], None),
            ])
        })
        .unwrap();
        let blocked = plan.entries[0].blocked.as_ref().unwrap();
        assert_eq!(blocked.code, "E3004");
    }

    #[test]
    fn plan_no_header_aligns_by_position() {
        let plan = plan_f06(input(vec![
            PlanF06File {
                has_header: false,
                columns: Vec::new(),
                column_count: 2,
                ..file("/data/a.csv", &[], None)
            },
            PlanF06File {
                has_header: false,
                columns: Vec::new(),
                column_count: 3,
                ..file("/data/b.csv", &[], None)
            },
        ]))
        .unwrap();
        let payload = plan.entries[0].csv_merge.as_ref().unwrap();
        assert!(!payload.has_header);
        assert!(payload.union.is_empty());
        assert_eq!(payload.positional_columns, 3);
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "no-header-positional"));
    }

    #[test]
    fn plan_warns_when_headers_need_normalization_or_are_duplicated() {
        let plan = plan_f06(input(vec![file(
            "/data/t08.csv",
            &["姓名", "姓名", " 部门 ", "金额 "],
            None,
        )]))
        .unwrap();
        let kinds: Vec<&str> = plan
            .warnings
            .iter()
            .map(|warning| warning.kind.as_str())
            .collect();
        assert!(kinds.contains(&"header-normalized"));
        assert!(kinds.contains(&"duplicate-column-names"));
        let payload = plan.entries[0].csv_merge.as_ref().unwrap();
        assert_eq!(payload.union, vec!["姓名", "姓名", "部门", "金额"]);
        assert_eq!(payload.positional_columns, 4);
    }

    #[test]
    fn plan_blocks_source_column_without_header() {
        let plan = plan_f06(PlanF06Input {
            add_source_column: true,
            ..input(vec![PlanF06File {
                has_header: false,
                columns: Vec::new(),
                column_count: 2,
                ..file("/data/a.csv", &[], Some("a.csv"))
            }])
        })
        .unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3004");
    }

    #[test]
    fn plan_blocks_mixed_header_modes() {
        let plan = plan_f06(input(vec![
            file("/data/a.csv", &["id"], None),
            PlanF06File {
                has_header: false,
                columns: Vec::new(),
                column_count: 2,
                ..file("/data/b.csv", &[], None)
            },
        ]))
        .unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3004");
    }

    #[test]
    fn plan_blocks_manual_encoding_confirmation() {
        let plan = plan_f06(input(vec![PlanF06File {
            source_encoding: None,
            ..file("/data/a.csv", &["id"], None)
        }]))
        .unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3001");
    }

    #[test]
    fn existing_target_is_a_conflict_skipped_by_default() {
        let plan = plan_f06(PlanF06Input {
            target_occupied: true,
            ..input(vec![file("/data/a.csv", &["id"], None)])
        })
        .unwrap();
        let entry = &plan.entries[0];
        assert_eq!(entry.conflict.as_ref().unwrap().kind, "existing-target");
        assert_eq!(entry.resolution, Some(PlanResolution::Skip));
        assert_eq!(plan.stats.conflict, 1);
    }
}
