//! 壳层清单/规则/压缩包计划函数（UI 接入切片第五批，ADR-0044）。
//!
//! c07 清单驱动移动/复制／c08 按规则归档／c10 ZIP 解压与压缩：
//! web 参数（serde camelCase）→ 壳层事实采集（与 CLI 同源 platform 函数）→
//! kernel 计划纯函数 → 存入 `AppState.plans` 供 `applyPlan` 既有通道执行。
//! 限额（解压总量/比例、外排块行数等）一律由调用方显式传入，壳层不设产品默认值。

use std::path::{Path, PathBuf};

use kernel::archive::{
    plan_c10, plan_c10_compress, ArchiveCompression, ArchiveService, PlanC10CompressInput,
    PlanC10Input,
};
use kernel::c07::{plan_c07, C07Operation, MissingPolicy, PlanC07Fact, PlanC07Input, PlanC07Row};
use kernel::c08::{
    plan_c08, ArchiveCondition, ArchiveRule, PlanC08File, PlanC08Input, UnmatchedPolicy,
};
use kernel::csv::Delimiter;
use kernel::encoding::PlatformPrior;
use kernel::error::KernelError;
use kernel::fs::CaseSense;
use kernel::plan::PlanDocument;
use platform::archive::{existing_paths, inspect_sources, ZipArchiveService};
use platform::csv_merge::{csv_dedup_inspect, CsvMergeScanOptions};
use platform::ids::new_ulid;
use platform::listing::collect_tree_files;

use crate::plan_kinds::store;
use crate::AppState;

fn missing(message: &str) -> KernelError {
    KernelError::missing_param(message.to_string())
}

fn adapter_error(context: String) -> KernelError {
    KernelError::new("E1004", context, None, false)
}

fn require_dir(path: &Path, kind: &str) -> Result<PathBuf, KernelError> {
    if !path.is_dir() {
        return Err(KernelError::new(
            "E1003",
            format!(
                "{kind}: root must be an existing directory: {}",
                path.display()
            ),
            None,
            false,
        ));
    }
    Ok(path.to_path_buf())
}

// ── C07 清单驱动移动/复制 ──────────────────────────────────────────────

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestParams {
    pub manifest: String,
    pub source_root: String,
    pub target_root: String,
    /// `"move"`（缺省）或 `"copy"`。
    #[serde(default)]
    pub operation: Option<String>,
    /// 清单行源文件缺失时的策略：`"skip"`（缺省）或 `"error"`。
    #[serde(default)]
    pub missing: Option<String>,
    #[serde(default)]
    pub delimiter: Option<String>,
    #[serde(default)]
    pub plan_id: Option<String>,
}

