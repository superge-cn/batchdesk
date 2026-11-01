//! 壳层 CSV 家族与目录清单计划函数（UI 接入切片第三批，ADR-0042）。
//!
//! f06/f07/f08/f09/f10：web 参数（serde camelCase）→ 壳层事实采集
//! （`platform::csv_merge` / `platform::csv_range` / `platform::listing`，与 CLI 同源）
//! → kernel 计划纯函数 → 存入 `AppState.plans` 供 `applyPlan` 既有通道执行。
//! 本模块不承载领域规则，判定一律留在 kernel。

use std::path::{Path, PathBuf};

use kernel::csv::Delimiter;
use kernel::csv_extract::{plan_f07, ColumnPick, PlanF07File, PlanF07Input};
use kernel::csv_merge::{plan_f06, PlanF06File, PlanF06Input};
use kernel::csv_range::{plan_f09, PlanF09File, PlanF09Input};
use kernel::csv_rows::{plan_f08, PlanF08File, PlanF08Input, RowWindow};
use kernel::error::KernelError;
use kernel::fs::CaseSense;
use kernel::listing::{plan_f10, ListingOptions, PlanF10File, PlanF10Input};
use kernel::plan::PlanDocument;
use platform::csv_merge::{csv_merge_inspect, CsvMergeScanOptions};
use platform::csv_range::{csv_range_layout, CsvRangeScanOptions};
use platform::ids::new_ulid;
use platform::listing::inspect_tree;

use crate::plan_kinds::{
    default_invalid, parse_invalid, parse_prior, parse_source, parse_target, require_file, store,
};
use crate::AppState;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CsvPlanParams {
    /// f06–f09 的输入文件；f10 复用 `roots`。
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub roots: Vec<String>,
    /// 单输出文件（f06 必填；f07/f08/f09/f10 与 `outputDir` 二选一）。
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub output_dir: Option<String>,
    /// 目标编码（f06–f09）。
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default = "default_invalid")]
    pub invalid: String,
    #[serde(default)]
    pub delimiter: Option<String>,
    /// 表头存在性；缺省 true（对应 CLI 的 `--no-header` 取反）。
    #[serde(default)]
    pub has_header: Option<bool>,
    /// 显式源编码，`Some` 时跳过检测。
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub prior: Option<String>,
    #[serde(default)]
    pub sample_window: Option<u64>,
    #[serde(default)]
    pub sample_blocks: Option<usize>,
    /// f06：来源列。
    #[serde(default)]
    pub source_column: bool,
    #[serde(default)]
    pub source_column_name: Option<String>,
    #[serde(default)]
    pub source_column_relative: bool,
    #[serde(default)]
    pub base_dir: Option<String>,
    #[serde(default)]
    pub strict: bool,
    /// f07：按表头名与 1-based 序号选择列（名称在前、序号在后，与表单顺序一致）。
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub column_indexes: Vec<u64>,
    /// f08：`first` / `last` 窗口与行数。
    #[serde(default)]
    pub window: Option<String>,
    #[serde(default)]
    pub count: Option<u64>,
    /// f09：1-based 数据记录区间。
    #[serde(default)]
    pub start_row: Option<u64>,
    #[serde(default)]
    pub end_row: Option<u64>,
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

/// 逐输入 1:1 的输出目标：`output` 只允许单输入，`outputDir` 按同名落盘。
fn per_file_targets(
    params: &CsvPlanParams,
    inputs: &[PathBuf],
    fallback: &str,
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
                    .unwrap_or_else(|| fallback.into());
                PathBuf::from(dir).join(name)
            })
            .collect()),
        _ => Err(missing(&format!("{kind}: requires output or outputDir"))),
    }
}

