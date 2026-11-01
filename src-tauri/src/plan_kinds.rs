//! 壳层 per-kind 计划函数（UI 接入切片第二批，ADR-0041）。
//!
//! 每个函数：web 参数（serde camelCase）→ 壳层事实采集（platform 同源函数）→
//! kernel 计划纯函数 → 存入 `AppState.plans` 供 `applyPlan` 既有通道执行。
//! 与 CLI 的 run_* 逻辑同源（错误经 IpcError 走 06 码位）；本模块不承载领域规则。

use std::path::{Path, PathBuf};

use kernel::csv::Delimiter;
use kernel::encoding::{detect, InvalidSequencePolicy, PlatformPrior, SourceEncoding, TargetEncoding};
use kernel::error::KernelError;
use kernel::fs::CaseSense;
use kernel::plan::{PlanC01File, PlanDocument};
use kernel::line_endings::{plan_c02, LineEnding, LineEndingStructureMode, PlanC02File, PlanC02Input};
use kernel::merge::{plan_f04, PlanF04File, PlanF04Input};
use kernel::replace_content::{plan_c03, CsvMode, ReplaceMode, PlanC03File, PlanC03Input};
use kernel::split::{plan_f01, plan_f02, plan_f03, PlanF01File, PlanF01Input, PlanF02Input, PlanF03Input};
use platform::ids::{derived_ulid, new_ulid};
use platform::text::{sample_bytes, split_layout, split_layout_by_lines, split_layout_by_parts, SampleSpec};

use crate::{AppState, ProtectedSource, StoredPlan};

fn store(state: &tauri::State<'_, AppState>, plan: &PlanDocument) {
    state.plans.lock().unwrap().insert(
        plan.plan_id.clone(),
        StoredPlan {
            plan: plan.clone(),
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            protected_source: ProtectedSource::Params,
        },
    );
}

fn require_file(path: &Path) -> Result<PathBuf, KernelError> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        KernelError::new(
            "E2001",
            format!("{}: {error}", path.display()),
            None,
            false,
        )
    })?;
    if !metadata.is_file() {
        return Err(KernelError::new(
            "E1002",
            format!("expected a regular file: {}", path.display()),
            None,
            false,
        ));
    }
    Ok(path.to_path_buf())
}

// ── F01/F02/F03 文本分片（共用 layout 采集）────────────────────────────

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitParams {
    pub file: String,
    pub output_dir: String,
    /// F01：目标分片字节数。
    #[serde(default)]
    pub target_bytes: Option<u64>,
    /// F02：每片行数。
    #[serde(default)]
    pub lines_per_part: Option<u64>,
    /// F03：目标份数。
    #[serde(default)]
    pub parts: Option<u64>,
    #[serde(default)]
    pub plan_id: Option<String>,
}

fn split_layout_for(
    kind: &str,
    params: &SplitParams,
    absolute: &Path,
) -> Result<kernel::split::SplitLayout, KernelError> {
    match kind {
        "f01" => {
            let bytes = params.target_bytes.ok_or_else(|| {
                KernelError::missing_param("f01 requires targetBytes")
            })?;
            split_layout(absolute, bytes).map_err(|error| {
                KernelError::new("E1004", error.to_string(), None, false)
            })
        }
        "f02" => {
            let lines = params.lines_per_part.ok_or_else(|| {
                KernelError::missing_param("f02 requires linesPerPart")
            })?;
            split_layout_by_lines(absolute, lines).map_err(|error| {
                KernelError::new("E1004", error.to_string(), None, false)
            })
        }
        "f03" => {
            let parts = params.parts.ok_or_else(|| {
                KernelError::missing_param("f03 requires parts")
            })?;
            split_layout_by_parts(absolute, parts).map_err(|error| {
                KernelError::new("E1004", error.to_string(), None, false)
            })
        }
        other => Err(KernelError::invalid_params(format!(
            "{other} is not a split kind"
        ))),
    }
}