pub fn plan_manifest_shell(
    state: &tauri::State<'_, AppState>,
    params: serde_json::Value,
) -> Result<PlanDocument, KernelError> {
    let params: ManifestParams = serde_json::from_value(params)
        .map_err(|error| KernelError::invalid_params(error.to_string()))?;
    let operation = match params.operation.as_deref() {
        None | Some("move") => C07Operation::Move,
        Some("copy") => C07Operation::Copy,
        Some(other) => {
            return Err(KernelError::invalid_params(format!(
                "c07: operation must be move or copy: {other}"
            )))
        }
    };
    let missing_policy = match params.missing.as_deref() {
        None | Some("skip") => MissingPolicy::Skip,
        Some("error") => MissingPolicy::Error,
        Some(other) => {
            return Err(KernelError::invalid_params(format!(
                "c07: missing must be skip or error: {other}"
            )))
        }
    };
    let delimiter = match &params.delimiter {
        Some(value) => Some(Delimiter::parse(value).ok_or_else(|| {
            KernelError::invalid_params(format!("c07: unknown delimiter: {value}"))
        })?),
        None => None,
    };
    let manifest = require_file(Path::new(&params.manifest), "c07")?;
    let source_root = require_dir(Path::new(&params.source_root), "c07")?;
    let target_root = require_dir(Path::new(&params.target_root), "c07")?;
    let manifest_size = std::fs::metadata(&manifest)
        .map_err(|error| {
            KernelError::new(
                "E2001",
                format!("{}: {error}", manifest.display()),
                None,
                false,
            )
        })?
        .len()
        .max(1);
    // 清单解析编码感知：整份清单全量读取（清单小，采样窗口取文件大小）。
    let inspected = csv_dedup_inspect(
        &manifest,
        CsvMergeScanOptions {
            delimiter,
            has_header: false,
            sample_window_bytes: manifest_size,
            sample_blocks: 1,
            platform_prior: PlatformPrior::Other,
            source_encoding: None,
        },
    )
    .map_err(|error| {
        adapter_error(format!(
            "manifest is not parseable ({}): {error}",
            manifest.display()
        ))
    })?;
    let mut rows = Vec::new();
    let mut facts = Vec::new();
    for record in &inspected.records {
        if record.len() < 2 {
            return Err(adapter_error(format!(
                "manifest row must have two columns (name, target sub-directory): {record:?}"
            )));
        }
        let name = record[0].trim();
        let sub_dir = record[1].trim();
        // 事实采集与 kernel 用同一相对拼接规则（root/relative）。
        let source = source_root.join(name);
        let source_meta = std::fs::metadata(&source);
        let target_dir = if sub_dir.is_empty() {
            target_root.clone()
        } else {
            target_root.join(sub_dir)
        };
        let target = target_dir.join(
            Path::new(name)
                .file_name()
                .map(|value| value.to_os_string())
                .unwrap_or_else(|| name.into()),
        );
        rows.push(PlanC07Row {
            name: name.to_string(),
            sub_dir: sub_dir.to_string(),
        });
        facts.push(PlanC07Fact {
            exists: source_meta
                .as_ref()
                .is_ok_and(|metadata| metadata.is_file()),
            size: source_meta
                .as_ref()
                .map(|metadata| metadata.len())
                .unwrap_or(0),
            target_occupied: target.exists(),
        });
    }
    let plan = plan_c07(PlanC07Input {
        plan_id: params.plan_id.clone().unwrap_or_else(|| new_ulid("c07")),
        operation,
        missing: missing_policy,
        source_root: source_root.to_string_lossy().into_owned(),
        target_root: target_root.to_string_lossy().into_owned(),
        rows,
        facts,
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        long_path_supported: platform::preflight::long_path_supported(),
    })?;
    store(state, &plan);
    Ok(plan)
}

fn require_file(path: &Path, kind: &str) -> Result<PathBuf, KernelError> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        KernelError::new("E2001", format!("{}: {error}", path.display()), None, false)
    })?;
    if !metadata.is_file() {
        return Err(KernelError::invalid_params(format!(
            "{kind}: requires a regular file: {}",
            path.display()
        )));
    }
    Ok(path.to_path_buf())
}

// ── C08 按规则归档 ──────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivePlanParams {
    pub source_root: String,
    pub target_root: String,
    /// 规则链文本，每行 `<子目录>|<type>:<params>`（与 CLI `--rule` 同格式）。
    pub rules: Vec<String>,
    #[serde(default)]
    pub recursive: bool,
    /// 未命中文件去向；留空＝留在原位。
    #[serde(default)]
    pub unclassified_dir: Option<String>,
    #[serde(default)]
    pub plan_id: Option<String>,
}

