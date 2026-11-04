//! 壳层 CSV 处理族与名称替换计划函数（UI 接入切片第四批，ADR-0043）。
//!
//! c04 名称查找替换／c11 CSV 去重／c12 CSV 过滤／c13 CSV 排序／c14 CSV 列拆分合并：
//! web 参数（serde camelCase）→ 壳层事实采集（与 CLI 同源 platform 函数）→
//! kernel 计划纯函数 → 存入 `AppState.plans` 供 `applyPlan` 既有通道执行。
//! 判定一律留在 kernel；本模块不复制领域规则。

use std::path::{Path, PathBuf};

use kernel::csv::Delimiter;
use kernel::csv_columns::{
    plan_c14, resolve_column, split_value, ColumnsMode, MergeSpec, PlanC14File, PlanC14Input,
    SplitBy, SplitSpec,
};
use kernel::csv_dedup::{plan_c11, PlanC11File, PlanC11Input};
use kernel::csv_extract::ColumnPick;
use kernel::csv_filter::{plan_c12, FilterOp, PlanC12File, PlanC12Input};
use kernel::csv_sort::{plan_c13, PlanC13File, PlanC13Input, SortOrder};
use kernel::encoding::{PlatformPrior, SourceEncoding};
use kernel::error::KernelError;
use kernel::fs::CaseSense;
use kernel::name_replace::{plan_c04, C04Mode, PlanC04Input, PlanC04Rule};
use kernel::plan::PlanDocument;
use kernel::rename::{PlanF12Item, RenameScope};
use platform::csv_merge::{csv_dedup_inspect, csv_merge_inspect, CsvMergeScanOptions};
use platform::ids::new_ulid;

use crate::plan_kinds::{
    default_invalid, parse_invalid, parse_prior, parse_source, parse_target, require_file, store,
};
use crate::AppState;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpPlanParams {
    /// c11–c14 的输入文件。
    #[serde(default)]
    pub files: Vec<String>,
    /// c04 的目录。
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub output_dir: Option<String>,
    /// 目标编码（c11–c14）。
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default = "default_invalid")]
    pub invalid: String,
    #[serde(default)]
    pub delimiter: Option<String>,
    #[serde(default)]
    pub has_header: Option<bool>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub prior: Option<String>,
    #[serde(default)]
    pub sample_window: Option<u64>,
    #[serde(default)]
    pub sample_blocks: Option<usize>,
    /// c04 规则。
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub find: Option<String>,
    #[serde(default)]
    pub replace: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    /// c11 键列（留空＝整行判重）。
    #[serde(default)]
    pub key_columns: Vec<String>,
    #[serde(default)]
    pub key_column_indexes: Vec<u64>,
    /// c12 条件。
    #[serde(default)]
    pub column: Option<String>,
    #[serde(default)]
    pub column_index: Option<u64>,
    #[serde(default)]
    pub op: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    /// `"keep"`（默认）或 `"drop"`。
    #[serde(default)]
    pub filter_mode: Option<String>,
    /// c13 排序键与顺序。
    #[serde(default)]
    pub sort_columns: Vec<String>,
    #[serde(default)]
    pub sort_column_indexes: Vec<u64>,
    #[serde(default)]
    pub order: Option<String>,
    #[serde(default)]
    pub external_chunk_rows: Option<u64>,
    /// c14 拆分／合并（二选一）。
    #[serde(default)]
    pub split_column: Option<String>,
    #[serde(default)]
    pub split_column_index: Option<u64>,
    #[serde(default)]
    pub split_separator: Option<String>,
    #[serde(default)]
    pub split_regex: Option<String>,
    #[serde(default)]
    pub merge_columns: Vec<String>,
    #[serde(default)]
    pub merge_column_indexes: Vec<u64>,
    #[serde(default)]
    pub merge_join_with: Option<String>,
    #[serde(default)]
    pub merge_name: Option<String>,
    #[serde(default)]
    pub plan_id: Option<String>,
}

fn missing(message: &str) -> KernelError {
    KernelError::missing_param(message.to_string())
}

fn adapter_error(context: String) -> KernelError {
    KernelError::new("E1004", context, None, false)
}

