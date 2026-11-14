//!
//! C10 批量解压计划与执行（方案 §5.11）。
//!
//! 本切片覆盖 ZIP 解压：Z01 的 GBK 名回退、Z02 的路径穿越拒绝、解压总量/比例
//! 限额与逐文件 Journal 撤销。压缩与 GBK 名写入留待后续切片。

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use crate::apply::{
    execute_reverse, ApplyEntryReport, ApplyOptions, ApplyOutcome, ApplyReport, ApplyState,
    ApplyStatus, OnConflict,
};
use crate::conflict::{validate_target_name, NameRules};
use crate::error::KernelError;
use crate::fs::{CaseSense, FsAdapter};
use crate::journal::{FsOp, JournalStore};
use crate::license::{Capability, LicenseService};
use crate::path::{is_under, protected_by, try_normalize, ProtectedDir};
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanDocument, PlanEntry, PlanExpected, PlanParamValue,
    PlanResolution, PlanStats, PlanWarning, RiskLevel,
};
use crate::progress::{CancelToken, ProgressEvent, ProgressPhase, ProgressSink};

pub const C10_KIND: &str = "c10";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveEntryKind {
    File,
    Dir,
}

impl ArchiveEntryKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Dir => "dir",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "file" => Some(Self::File),
            "dir" => Some(Self::Dir),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveCompression {
    Store,
    Deflate,
}

impl ArchiveCompression {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Store => "store",
            Self::Deflate => "deflate",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "store" | "stored" => Some(Self::Store),
            "deflate" | "deflated" => Some(Self::Deflate),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ArchiveEntryInfo {
    pub index: u64,
    /// GBK/UTF-8 解码后的相对名；无法解码时为 None。
    pub name: Option<String>,
    pub kind: ArchiveEntryKind,
    pub size: u64,
    pub compressed_size: u64,
    pub gbk_fallback: bool,
    pub encrypted: bool,
    pub supported: bool,
}

#[derive(Clone, Debug)]
pub struct ArchiveInspect {
    pub archive_size: u64,
    pub entries: Vec<ArchiveEntryInfo>,
}

/// 解压端口：平台实现 ZIP 读取，kernel 负责路径/限额/Journal。
/// 逐文件清单项（方案 §5.11「压缩完成后自动生成文件清单（可含哈希）」）。
///
/// 只记名称与**未压缩**体积；哈希是「可含」的可选项，本次不默认计算
/// （对每个源文件额外一遍全量读，代价与压缩本身同量级，须由调用方显式要求）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveManifestItem {
    /// ZIP 内的条目名（`/` 分隔，与 ZIP 规范一致）。
    pub name: String,
    /// 未压缩字节数。
    pub size: u64,
}

/// 压缩产物＋逐文件清单。
#[derive(Clone, Debug)]
pub struct CompressedArchive {
    pub bytes: Vec<u8>,
    /// 按写入顺序的逐文件清单（不含目录条目）。
    pub manifest: Vec<ArchiveManifestItem>,
}

/// 把逐文件清单格式化成 `ApplyEntryReport.detail` 的可读文本。
///
/// 超过 `limit` 项时截断并注明剩余条数——`detail` 是**展示用**字段，
/// 完整清单以 ZIP 本体为准（可用任意解压工具列出）。
/// `detail` 里逐文件清单的展示条数上限（超出则截断并注明剩余条数）。
/// 完整清单以 ZIP 本体为准——这里是展示字段，不是数据面。
const MANIFEST_DETAIL_LIMIT: usize = 200;

pub fn format_archive_manifest(items: &[ArchiveManifestItem], limit: usize) -> String {
    use std::fmt::Write as _;
    let total: u64 = items.iter().map(|item| item.size).sum();
    let mut out = format!("{} 个文件，共 {} 字节", items.len(), total);
    if items.is_empty() {
        return out;
    }
    for item in items.iter().take(limit) {
        let _ = write!(out, "\n{} {}", item.name, item.size);
    }
    if items.len() > limit {
        let _ = write!(out, "\n… 其余 {} 项略", items.len() - limit);
        return out;
    }
    out.trim_end().to_string()
}

pub trait ArchiveService: Send + Sync {
    /// 只读中央目录元数据：**不需要密码**，加密包也能列出条目（ADR-0074 §3.1）。
    fn inspect(&self, archive: &str) -> Result<ArchiveInspect, KernelError>;
    /// 解出一条条目。`password` 为 `None` 时按未加密处理；加密条目缺密码返回 `E3008`，
    /// 密码不对返回 `E3009`。
    fn extract_entry(
        &self,
        archive: &str,
        index: u64,
        password: Option<&str>,
        writer: &mut dyn Write,
    ) -> Result<(), KernelError>;
    /// 将输入的源文件/目录打包为 ZIP 字节（UTF-8 文件名），并返回逐文件清单。
    fn compress(
        &self,
        sources: &[String],
        compression: ArchiveCompression,
    ) -> Result<CompressedArchive, KernelError>;
}

#[derive(Clone, Debug)]
pub struct PlanC10Input {
    pub plan_id: String,
    pub archive: String,
    pub archive_size: u64,
    pub entries: Vec<ArchiveEntryInfo>,
    pub output_dir: String,
    pub max_total_bytes: u64,
    pub max_ratio: f64,
    pub existing_targets: Vec<String>,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    /// 目标名合法性规则（平台能力；设计 04 §2.1）。
    pub name_rules: NameRules,
    /// 执行期是否会提供密码（**不是密码本身**；ADR-0074）。
    ///
    /// 含加密条目且为 `false` 时整包 blocked `E3008`；为 `true` 时出计划并在
    /// `params.passwordProvided` 记下这个事实。密码只经 `ArchiveApplyContext.password`
    /// 进执行期，不落计划文件。
    pub password_provided: bool,
}

#[derive(Clone, Debug)]
struct SelectedEntry {
    index: u64,
    name: String,
    kind: ArchiveEntryKind,
    size: u64,
}

#[derive(Default)]
struct Selection {
    selected: Vec<SelectedEntry>,
    unsafe_names: Vec<String>,
    existing_names: Vec<String>,
    protected_names: Vec<String>,
    gbk_fallback: bool,
}