/// 与 CLI `parse_archive_condition` 同语义：`ext:`/`mtime:`/`size:`/`name:` 四类条件。
fn parse_archive_condition(params: &str) -> Result<ArchiveCondition, String> {
    if let Some(set) = params.strip_prefix("ext:") {
        let extensions: Vec<String> = set
            .split(',')
            .map(|extension| extension.trim().to_string())
            .filter(|extension| !extension.is_empty())
            .collect();
        if extensions.is_empty() {
            return Err("ext rule requires at least one extension".to_string());
        }
        return Ok(ArchiveCondition::Extensions(extensions));
    }
    if let Some(range) = params.strip_prefix("mtime:") {
        let (from, to) = range
            .split_once("..")
            .ok_or_else(|| format!("mtime rule requires from..to: {range}"))?;
        let bound = |value: &str| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        };
        return Ok(ArchiveCondition::ModifiedDateRange {
            from: bound(from),
            to: bound(to),
        });
    }
    if let Some(range) = params.strip_prefix("size:") {
        let (min, max) = range
            .split_once("..")
            .ok_or_else(|| format!("size rule requires min..max (bytes): {range}"))?;
        let bound = |value: &str| -> Result<Option<u64>, String> {
            let value = value.trim();
            if value.is_empty() {
                Ok(None)
            } else {
                value
                    .parse::<u64>()
                    .map(Some)
                    .map_err(|_| format!("size bound must be a byte count: {value}"))
            }
        };
        return Ok(ArchiveCondition::SizeRange {
            min: bound(min)?,
            max: bound(max)?,
        });
    }
    if let Some(keyword) = params.strip_prefix("name:") {
        if keyword.is_empty() {
            return Err("name rule requires a keyword".to_string());
        }
        return Ok(ArchiveCondition::NameKeyword(keyword.to_string()));
    }
    Err(format!(
        "unknown rule type (expected ext:/mtime:/size:/name:): {params}"
    ))
}

pub fn plan_rule_archive_shell(
    state: &tauri::State<'_, AppState>,
    params: serde_json::Value,
) -> Result<PlanDocument, KernelError> {
    let params: ArchivePlanParams = serde_json::from_value(params)
        .map_err(|error| KernelError::invalid_params(error.to_string()))?;
    if params.rules.is_empty() {
        return Err(missing(
            "c08: requires at least one rule <subdir>|<type>:<params>",
        ));
    }
    let mut rules = Vec::with_capacity(params.rules.len());
    for rule_text in &params.rules {
        let Some((sub_dir, condition_text)) = rule_text.split_once('|') else {
            return Err(KernelError::invalid_params(format!(
                "c08: rule must be <subdir>|<type>:<params>: {rule_text}"
            )));
        };
        let condition =
            parse_archive_condition(condition_text).map_err(KernelError::invalid_params)?;
        rules.push(ArchiveRule {
            sub_dir: sub_dir.trim().to_string(),
            condition,
        });
    }
    let unmatched = match &params.unclassified_dir {
        Some(dir) if !dir.trim().is_empty() => UnmatchedPolicy::MoveTo(dir.trim().to_string()),
        _ => UnmatchedPolicy::Keep,
    };
    let source_root = require_dir(Path::new(&params.source_root), "c08")?;
    let target_root = require_dir(Path::new(&params.target_root), "c08")?;
    let files = collect_tree_files(&source_root, params.recursive).map_err(|error| {
        adapter_error(format!(
            "cannot walk the source root ({}): {error}",
            source_root.display()
        ))
    })?;
    let plan_files = files
        .iter()
        .map(|file| PlanC08File {
            rel_path: file.rel_path.clone(),
            size: file.size,
            modified_date: file.mtime.get(..10).map(String::from),
            exists: true,
        })
        .collect();
    let plan = plan_c08(PlanC08Input {
        plan_id: params.plan_id.clone().unwrap_or_else(|| new_ulid("c08")),
        source_root: source_root.to_string_lossy().into_owned(),
        target_root: target_root.to_string_lossy().into_owned(),
        rules,
        unmatched,
        files: plan_files,
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        long_path_supported: platform::preflight::long_path_supported(),
    })?;
    store(state, &plan);
    Ok(plan)
}

