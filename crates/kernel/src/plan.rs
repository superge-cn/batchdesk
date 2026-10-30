//! C09 plan generation. Pure: it never reads or writes the filesystem.
//!
//! The returned `PlanDocument` follows design 01 §5. C09 cleanup is a
//! `delete` action with no target; apply is responsible for moving the file
//! to the quarantine directory. Candidate (fast-mode) groups are rejected:
//! cleanup requires full-hash confirmation (baseline §5.10, 11.6-20).
//! The caller supplies `planId`; this module never generates IDs or reads time.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::duplicate::{Confidence, DuplicateGroup, DuplicateReport, GroupingMode};
use crate::encoding::{
    detect, detect_bom, Bom, InvalidSequencePolicy, PlatformPrior, SourceEncoding, TargetEncoding,
};
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::{protected_by, try_normalize, ProtectedDir};

pub const C09_KIND: &str = "c09";
pub const C01_KIND: &str = "c01";

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Metadata the planner may use for the keep suggestion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanFile {
    pub id: u64,
    pub path: String,
    pub canonical_path: Option<String>,
    pub mtime_nanos: i128,
    pub full_hash: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeepPolicy {
    Earliest,
    ShortestPath,
    Newest,
}

impl KeepPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Earliest => "earliest",
            Self::ShortestPath => "shortest-path",
            Self::Newest => "newest",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "earliest" => Some(Self::Earliest),
            "shortest-path" => Some(Self::ShortestPath),
            "newest" => Some(Self::Newest),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlanParamValue {
    String(String),
    Number(u64),
    Bool(bool),
    Null,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Safe,
    Caution,
    Danger,
}

impl RiskLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Caution => "caution",
            Self::Danger => "danger",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanAction {
    Rename,
    Move,
    Copy,
    Archive,
    Extract,
    CreateDir,
    Transcode,
    EditContent,
    Delete,
    /// F01 文本分片产物（1:N，每个分片一条 entry；方案 §5.1、设计 01 §5）。
    Split,
    /// F04 TXT 合并产物（N:1，一条 entry 汇总全部输入；方案 §5.15）。
    Merge,
    /// F07 CSV 抽取指定列产物（1:1，每个输入一条 entry；方案 §5.15）。
    CsvExtract,
}

impl PlanAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rename => "rename",
            Self::Move => "move",
            Self::Copy => "copy",
            Self::Archive => "archive",
            Self::Extract => "extract",
            Self::CreateDir => "create-dir",
            Self::Transcode => "transcode",
            Self::EditContent => "edit-content",
            Self::Delete => "delete",
            Self::Split => "split",
            Self::Merge => "merge",
            Self::CsvExtract => "csv-extract",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanResolution {
    Proceed,
    Skip,
    Sequence,
    Overwrite,
}

impl PlanResolution {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proceed => "proceed",
            Self::Skip => "skip",
            Self::Sequence => "sequence",
            Self::Overwrite => "overwrite",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanRole {
    Source,
    Output,
}

impl PlanRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Output => "output",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRelation {
    pub group_id: String,
    pub role: PlanRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part_total: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanConflict {
    pub kind: String,
    pub risk_level: RiskLevel,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanBlocked {
    pub code: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanExpected {
    pub size: u64,
    pub algorithm: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

/// F01 分片在源文件中的字节区间（左闭右开；仅 `split` 动作使用）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRange {
    pub start: u64,
    pub end: u64,
}

/// F04 合并的单个输入（按顺序追加；`size` 用于 plan→apply 期间的变化复核）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanMergeSource {
    pub path: String,
    /// 检测或用户确认的源编码；`None` 表示需人工确认（计划条目按 E3001 阻断）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<crate::encoding::SourceEncoding>,
    pub size: u64,
}

/// F04 合并的载荷（仅 `merge` 动作使用）：按序拼接的输入＋目标编码与非法序列策略。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanMerge {
    pub sources: Vec<PlanMergeSource>,
    /// 目标编码（`TargetEncoding::as_str` 的稳定字符串，含 BOM 策略）。
    pub target_encoding: String,
    /// 非法序列策略（`InvalidSequencePolicy::as_str`）。
    pub invalid: String,
}

/// F05 CSV 分割的载荷（仅 `split` 动作使用）：表头区间、分隔符与本片统计。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanCsvSplit {
    /// 表头在源文件中的字节区间；每片先写该区间再写 `range` 的记录。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<PlanRange>,
    /// 分隔符的稳定字符串（`,` `;` `\t` `|`）。
    pub delimiter: String,
    /// 本片记录数（不含表头）。
    pub rows: u64,
    /// 本片列数与表头不一致的记录数（只上报，不静默修正）。
    pub dirty_rows: u64,
}

/// F06 CSV 合并的单个输入（表头已按规范化规则处理；`columns` 保留重复项与顺序）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanCsvMergeFile {
    pub path: String,
    /// 检测或用户确认的源编码；`None` 表示需人工确认（计划条目按 E3001 阻断）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<crate::encoding::SourceEncoding>,
    pub size: u64,
    /// 该文件的分隔符（稳定字符串），apply 按它解析该文件。
    pub delimiter: String,
    /// 规范化后的表头字段（保留重复项）；无表头时为空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    /// 来源列取值（文件名或相对路径）；未启用来源列时为 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_label: Option<String>,
    /// 数据行列数与表头不一致的记录数（只上报，不静默修正）。
    pub dirty_rows: u64,
}

/// F06 CSV 合并的载荷（仅 `merge` 动作使用）：列并集、来源列与目标编码。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanCsvMerge {
    pub files: Vec<PlanCsvMergeFile>,
    /// 输出列并集（规范化列名，保留重复项）；无表头时为空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub union: Vec<String>,
    /// 无表头时按列序号对齐的列宽（取各文件最大列数）。
    pub positional_columns: u64,
    /// 全部输入是否都有表头（混合模式在计划期阻断 E3004）。
    pub has_header: bool,
    /// 来源列名（写入输出首列）；未启用时为 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_column: Option<String>,
    /// 目标编码（`TargetEncoding::as_str` 的稳定字符串，含 BOM 策略）。
    pub target_encoding: String,
    /// 非法序列策略（`InvalidSequencePolicy::as_str`）。
    pub invalid: String,
    /// 输出分隔符的稳定字符串。
    pub delimiter: String,
    /// 严格模式：列不一致在计划期阻断 E3004。
    pub strict: bool,
}