fn normalize_entry_name(raw: &str, rules: NameRules) -> Option<String> {
    let trimmed = raw.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return None;
    }
    let replaced = trimmed.replace('\\', "/");
    if replaced.starts_with('/') {
        return None;
    }
    let bytes = replaced.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return None;
    }
    let mut parts = Vec::new();
    for part in replaced.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains('\0') {
            return None;
        }
        if !validate_target_name(part, rules).is_empty() {
            return None;
        }
        parts.push(part);
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

fn join_target(root: &str, name: &str, sense: CaseSense) -> Option<String> {
    let root = root.trim_end_matches(['/', '\\']);
    let target = try_normalize(&format!("{root}/{name}")).ok()?;
    let normalized_root = try_normalize(root).ok()?;
    (target != normalized_root && is_under(&target, &normalized_root, sense)).then_some(target)
}

fn select_entries(
    entries: &[ArchiveEntryInfo],
    output_dir: &str,
    existing: &BTreeSet<String>,
    protected: &[ProtectedDir],
    sense: CaseSense,
    rules: NameRules,
) -> Selection {
    let mut selection = Selection::default();
    for entry in entries {
        // 加密条目照样参与选择：密码由执行期注入（ADR-0074）。此前这里把 encrypted
        // 一起跳过，等于"就算给了密码也不会解"——三层判断（inspect／blocked／selection）
        // 都假设"加密＝不支持"，一处漏改就整条链走不通。
        if !entry.supported {
            continue;
        }
        if entry.gbk_fallback {
            selection.gbk_fallback = true;
        }
        let Some(name) = entry
            .name
            .as_deref()
            .and_then(|name| normalize_entry_name(name, rules))
        else {
            selection.unsafe_names.push(
                entry
                    .name
                    .clone()
                    .unwrap_or_else(|| "<undecodable>".to_string()),
            );
            continue;
        };
        let Some(target) = join_target(output_dir, &name, sense) else {
            selection.unsafe_names.push(name);
            continue;
        };
        let key = crate::path::normalize(&target);
        if existing.contains(&key) {
            selection.existing_names.push(name);
            continue;
        }
        if protected_by(&target, None, protected, sense).is_some() {
            selection.protected_names.push(name);
            continue;
        }
        selection.selected.push(SelectedEntry {
            index: entry.index,
            name,
            kind: entry.kind,
            size: entry.size,
        });
    }
    selection
}