fn scan_options(
    params: &CsvPlanParams,
    delimiter: Option<Delimiter>,
    has_header: bool,
    source: Option<kernel::encoding::SourceEncoding>,
    prior: kernel::encoding::PlatformPrior,
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

pub fn plan_csv_shell(
    state: &tauri::State<'_, AppState>,
    kind: &str,
    params: serde_json::Value,
) -> Result<PlanDocument, KernelError> {
    let params: CsvPlanParams = serde_json::from_value(params)
        .map_err(|error| KernelError::invalid_params(error.to_string()))?;
    if params.files.is_empty() {
        return Err(missing(&format!("{kind}: requires at least one file")));
    }
    let plan_id = params
        .plan_id
        .clone()
        .unwrap_or_else(|| new_ulid(kind.to_string().as_str()));
    let delimiter = parse_delimiter(&params.delimiter)?;
    let has_header = params.has_header.unwrap_or(true);
    let source = parse_source(&params.source)?;
    let prior = parse_prior(&params.prior)?;
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
    let targets = per_file_targets(&params, &inputs, "output.csv", kind)?;

    let plan = match kind {
        "f06" => {
            let source_column_name = params
                .source_column_name
                .clone()
                .unwrap_or_else(|| "来源文件".to_string());
            if params.source_column_name.as_deref().is_some()
                && source_column_name.trim().is_empty()
            {
                return Err(KernelError::invalid_params(
                    "sourceColumnName must not be empty",
                ));
            }
            let base_dir = if params.source_column_relative {
                let dir = params
                    .base_dir
                    .as_ref()
                    .ok_or_else(|| missing("f06: sourceColumnRelative requires baseDir"))?;
                Some(PathBuf::from(dir))
            } else {
                None
            };
            let target = targets
                .first()
                .ok_or_else(|| missing("f06: requires output"))?
                .clone();
            let mut files = Vec::with_capacity(inputs.len());
            for absolute in &inputs {
                let inspection = csv_merge_inspect(
                    absolute,
                    scan_options(&params, delimiter, has_header, source, prior)?,
                )
                .map_err(|error| {
                    adapter_error(format!(
                        "cannot inspect CSV for merge ({}): {error}",
                        absolute.display()
                    ))
                })?;
                let source_label = if params.source_column {
                    if params.source_column_relative {
                        let base = base_dir.as_ref().expect("checked above");
                        match absolute.strip_prefix(base) {
                            Ok(relative) if !relative.as_os_str().is_empty() => {
                                Some(relative.to_string_lossy().into_owned())
                            }
                            _ => {
                                return Err(KernelError::invalid_params(format!(
                                    "source is outside baseDir: {}",
                                    absolute.display()
                                )))
                            }
                        }
                    } else {
                        Some(
                            absolute
                                .file_name()
                                .and_then(|value| value.to_str())
                                .unwrap_or("file")
                                .to_string(),
                        )
                    }
                } else {
                    None
                };
                files.push(PlanF06File {
                    path: absolute.to_string_lossy().into_owned(),
                    canonical_path: platform::fs::canonicalize_display(absolute),
                    file_size: inspection.file_size,
                    source_encoding: source.or(inspection.source_encoding),
                    delimiter: inspection.delimiter,
                    has_header: inspection.has_header,
                    columns: inspection.columns,
                    column_count: inspection.column_count,
                    data_rows: inspection.data_rows,
                    dirty_rows: inspection.dirty_rows,
                    source_label,
                });
            }
            plan_f06(PlanF06Input {
                plan_id,
                target: target.to_string_lossy().into_owned(),
                target_encoding,
                invalid,
                output_delimiter: delimiter,
                add_source_column: params.source_column,
                source_column_name,
                strict: params.strict,
                protected_dirs: Vec::new(),
                case_sense: CaseSense::Sensitive,
                target_occupied: target.exists(),
                files,
            })?
        }
        "f07" => {
            if params.columns.is_empty() && params.column_indexes.is_empty() {
                return Err(missing("f07: requires columns or columnIndexes"));
            }
            let mut selections: Vec<ColumnPick> = params
                .columns
                .iter()
                .filter(|name| !name.trim().is_empty())
                .map(|name| ColumnPick::Name(name.clone()))
                .collect();
            selections.extend(
                params
                    .column_indexes
                    .iter()
                    .filter(|index| **index > 0)
                    .map(|index| ColumnPick::Index(*index)),
            );
            if selections.is_empty() {
                return Err(missing("f07: selections resolved to an empty list"));
            }
            let mut files = Vec::with_capacity(inputs.len());
            for (absolute, target) in inputs.iter().zip(targets.iter()) {
                let inspection = csv_merge_inspect(
                    absolute,
                    scan_options(&params, delimiter, has_header, source, prior)?,
                )
                .map_err(|error| {
                    adapter_error(format!(
                        "cannot inspect CSV for extract ({}): {error}",
                        absolute.display()
                    ))
                })?;
                files.push(PlanF07File {
                    path: absolute.to_string_lossy().into_owned(),
                    canonical_path: platform::fs::canonicalize_display(absolute),
                    file_size: inspection.file_size,
                    source_encoding: inspection.source_encoding,
                    delimiter: inspection.delimiter,
                    has_header: inspection.has_header,
                    columns: inspection.columns,
                    column_count: inspection.column_count,
                    data_rows: inspection.data_rows,
                    dirty_rows: inspection.dirty_rows,
                    target: target.to_string_lossy().into_owned(),
                    target_occupied: target.exists(),
                });
            }
            plan_f07(PlanF07Input {
                plan_id,
                target_encoding,
                invalid,
                protected_dirs: Vec::new(),
                case_sense: CaseSense::Sensitive,
                selections,
                files,
            })?
        }
        "f08" => {
            let window = match params.window.as_deref() {
                Some("first") => RowWindow::First,
                Some("last") => RowWindow::Last,
                Some(other) => {
                    return Err(KernelError::invalid_params(format!(
                        "unknown row window: {other}"
                    )))
                }
                None => return Err(missing("f08: requires window first|last")),
            };
            let count = params.count.ok_or_else(|| missing("f08: requires count"))?;
            let mut files = Vec::with_capacity(inputs.len());
            for (absolute, target) in inputs.iter().zip(targets.iter()) {
                let inspection = csv_merge_inspect(
                    absolute,
                    scan_options(&params, delimiter, has_header, source, prior)?,
                )
                .map_err(|error| {
                    adapter_error(format!(
                        "cannot inspect CSV for rows ({}): {error}",
                        absolute.display()
                    ))
                })?;
                files.push(PlanF08File {
                    path: absolute.to_string_lossy().into_owned(),
                    canonical_path: platform::fs::canonicalize_display(absolute),
                    file_size: inspection.file_size,
                    source_encoding: inspection.source_encoding,
                    delimiter: inspection.delimiter,
                    has_header: inspection.has_header,
                    columns: inspection.columns,
                    column_count: inspection.column_count,
                    data_rows: inspection.data_rows,
                    dirty_rows: inspection.dirty_rows,
                    target: target.to_string_lossy().into_owned(),
                    target_occupied: target.exists(),
                });
            }
            plan_f08(PlanF08Input {
                plan_id,
                target_encoding,
                invalid,
                window,
                count,
                protected_dirs: Vec::new(),
                case_sense: CaseSense::Sensitive,
                files,
            })?
        }
        "f09" => {
            let start_row = params
                .start_row
                .ok_or_else(|| missing("f09: requires startRow"))?;
            let end_row = params
                .end_row
                .ok_or_else(|| missing("f09: requires endRow"))?;
            if start_row == 0 || end_row < start_row {
                return Err(KernelError::invalid_params(
                    "f09: startRow must be 1-based and endRow >= startRow",
                ));
            }
            let sample_window = params.sample_window.ok_or_else(|| {
                missing("f09: sampleWindow is required (方案 §5.2 sets no numeric window)")
            })?;
            let mut files = Vec::with_capacity(inputs.len());
            for (absolute, target) in inputs.iter().zip(targets.iter()) {
                let layout = csv_range_layout(
                    absolute,
                    CsvRangeScanOptions {
                        delimiter,
                        has_header,
                        start_row,
                        end_row,
                        sample_window_bytes: sample_window,
                        platform_prior: prior,
                        source_encoding: source,
                    },
                )
                .map_err(|error| {
                    adapter_error(format!(
                        "cannot scan CSV row range ({}): {error}",
                        absolute.display()
                    ))
                })?;
                files.push(PlanF09File {
                    path: absolute.to_string_lossy().into_owned(),
                    canonical_path: platform::fs::canonicalize_display(absolute),
                    file_size: layout.file_size,
                    source_encoding: layout.source_encoding,
                    delimiter: layout.delimiter,
                    delimiter_detected: layout.delimiter_detected,
                    has_header,
                    columns: layout.header_columns,
                    column_count: layout.column_count,
                    data_rows: layout.data_rows,
                    total_known: layout.total_known,
                    selected_rows: layout.selected_rows,
                    header_range: layout.header.map(|span| (span.start, span.end)),
                    data_range: layout.selected.map(|span| (span.start, span.end)),
                    dirty_rows: layout.dirty_rows,
                    multi_line_header: layout.multi_line_header,
                    unclosed_quote: layout.unclosed_quote,
                    target: target.to_string_lossy().into_owned(),
                    target_occupied: target.exists(),
                });
            }
            plan_f09(PlanF09Input {
                plan_id,
                target_encoding,
                invalid,
                start_row,
                end_row,
                protected_dirs: Vec::new(),
                case_sense: CaseSense::Sensitive,
                files,
            })?
        }
        other => {
            return Err(KernelError::invalid_params(format!(
                "{other} is not a csv plan kind in this shell"
            )))
        }
    };
    store(state, &plan);
    Ok(plan)
}

/// F10 目录清单：计划只存摘要计数，实际 CSV 写出由通用执行器的 Listing 分支完成。
pub fn plan_listing_shell(
    state: &tauri::State<'_, AppState>,
    params: serde_json::Value,
) -> Result<PlanDocument, KernelError> {
    let params: CsvPlanParams = serde_json::from_value(params)
        .map_err(|error| KernelError::invalid_params(error.to_string()))?;
    if params.roots.is_empty() {
        return Err(missing("f10: requires at least one root"));
    }
    let plan_id = params.plan_id.clone().unwrap_or_else(|| new_ulid("f10"));
    let roots: Vec<PathBuf> = params
        .roots
        .iter()
        .map(PathBuf::from)
        .map(|root| {
            let metadata = std::fs::metadata(&root).map_err(|error| {
                KernelError::new("E2001", format!("{}: {error}", root.display()), None, false)
            })?;
            if !metadata.is_dir() {
                return Err(KernelError::invalid_params(format!(
                    "f10 requires directory roots: {}",
                    root.display()
                )));
            }
            Ok(root)
        })
        .collect::<Result<_, KernelError>>()?;
    let targets = per_file_targets(&params, &roots, "listing.csv", "f10")?;
    let options = ListingOptions::default();
    let mut files = Vec::with_capacity(roots.len());
    for (root, target) in roots.iter().zip(targets.iter()) {
        let stats = inspect_tree(root, &options).map_err(|error| {
            adapter_error(format!(
                "cannot inspect directory tree ({}): {error}",
                root.display()
            ))
        })?;
        files.push(PlanF10File {
            root: root.to_string_lossy().into_owned(),
            canonical_root: platform::fs::canonicalize_display(root),
            target: target.to_string_lossy().into_owned(),
            target_occupied: target.exists(),
            files: stats.files,
            dirs: stats.dirs,
            placeholders: stats.placeholders,
            skipped: stats.skipped,
            max_depth: stats.max_depth,
        });
    }
    let plan = plan_f10(PlanF10Input {
        plan_id,
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        files,
    })?;
    store(state, &plan);
    Ok(plan)
}