pub fn plan_split_shell(
    state: &tauri::State<'_, AppState>,
    kind: &str,
    params: serde_json::Value,
) -> Result<PlanDocument, KernelError> {
    let params: SplitParams = serde_json::from_value(params).map_err(|error| KernelError::invalid_params(error.to_string()))?;
    let absolute = require_file(Path::new(&params.file))?;
    let output_dir = PathBuf::from(&params.output_dir);
    std::fs::create_dir_all(&output_dir).map_err(|error| {
        KernelError::new(
            "E1004",
            format!("{}: {error}", output_dir.display()),
            None,
            false,
        )
    })?;
    let resolved_plan_id = params.plan_id.clone().unwrap_or_else(|| new_ulid(kind));
    let layout = split_layout_for(kind, &params, &absolute)?;
    let canonical = platform::fs::canonicalize_display(&absolute);
    let plan = match kind {
        "f01" => plan_f01(PlanF01Input {
            plan_id: resolved_plan_id.clone(),
            target_bytes: params.target_bytes.unwrap_or_default(),
            output_dir: output_dir.to_string_lossy().into_owned(),
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            existing_targets: Vec::new(),
            files: vec![PlanF01File {
                path: absolute.to_string_lossy().into_owned(),
                canonical_path: canonical,
                group_id: derived_ulid(&format!("{kind}:{resolved_plan_id}")),
                layout,
            }],
        }),
        "f02" => plan_f02(PlanF02Input {
            plan_id: resolved_plan_id.clone(),
            lines_per_part: params.lines_per_part.unwrap_or_default(),
            output_dir: output_dir.to_string_lossy().into_owned(),
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            existing_targets: Vec::new(),
            files: vec![PlanF01File {
                path: absolute.to_string_lossy().into_owned(),
                canonical_path: canonical,
                group_id: derived_ulid(&format!("{kind}:{resolved_plan_id}")),
                layout,
            }],
        }),
        "f03" => plan_f03(PlanF03Input {
            plan_id: resolved_plan_id.clone(),
            parts: params.parts.unwrap_or_default(),
            output_dir: output_dir.to_string_lossy().into_owned(),
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            existing_targets: Vec::new(),
            files: vec![PlanF01File {
                path: absolute.to_string_lossy().into_owned(),
                canonical_path: canonical,
                group_id: derived_ulid(&format!("{kind}:{resolved_plan_id}")),
                layout,
            }],
        }),
        other => Err(KernelError::invalid_params(format!(
            "{other} is not a split kind"
        ))),
    }?;
    store(state, &plan);
    Ok(plan)
}

// ── F04 TXT 合并 ────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeParams {
    pub files: Vec<String>,
    pub output: String,
    pub target: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default = "default_invalid")]
    pub invalid: String,
    #[serde(default)]
    pub plan_id: Option<String>,
}

fn default_invalid() -> String {
    "replace-and-count".to_string()
}

fn parse_target(value: &str) -> Result<TargetEncoding, KernelError> {
    TargetEncoding::parse(value)
        .ok_or_else(|| KernelError::invalid_params(format!("unknown target encoding: {value}")))
}

fn parse_invalid(value: &str) -> Result<InvalidSequencePolicy, KernelError> {
    InvalidSequencePolicy::parse(value).ok_or_else(|| {
        KernelError::invalid_params(format!("unknown invalid-sequence policy: {value}"))
    })
}

fn parse_source(value: &Option<String>) -> Result<Option<SourceEncoding>, KernelError> {
    match value {
        Some(value) => SourceEncoding::parse(value)
            .map(Some)
            .ok_or_else(|| KernelError::invalid_params(format!("unknown source encoding: {value}"))),
        None => Ok(None),
    }
}