pub fn plan_c10(input: PlanC10Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.archive.trim().is_empty() {
        return Err(KernelError::missing_param("archive path is required"));
    }
    if input.output_dir.trim().is_empty() {
        return Err(KernelError::missing_param("output directory is required"));
    }
    if input.max_total_bytes == 0 {
        return Err(KernelError::invalid_params(
            "maxTotalBytes must be a positive byte count",
        ));
    }
    if !input.max_ratio.is_finite() || input.max_ratio <= 0.0 {
        return Err(KernelError::invalid_params(
            "maxRatio must be a positive finite number",
        ));
    }

    let existing: BTreeSet<String> = input
        .existing_targets
        .iter()
        .map(|path| crate::path::normalize(path))
        .collect();
    let selection = select_entries(
        &input.entries,
        &input.output_dir,
        &existing,
        &input.protected_dirs,
        input.case_sense,
        input.name_rules,
    );
    let total: u64 = selection.selected.iter().map(|entry| entry.size).sum();
    let ratio = if input.archive_size == 0 {
        if total > 0 {
            f64::INFINITY
        } else {
            0.0
        }
    } else {
        total as f64 / input.archive_size as f64
    };

    // 加密条目的处置（ADR-0074）：计划期只认"执行期会不会给密码"这个事实。
    // 给不出密码 → 整包 blocked E3008（"需要密码"与"格式不支持"是两种处置动作）；
    // 给得出 → 出计划，但不是已经能解：密码正确性只有执行期才知道（预览不试解）。
    let has_encrypted = input.entries.iter().any(|entry| entry.encrypted);
    let uses_password = has_encrypted && input.password_provided;
    let mut blocked = if has_encrypted && !input.password_provided {
        Some(PlanBlocked {
            code: "E3008".to_string(),
            reason: "archive is encrypted; a password is required".to_string(),
            hint: Some(
                "supply a password and preview again (the password is not stored in the plan)"
                    .to_string(),
            ),
        })
    } else {
        input
            .entries
            .iter()
            .find(|entry| !entry.supported)
            .map(|_| PlanBlocked {
                code: "E3003".to_string(),
                reason: "archive entry uses an unsupported compression method".to_string(),
                hint: Some("only stored/deflated ZIP entries are supported".to_string()),
            })
    };
    if blocked.is_none() && (total > input.max_total_bytes || ratio > input.max_ratio) {
        blocked = Some(PlanBlocked {
            code: "E3007".to_string(),
            reason: format!(
                "archive extraction limits exceeded (total {total} bytes, ratio {ratio:.2})"
            ),
            hint: Some(
                "raise --max-total-bytes/--max-ratio or choose a smaller archive".to_string(),
            ),
        });
    }
    let no_safe_entries = selection.selected.is_empty();
    let resolution = if blocked.is_none() && no_safe_entries {
        Some(PlanResolution::Skip)
    } else {
        None
    };

    let files = selection
        .selected
        .iter()
        .filter(|entry| entry.kind == ArchiveEntryKind::File)
        .count();
    let dirs = selection.selected.len().saturating_sub(files);
    let reason = match &blocked {
        Some(block) => block.reason.clone(),
        None if no_safe_entries => "no safe entries to extract".to_string(),
        None => format!(
            "extract {files} files, {dirs} dirs, total {total} bytes; skipped unsafe={} existing={} protected={}{}",
            selection.unsafe_names.len(),
            selection.existing_names.len(),
            selection.protected_names.len(),
            if uses_password {
                "; encrypted entries are decrypted with the password supplied at apply"
            } else {
                ""
            }
        ),
    };

    let mut warnings = Vec::new();
    if !selection.unsafe_names.is_empty() {
        warnings.push(PlanWarning {
            kind: "unsafe-entry-skipped".to_string(),
            suggestion: Some(format!(
                "unsafe or undecodable entries are skipped: {}",
                selection.unsafe_names.join(", ")
            )),
            risk_level: RiskLevel::Danger,
        });
    }
    if !selection.existing_names.is_empty() {
        warnings.push(PlanWarning {
            kind: "existing-target-skipped".to_string(),
            suggestion: Some(format!(
                "existing targets are skipped: {}",
                selection.existing_names.join(", ")
            )),
            risk_level: RiskLevel::Caution,
        });
    }
    if !selection.protected_names.is_empty() {
        warnings.push(PlanWarning {
            kind: "protected-target-skipped".to_string(),
            suggestion: Some(format!(
                "protected targets are skipped: {}",
                selection.protected_names.join(", ")
            )),
            risk_level: RiskLevel::Danger,
        });
    }
    if selection.gbk_fallback {
        warnings.push(PlanWarning {
            kind: "gbk-filename-fallback".to_string(),
            suggestion: Some("entries without the UTF-8 flag were decoded as GBK".to_string()),
            risk_level: RiskLevel::Caution,
        });
    }
    if uses_password {
        warnings.push(PlanWarning {
            kind: "encrypted-archive".to_string(),
            suggestion: Some(
                "encrypted entries are decrypted at execution; a wrong password fails the whole archive and preview cannot tell".to_string(),
            ),
            risk_level: RiskLevel::Caution,
        });
    }
    if input.password_provided && !has_encrypted {
        warnings.push(PlanWarning {
            kind: "password-unused".to_string(),
            suggestion: Some(
                "the archive is not encrypted; the supplied password will not be used".to_string(),
            ),
            risk_level: RiskLevel::Caution,
        });
    }
    if no_safe_entries && blocked.is_none() {
        warnings.push(PlanWarning {
            kind: "no-safe-entries".to_string(),
            suggestion: Some("archive contains no extractable entries".to_string()),
            risk_level: RiskLevel::Caution,
        });
    }

    let mut params = BTreeMap::new();
    params.insert(
        "maxTotalBytes".to_string(),
        PlanParamValue::Number(input.max_total_bytes),
    );
    params.insert(
        "maxRatio".to_string(),
        PlanParamValue::String(format!("{}", input.max_ratio)),
    );
    params.insert(
        "entryCount".to_string(),
        PlanParamValue::Number(input.entries.len() as u64),
    );
    params.insert(
        "safeCount".to_string(),
        PlanParamValue::Number(selection.selected.len() as u64),
    );
    // 只有"确实会用到密码"才记这个事实（ADR-0074 §3.2）：计划里没有密码本身。
    if uses_password {
        params.insert("passwordProvided".to_string(), PlanParamValue::Bool(true));
    }

    let entries = vec![PlanEntry {
        entry_id: format!("{}-0001", input.plan_id),
        source: input.archive.clone(),
        target: Some(input.output_dir.clone()),
        action: PlanAction::Extract,
        relation: None,
        reason,
        conflict: None,
        resolution,
        blocked,
        expected: Some(PlanExpected {
            size: input.archive_size,
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
        csv_columns: None,
        csv_sort: None,
    }];
    let stats = PlanStats {
        total: 1,
        conflict: 0,
        warning: warnings.len(),
        blocked: entries
            .iter()
            .filter(|entry| entry.blocked.is_some())
            .count(),
    };
    Ok(PlanDocument {
        plan_id: input.plan_id,
        kind: C10_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

#[derive(Clone, Debug)]
pub struct PlanC10CompressInput {
    pub plan_id: String,
    pub output: String,
    pub sources: Vec<String>,
    pub compression: ArchiveCompression,
    pub target_exists: bool,
    pub source_file_count: u64,
    pub source_bytes: u64,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
}

pub fn plan_c10_compress(input: PlanC10CompressInput) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.output.trim().is_empty() {
        return Err(KernelError::missing_param("output path is required"));
    }
    if input.sources.is_empty() {
        return Err(KernelError::missing_param(
            "c10 compression requires at least one source",
        ));
    }
    let mut blocked = None;
    for source in &input.sources {
        let same = crate::path::normalize(source) == crate::path::normalize(&input.output);
        if same || is_under(&input.output, source, input.case_sense) {
            blocked = Some(PlanBlocked {
                code: "E1002".to_string(),
                reason: "output must not be one of the sources or inside a source directory"
                    .to_string(),
                hint: Some("choose an output path outside the input tree".to_string()),
            });
            break;
        }
    }
    if blocked.is_none() {
        blocked =
            crate::csv_merge::target_block(&input.output, &input.protected_dirs, input.case_sense);
    }
    let resolution = if blocked.is_none() && input.target_exists {
        Some(PlanResolution::Skip)
    } else {
        None
    };
    let reason = match &blocked {
        Some(block) => block.reason.clone(),
        None if input.target_exists => "target exists; skipped by default".to_string(),
        None => format!(
            "compress {} files, {} bytes -> {} ZIP (UTF-8 names)",
            input.source_file_count,
            input.source_bytes,
            input.compression.as_str()
        ),
    };
    let mut warnings = Vec::new();
    if input.target_exists && blocked.is_none() {
        warnings.push(PlanWarning {
            kind: "target-exists-skipped".to_string(),
            suggestion: Some("existing output files are skipped by default".to_string()),
            risk_level: RiskLevel::Caution,
        });
    }
    let mut params = BTreeMap::new();
    // mode 必须随计划保存（ADR-0080）：套用通道按快照里的 `mode` 分流路径槽位
    // （解压 archive＋outputDir、压缩 sources＋output），压缩计划不写 mode 会被当成解压。
    // `sources` 仍是 JSON 串且会被保存通道按路径键剔除，这里只补模式标记。
    params.insert(
        "mode".to_string(),
        PlanParamValue::String("compress".to_string()),
    );
    params.insert(
        "sources".to_string(),
        PlanParamValue::String(serde_json::to_string(&input.sources).map_err(|error| {
            KernelError::internal("source list is not serializable", error.to_string())
        })?),
    );
    params.insert(
        "compression".to_string(),
        PlanParamValue::String(input.compression.as_str().to_string()),
    );
    params.insert(
        "sourceCount".to_string(),
        PlanParamValue::Number(input.source_file_count),
    );
    params.insert(
        "sourceBytes".to_string(),
        PlanParamValue::Number(input.source_bytes),
    );
    let entries = vec![PlanEntry {
        entry_id: format!("{}-0001", input.plan_id),
        source: input.sources[0].clone(),
        target: Some(input.output),
        action: PlanAction::Archive,
        relation: None,
        reason,
        conflict: None,
        resolution,
        blocked,
        expected: None,
        range: None,
        merge: None,
        csv: None,
        csv_merge: None,
        csv_extract: None,
        csv_rows: None,
        csv_dedup: None,
        csv_filter: None,
        csv_columns: None,
        csv_sort: None,
    }];
    let stats = PlanStats {
        total: 1,
        conflict: 0,
        warning: warnings.len(),
        blocked: entries
            .iter()
            .filter(|entry| entry.blocked.is_some())
            .count(),
    };
    Ok(PlanDocument {
        plan_id: input.plan_id,
        kind: C10_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

pub struct ArchiveApplyContext<'a> {
    pub fs: &'a dyn FsAdapter,
    pub license: &'a dyn LicenseService,
    pub journal: &'a mut dyn JournalStore,
    pub archive: &'a dyn ArchiveService,
    pub task_id: String,
    pub now_rfc3339: String,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    /// 目标名合法性规则（平台能力；设计 04 §2.1）。与计划期同一套，见 `ApplyContext`。
    pub name_rules: NameRules,
    /// 解密密码（**执行期秘密**，不来自计划文件；ADR-0074）。
    ///
    /// 计划里只记 `params.passwordProvided` 这个事实；密码由调用方在内存中持有、
    /// 执行时注入。为 `None` 而计划要求密码时，执行器整单拒绝（`E3008`），
    /// 不会"干到一半逐条报错"。空串按 `None` 处理。
    pub password: Option<&'a str>,
    pub progress: Option<&'a dyn ProgressSink>,
    pub cancel: Option<&'a dyn CancelToken>,
    /// D1：网络断线的退避等待与自动暂停（None＝不重试）。
    pub network_retry: Option<&'a dyn crate::retry::NetworkRetry>,
}

/// D1：归档读写的 E2005 重试包装。外层 `Err`＝宿主无暂停面、整单中止；
/// 内层 `Err`＝条目失败，继续下一条。
fn retry_archive_io<T>(
    context: &ArchiveApplyContext<'_>,
    network: bool,
    op: impl FnMut() -> Result<T, KernelError>,
) -> Result<Result<T, KernelError>, KernelError> {
    match crate::retry::with_network_retry(context.network_retry, network, op) {
        Ok(value) => Ok(Ok(value)),
        Err(failure) if failure.abort => Err(failure.error),
        Err(failure) => Ok(Err(failure.error)),
    }
}

pub fn apply_archive_plan(
    plan: &PlanDocument,
    options: &ApplyOptions,
    context: &mut ArchiveApplyContext<'_>,
) -> Result<ApplyOutcome, KernelError> {
    crate::apply::validate_apply_limits(plan, options)?;
    if plan.kind != C10_KIND {
        return Err(KernelError::invalid_params(
            "only c10 plans are supported by the archive executor",
        ));
    }
    if plan.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if plan
        .entries
        .iter()
        .any(|entry| !matches!(entry.action, PlanAction::Extract | PlanAction::Archive))
    {
        return Err(KernelError::invalid_params(
            "c10 plans only support extract/archive entries",
        ));
    }
    if options.on_conflict == OnConflict::Overwrite && !options.overwrite_confirmed {
        return Err(KernelError::invalid_params(
            "overwrite requires explicit confirmation (overwriteConfirmed)",
        ));
    }
    // 秘密前置门（ADR-0074 §3.3）：计划说"执行期会提供密码"却没给，就整单拒绝。
    // 这与 `case_sense`／`name_rules` 的"两期同源"是同一纪律的显式例外：秘密本身
    // 不同源（计划里没有），但"必须有密码"这个事实同源（计划里有）。缺席时不进
    // 条目循环——不消耗授权、不写 Journal、不留半截产物。
    let password = context.password.filter(|value| !value.is_empty());
    let password_required = matches!(
        plan.params.get("passwordProvided"),
        Some(PlanParamValue::Bool(true))
    );
    if password_required && password.is_none() {
        return Err(KernelError::new(
            "E3008",
            "this plan decrypts encrypted entries but no password was supplied",
            Some(
                "preview again and pass the password when applying; passwords are never stored in plans or task snapshots"
                    .to_string(),
            ),
            false,
        ));
    }
    let mut reports = Vec::with_capacity(plan.entries.len());
    let mut failed = 0usize;
    let mut ok = 0usize;
    let mut cancelled = false;
    let total_entries = plan.entries.len() as u64;
    // D1：只要计划里任一源/目标或压缩源是网络路径，整单的归档读/写都走同一套断线重试。
    // 判定经适配层能力（UNC 或 Windows 映射盘），不在执行器里自拼盘符。
    let network = plan
        .entries
        .iter()
        .any(|entry| crate::retry::entry_touches_network(context.fs, entry))
        || plan_param_sources(plan)
            .map(|sources| {
                sources
                    .iter()
                    .any(|source| context.fs.is_network_path(source))
            })
            .unwrap_or(false);
    report_progress(context, 0, total_entries);
    for (position, entry) in plan.entries.iter().enumerate() {
        report_progress(context, position as u64, total_entries);
        if is_cancelled(context) {
            cancelled = true;
            break;
        }
        if let Some(block) = &entry.blocked {
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Skipped,
                error: Some(KernelError::new(
                    block.code.clone(),
                    block.reason.clone(),
                    block.hint.clone(),
                    false,
                )),
                detail: None,
            });
            continue;
        }
        if entry.resolution == Some(PlanResolution::Skip) {
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Skipped,
                error: None,
                detail: None,
            });
            continue;
        }
        let source_stat =
            match retry_archive_io(context, network, || context.fs.stat(&entry.source)) {
                Ok(Ok(stat)) => stat,
                Ok(Err(error)) => {
                    failed += 1;
                    reports.push(ApplyEntryReport {
                        entry_id: entry.entry_id.clone(),
                        status: ApplyStatus::Failed,
                        error: Some(error),
                        detail: None,
                    });
                    continue;
                }
                Err(error) => return Err(error),
            };
        if entry
            .expected
            .as_ref()
            .is_some_and(|expected| expected.size != source_stat.size)
        {
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Skipped,
                error: Some(KernelError::new(
                    "E2010",
                    "archive changed since plan preview; re-plan before applying",
                    None,
                    true,
                )),
                detail: None,
            });
            continue;
        }
        match context.license.capability(&plan.kind) {
            Capability::Allow => {}
            Capability::Deny { reason } => {
                failed += 1;
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Failed,
                    error: Some(KernelError::new(
                        "E4001",
                        format!("operation not licensed: {reason}"),
                        None,
                        false,
                    )),
                    detail: None,
                });
                continue;
            }
            Capability::Quota { used, limit } => {
                failed += 1;
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Failed,
                    error: Some(KernelError::new(
                        "E4004",
                        format!("license quota exhausted ({used}/{limit})"),
                        None,
                        false,
                    )),
                    detail: None,
                });
                continue;
            }
        }
        if let Err(error) = context.license.consume(&plan.kind, 1) {
            failed += 1;
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Failed,
                error: Some(error),
                detail: None,
            });
            continue;
        }
        if entry.action == PlanAction::Archive {
            let Some(target) = entry.target.clone() else {
                context.license.release(&plan.kind, 1);
                failed += 1;
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Failed,
                    error: Some(KernelError::missing_param("archive target is required")),
                    detail: None,
                });
                continue;
            };
            if protected_by(&target, None, &context.protected_dirs, context.case_sense).is_some() {
                context.license.release(&plan.kind, 1);
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Skipped,
                    error: Some(KernelError::new(
                        "E2009",
                        "target is under a protected directory",
                        None,
                        false,
                    )),
                    detail: None,
                });
                continue;
            }
            let occupied = match retry_archive_io(context, network, || context.fs.stat(&target)) {
                Ok(Ok(_)) => true,
                Ok(Err(_)) => false,
                Err(error) => return Err(error),
            };
            if occupied {
                context.license.release(&plan.kind, 1);
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Skipped,
                    error: None,
                    detail: None,
                });
                continue;
            }
            let sources = match plan_param_sources(plan) {
                Ok(sources) => sources,
                Err(error) => {
                    context.license.release(&plan.kind, 1);
                    failed += 1;
                    reports.push(ApplyEntryReport {
                        entry_id: entry.entry_id.clone(),
                        status: ApplyStatus::Failed,
                        error: Some(error),
                        detail: None,
                    });
                    continue;
                }
            };
            let compression = match plan_param_compression(plan) {
                Ok(compression) => compression,
                Err(error) => {
                    context.license.release(&plan.kind, 1);
                    failed += 1;
                    reports.push(ApplyEntryReport {
                        entry_id: entry.entry_id.clone(),
                        status: ApplyStatus::Failed,
                        error: Some(error),
                        detail: None,
                    });
                    continue;
                }
            };
            let compressed = match retry_archive_io(context, network, || {
                context.archive.compress(&sources, compression)
            }) {
                Ok(Ok(compressed)) => compressed,
                Ok(Err(error)) => {
                    context.license.release(&plan.kind, 1);
                    failed += 1;
                    reports.push(ApplyEntryReport {
                        entry_id: entry.entry_id.clone(),
                        status: ApplyStatus::Failed,
                        error: Some(error),
                        detail: None,
                    });
                    continue;
                }
                Err(error) => return Err(error),
            };
            match retry_archive_io(context, network, || {
                context.fs.write_atomic(&target, &compressed.bytes)
            }) {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    context.license.release(&plan.kind, 1);
                    failed += 1;
                    reports.push(ApplyEntryReport {
                        entry_id: entry.entry_id.clone(),
                        status: ApplyStatus::Failed,
                        error: Some(error),
                        detail: None,
                    });
                    continue;
                }
                Err(error) => return Err(error),
            }
            let forward = FsOp::WriteAtomic {
                path: target.clone(),
                data_ref: None,
            };
            let reverse = match context
                .fs
                .quarantine_target_path(crate::conflict::file_name_of(&target))
            {
                Ok(path) => FsOp::Quarantine {
                    from: target.clone(),
                    to: path,
                },
                Err(error) => {
                    context.license.release(&plan.kind, 1);
                    failed += 1;
                    reports.push(ApplyEntryReport {
                        entry_id: entry.entry_id.clone(),
                        status: ApplyStatus::Failed,
                        error: Some(error),
                        detail: None,
                    });
                    continue;
                }
            };
            if let Err(error) = append_journal(context, plan, entry, forward, reverse.clone()) {
                let _ = execute_reverse(context.fs, &reverse);
                context.license.release(&plan.kind, 1);
                failed += 1;
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Failed,
                    error: Some(error),
                    detail: None,
                });
                continue;
            }
            ok += 1;
            // 方案 §5.11「压缩完成后自动生成文件清单」：逐文件清单进条目明细，
            // 报告因此能直接回答"这个包里有哪些文件、各多大"（ADR-0030 的已知缺口）。
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Ok,
                error: None,
                detail: Some(format_archive_manifest(
                    &compressed.manifest,
                    MANIFEST_DETAIL_LIMIT,
                )),
            });
            continue;
        }
        let inspect =
            match retry_archive_io(context, network, || context.archive.inspect(&entry.source)) {
                Ok(Ok(inspect)) => inspect,
                Ok(Err(error)) => {
                    context.license.release(&plan.kind, 1);
                    failed += 1;
                    reports.push(ApplyEntryReport {
                        entry_id: entry.entry_id.clone(),
                        status: ApplyStatus::Failed,
                        error: Some(error),
                        detail: None,
                    });
                    continue;
                }
                Err(error) => return Err(error),
            };
        // 执行期只复核"压缩方法"这一条；加密条目不在这里拒绝——它在下面按"有没有
        // 密码"分流（有密码就解，没密码报 E3008）。ADR-0074：这条与 `select_entries`
        // 是同一处假设的两个副本，必须同时改。
        if inspect.entries.iter().any(|item| !item.supported) {
            context.license.release(&plan.kind, 1);
            failed += 1;
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Failed,
                error: Some(KernelError::new(
                    "E3003",
                    "archive entry uses an unsupported compression method",
                    None,
                    false,
                )),
                detail: None,
            });
            continue;
        }
        let max_total = match plan.params.get("maxTotalBytes") {
            Some(PlanParamValue::Number(value)) => *value,
            _ => {
                context.license.release(&plan.kind, 1);
                failed += 1;
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Failed,
                    error: Some(KernelError::missing_param(
                        "plan param maxTotalBytes is required",
                    )),
                    detail: None,
                });
                continue;
            }
        };
        let max_ratio = match plan.params.get("maxRatio") {
            Some(PlanParamValue::String(value)) => value
                .parse::<f64>()
                .map_err(|_| KernelError::invalid_params("plan param maxRatio is not a number"))?,
            _ => {
                context.license.release(&plan.kind, 1);
                failed += 1;
                reports.push(ApplyEntryReport {
                    entry_id: entry.entry_id.clone(),
                    status: ApplyStatus::Failed,
                    error: Some(KernelError::missing_param(
                        "plan param maxRatio is required",
                    )),
                    detail: None,
                });
                continue;
            }
        };
        let output_dir = entry.target.clone().unwrap_or_default();
        let selection = select_entries(
            &inspect.entries,
            &output_dir,
            &BTreeSet::new(),
            &context.protected_dirs,
            context.case_sense,
            context.name_rules,
        );
        let total: u64 = selection.selected.iter().map(|item| item.size).sum();
        let ratio = if inspect.archive_size == 0 {
            if total > 0 {
                f64::INFINITY
            } else {
                0.0
            }
        } else {
            total as f64 / inspect.archive_size as f64
        };
        if total > max_total || ratio > max_ratio {
            context.license.release(&plan.kind, 1);
            failed += 1;
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Failed,
                error: Some(KernelError::new(
                    "E3007",
                    "archive extraction limits exceeded",
                    Some(format!("total {total} bytes, ratio {ratio:.2}")),
                    false,
                )),
                detail: None,
            });
            continue;
        }
        // 逐条目兜底（旧计划文件或手改计划）：选中条目里只要有一条加密而没给密码，
        // 整条 entry 报 E3008 失败——绝不"解一半"。
        let encrypted_selected = selection.selected.iter().any(|item| {
            usize::try_from(item.index)
                .ok()
                .and_then(|index| inspect.entries.get(index))
                .is_some_and(|found| found.encrypted)
        });
        if encrypted_selected && password.is_none() {
            context.license.release(&plan.kind, 1);
            failed += 1;
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Failed,
                error: Some(KernelError::new(
                    "E3008",
                    "archive is encrypted; a password is required",
                    Some("preview again and pass the password when applying".to_string()),
                    false,
                )),
                detail: None,
            });
            continue;
        }
        let mut first_error = None;
        let mut failures = 0usize;
        // 逐文件清单（方案 §5.11）：只记真正写出的文件，目录条目不算。
        let mut written: Vec<ArchiveManifestItem> = Vec::new();
        for item in &selection.selected {
            if is_cancelled(context) {
                cancelled = true;
                break;
            }
            let Some(target) = join_target(&output_dir, &item.name, context.case_sense) else {
                continue;
            };
            let occupied = match retry_archive_io(context, network, || context.fs.stat(&target)) {
                Ok(Ok(_)) => true,
                Ok(Err(_)) => false,
                Err(error) => return Err(error),
            };
            if occupied {
                continue;
            }
            let relative_parent = item.name.rsplit_once('/').map(|(parent, _)| parent);
            if let Some(relative_parent) = relative_parent {
                let mut current = output_dir.trim_end_matches(['/', '\\']).to_string();
                let mut created_ok = true;
                for part in relative_parent.split('/') {
                    current.push('/');
                    current.push_str(part);
                    let exists =
                        match retry_archive_io(context, network, || context.fs.stat(&current)) {
                            Ok(Ok(_)) => true,
                            Ok(Err(_)) => false,
                            Err(error) => return Err(error),
                        };
                    if !exists {
                        match retry_archive_io(context, network, || context.fs.create_dir(&current))
                        {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => {
                                first_error.get_or_insert(error);
                                failures += 1;
                                created_ok = false;
                                break;
                            }
                            Err(error) => return Err(error),
                        }
                        let forward = FsOp::CreateDir {
                            path: current.clone(),
                        };
                        let reverse = FsOp::RemoveDir {
                            path: current.clone(),
                        };
                        if let Err(error) = append_journal(context, plan, entry, forward, reverse) {
                            first_error.get_or_insert(error);
                            failures += 1;
                            created_ok = false;
                            break;
                        }
                    }
                }
                if !created_ok {
                    continue;
                }
            }
            if item.kind == ArchiveEntryKind::Dir {
                match retry_archive_io(context, network, || context.fs.create_dir(&target)) {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        first_error.get_or_insert(error);
                        failures += 1;
                        continue;
                    }
                    Err(error) => return Err(error),
                }
                let forward = FsOp::CreateDir {
                    path: target.clone(),
                };
                let reverse = FsOp::RemoveDir {
                    path: target.clone(),
                };
                if let Err(error) = append_journal(context, plan, entry, forward, reverse) {
                    first_error.get_or_insert(error);
                    failures += 1;
                }
                continue;
            }
            let write = retry_archive_io(context, network, || {
                context.fs.write_atomic_with(&target, &mut |writer| {
                    context
                        .archive
                        .extract_entry(&entry.source, item.index, password, writer)
                })
            });
            match write {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    first_error.get_or_insert(error);
                    failures += 1;
                    continue;
                }
                Err(error) => return Err(error),
            }
            written.push(ArchiveManifestItem {
                name: item.name.clone(),
                size: item.size,
            });
            let forward = FsOp::WriteAtomic {
                path: target.clone(),
                data_ref: None,
            };
            let reverse = match context
                .fs
                .quarantine_target_path(crate::conflict::file_name_of(&target))
            {
                Ok(path) => FsOp::Quarantine {
                    from: target.clone(),
                    to: path,
                },
                Err(error) => {
                    first_error.get_or_insert(error);
                    failures += 1;
                    continue;
                }
            };
            if let Err(error) =
                append_journal(context, plan, entry, forward.clone(), reverse.clone())
            {
                let _ = execute_reverse(context.fs, &reverse);
                first_error.get_or_insert(error);
                failures += 1;
            }
        }
        if failures > 0 {
            failed += 1;
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Failed,
                error: first_error,
                detail: None,
            });
        } else {
            ok += 1;
            // 方案 §5.11「压缩完成后自动生成文件清单」——解压侧同口径：
            // 报告逐文件列出写出了哪些文件、各多大（ADR-0029 的"不逐文件报告"缺口）。
            reports.push(ApplyEntryReport {
                entry_id: entry.entry_id.clone(),
                status: ApplyStatus::Ok,
                error: None,
                detail: Some(format_archive_manifest(&written, MANIFEST_DETAIL_LIMIT)),
            });
        }
    }
    // 取消处置（方案 §7.7）：归档专用执行器没有"执行中回滚"通道——取消一律保留
    // 已应用条目并留在 Journal 里（`undone_by=None` 如实报告），由调用方询问用户后
    // 走 undoPlan 撤销。壳层对所有 apply 任务统一 keepApplied，因此用户看到的行为
    // 与通用执行器一致；请求 rollbackApplied 的调用方从报告即可看出"未撤销"。
    let state = if cancelled {
        ApplyState::Cancelled
    } else if failed == 0 {
        ApplyState::Completed
    } else if ok > 0 {
        ApplyState::PartialFailed
    } else {
        ApplyState::Failed
    };
    Ok(ApplyOutcome {
        state,
        report: ApplyReport {
            plan_id: plan.plan_id.clone(),
            task_id: context.task_id.clone(),
            entries: reports,
            undone_by: None,
        },
    })
}

