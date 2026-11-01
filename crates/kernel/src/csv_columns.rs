//! 纯函数层：C14 CSV 列拆分/合并计划与执行（方案 §5.15 一句话思路：
//! "拆分：按分隔符/正则切分并映射到新列；合并：多列拼接加分隔符"）。
//!
//! 口径（ADR-0035）：
//! - 拆分与合并互斥（同一 kind `c14`，payload `csvColumns` 按 mode 二选一）；
//! - 拆分：源列按字面分隔符或正则切段，**新列紧随源列之后**（源列保留，无损）；
//!   新列数 `parts` 为壳层采样评估的最大段数（§5.15 风险点"拆分后列数评估"——
//!   计划警告明示评估性质），执行期行内段数超出 parts 的部分**原样保留不截断**，
//!   不足补空串（与既有 dirty rows 口径一致）；
//! - 合并：按用户顺序把多列拼接为**一个新列追加在表尾**，源列保留；
//!   新列名缺省由各列名按 joiner 连接，无表头时无列名；
//! - 空值处理：切不出段的行补空串；任一源列缺失按空串参与拼接；均计入 dirty 统计；
//! - 无表头必须用列序号（E3004）；源列不存在按 E3004 阻断；正则非法按 E1005；
//!   merge 少于 2 列或重复选列按 E1002；
//! - 本模块不读盘、不取时钟（不变量 1）；文件事实（编码/表头/列宽/dirty）由壳层采集。

use std::collections::BTreeSet;

use crate::csv::Delimiter;
use crate::csv_extract::ColumnPick;
use crate::csv_merge::{normalize_header_name, normalized_columns, source_block, target_block};
use crate::encoding::{InvalidSequencePolicy, SourceEncoding, TargetEncoding};
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::ProtectedDir;
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanConflict, PlanCsvColumns, PlanCsvColumnsMerge,
    PlanCsvColumnsSplit, PlanCsvExtractColumn, PlanDocument, PlanEntry, PlanExpected,
    PlanParamValue, PlanResolution, PlanStats, PlanWarning, RiskLevel,
};

pub const C14_KIND: &str = "c14";

/// 拆分模式：字面分隔符或正则（二选一，计划期校验）。
#[derive(Clone, Debug)]
pub enum SplitBy {
    Separator(String),
    Regex(String),
}

#[derive(Clone, Debug)]
pub struct SplitSpec {
    pub source: ColumnPick,
    pub by: SplitBy,
}

#[derive(Clone, Debug)]
pub struct MergeSpec {
    pub picks: Vec<ColumnPick>,
    pub joiner: String,
    pub target_name: Option<String>,
}

#[derive(Clone)]
pub enum ColumnsMode {
    Split(SplitSpec),
    Merge(MergeSpec),
}

/// C14 的单个输入；文件事实由壳层采集（与 F07 同源）。
#[derive(Clone)]
pub struct PlanC14File {
    pub path: String,
    pub canonical_path: Option<String>,
    pub file_size: u64,
    pub source_encoding: Option<SourceEncoding>,
    pub delimiter: Delimiter,
    pub has_header: bool,
    pub columns: Vec<String>,
    pub column_count: u64,
    pub data_rows: u64,
    pub dirty_rows: u64,
    /// 拆分模式：壳层对采样记录按切分规则评估的**最大段数**（§5.15"列数评估"）。
    pub parts: u64,
    pub target: String,
    pub target_occupied: bool,
}

#[derive(Clone)]
pub struct PlanC14Input {
    pub plan_id: String,
    pub mode: ColumnsMode,
    pub target_encoding: TargetEncoding,
    pub invalid: InvalidSequencePolicy,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub files: Vec<PlanC14File>,
}

/// 按切分规则把一个字段值切段（字面分隔符或正则）；供壳层评估与执行器共用，
/// 保证两处语义一致。连续分隔符产生空段（保留，不压缩）。
pub fn split_value(value: &str, by: &SplitBy) -> Vec<String> {
    match by {
        SplitBy::Separator(separator) => {
            value.split(separator.as_str()).map(String::from).collect()
        }
        SplitBy::Regex(pattern) => crate::regex_rename::compile_regex(pattern)
            .map(|regex| regex.split(value).map(String::from).collect())
            .unwrap_or_else(|_| vec![value.to_string()]),
    }
}