// ── C10 ZIP 解压 / 压缩 ────────────────────────────────────────────────

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZipPlanParams {
    /// `"extract"`（缺省）或 `"compress"`。
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub archive: Option<String>,
    #[serde(default)]
    pub output_dir: Option<String>,
    /// 解压限额（方案 §5.7 由调用方显式传入，壳层不设默认）。
    #[serde(default)]
    pub max_total_bytes: Option<u64>,
    #[serde(default)]
    pub max_ratio: Option<f64>,
    /// 压缩：产物路径与源集合。
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub compression: Option<String>,
    #[serde(default)]
    pub plan_id: Option<String>,
}

pub fn plan_zip_shell(
    state: &tauri::State<'_, AppState>,
    params: serde_json::Value,
) -> Result<PlanDocument, KernelError> {
    let params: ZipPlanParams = serde_json::from_value(params)
        .map_err(|error| KernelError::invalid_params(error.to_string()))?;
    let plan_id = params
        .plan_id
        .clone()
        .unwrap_or_else(|| new_ulid(params.mode.as_deref().unwrap_or("c10")));
    let service = ZipArchiveService;
    let plan = match params.mode.as_deref() {
        None | Some("extract") => {
            let archive = require_file(
                Path::new(
                    params
                        .archive
                        .as_deref()
                        .ok_or_else(|| missing("c10 extract: requires archive"))?,
                ),
                "c10",
            )?;
            let output_dir = require_dir(
                Path::new(
                    params
                        .output_dir
                        .as_deref()
                        .ok_or_else(|| missing("c10 extract: requires outputDir"))?,
                ),
                "c10",
            )?;
            let max_total_bytes = params
                .max_total_bytes
                .ok_or_else(|| missing("c10 extract: requires maxTotalBytes"))?;
            let max_ratio = params
                .max_ratio
                .filter(|ratio| ratio.is_finite() && *ratio > 0.0)
                .ok_or_else(|| missing("c10 extract: requires a positive maxRatio"))?;
            let inspect = service
                .inspect(&archive.to_string_lossy())
                .map_err(|error| {
                    KernelError::new(
                        "E1004",
                        format!(
                            "cannot inspect ZIP ({}): {}",
                            archive.display(),
                            error.message
                        ),
                        None,
                        false,
                    )
                })?;
            plan_c10(PlanC10Input {
                plan_id,
                archive: archive.to_string_lossy().into_owned(),
                archive_size: inspect.archive_size,
                entries: inspect.entries,
                output_dir: output_dir.to_string_lossy().into_owned(),
                max_total_bytes,
                max_ratio,
                existing_targets: existing_paths(&output_dir),
                protected_dirs: Vec::new(),
                case_sense: CaseSense::Sensitive,
                name_rules: platform::fs::name_rules(),
            })?
        }
        Some("compress") => {
            if params.sources.is_empty() {
                return Err(missing("c10 compress: requires at least one source"));
            }
            let output = PathBuf::from(
                params
                    .output
                    .as_deref()
                    .ok_or_else(|| missing("c10 compress: requires output"))?,
            );
            let compression =
                ArchiveCompression::parse(params.compression.as_deref().unwrap_or("deflate"))
                    .ok_or_else(|| {
                        KernelError::invalid_params(format!(
                            "c10 compress: unknown compression {}",
                            params.compression.as_deref().unwrap_or_default()
                        ))
                    })?;
            let inspect = inspect_sources(&params.sources)?;
            plan_c10_compress(PlanC10CompressInput {
                plan_id,
                output: output.to_string_lossy().into_owned(),
                sources: params.sources.clone(),
                compression,
                target_exists: output.exists(),
                source_file_count: inspect.file_count,
                source_bytes: inspect.total_bytes,
                protected_dirs: Vec::new(),
                case_sense: CaseSense::Sensitive,
            })?
        }
        Some(other) => {
            return Err(KernelError::invalid_params(format!(
                "c10: mode must be extract or compress: {other}"
            )))
        }
    };
    store(state, &plan);
    Ok(plan)
}