fn parse_delimiter(value: &Option<String>) -> Result<Option<Delimiter>, KernelError> {
    match value {
        Some(value) => Delimiter::parse(value)
            .map(Some)
            .ok_or_else(|| KernelError::invalid_params(format!("unknown delimiter: {value}"))),
        None => Ok(None),
    }
}

fn picks(names: &[String], indexes: &[u64]) -> Vec<ColumnPick> {
    let mut result: Vec<ColumnPick> = names
        .iter()
        .filter(|name| !name.trim().is_empty())
        .map(|name| ColumnPick::Name(name.clone()))
        .collect();
    result.extend(
        indexes
            .iter()
            .filter(|index| **index > 0)
            .map(|index| ColumnPick::Index(*index)),
    );
    result
}

fn single_pick(
    name: &Option<String>,
    index: &Option<u64>,
    what: &str,
) -> Result<ColumnPick, KernelError> {
    match (name, index) {
        (Some(name), None) if !name.trim().is_empty() => Ok(ColumnPick::Name(name.clone())),
        (None, Some(index)) if *index > 0 => Ok(ColumnPick::Index(*index)),
        _ => Err(missing(&format!("{what}: requires column or columnIndex"))),
    }
}

/// 逐输入 1:1 的输出目标（与 CLI 同源）：`output` 单输入专用，`outputDir` 按同名落盘。
fn per_file_targets(
    params: &OpPlanParams,
    inputs: &[PathBuf],
    kind: &str,
) -> Result<Vec<PathBuf>, KernelError> {
    match (&params.output, &params.output_dir) {
        (Some(output), None) => {
            if inputs.len() != 1 {
                return Err(KernelError::invalid_params(format!(
                    "{kind}: output requires exactly one input"
                )));
            }
            Ok(vec![PathBuf::from(output)])
        }
        (None, Some(dir)) => Ok(inputs
            .iter()
            .map(|path| {
                let name = path
                    .file_name()
                    .map(|value| value.to_os_string())
                    .unwrap_or_else(|| "output.csv".into());
                PathBuf::from(dir).join(name)
            })
            .collect()),
        _ => Err(missing(&format!("{kind}: requires output or outputDir"))),
    }
}

fn scan_options(
    params: &OpPlanParams,
    delimiter: Option<Delimiter>,
    has_header: bool,
    source: Option<SourceEncoding>,
    prior: PlatformPrior,
) -> Result<CsvMergeScanOptions, KernelError> {
    let sample_window_bytes = params
        .sample_window
        .ok_or_else(|| missing("sampleWindow is required (方案 §5.2 sets no numeric window)"))?;
    Ok(CsvMergeScanOptions {
        delimiter,
        has_header,
        sample_window_bytes,
        sample_blocks: params.sample_blocks.unwrap_or(0),
        platform_prior: prior,
        source_encoding: source,
    })
}

/// 数据行（有表头时剔除首行），供键列取值与拆分段数评估使用。
fn data_rows(records: &[Vec<String>], has_header: bool) -> &[Vec<String>] {
    if has_header {
        records.get(1..).unwrap_or(&[])
    } else {
        records
    }
}

/// 与 CLI 同源：按首个可解析的列选择取键列取值（表头不参与类型推断）。
fn key_values(
    keys: &[ColumnPick],
    columns: &[String],
    records: &[Vec<String>],
    has_header: bool,
) -> Vec<String> {
    if records.is_empty() {
        return Vec::new();
    }
    let normalized: Vec<String> = columns
        .iter()
        .map(|column| column.trim_start_matches('\u{feff}').trim().to_string())
        .collect();
    let resolved = keys.iter().find_map(|pick| match pick {
        ColumnPick::Name(name) => normalized
            .iter()
            .position(|column| column == name)
            .map(|position| position + 1),
        ColumnPick::Index(index) => Some(*index as usize),
    });
    match resolved {
        Some(column) => data_rows(records, has_header)
            .iter()
            .filter_map(|row| row.get(column.saturating_sub(1)).cloned())
            .collect(),
        None => Vec::new(),
    }
}