/// 解析用户列选择 → (1-based 序号, 规范化列名)。与 F07 同语义：
/// 名称按规范化后比对取首次出现；无表头禁用名称选择；越界/未命中 E3004。
/// 解析用户列选择（壳层事实采集与计划层共用）：名称按规范化后比对取首次
/// 出现；无表头禁用名称选择；越界/未命中按 E3004。
pub fn resolve_column(
    pick: &ColumnPick,
    has_header: bool,
    columns: &[String],
    column_count: u64,
) -> Result<PlanCsvExtractColumn, KernelError> {
    let normalized = normalized_columns(columns);
    let index = match pick {
        ColumnPick::Name(name) => {
            if !has_header {
                return Err(KernelError::new(
                    "E3004",
                    "name-based column selection requires a header row",
                    None,
                    false,
                ));
            }
            let wanted = normalize_header_name(name);
            normalized
                .iter()
                .position(|column| *column == wanted)
                .map(|position| position as u64 + 1)
                .ok_or_else(|| {
                    KernelError::new("E3004", format!("column not found: {name}"), None, false)
                })?
        }
        ColumnPick::Index(index) => {
            if *index == 0 || *index > column_count {
                return Err(KernelError::new(
                    "E3004",
                    format!("column index out of range: {index}"),
                    None,
                    false,
                ));
            }
            *index
        }
    };
    Ok(PlanCsvExtractColumn {
        index,
        name: normalized.get(index as usize - 1).cloned(),
    })
}

fn resolve_pick(
    pick: &ColumnPick,
    file_path: &str,
    has_header: bool,
    columns: &[String],
    column_count: u64,
) -> Result<PlanCsvExtractColumn, KernelError> {
    resolve_column(pick, has_header, columns, column_count).map_err(|error| {
        KernelError::new(
            error.code,
            error.message,
            Some(file_path.to_string()),
            error.retryable,
        )
    })
}