pub fn plan_merge_shell(
    state: &tauri::State<'_, AppState>,
    params: serde_json::Value,
) -> Result<PlanDocument, KernelError> {
    let params: MergeParams = serde_json::from_value(params).map_err(|error| KernelError::invalid_params(error.to_string()))?;
    if params.files.is_empty() {
        return Err(KernelError::missing_param("f04 requires at least one file"));
    }
    let target = parse_target(&params.target)?;
    let invalid = parse_invalid(&params.invalid)?;
    let source = parse_source(&params.source)?;
    let output = PathBuf::from(&params.output);
    let mut files = Vec::with_capacity(params.files.len());
    for path in &params.files {
        let absolute = require_file(Path::new(path))?;
        let sample = sample_bytes(
            &absolute,
            SampleSpec {
                window_bytes: 4096,
                blocks: 0,
            },
        )
        .map_err(|error| {
            KernelError::new("E1004", error.to_string(), None, false)
        })?;
        let source_encoding = match source {
            Some(encoding) => Some(encoding),
            None => detect(&sample.bytes.clone(), PlatformPrior::Other)
                .require_encoding()
                .ok(),
        };
        files.push(PlanF04File {
            path: absolute.to_string_lossy().into_owned(),
            canonical_path: platform::fs::canonicalize_display(&absolute),
            file_size: std::fs::metadata(&absolute)
                .map(|metadata| metadata.len())
                .unwrap_or_default(),
            source_encoding,
        });
    }
    let plan_id = params.plan_id.unwrap_or_else(|| new_ulid("f04"));
    let plan = plan_f04(PlanF04Input {
        plan_id: plan_id.clone(),
        target: output.to_string_lossy().into_owned(),
        target_encoding: target,
        invalid,
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        target_occupied: output.exists(),
        files,
    })?;
    store(state, &plan);
    Ok(plan)
}

// ── C01 编码转换 ────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscodeParams {
    pub files: Vec<String>,
    pub target: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default = "default_invalid")]
    pub invalid: String,
    #[serde(default)]
    pub prior: Option<String>,
    pub sample_window: u64,
    #[serde(default)]
    pub sample_blocks: u64,
    #[serde(default)]
    pub plan_id: Option<String>,
}

fn parse_prior(value: &Option<String>) -> Result<PlatformPrior, KernelError> {
    match value {
        Some(value) => PlatformPrior::parse(value).ok_or_else(|| {
            KernelError::invalid_params(format!("unknown platform prior: {value}"))
        }),
        None => Ok(PlatformPrior::Other),
    }
}

pub fn plan_transcode_shell(
    state: &tauri::State<'_, AppState>,
    params: serde_json::Value,
) -> Result<PlanDocument, KernelError> {
    let params: TranscodeParams = serde_json::from_value(params).map_err(|error| KernelError::invalid_params(error.to_string()))?;
    if params.files.is_empty() {
        return Err(KernelError::missing_param("c01 requires at least one file"));
    }
    let target = parse_target(&params.target)?;
    let invalid = parse_invalid(&params.invalid)?;
    let source = parse_source(&params.source)?;
    let prior = parse_prior(&params.prior)?;
    let mut files = Vec::with_capacity(params.files.len());
    for path in &params.files {
        let absolute = require_file(Path::new(path))?;
        let size = std::fs::metadata(&absolute)
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        let sampled = sample_bytes(
            &absolute,
            SampleSpec {
                window_bytes: params.sample_window,
                blocks: params.sample_blocks as usize,
            },
        )
        .map_err(|error| KernelError::new("E1004", error.to_string(), None, false))?;
        files.push(PlanC01File {
            path: absolute.to_string_lossy().into_owned(),
            canonical_path: platform::fs::canonicalize_display(&absolute),
            file_size: size,
            sample: sampled.bytes,
        });
    }
    let plan_id = params.plan_id.unwrap_or_else(|| new_ulid("c01"));
    let plan = kernel::plan::plan_c01(kernel::plan::PlanC01Input {
        plan_id: plan_id.clone(),
        target,
        invalid,
        prior,
        confirmed_source: source,
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        files,
    })?;
    store(state, &plan);
    Ok(plan)
}

// ── C02 换行转换 ────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineEndingsParams {
    pub files: Vec<String>,
    pub target: String,
    #[serde(default)]
    pub structure: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub prior: Option<String>,
    pub sample_bytes: usize,
    #[serde(default)]
    pub plan_id: Option<String>,
}