fn plan_param_sources(plan: &PlanDocument) -> Result<Vec<String>, KernelError> {
    let raw = match plan.params.get("sources") {
        Some(PlanParamValue::String(value)) => value,
        _ => return Err(KernelError::missing_param("plan param sources is required")),
    };
    serde_json::from_str(raw).map_err(|error| {
        KernelError::invalid_params(format!("plan param sources is not a string list: {error}"))
    })
}

fn plan_param_compression(plan: &PlanDocument) -> Result<ArchiveCompression, KernelError> {
    let raw = match plan.params.get("compression") {
        Some(PlanParamValue::String(value)) => value,
        _ => {
            return Err(KernelError::missing_param(
                "plan param compression is required",
            ))
        }
    };
    ArchiveCompression::parse(raw)
        .ok_or_else(|| KernelError::invalid_params("plan param compression is not supported"))
}

fn append_journal(
    context: &mut ArchiveApplyContext<'_>,
    plan: &PlanDocument,
    entry: &PlanEntry,
    forward: FsOp,
    reverse: FsOp,
) -> Result<(), KernelError> {
    context
        .journal
        .append_with_kind(
            &plan.plan_id,
            &entry.entry_id,
            &context.now_rfc3339,
            &plan.kind,
            forward,
            reverse,
        )
        .map(|_| ())
}