/// F07 抽取列在输出中的位置（计划期按表头名/列序号解析后的 0-based 字段序号）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanCsvExtractColumn {
    pub index: u64,
    /// 有表头时的规范化列名（用于输出表头）；无表头为 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// F07 CSV 抽取的载荷（仅 `csv-extract` 动作使用）：源结构＋输出顺序。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanCsvExtract {
    /// 检测或用户确认的源编码；`None` 表示需人工确认（计划条目按 E3001 阻断）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<crate::encoding::SourceEncoding>,
    pub size: u64,
    /// 输入分隔符（输出沿用同一分隔符）。
    pub delimiter: String,
    pub has_header: bool,
    /// 规范化表头字段（保留重复项）；无表头时为空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    /// 源列宽（有表头时取表头字段数；无表头时取数据行最大字段数）。
    pub column_count: u64,
    /// 按用户选择顺序解析后的输出列。
    pub selections: Vec<PlanCsvExtractColumn>,
    /// 目标编码（`TargetEncoding::as_str` 的稳定字符串，含 BOM 策略）。
    pub target_encoding: String,
    /// 非法序列策略（`InvalidSequencePolicy::as_str`）。
    pub invalid: String,
    /// 数据行字段数与列宽不一致的记录数（只上报，不静默修正）。
    pub dirty_rows: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntry {
    pub entry_id: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub action: PlanAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relation: Option<PlanRelation>,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<PlanConflict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<PlanResolution>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<PlanBlocked>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<PlanExpected>,
    /// 追加式可选字段：分片产物的源字节区间（仅 `split` 动作使用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<PlanRange>,
    /// 追加式可选字段：合并的输入清单与编码策略（仅 `merge` 动作使用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge: Option<PlanMerge>,
    /// 追加式可选字段：CSV 表头区间与分隔符（仅 F05 的 `split` 条目使用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csv: Option<PlanCsvSplit>,
    /// 追加式可选字段：CSV 合并的列并集与来源列（仅 F06 的 `merge` 条目使用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csv_merge: Option<PlanCsvMerge>,
    /// 追加式可选字段：CSV 抽取列的选择与源结构（仅 F07 的 `csv-extract` 条目使用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csv_extract: Option<PlanCsvExtract>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanWarning {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    pub risk_level: RiskLevel,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStats {
    pub total: usize,
    pub conflict: usize,
    pub warning: usize,
    pub blocked: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDocument {
    pub plan_id: String,
    pub kind: String,
    pub params: BTreeMap<String, PlanParamValue>,
    pub entries: Vec<PlanEntry>,
    pub stats: PlanStats,
    pub warnings: Vec<PlanWarning>,
}

#[derive(Clone, Debug)]
pub struct PlanC09Input {
    pub plan_id: String,
    pub keep: KeepPolicy,
    pub mode: GroupingMode,
    pub hash_algorithm: String,
    pub quick_window_bytes: Option<u64>,
    pub include_zero_byte: bool,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub files: Vec<PlanFile>,
    pub report: DuplicateReport,
    pub skipped_paths: Vec<String>,
}

/// Build a C09 cleanup plan for confirmed duplicate groups.
///
/// Groups with unknown file identity are excluded and surfaced as a warning;
/// candidate groups and fast-mode inputs are rejected because cleanup requires
/// full-hash confirmation.
pub fn plan_c09(input: PlanC09Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.hash_algorithm.trim().is_empty() {
        return Err(KernelError::missing_param("hashAlgorithm is required"));
    }
    if input.mode == GroupingMode::Fast {
        return Err(KernelError::invalid_params(
            "fast mode produces candidate groups only; run full hashing before planning cleanup",
        ));
    }
    if input
        .report
        .groups
        .iter()
        .any(|group| group.confidence == Confidence::Candidate)
    {
        return Err(KernelError::invalid_params(
            "candidate duplicate groups cannot be planned for cleanup",
        ));
    }

    let files = index_files(&input.files)?;
    let mut params = BTreeMap::new();
    params.insert(
        "keep".to_string(),
        PlanParamValue::String(input.keep.as_str().to_string()),
    );
    params.insert(
        "mode".to_string(),
        PlanParamValue::String("full".to_string()),
    );
    params.insert(
        "hash".to_string(),
        PlanParamValue::String(input.hash_algorithm.clone()),
    );
    params.insert(
        "quickWindowBytes".to_string(),
        match input.quick_window_bytes {
            Some(window) => PlanParamValue::Number(window),
            None => PlanParamValue::Null,
        },
    );
    params.insert(
        "includeZeroByte".to_string(),
        PlanParamValue::Bool(input.include_zero_byte),
    );

    let mut entries = Vec::new();
    let mut warnings = Vec::new();

    for (group_index, group) in input.report.groups.iter().enumerate() {
        let members = group_members(group, &files)?;
        if group.needs_manual_review {
            warnings.push(PlanWarning {
                kind: "needs-manual-review".to_string(),
                suggestion: Some(format!(
                    "group {} (size {}, {} files) has unknown file identity; excluded from cleanup",
                    group_index + 1,
                    group.size,
                    group.members.len()
                )),
                risk_level: RiskLevel::Caution,
            });
            continue;
        }
        let keep = select_keep(&members, input.keep)?;
        for member in &members {
            if member.id == keep.id {
                continue;
            }
            let blocked = match try_normalize(&member.path) {
                Ok(normalized) => protected_by(
                    &normalized,
                    member.canonical_path.as_deref(),
                    &input.protected_dirs,
                    input.case_sense,
                )
                .map(|hit| PlanBlocked {
                    code: "E2009".to_string(),
                    reason: format!("path is under protected directory {hit}"),
                    hint: Some("remove it from protectedDirs or choose another source".to_string()),
                }),
                Err(error) => Some(PlanBlocked {
                    code: error.code,
                    reason: error.message,
                    hint: None,
                }),
            };
            entries.push(PlanEntry {
                entry_id: format!("{}-{:04}", input.plan_id, entries.len() + 1),
                source: member.path.clone(),
                target: None,
                action: PlanAction::Delete,
                relation: None,
                reason: format!(
                    "duplicate of {} (group size {}, keep by {}); apply moves this file to the quarantine directory",
                    keep.path,
                    group.size,
                    input.keep.as_str()
                ),
                conflict: None,
                resolution: None,
                blocked,
                expected: Some(PlanExpected {
                    size: group.size,
                    algorithm: input.hash_algorithm.clone(),
                    hash: member.full_hash.as_deref().map(hex_encode),
                }),
                range: None,
                merge: None,
                csv: None,
                csv_merge: None,
                csv_extract: None,
            });
        }
    }

    if !input.report.hardlink_skipped.is_empty() {
        warnings.push(PlanWarning {
            kind: "hardlink-skipped".to_string(),
            suggestion: Some(format!(
                "{} hardlink entries were skipped as the same file",
                input.report.hardlink_skipped.len()
            )),
            risk_level: RiskLevel::Safe,
        });
    }
    if !input.report.incomplete.is_empty() {
        warnings.push(PlanWarning {
            kind: "incomplete-hash".to_string(),
            suggestion: Some(format!(
                "{} candidates lack a full digest; excluded from cleanup",
                input.report.incomplete.len()
            )),
            risk_level: RiskLevel::Caution,
        });
    }
    if !input.report.zero_byte_ids.is_empty() {
        warnings.push(PlanWarning {
            kind: "zero-byte-ignored".to_string(),
            suggestion: Some(format!(
                "{} zero-byte files ignored",
                input.report.zero_byte_ids.len()
            )),
            risk_level: RiskLevel::Safe,
        });
    }
    if !input.skipped_paths.is_empty() {
        warnings.push(PlanWarning {
            kind: "scan-skipped".to_string(),
            suggestion: Some(format!(
                "{} entries were skipped during scanning; not planned",
                input.skipped_paths.len()
            )),
            risk_level: RiskLevel::Caution,
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
        kind: C09_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

/// C01 逐文件输入。`sample` 由壳层采样读取（`platform::text::sample_bytes`），
/// kernel 只做纯判定（方案 §5.2"头部与随机采样块"）。
#[derive(Clone, Debug)]
pub struct PlanC01File {
    pub path: String,
    pub canonical_path: Option<String>,
    pub file_size: u64,
    pub sample: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct PlanC01Input {
    pub plan_id: String,
    pub target: TargetEncoding,
    pub invalid: InvalidSequencePolicy,
    pub prior: PlatformPrior,
    /// 用户显式选定的**全局**源编码；None 时逐文件按采样检测。
    pub confirmed_source: Option<SourceEncoding>,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub files: Vec<PlanC01File>,
}

/// Build a C01 transcode plan (in-place `edit-content` entries).
///
/// Detection runs on the shell-provided sample; a manual-confirmation result
/// becomes a blocked E3001 entry (never a silent guess, §5.2). Files already in
/// the target encoding/BOM policy are `resolution: skip`. Apply re-detects on
/// the full content unless `confirmed_source` was recorded in `params.source`.
pub fn plan_c01(input: PlanC01Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.files.is_empty() {
        return Err(KernelError::missing_param("c01 requires at least one file"));
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
        "platformPrior".to_string(),
        PlanParamValue::String(input.prior.as_str().to_string()),
    );
    if let Some(source) = input.confirmed_source {
        params.insert(
            "source".to_string(),
            PlanParamValue::String(source.as_str().to_string()),
        );
    }

    let mut entries = Vec::with_capacity(input.files.len());
    for file in &input.files {
        let detection = detect(&file.sample, input.prior);
        let selected = input.confirmed_source.or_else(|| {
            (!detection.needs_manual)
                .then_some(detection.encoding)
                .flatten()
        });
        let (blocked, reason, skip) = match selected {
            Some(source) => {
                let blocked = c01_path_block(
                    &file.path,
                    file.canonical_path.as_deref(),
                    &input.protected_dirs,
                    input.case_sense,
                );
                let origin = if input.confirmed_source.is_some() {
                    "user-selected"
                } else {
                    "detected"
                };
                let skip = already_in_target(&file.sample, source, input.target);
                let reason = if skip {
                    format!(
                        "{origin} {}; already in target encoding {}, no conversion needed",
                        source.as_str(),
                        input.target.as_str()
                    )
                } else {
                    let mut reason =
                        format!("{origin} {} -> {}", source.as_str(), input.target.as_str());
                    if let Some(note) = &detection.note {
                        reason.push_str("; note: ");
                        reason.push_str(note);
                    }
                    reason
                };
                (blocked, reason, skip)
            }
            None => {
                let detected = detection.encoding.map_or("unknown", |value| value.as_str());
                let mut reason = format!(
                    "encoding is undetermined (detected {detected}); user confirmation is required"
                );
                if let Some(note) = &detection.note {
                    reason.push_str("; note: ");
                    reason.push_str(note);
                }
                (
                    Some(PlanBlocked {
                        code: "E3001".to_string(),
                        reason: "encoding is undetermined; user confirmation is required"
                            .to_string(),
                        hint: Some(
                            "preview candidate decodings and choose the source encoding"
                                .to_string(),
                        ),
                    }),
                    reason,
                    false,
                )
            }
        };

        entries.push(PlanEntry {
            entry_id: format!("{}-{:04}", input.plan_id, entries.len() + 1),
            source: file.path.clone(),
            target: None,
            action: PlanAction::EditContent,
            relation: None,
            reason,
            conflict: None,
            resolution: skip.then_some(PlanResolution::Skip),
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
        });
    }

    let stats = PlanStats {
        total: entries.len(),
        conflict: 0,
        warning: 0,
        blocked: entries
            .iter()
            .filter(|entry| entry.blocked.is_some())
            .count(),
    };

    Ok(PlanDocument {
        plan_id: input.plan_id,
        kind: C01_KIND.to_string(),
        params,
        entries,
        stats,
        warnings: Vec::new(),
    })
}

fn c01_path_block(
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

/// 源编码族与目标一致、且 BOM 已符合目标策略时，无需改写。
fn already_in_target(sample: &[u8], family: SourceEncoding, target: TargetEncoding) -> bool {
    if family != target.source_family() {
        return false;
    }
    let Some(required) = target.bom_requirement() else {
        return true;
    };
    let present = matches!(
        (family, detect_bom(sample)),
        (SourceEncoding::Utf8, Some(Bom::Utf8))
            | (SourceEncoding::Utf16Le, Some(Bom::Utf16Le))
            | (SourceEncoding::Utf16Be, Some(Bom::Utf16Be))
    );
    present == required
}

fn index_files(files: &[PlanFile]) -> Result<BTreeMap<u64, &PlanFile>, KernelError> {
    let mut index = BTreeMap::new();
    for file in files {
        if index.insert(file.id, file).is_some() {
            return Err(KernelError::internal(
                "duplicate file id in plan input",
                format!("id {}", file.id),
            ));
        }
    }
    Ok(index)
}

fn group_members<'a>(
    group: &DuplicateGroup,
    files: &'a BTreeMap<u64, &'a PlanFile>,
) -> Result<Vec<&'a PlanFile>, KernelError> {
    group
        .members
        .iter()
        .map(|id| {
            files.get(id).copied().ok_or_else(|| {
                KernelError::internal(
                    "plan input is missing a file referenced by the duplicate report",
                    format!("missing id {id}"),
                )
            })
        })
        .collect()
}

fn select_keep<'a>(
    members: &[&'a PlanFile],
    policy: KeepPolicy,
) -> Result<&'a PlanFile, KernelError> {
    members
        .iter()
        .copied()
        .min_by(|left, right| compare_keep(left, right, policy))
        .ok_or_else(|| {
            KernelError::internal("duplicate group has no members", "group members were empty")
        })
}

fn compare_keep(left: &PlanFile, right: &PlanFile, policy: KeepPolicy) -> Ordering {
    let by_path = path_len(left)
        .cmp(&path_len(right))
        .then_with(|| left.path.cmp(&right.path));
    match policy {
        KeepPolicy::Earliest => left.mtime_nanos.cmp(&right.mtime_nanos).then(by_path),
        KeepPolicy::Newest => right.mtime_nanos.cmp(&left.mtime_nanos).then(by_path),
        KeepPolicy::ShortestPath => by_path,
    }
}

fn path_len(file: &PlanFile) -> usize {
    file.path.chars().count()
}

pub(crate) fn is_ulid(value: &str) -> bool {
    value.len() == 26
        && value.as_bytes()[0] <= b'7'
        && value.bytes().all(|byte| CROCKFORD.contains(&byte))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN_ID: &str = "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8";

    fn file(id: u64, path: &str, mtime_nanos: i128) -> PlanFile {
        PlanFile {
            id,
            path: path.to_string(),
            canonical_path: None,
            mtime_nanos,
            full_hash: Some(vec![id as u8; 4]),
        }
    }

    fn group(size: u64, members: Vec<u64>) -> DuplicateGroup {
        DuplicateGroup {
            size,
            hash: None,
            members,
            confidence: Confidence::Confirmed,
            needs_manual_review: false,
        }
    }

    fn input(files: Vec<PlanFile>, groups: Vec<DuplicateGroup>, keep: KeepPolicy) -> PlanC09Input {
        PlanC09Input {
            plan_id: PLAN_ID.to_string(),
            keep,
            mode: GroupingMode::Full,
            hash_algorithm: "blake3".to_string(),
            quick_window_bytes: Some(4096),
            include_zero_byte: false,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            files,
            report: DuplicateReport {
                groups,
                zero_byte_ids: Vec::new(),
                hardlink_skipped: Vec::new(),
                incomplete: Vec::new(),
            },
            skipped_paths: Vec::new(),
        }
    }

    #[test]
    fn earliest_keeps_oldest_mtime() {
        let plan = plan_c09(input(
            vec![file(1, "a", 20), file(2, "b", 10), file(3, "c", 30)],
            vec![group(100, vec![1, 2, 3])],
            KeepPolicy::Earliest,
        ))
        .unwrap();
        assert_eq!(plan.entries.len(), 2);
        assert!(plan
            .entries
            .iter()
            .all(|entry| !entry.source.ends_with("b")));
        assert!(plan.entries[0].reason.contains("b"));
        assert_eq!(plan.stats.total, 2);
        assert_eq!(
            plan.params["keep"],
            PlanParamValue::String("earliest".into())
        );
    }

    #[test]
    fn entries_carry_expected_size_and_hex_hash() {
        let plan = plan_c09(input(
            vec![file(1, "a", 20), file(2, "b", 10)],
            vec![group(100, vec![1, 2])],
            KeepPolicy::Earliest,
        ))
        .unwrap();
        let expected = plan.entries[0].expected.as_ref().unwrap();
        assert_eq!(expected.size, 100);
        assert_eq!(expected.algorithm, "blake3");
        assert_eq!(expected.hash.as_deref(), Some("01010101"));
    }

    #[test]
    fn newest_keeps_latest_mtime() {
        let plan = plan_c09(input(
            vec![file(1, "a", 20), file(2, "b", 10), file(3, "c", 30)],
            vec![group(100, vec![1, 2, 3])],
            KeepPolicy::Newest,
        ))
        .unwrap();
        assert!(plan
            .entries
            .iter()
            .all(|entry| !entry.source.ends_with("c")));
        assert!(plan.entries[0].reason.contains("c"));
    }

    #[test]
    fn shortest_path_keeps_shortest_and_breaks_ties_lexicographically() {
        let plan = plan_c09(input(
            vec![
                file(1, "root/long.txt", 20),
                file(2, "root/b", 20),
                file(3, "root/a", 20),
            ],
            vec![group(100, vec![1, 2, 3])],
            KeepPolicy::ShortestPath,
        ))
        .unwrap();
        assert_eq!(plan.entries.len(), 2);
        assert!(plan
            .entries
            .iter()
            .all(|entry| !entry.source.ends_with("root/a")));
        assert!(plan.entries[0].reason.contains("root/a"));
    }

    #[test]
    fn manual_review_groups_are_excluded_with_a_warning() {
        let mut duplicate_group = group(100, vec![1, 2]);
        duplicate_group.needs_manual_review = true;
        let plan = plan_c09(input(
            vec![file(1, "a", 10), file(2, "b", 20)],
            vec![duplicate_group],
            KeepPolicy::Earliest,
        ))
        .unwrap();
        assert!(plan.entries.is_empty());
        assert_eq!(plan.warnings.len(), 1);
        assert_eq!(plan.warnings[0].kind, "needs-manual-review");
        assert_eq!(plan.stats.warning, 1);
    }

    #[test]
    fn protected_group_member_is_blocked_with_e2009() {
        let mut plan_input = input(
            vec![file(1, "/allowed/a", 10), file(2, "/protected/b", 20)],
            vec![group(100, vec![1, 2])],
            KeepPolicy::Earliest,
        );
        plan_input.protected_dirs = vec![ProtectedDir::new("/protected")];
        let plan = plan_c09(plan_input).unwrap();
        assert_eq!(plan.entries.len(), 1);
        let blocked = plan.entries[0].blocked.as_ref().unwrap();
        assert_eq!(blocked.code, "E2009");
        assert_eq!(plan.stats.blocked, 1);
    }

    #[test]
    fn candidate_groups_are_rejected() {
        let mut candidate_group = group(100, vec![1, 2]);
        candidate_group.confidence = Confidence::Candidate;
        let error = plan_c09(input(
            vec![file(1, "a", 10), file(2, "b", 20)],
            vec![candidate_group],
            KeepPolicy::Earliest,
        ))
        .unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn fast_mode_is_rejected() {
        let mut plan_input = input(
            vec![file(1, "a", 10), file(2, "b", 20)],
            vec![group(100, vec![1, 2])],
            KeepPolicy::Earliest,
        );
        plan_input.mode = GroupingMode::Fast;
        let error = plan_c09(plan_input).unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn empty_plan_id_is_rejected() {
        let mut plan_input = input(
            vec![file(1, "a", 10), file(2, "b", 20)],
            vec![group(100, vec![1, 2])],
            KeepPolicy::Earliest,
        );
        plan_input.plan_id = "  ".to_string();
        let error = plan_c09(plan_input).unwrap_err();
        assert_eq!(error.code, "E1001");
    }

    #[test]
    fn invalid_plan_id_is_rejected() {
        let mut plan_input = input(
            vec![file(1, "a", 10), file(2, "b", 20)],
            vec![group(100, vec![1, 2])],
            KeepPolicy::Earliest,
        );
        plan_input.plan_id = "not-a-ulid".to_string();
        let error = plan_c09(plan_input).unwrap_err();
        assert_eq!(error.code, "E1002");
    }

    #[test]
    fn empty_hash_algorithm_is_rejected() {
        let mut plan_input = input(
            vec![file(1, "a", 10), file(2, "b", 20)],
            vec![group(100, vec![1, 2])],
            KeepPolicy::Earliest,
        );
        plan_input.hash_algorithm = "  ".to_string();
        let error = plan_c09(plan_input).unwrap_err();
        assert_eq!(error.code, "E1001");
    }

    #[test]
    fn incomplete_zero_byte_and_scan_skips_become_warnings() {
        let mut plan_input = input(
            vec![file(1, "a", 10), file(2, "b", 20)],
            vec![group(100, vec![1, 2])],
            KeepPolicy::Earliest,
        );
        plan_input.report.incomplete = vec![7];
        plan_input.report.zero_byte_ids = vec![8];
        plan_input.report.hardlink_skipped = vec![9];
        plan_input.skipped_paths = vec!["locked".to_string()];
        let plan = plan_c09(plan_input).unwrap();
        let kinds: Vec<&str> = plan
            .warnings
            .iter()
            .map(|warning| warning.kind.as_str())
            .collect();
        assert_eq!(
            kinds,
            vec![
                "hardlink-skipped",
                "incomplete-hash",
                "zero-byte-ignored",
                "scan-skipped"
            ]
        );
        assert_eq!(plan.stats.warning, 4);
    }

    #[test]
    fn missing_group_member_is_an_internal_error() {
        let error = plan_c09(input(
            vec![file(1, "a", 10)],
            vec![group(100, vec![1, 2])],
            KeepPolicy::Earliest,
        ))
        .unwrap_err();
        assert_eq!(error.code, "E9001");
    }

    #[test]
    fn entry_ids_are_unique_and_include_the_plan_id() {
        let plan = plan_c09(input(
            vec![
                file(1, "a", 10),
                file(2, "b", 20),
                file(3, "c", 30),
                file(4, "d", 40),
            ],
            vec![group(100, vec![1, 2, 3, 4])],
            KeepPolicy::Earliest,
        ))
        .unwrap();
        let mut ids: Vec<&str> = plan
            .entries
            .iter()
            .map(|entry| entry.entry_id.as_str())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3);
        assert!(plan
            .entries
            .iter()
            .all(|entry| entry.entry_id.starts_with(PLAN_ID)));
        assert!(plan
            .entries
            .iter()
            .all(|entry| entry.action == PlanAction::Delete && entry.target.is_none()));
    }

    fn c01_file(path: &str, sample: &[u8]) -> PlanC01File {
        PlanC01File {
            path: path.to_string(),
            canonical_path: None,
            file_size: sample.len() as u64,
            sample: sample.to_vec(),
        }
    }

    fn c01_input(files: Vec<PlanC01File>, target: TargetEncoding) -> PlanC01Input {
        PlanC01Input {
            plan_id: PLAN_ID.to_string(),
            target,
            invalid: InvalidSequencePolicy::ReplaceAndCount,
            prior: PlatformPrior::Other,
            confirmed_source: None,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            files,
        }
    }

    const CHINESE: &[u8] = "中文内容，用于编码检测采样。".as_bytes();

    #[test]
    fn c01_plans_edit_content_for_detected_utf8() {
        let plan = plan_c01(c01_input(
            vec![c01_file("a.txt", CHINESE)],
            TargetEncoding::Gb18030,
        ))
        .unwrap();
        assert_eq!(plan.kind, C01_KIND);
        assert_eq!(plan.entries.len(), 1);
        let entry = &plan.entries[0];
        assert_eq!(entry.action, PlanAction::EditContent);
        assert!(entry.target.is_none(), "就地改写不应有目标路径");
        assert!(entry.blocked.is_none());
        assert_eq!(entry.resolution, None);
        assert!(entry.reason.contains("detected utf-8"));
        assert!(entry.reason.contains("-> gb18030"));
        assert_eq!(entry.expected.as_ref().unwrap().size, CHINESE.len() as u64);
        assert_eq!(
            plan.params["target"],
            PlanParamValue::String("gb18030".into())
        );
        assert_eq!(plan.stats.total, 1);
        assert_eq!(plan.stats.blocked, 0);
    }

    #[test]
    fn c01_blocks_ambiguous_samples_with_e3001() {
        // 控制字符污染 → 检测要求人工确认（§5.2 绝不静默猜错）。
        let plan = plan_c01(c01_input(
            vec![c01_file("binary.bin", &[0x01, 0x02, 0x03, 0x04])],
            TargetEncoding::Utf8 { bom: false },
        ))
        .unwrap();
        let entry = &plan.entries[0];
        assert_eq!(entry.blocked.as_ref().unwrap().code, "E3001");
        assert_eq!(entry.resolution, None);
        assert_eq!(plan.stats.blocked, 1);
        assert!(entry.reason.contains("user confirmation is required"));
    }

    #[test]
    fn c01_confirmed_source_overrides_detection_and_is_recorded() {
        let mut input = c01_input(
            vec![c01_file("binary.bin", &[0x01, 0x02, 0x03, 0x04])],
            TargetEncoding::Utf8 { bom: false },
        );
        input.confirmed_source = Some(SourceEncoding::Gb18030);
        let plan = plan_c01(input).unwrap();
        let entry = &plan.entries[0];
        assert!(entry.blocked.is_none());
        assert!(entry.reason.contains("user-selected gb18030"));
        assert_eq!(
            plan.params["source"],
            PlanParamValue::String("gb18030".into())
        );
    }

    #[test]
    fn c01_skips_files_already_in_target_encoding() {
        let plan = plan_c01(c01_input(
            vec![c01_file("a.txt", CHINESE)],
            TargetEncoding::Utf8 { bom: false },
        ))
        .unwrap();
        let entry = &plan.entries[0];
        assert_eq!(entry.resolution, Some(PlanResolution::Skip));
        assert!(entry.reason.contains("already in target encoding"));
    }

    #[test]
    fn c01_treats_bom_policy_mismatch_as_conversion() {
        let with_bom = [b"\xEF\xBB\xBF".as_slice(), CHINESE].concat();
        let plan = plan_c01(c01_input(
            vec![c01_file("bom.txt", &with_bom)],
            TargetEncoding::Utf8 { bom: false },
        ))
        .unwrap();
        assert_eq!(plan.entries[0].resolution, None, "需去掉 BOM");

        let plan = plan_c01(c01_input(
            vec![c01_file("plain.txt", CHINESE)],
            TargetEncoding::Utf8 { bom: true },
        ))
        .unwrap();
        assert_eq!(plan.entries[0].resolution, None, "需补写 BOM");
    }

    #[test]
    fn c01_blocks_protected_paths() {
        let mut input = c01_input(
            vec![c01_file("/protected/a.txt", CHINESE)],
            TargetEncoding::Gb18030,
        );
        input.protected_dirs = vec![ProtectedDir::new("/protected")];
        let plan = plan_c01(input).unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E2009");
    }

    #[test]
    fn c01_rejects_empty_file_list_and_invalid_plan_ids() {
        let error = plan_c01(c01_input(Vec::new(), TargetEncoding::Gb18030)).unwrap_err();
        assert_eq!(error.code, "E1001");

        let error = plan_c01(PlanC01Input {
            plan_id: "not-a-ulid".to_string(),
            ..c01_input(vec![c01_file("a.txt", CHINESE)], TargetEncoding::Gb18030)
        })
        .unwrap_err();
        assert_eq!(error.code, "E1002");
    }
}