pub fn plan_op_shell(
    state: &tauri::State<'_, AppState>,
    kind: &str,
    params: serde_json::Value,
) -> Result<PlanDocument, KernelError> {
    let params: OpPlanParams = serde_json::from_value(params)
        .map_err(|error| KernelError::invalid_params(error.to_string()))?;
    let plan_id = params
        .plan_id
        .clone()
        .unwrap_or_else(|| new_ulid(kind.to_string().as_str()));

    if kind == "c04" {
        return plan_rename_match_shell(state, &params, plan_id);
    }

    if params.files.is_empty() {
        return Err(missing(&format!("{kind}: requires at least one file")));
    }
    let delimiter = parse_delimiter(&params.delimiter)?;
    let has_header = params.has_header.unwrap_or(true);
    let source = parse_source(&params.source)?;
    let prior = parse_prior(&params.prior)?;
    let options = scan_options(&params, delimiter, has_header, source, prior)?;
    let target_encoding = params
        .target
        .as_deref()
        .map(parse_target)
        .transpose()?
        .ok_or_else(|| missing(&format!("{kind}: requires target encoding")))?;
    let invalid = parse_invalid(&params.invalid)?;

    let inputs: Vec<PathBuf> = params
        .files
        .iter()
        .map(|path| require_file(Path::new(path)))
        .collect::<Result<_, _>>()?;
    let targets = per_file_targets(&params, &inputs, kind)?;

    let plan = match kind {
        "c11" => {
            let key = picks(&params.key_columns, &params.key_column_indexes);
            let mut files = Vec::with_capacity(inputs.len());
            for (absolute, target) in inputs.iter().zip(targets.iter()) {
                let inspection = csv_dedup_inspect(absolute, options).map_err(|error| {
                    adapter_error(format!(
                        "cannot inspect CSV for dedup ({}): {error}",
                        absolute.display()
                    ))
                })?;
                files.push(PlanC11File {
                    path: absolute.to_string_lossy().into_owned(),
                    canonical_path: platform::fs::canonicalize_display(absolute),
                    output: target.to_string_lossy().into_owned(),
                    file_size: inspection.inspection.file_size,
                    source_encoding: inspection.inspection.source_encoding,
                    delimiter: inspection.inspection.delimiter,
                    has_header,
                    columns: inspection.inspection.columns,
                    column_count: inspection.inspection.column_count,
                    dirty_rows: inspection.inspection.dirty_rows,
                    records: inspection.records,
                    target_occupied: target.exists(),
                });
            }
            plan_c11(PlanC11Input {
                plan_id,
                target: target_encoding,
                invalid,
                protected_dirs: Vec::new(),
                case_sense: CaseSense::Sensitive,
                key,
                files,
            })?
        }
        "c12" => {
            let condition_column = single_pick(&params.column, &params.column_index, "c12")?;
            let condition_op = FilterOp::parse(
                params
                    .op
                    .as_deref()
                    .ok_or_else(|| missing("c12: requires op"))?,
            )
            .ok_or_else(|| {
                KernelError::invalid_params(format!(
                    "c12: unknown op {}",
                    params.op.as_deref().unwrap_or_default()
                ))
            })?;
            let condition_value = params
                .value
                .clone()
                .ok_or_else(|| missing("c12: requires value"))?;
            let mode_keep = match params.filter_mode.as_deref() {
                None => true,
                Some("keep") => true,
                Some("drop") => false,
                Some(other) => {
                    return Err(KernelError::invalid_params(format!(
                        "c12: filterMode must be keep or drop: {other}"
                    )))
                }
            };
            let mut files = Vec::with_capacity(inputs.len());
            for (absolute, target) in inputs.iter().zip(targets.iter()) {
                let inspection = csv_dedup_inspect(absolute, options).map_err(|error| {
                    adapter_error(format!(
                        "cannot inspect CSV for filter ({}): {error}",
                        absolute.display()
                    ))
                })?;
                files.push(PlanC12File {
                    path: absolute.to_string_lossy().into_owned(),
                    canonical_path: platform::fs::canonicalize_display(absolute),
                    output: target.to_string_lossy().into_owned(),
                    file_size: inspection.inspection.file_size,
                    source_encoding: inspection.inspection.source_encoding,
                    delimiter: inspection.inspection.delimiter,
                    has_header,
                    columns: inspection.inspection.columns,
                    column_count: inspection.inspection.column_count,
                    dirty_rows: inspection.inspection.dirty_rows,
                    records: inspection.records,
                    target_occupied: target.exists(),
                });
            }
            plan_c12(PlanC12Input {
                plan_id,
                target: target_encoding,
                invalid,
                protected_dirs: Vec::new(),
                case_sense: CaseSense::Sensitive,
                mode_keep,
                condition_column,
                condition_op,
                condition_value,
                files,
            })?
        }
        "c13" => {
            let keys = picks(&params.sort_columns, &params.sort_column_indexes);
            if keys.is_empty() {
                return Err(missing("c13: requires sortColumns or sortColumnIndexes"));
            }
            let order = match params.order.as_deref() {
                None => SortOrder::Asc,
                Some(value) => SortOrder::parse(value).ok_or_else(|| {
                    KernelError::invalid_params(format!("c13: unknown order: {value}"))
                })?,
            };
            let mut files = Vec::with_capacity(inputs.len());
            for (absolute, target) in inputs.iter().zip(targets.iter()) {
                let dedup = csv_dedup_inspect(absolute, options).map_err(|error| {
                    adapter_error(format!(
                        "cannot inspect CSV for sorting ({}): {error}",
                        absolute.display()
                    ))
                })?;
                let inspection = dedup.inspection;
                let values = key_values(&keys, &inspection.columns, &dedup.records, has_header);
                files.push(PlanC13File {
                    path: absolute.to_string_lossy().into_owned(),
                    canonical_path: platform::fs::canonicalize_display(absolute),
                    file_size: inspection.file_size,
                    source_encoding: inspection.source_encoding,
                    delimiter: inspection.delimiter,
                    has_header,
                    columns: inspection.columns,
                    column_count: inspection.column_count,
                    data_rows: inspection.data_rows,
                    dirty_rows: inspection.dirty_rows,
                    key_values: values,
                    target: target.to_string_lossy().into_owned(),
                    target_occupied: target.exists(),
                });
            }
            plan_c13(PlanC13Input {
                plan_id,
                keys,
                order,
                external_chunk_rows: params.external_chunk_rows,
                target_encoding,
                invalid,
                protected_dirs: Vec::new(),
                case_sense: CaseSense::Sensitive,
                files,
            })?
        }
        "c14" => {
            let source_pick =
                single_pick(&params.split_column, &params.split_column_index, "c14").ok();
            let by = match (
                params.split_separator.filter(|value| !value.is_empty()),
                params.split_regex.filter(|value| !value.is_empty()),
            ) {
                (Some(separator), None) => Some(SplitBy::Separator(separator)),
                (None, Some(pattern)) => Some(SplitBy::Regex(pattern)),
                (Some(_), Some(_)) => {
                    return Err(KernelError::invalid_params(
                        "c14: splitSeparator and splitRegex are mutually exclusive",
                    ))
                }
                (None, None) => None,
            };
            let merge_picks = picks(&params.merge_columns, &params.merge_column_indexes);
            let mode = match (source_pick.zip(by), merge_picks.is_empty()) {
                (Some((source, by)), true) => ColumnsMode::Split(SplitSpec { source, by }),
                (None, false) => ColumnsMode::Merge(MergeSpec {
                    picks: merge_picks,
                    joiner: params.merge_join_with.clone().unwrap_or_default(),
                    target_name: params
                        .merge_name
                        .clone()
                        .filter(|value| !value.trim().is_empty()),
                }),
                _ => {
                    return Err(missing(
                        "c14: requires split (splitColumn + splitSeparator|splitRegex) or merge (mergeColumns + mergeJoinWith)",
                    ))
                }
            };
            let is_split = matches!(mode, ColumnsMode::Split(_));
            let mut files = Vec::with_capacity(inputs.len());
            for (absolute, target) in inputs.iter().zip(targets.iter()) {
                let (inspection, records) = if is_split {
                    let dedup = csv_dedup_inspect(absolute, options).map_err(|error| {
                        adapter_error(format!(
                            "cannot inspect CSV for column split ({}): {error}",
                            absolute.display()
                        ))
                    })?;
                    (dedup.inspection, dedup.records)
                } else {
                    let inspected = csv_merge_inspect(absolute, options).map_err(|error| {
                        adapter_error(format!(
                            "cannot inspect CSV for column merge ({}): {error}",
                            absolute.display()
                        ))
                    })?;
                    (inspected, Vec::new())
                };
                let mut parts = 1u64;
                if let ColumnsMode::Split(spec) = &mode {
                    let resolved = resolve_column(
                        &spec.source,
                        has_header,
                        &inspection.columns,
                        inspection.column_count,
                    )?;
                    parts = data_rows(&records, has_header)
                        .iter()
                        .map(|row| {
                            split_value(
                                row.get(resolved.index as usize - 1)
                                    .map(String::as_str)
                                    .unwrap_or_default(),
                                &spec.by,
                            )
                            .len() as u64
                        })
                        .max()
                        .unwrap_or(1);
                }
                files.push(PlanC14File {
                    path: absolute.to_string_lossy().into_owned(),
                    canonical_path: platform::fs::canonicalize_display(absolute),
                    file_size: inspection.file_size,
                    source_encoding: inspection.source_encoding,
                    delimiter: inspection.delimiter,
                    has_header,
                    columns: inspection.columns,
                    column_count: inspection.column_count,
                    data_rows: inspection.data_rows,
                    dirty_rows: inspection.dirty_rows,
                    parts,
                    target: target.to_string_lossy().into_owned(),
                    target_occupied: target.exists(),
                });
            }
            plan_c14(PlanC14Input {
                plan_id,
                mode,
                target_encoding,
                invalid,
                protected_dirs: Vec::new(),
                case_sense: CaseSense::Sensitive,
                files,
            })?
        }
        other => {
            return Err(KernelError::invalid_params(format!(
                "{other} is not an op plan kind in this shell"
            )))
        }
    };
    store(state, &plan);
    Ok(plan)
}