pub fn plan_line_endings_shell(
    state: &tauri::State<'_, AppState>,
    params: serde_json::Value,
) -> Result<PlanDocument, KernelError> {
    let params: LineEndingsParams = serde_json::from_value(params).map_err(|error| KernelError::invalid_params(error.to_string()))?;
    if params.files.is_empty() {
        return Err(KernelError::missing_param("c02 requires at least one file"));
    }
    let target = LineEnding::parse(&params.target).ok_or_else(|| {
        KernelError::invalid_params(format!("unknown line ending: {}", params.target))
    })?;
    let structure = match &params.structure {
        Some(value) => LineEndingStructureMode::parse(value).ok_or_else(|| {
            KernelError::invalid_params(format!("unknown structure mode: {value}"))
        })?,
        None => LineEndingStructureMode::Auto,
    };
    let source = parse_source(&params.source)?;
    let prior = parse_prior(&params.prior)?;
    let mut files = Vec::with_capacity(params.files.len());
    for path in &params.files {
        let absolute = require_file(Path::new(path))?;
        let size = std::fs::metadata(&absolute)
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        // 结构模式 → 每文件解析为具体结构（与 CLI 同源：mode.resolve）。
        let resolved = structure.resolve(&absolute.to_string_lossy());
        let inspect = platform::line_endings::inspect_line_endings(
            &absolute,
            resolved,
            params.sample_bytes,
        )
        .map_err(|error| KernelError::new("E1004", error.message, None, false))?;
        files.push(PlanC02File {
            path: absolute.to_string_lossy().into_owned(),
            canonical_path: platform::fs::canonicalize_display(&absolute),
            file_size: size,
            sample: inspect.sample,
            counts: inspect.counts,
            structure: inspect.structure,
        });
    }
    let plan_id = params.plan_id.unwrap_or_else(|| new_ulid("c02"));
    let plan = plan_c02(PlanC02Input {
        plan_id: plan_id.clone(),
        target,
        structure,
        files,
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        confirmed_source: source,
        prior,
    })?;
    store(state, &plan);
    Ok(plan)
}

// ── C03 查找替换 ────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceParams {
    pub files: Vec<String>,
    pub mode: String,
    pub find: String,
    pub replacement: String,
    #[serde(default = "default_csv_mode")]
    pub csv_mode: String,
    #[serde(default)]
    pub delimiter: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub prior: Option<String>,
    pub sample_bytes: usize,
    #[serde(default)]
    pub plan_id: Option<String>,
}

fn default_csv_mode() -> String {
    "plain".to_string()
}

pub fn plan_replace_shell(
    state: &tauri::State<'_, AppState>,
    params: serde_json::Value,
) -> Result<PlanDocument, KernelError> {
    let params: ReplaceParams = serde_json::from_value(params).map_err(|error| KernelError::invalid_params(error.to_string()))?;
    if params.files.is_empty() {
        return Err(KernelError::missing_param("c03 requires at least one file"));
    }
    if params.find.is_empty() {
        return Err(KernelError::invalid_params("find must not be empty"));
    }
    let mode = ReplaceMode::parse(&params.mode).ok_or_else(|| {
        KernelError::invalid_params(format!("unknown replace mode: {}", params.mode))
    })?;
    let csv_mode = CsvMode::parse(&params.csv_mode).ok_or_else(|| {
        KernelError::invalid_params(format!("unknown csv mode: {}", params.csv_mode))
    })?;
    let delimiter = match &params.delimiter {
        Some(value) => Some(Delimiter::parse(value).ok_or_else(|| {
            KernelError::invalid_params(format!("unknown delimiter: {value}"))
        })?),
        None => None,
    };
    let source = parse_source(&params.source)?;
    let prior = parse_prior(&params.prior)?;
    let mut files = Vec::with_capacity(params.files.len());
    for path in &params.files {
        let absolute = require_file(Path::new(path))?;
        let inspect = platform::replace_content::inspect_file(
            &absolute,
            mode,
            &params.find,
            &params.replacement,
            csv_mode,
            delimiter,
            params.sample_bytes,
        )
        .map_err(|error| KernelError::new("E1004", error.message, None, false))?;
        files.push(PlanC03File {
            path: absolute.to_string_lossy().into_owned(),
            canonical_path: platform::fs::canonicalize_display(&absolute),
            file_size: inspect.file_size,
            sample: inspect.sample,
            structure: inspect.structure,
            inspect: inspect.inspect,
        });
    }
    let plan_id = params.plan_id.unwrap_or_else(|| new_ulid("c03"));
    let plan = plan_c03(PlanC03Input {
        plan_id: plan_id.clone(),
        mode,
        find: params.find,
        replacement: params.replacement,
        csv_mode,
        files,
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        confirmed_source: source,
        prior,
        delimiter,
    })?;
    store(state, &plan);
    Ok(plan)
}