/// 生成 C14 计划：每个输入文件一条 `csv-columns` 条目（1:1）。
pub fn plan_c14(input: PlanC14Input) -> Result<PlanDocument, KernelError> {
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
            "column plans require at least one file",
        ));
    }
    // 模式合法性在此统一校验（CLI 已分参数组，计划层兜底）。
    let (mode_name, split_payload, merge_payload) = match &input.mode {
        ColumnsMode::Split(spec) => {
            if input.files.iter().any(|file| file.parts == 0) {
                return Err(KernelError::invalid_params(
                    "split plans require an evaluated parts count of at least 1",
                ));
            }
            let (separator, regex) = match &spec.by {
                SplitBy::Separator(value) => {
                    if value.is_empty() {
                        return Err(KernelError::invalid_params(
                            "split separator must not be empty",
                        ));
                    }
                    (Some(value.clone()), None)
                }
                SplitBy::Regex(pattern) => {
                    if crate::regex_rename::compile_regex(pattern).is_err() {
                        return Err(KernelError::new(
                            "E1005",
                            "invalid split regex",
                            Some(pattern.clone()),
                            false,
                        ));
                    }
                    (None, Some(pattern.clone()))
                }
            };
            let first = input.files.first().expect("files non-empty");
            let normalized = normalized_columns(&first.columns);
            let source = resolve_pick(
                &spec.source,
                &first.path,
                first.has_header,
                &normalized,
                first.column_count,
            )?;
            let new_names = if first.has_header {
                let stem = source.name.clone().unwrap_or_default();
                (1..=first.parts)
                    .map(|part| format!("{stem}.{part}"))
                    .collect()
            } else {
                Vec::new()
            };
            (
                "split",
                Some(PlanCsvColumnsSplit {
                    index: source.index,
                    name: source.name,
                    separator,
                    regex,
                    parts: first.parts,
                    new_names,
                }),
                None,
            )
        }
        ColumnsMode::Merge(spec) => {
            if spec.picks.len() < 2 {
                return Err(KernelError::invalid_params(
                    "merge plans require at least two columns",
                ));
            }
            let mut seen = BTreeSet::new();
            for pick in &spec.picks {
                if !seen.insert(format!("{pick:?}")) {
                    return Err(KernelError::invalid_params(
                        "merge columns must not repeat a selection",
                    ));
                }
            }
            let first = input.files.first().expect("files non-empty");
            let normalized = normalized_columns(&first.columns);
            let mut resolved = Vec::with_capacity(spec.picks.len());
            for pick in &spec.picks {
                resolved.push(resolve_pick(
                    pick,
                    &first.path,
                    first.has_header,
                    &normalized,
                    first.column_count,
                )?);
            }
            let target_name = spec.target_name.clone().unwrap_or_else(|| {
                resolved
                    .iter()
                    .map(|column| column.name.clone().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join(joiner_of(spec))
            });
            (
                "merge",
                None,
                Some(PlanCsvColumnsMerge {
                    picks: resolved,
                    joiner: joiner_of(spec).to_string(),
                    target_name,
                }),
            )
        }
    };

    let has_header = input.files[0].has_header;
    if input.files.iter().any(|file| file.has_header != has_header) {
        return Err(KernelError::invalid_params(
            "inputs mix files with and without a header row",
        ));
    }
    crate::csv_extract::ensure_unique_targets(
        input.files.iter().map(|file| file.target.as_str()),
        input.case_sense,
    )?;

    let mut warnings = Vec::new();
    if let ColumnsMode::Split(spec) = &input.mode {
        warnings.push(PlanWarning {
            kind: "split-parts-estimate".to_string(),
            suggestion: Some(format!(
                "parts count is sampled from the source ({:?}); rows may produce more fields at apply time and are written as-is (方案 §5.15 列数评估)",
                match &spec.by {
                    SplitBy::Separator(value) => value.clone(),
                    SplitBy::Regex(pattern) => pattern.clone(),
                }
            )),
            risk_level: RiskLevel::Caution,
        });
    }
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
        if blocked.is_none() {
            // 逐文件复核列选择（各文件列宽可能不同）。
            let recheck = match &input.mode {
                ColumnsMode::Split(spec) => resolve_pick(
                    &spec.source,
                    &file.path,
                    has_header,
                    &normalized,
                    file.column_count,
                )
                .map(|_| ()),
                ColumnsMode::Merge(spec) => spec.picks.iter().try_for_each(|pick| {
                    resolve_pick(pick, &file.path, has_header, &normalized, file.column_count)
                        .map(|_| ())
                }),
            };
            if let Err(error) = recheck {
                blocked = Some(PlanBlocked {
                    code: error.code,
                    reason: error.message,
                    hint: error.detail,
                });
            }
        }
        if file.dirty_rows > 0 {
            dirty_files.push(format!("{} ({} row(s))", file.path, file.dirty_rows));
        }

        let conflict = file.target_occupied.then(|| PlanConflict {
            kind: "existing-target".to_string(),
            risk_level: RiskLevel::Danger,
        });
        entries.push(PlanEntry {
            entry_id: format!("{}-{:04}", input.plan_id, entries.len() + 1),
            source: file.path.clone(),
            target: Some(file.target.clone()),
            action: PlanAction::CsvColumns,
            relation: None,
            reason: format!(
                "apply {} on {} into {} as {}",
                mode_name,
                file.path,
                crate::conflict::file_name_of(&file.target),
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
            csv_extract: None,
            csv_rows: None,
            csv_dedup: None,
            csv_filter: None,
            csv_columns: Some(PlanCsvColumns {
                mode: mode_name.to_string(),
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
                split: split_payload.clone(),
                merge: merge_payload.clone(),
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
                "records with a different field count (missing fields participate as empty): {}",
                dirty_files.join(", ")
            )),
            risk_level: RiskLevel::Caution,
        });
    }

    let mut params = std::collections::BTreeMap::new();
    params.insert(
        "mode".to_string(),
        PlanParamValue::String(mode_name.to_string()),
    );
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
        kind: C14_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

fn joiner_of(spec: &MergeSpec) -> &str {
    &spec.joiner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, columns: &[&str], has_header: bool) -> PlanC14File {
        PlanC14File {
            path: path.to_string(),
            canonical_path: None,
            file_size: 100,
            source_encoding: Some(SourceEncoding::Utf8),
            delimiter: Delimiter::Comma,
            has_header,
            columns: columns.iter().map(|name| name.to_string()).collect(),
            column_count: columns.len() as u64,
            data_rows: 2,
            dirty_rows: 0,
            parts: 3,
            target: format!("/out/{}.csv", crate::conflict::file_name_of(path)),
            target_occupied: false,
        }
    }

    fn input(mode: ColumnsMode, files: Vec<PlanC14File>) -> PlanC14Input {
        PlanC14Input {
            plan_id: "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8".to_string(),
            mode,
            target_encoding: TargetEncoding::Utf8 { bom: true },
            invalid: InvalidSequencePolicy::ReplaceAndCount,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            files,
        }
    }

    #[test]
    fn split_plan_inserts_new_columns_after_the_source_column() {
        let document = plan_c14(input(
            ColumnsMode::Split(SplitSpec {
                source: ColumnPick::Name("tags".to_string()),
                by: SplitBy::Separator(";".to_string()),
            }),
            vec![file("/data/a.csv", &["id", "tags", "note"], true)],
        ))
        .unwrap();
        let entry = &document.entries[0];
        assert_eq!(entry.action, PlanAction::CsvColumns);
        let payload = entry.csv_columns.as_ref().unwrap();
        let split = payload.split.as_ref().unwrap();
        assert_eq!(split.index, 2);
        assert_eq!(split.parts, 3);
        assert_eq!(split.new_names, vec!["tags.1", "tags.2", "tags.3"]);
        assert!(payload.merge.is_none());
        assert!(document
            .warnings
            .iter()
            .any(|warning| warning.kind == "split-parts-estimate"));
    }

    #[test]
    fn merge_plan_appends_a_target_column_and_defaults_its_name() {
        let document = plan_c14(input(
            ColumnsMode::Merge(MergeSpec {
                picks: vec![ColumnPick::Name("first".to_string()), ColumnPick::Index(2)],
                joiner: "-".to_string(),
                target_name: None,
            }),
            vec![file("/data/a.csv", &["first", "last", "city"], true)],
        ))
        .unwrap();
        let payload = document.entries[0].csv_columns.as_ref().unwrap();
        let merge = payload.merge.as_ref().unwrap();
        assert_eq!(merge.picks.len(), 2);
        assert_eq!(merge.joiner, "-");
        assert_eq!(merge.target_name, "first-last");
    }

    #[test]
    fn split_rejects_invalid_regex_with_e1005() {
        let error = plan_c14(input(
            ColumnsMode::Split(SplitSpec {
                source: ColumnPick::Index(1),
                by: SplitBy::Regex("(unclosed".to_string()),
            }),
            vec![file("/data/a.csv", &["id"], true)],
        ))
        .unwrap_err();
        assert_eq!(error.code, "E1005");
    }

    #[test]
    fn merge_rejects_single_or_repeated_columns_with_e1002() {
        let error = plan_c14(input(
            ColumnsMode::Merge(MergeSpec {
                picks: vec![ColumnPick::Index(1)],
                joiner: "-".to_string(),
                target_name: None,
            }),
            vec![file("/data/a.csv", &["id"], true)],
        ))
        .unwrap_err();
        assert_eq!(error.code, "E1002");

        let error = plan_c14(input(
            ColumnsMode::Merge(MergeSpec {
                picks: vec![ColumnPick::Index(1), ColumnPick::Index(1)],
                joiner: "-".to_string(),
                target_name: None,
            }),
            vec![file("/data/a.csv", &["id", "name"], true)],
        ))
        .unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn name_selection_without_header_is_blocked_with_e3004() {
        let mut file = file("/data/a.csv", &[], false);
        file.has_header = false;
        let error = plan_c14(input(
            ColumnsMode::Split(SplitSpec {
                source: ColumnPick::Name("tags".to_string()),
                by: SplitBy::Separator(";".to_string()),
            }),
            vec![file],
        ))
        .unwrap_err();
        assert_eq!(error.code, "E3004");
    }

    #[test]
    fn split_value_covers_separator_regex_and_empty_segments() {
        assert_eq!(
            split_value("a;b;;c", &SplitBy::Separator(";".to_string())),
            vec!["a", "b", "", "c"]
        );
        assert_eq!(
            split_value("a1b22c", &SplitBy::Regex("[0-9]+".to_string())),
            vec!["a", "b", "c"]
        );
        assert_eq!(
            split_value("plain", &SplitBy::Separator(";".to_string())),
            vec!["plain"]
        );
    }
}