/// C04 名称查找替换：目录扫描事实与 f12 同源（符号链接跳过、按名称排序）。
fn plan_rename_match_shell(
    state: &tauri::State<'_, AppState>,
    params: &OpPlanParams,
    plan_id: String,
) -> Result<PlanDocument, KernelError> {
    let dir = PathBuf::from(
        params
            .dir
            .as_deref()
            .ok_or_else(|| missing("c04: requires dir"))?,
    );
    if !dir.is_dir() {
        return Err(KernelError::new(
            "E1003",
            format!("c04 dir must be a directory: {}", dir.display()),
            None,
            false,
        ));
    }
    let mode = C04Mode::parse(params.mode.as_deref().unwrap_or("plain")).ok_or_else(|| {
        KernelError::invalid_params(format!(
            "c04: unknown mode {}",
            params.mode.as_deref().unwrap_or_default()
        ))
    })?;
    let scope = RenameScope::parse(params.scope.as_deref().unwrap_or("full")).ok_or_else(|| {
        KernelError::invalid_params(format!(
            "c04: unknown scope {}",
            params.scope.as_deref().unwrap_or_default()
        ))
    })?;
    let find = params
        .find
        .clone()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| missing("c04: requires find"))?;
    let mut items = Vec::new();
    let mut existing_targets = Vec::new();
    let long_path_supported = platform::preflight::long_path_supported();
    let mut access_facts = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .map_err(|error| adapter_error(format!("{}: {error}", dir.display())))?
    {
        let entry = entry.map_err(|error| adapter_error(error.to_string()))?;
        let path = entry.path();
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => continue,
            Err(_) => continue,
            _ => {}
        }
        existing_targets.push(path.to_string_lossy().into_owned());
        // 与 F12 同族：只读在预览期点名（方案 §5.13）。
        access_facts.push(platform::preflight::path_check_light(
            &path,
            long_path_supported,
        ));
        items.push(PlanF12Item {
            path: path.to_string_lossy().into_owned(),
            name: entry.file_name().to_string_lossy().into_owned(),
        });
    }
    items.sort_by(|left, right| left.name.cmp(&right.name));
    let plan = plan_c04(PlanC04Input {
        plan_id,
        items,
        existing_targets,
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        long_path_supported,
        access_facts,
        rule: PlanC04Rule {
            mode,
            find,
            replace: params.replace.clone().unwrap_or_default(),
            scope,
        },
    })?;
    store(state, &plan);
    Ok(plan)
}