fn is_cancelled(context: &ArchiveApplyContext<'_>) -> bool {
    context
        .cancel
        .map(CancelToken::is_cancelled)
        .unwrap_or(false)
}

fn report_progress(context: &ArchiveApplyContext<'_>, done: u64, total: u64) {
    if let Some(progress) = context.progress {
        progress.report(ProgressEvent {
            task_id: context.task_id.clone(),
            phase: ProgressPhase::Executing,
            done,
            total,
            message: None,
            entry_done: None,
            entry_total: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_names_reject_traversal_and_absolute_paths() {
        assert_eq!(
            normalize_entry_name("safe/ok.txt", NameRules::windows()).as_deref(),
            Some("safe/ok.txt")
        );
        assert_eq!(
            normalize_entry_name("safe/../escape.txt", NameRules::windows()),
            None
        );
        assert_eq!(
            normalize_entry_name("../escape.txt", NameRules::windows()),
            None
        );
        assert_eq!(
            normalize_entry_name("/etc/passwd", NameRules::windows()),
            None
        );
        assert_eq!(
            normalize_entry_name(r"C:\windows\x", NameRules::windows()),
            None
        );
        assert_eq!(normalize_entry_name("CON.txt", NameRules::windows()), None);
        assert_eq!(
            normalize_entry_name("dir/", NameRules::windows()),
            Some("dir".to_string())
        );
    }

    #[test]
    fn target_join_stays_under_output_root() {
        assert_eq!(
            join_target("/out", "safe/a.txt", CaseSense::Sensitive).as_deref(),
            Some("/out/safe/a.txt")
        );
        assert!(join_target("/out", "safe/a.txt", CaseSense::Sensitive).is_some());
        assert!(join_target("/out", "../a.txt", CaseSense::Sensitive).is_none());
    }

    fn entry(index: u64, name: &str, kind: ArchiveEntryKind, size: u64) -> ArchiveEntryInfo {
        ArchiveEntryInfo {
            index,
            name: Some(name.to_string()),
            kind,
            size,
            compressed_size: size,
            gbk_fallback: false,
            encrypted: false,
            supported: true,
        }
    }

    fn input(entries: Vec<ArchiveEntryInfo>) -> PlanC10Input {
        PlanC10Input {
            plan_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            archive: "/data/a.zip".to_string(),
            archive_size: 100,
            entries,
            output_dir: "/out".to_string(),
            max_total_bytes: 10_000,
            max_ratio: 100.0,
            existing_targets: Vec::new(),
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            name_rules: crate::conflict::NameRules::windows(),
            password_provided: false,
        }
    }

    #[test]
    fn plan_skips_unsafe_entries_and_extracts_safe_ones() {
        let plan = plan_c10(input(vec![
            entry(0, "safe/ok.txt", ArchiveEntryKind::File, 10),
            entry(1, "../escape.txt", ArchiveEntryKind::File, 10),
            entry(2, "safe/../../escape2.txt", ArchiveEntryKind::File, 10),
        ]))
        .unwrap();
        assert_eq!(plan.kind, C10_KIND);
        assert_eq!(plan.entries[0].action, PlanAction::Extract);
        assert!(plan.entries[0].blocked.is_none());
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "unsafe-entry-skipped"));
        assert!(plan.entries[0].reason.contains("skipped unsafe=2"));
    }

    #[test]
    fn plan_blocks_when_limits_are_exceeded() {
        let mut input = input(vec![entry(0, "a.txt", ArchiveEntryKind::File, 1_000)]);
        input.max_total_bytes = 100;
        let plan = plan_c10(input).unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3007");
    }

    #[test]
    fn plan_blocks_encrypted_entries_until_a_password_is_promised() {
        // 不给密码 → 需要密码（E3008），而不是"格式不支持"（E3003）。
        let mut item = entry(0, "a.txt", ArchiveEntryKind::File, 1);
        item.encrypted = true;
        let blocked = plan_c10(input(vec![item.clone()])).unwrap();
        assert_eq!(blocked.entries[0].blocked.as_ref().unwrap().code, "E3008");

        // 承诺执行期给密码 → 出计划，但只记事实，密码本身不进计划。
        let mut ready = input(vec![item]);
        ready.password_provided = true;
        let plan = plan_c10(ready).unwrap();
        assert!(plan.entries[0].blocked.is_none());
        assert_eq!(
            plan.params.get("passwordProvided"),
            Some(&PlanParamValue::Bool(true))
        );
        let warning = plan
            .warnings
            .iter()
            .find(|warning| warning.kind == "encrypted-archive")
            .expect("加密包必须出 encrypted-archive 告警");
        assert_eq!(warning.risk_level, RiskLevel::Caution);
        // ADR-0074 §5.2：预览验不了密码，文案必须如实说——不能让用户以为已经验过。
        let suggestion = warning.suggestion.clone().unwrap_or_default();
        assert!(
            suggestion.contains("preview cannot tell"),
            "告警必须写明「预览判不出密码对错」，实际 {suggestion}"
        );
        assert!(plan.entries[0]
            .reason
            .contains("decrypted with the password"));
    }

    #[test]
    fn plan_warns_but_does_not_record_a_password_for_a_plain_archive() {
        // 包没加密却声明有密码：发警告，但计划不能把"要密码"记成事实。
        let mut ready = input(vec![entry(0, "a.txt", ArchiveEntryKind::File, 1)]);
        ready.password_provided = true;
        let plan = plan_c10(ready).unwrap();
        assert!(!plan.params.contains_key("passwordProvided"));
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "password-unused"));
    }

    #[test]
    fn existing_and_protected_targets_are_skipped() {
        let mut input = input(vec![
            entry(0, "a.txt", ArchiveEntryKind::File, 1),
            entry(1, "b.txt", ArchiveEntryKind::File, 1),
            entry(2, "c.txt", ArchiveEntryKind::File, 1),
        ]);
        input.existing_targets = vec!["/out/a.txt".to_string()];
        input.protected_dirs = vec![ProtectedDir::new("/out/b.txt")];
        let plan = plan_c10(input).unwrap();
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "existing-target-skipped"));
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "protected-target-skipped"));
        assert!(plan.entries[0]
            .reason
            .contains("skipped unsafe=0 existing=1 protected=1"));
    }

    /// 方案 §5.11 的逐文件清单格式化：空包、正常、超限截断三种形态。
    #[test]
    fn archive_manifest_formatting_handles_empty_and_overflow() {
        use super::{format_archive_manifest, ArchiveManifestItem};

        assert_eq!(format_archive_manifest(&[], 200), "0 个文件，共 0 字节");

        let items = vec![
            ArchiveManifestItem {
                name: "a.txt".to_string(),
                size: 6,
            },
            ArchiveManifestItem {
                name: "sub/b.txt".to_string(),
                size: 5,
            },
        ];
        let formatted = format_archive_manifest(&items, 200);
        assert!(formatted.contains("2 个文件，共 11 字节"), "{formatted}");
        assert!(formatted.contains("a.txt 6"), "{formatted}");
        assert!(formatted.contains("sub/b.txt 5"), "{formatted}");
        assert_eq!(formatted.lines().count(), 3, "表头＋两行明细：{formatted}");

        // 超限时截断并注明剩余条数——展示字段不假装是完整数据面。
        let many: Vec<ArchiveManifestItem> = (0..5)
            .map(|index| ArchiveManifestItem {
                name: format!("f{index}.txt"),
                size: 1,
            })
            .collect();
        let truncated = format_archive_manifest(&many, 2);
        assert!(truncated.contains("5 个文件，共 5 字节"), "{truncated}");
        assert!(truncated.contains("f0.txt 1"), "{truncated}");
        assert!(truncated.contains("f1.txt 1"), "{truncated}");
        assert!(truncated.contains("… 其余 3 项略"), "{truncated}");
        assert!(
            !truncated.contains("f2.txt"),
            "截断后不得再列第 3 项：{truncated}"
        );
    }

    fn compress_input(sources: Vec<&str>, output: &str) -> PlanC10CompressInput {
        PlanC10CompressInput {
            plan_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            output: output.to_string(),
            sources: sources.into_iter().map(str::to_string).collect(),
            compression: ArchiveCompression::Deflate,
            target_exists: false,
            source_file_count: 2,
            source_bytes: 20,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
        }
    }

    #[test]
    fn compression_plan_blocks_self_inclusion() {
        let plan = plan_c10_compress(compress_input(vec!["/data/a"], "/data/a.zip")).unwrap();
        assert!(plan.entries[0].blocked.is_none());
        let inside =
            plan_c10_compress(compress_input(vec!["/data/dir"], "/data/dir/out.zip")).unwrap();
        assert_eq!(inside.entries[0].blocked.as_ref().unwrap().code, "E1002");
        let same = plan_c10_compress(compress_input(vec!["/data/a"], "/data/a")).unwrap();
        assert_eq!(same.entries[0].blocked.as_ref().unwrap().code, "E1002");
    }

    #[test]
    fn compression_plan_skips_existing_target() {
        let mut input = compress_input(vec!["/data/a", "/data/b"], "/data/out.zip");
        input.target_exists = true;
        let plan = plan_c10_compress(input).unwrap();
        assert_eq!(plan.entries[0].resolution, Some(PlanResolution::Skip));
        match &plan.params["sources"] {
            PlanParamValue::String(sources) => assert!(sources.contains("/data/a")),
            other => panic!("unexpected sources param: {other:?}"),
        }
    }

    #[test]
    fn gbk_fallback_is_reported() {
        let mut item = entry(0, "a.txt", ArchiveEntryKind::File, 1);
        item.gbk_fallback = true;
        let plan = plan_c10(input(vec![item])).unwrap();
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "gbk-filename-fallback"));
    }
}
