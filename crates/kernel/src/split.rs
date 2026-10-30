//! F01 TXT 按文件大小分割（方案 §5.1；设计 01 §5/§8）。
//!
//! 纯函数层：分片边界只由"逐行字节长度＋目标字节数"决定，不读盘、不取时钟。
//! 壳层用 `platform::text::stream_segments` 驱动 [`SplitAccumulator`] 得到
//! [`SplitLayout`]，再交给 [`plan_f01`] 生成 1:N 的 `split` 计划条目。
//!
//! 切分口径（方案 §5.1）：
//! - 只在行边界断开（`\n` 完结的行）；
//! - 单行超过目标值 → 该行独占一片并显著标注（"至少 1 行/片"）；
//! - 全程没有换行 → 按字节硬切并告警（T06）；
//! - 空文件 → 不产生分片，仅告警。
//!
//! 本切片只做**源字节直通**（不转码）；分片编码跟随源文件，转码方案留待后续
//! 切片（见 ADR-0015）。

use std::collections::BTreeSet;
use std::path::Path;

use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::{normalize, protected_by, try_normalize, ProtectedDir};
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanConflict, PlanDocument, PlanEntry, PlanExpected,
    PlanParamValue, PlanRange, PlanRelation, PlanResolution, PlanRole, PlanStats, PlanWarning,
    RiskLevel,
};

pub const F01_KIND: &str = "f01";
pub const F02_KIND: &str = "f02";

/// 分片计量基准：F01 按字节、F02 按行数（两者共用同一累加器与计划结构）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitBasis {
    Bytes(u64),
    Lines(u64),
}

/// 一个分片在源文件中的字节区间（左闭右开）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShardSpan {
    /// 1 起的分片序号。
    pub index: u32,
    pub start: u64,
    pub end: u64,
    /// 该分片包含的完整行数；无换行硬切时为 0。
    pub lines: u64,
    /// 单行超过目标值，本片只放这一行。
    pub overlong_line: bool,
    /// 文件无换行，按字节硬切。
    pub hard_cut: bool,
}

impl ShardSpan {
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// 分片布局（由壳层扫描后传入，kernel 只消费）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SplitLayout {
    pub file_size: u64,
    pub spans: Vec<ShardSpan>,
    pub max_line_bytes: u64,
    pub had_newline: bool,
}

/// 逐行累积分片边界（纯计算，内存 O(分片数)）。
#[derive(Clone, Debug)]
pub struct SplitAccumulator {
    basis: SplitBasis,
    offset: u64,
    current_start: u64,
    current_bytes: u64,
    current_lines: u64,
    spans: Vec<ShardSpan>,
    max_line_bytes: u64,
    had_newline: bool,
}

impl SplitAccumulator {
    pub fn new(target_bytes: u64) -> Result<Self, KernelError> {
        Self::with_basis(SplitBasis::Bytes(target_bytes))
    }

    /// F02：每满 `target_lines` 行收一片（尾片保留）。
    pub fn new_lines(target_lines: u64) -> Result<Self, KernelError> {
        Self::with_basis(SplitBasis::Lines(target_lines))
    }

    fn with_basis(basis: SplitBasis) -> Result<Self, KernelError> {
        match basis {
            SplitBasis::Bytes(0) => {
                return Err(KernelError::invalid_params(
                    "targetBytes must be greater than zero",
                ))
            }
            SplitBasis::Lines(0) => {
                return Err(KernelError::invalid_params(
                    "targetLines must be greater than zero",
                ))
            }
            _ => {}
        }
        Ok(Self {
            basis,
            offset: 0,
            current_start: 0,
            current_bytes: 0,
            current_lines: 0,
            spans: Vec::new(),
            max_line_bytes: 0,
            had_newline: false,
        })
    }

    /// 推入一行（`line_bytes` 含行尾换行符；`ends_with_newline` 表示该行以 `\n` 结束）。
    pub fn push_line(&mut self, line_bytes: u64, ends_with_newline: bool) {
        self.max_line_bytes = self.max_line_bytes.max(line_bytes);
        self.had_newline |= ends_with_newline;

        if let SplitBasis::Bytes(target) = self.basis {
            if line_bytes > target {
                // 超长行独占一片（"至少 1 行/片"）。
                self.close_current();
                self.push_span(line_bytes, 1, true, false);
                return;
            }
        }
        self.current_bytes += line_bytes;
        self.current_lines += 1;
        let full = match self.basis {
            SplitBasis::Bytes(target) => self.current_bytes >= target,
            SplitBasis::Lines(target) => self.current_lines >= target,
        };
        if full {
            self.close_current();
        }
    }

    /// 扫描结束：按字节计量且无换行时硬切；否则收尾当前片（尾片保留）。
    pub fn finish(mut self, file_size: u64) -> SplitLayout {
        if matches!(self.basis, SplitBasis::Bytes(_)) && !self.had_newline {
            let SplitBasis::Bytes(target) = self.basis else {
                unreachable!("checked bytes basis above");
            };
            self.spans.clear();
            if file_size > 0 {
                let mut start = 0u64;
                while start < file_size {
                    let end = (start + target).min(file_size);
                    self.spans.push(ShardSpan {
                        index: self.spans.len() as u32 + 1,
                        start,
                        end,
                        lines: 0,
                        overlong_line: false,
                        hard_cut: true,
                    });
                    start = end;
                }
            }
        } else {
            self.close_current();
        }
        SplitLayout {
            file_size,
            spans: self.spans,
            max_line_bytes: self.max_line_bytes,
            had_newline: self.had_newline,
        }
    }

    fn close_current(&mut self) {
        if self.current_lines == 0 {
            return;
        }
        let bytes = self.current_bytes;
        let lines = self.current_lines;
        self.push_span(bytes, lines, false, false);
    }

    fn push_span(&mut self, bytes: u64, lines: u64, overlong_line: bool, hard_cut: bool) {
        debug_assert!(bytes > 0);
        let start = self.current_start.max(self.offset);
        let end = start + bytes;
        self.spans.push(ShardSpan {
            index: self.spans.len() as u32 + 1,
            start,
            end,
            lines,
            overlong_line,
            hard_cut,
        });
        self.offset = end;
        self.current_start = end;
        self.current_bytes = 0;
        self.current_lines = 0;
    }
}

/// 分片产物命名："原名＋序号"（方案 §5.1）。序号按目标分片总数补零。
pub fn shard_name(file_name: &str, index: u32, total: u32) -> String {
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let extension = path.extension().and_then(|value| value.to_str());
    let width = total.max(1).to_string().len();
    match extension {
        Some(extension) => format!("{stem}.{index:0width$}.{extension}"),
        None => format!("{stem}.{index:0width$}"),
    }
}

/// 单个待分割文件：布局由壳层扫描得到。
#[derive(Clone, Debug)]
pub struct PlanF01File {
    pub path: String,
    pub canonical_path: Option<String>,
    pub group_id: String,
    pub layout: SplitLayout,
}

#[derive(Clone, Debug)]
pub struct PlanF01Input {
    pub plan_id: String,
    /// 目标分片字节数（用户输入，不设固定下限；方案 11.6-11 已确认）。
    pub target_bytes: u64,
    pub output_dir: String,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    /// 壳层预检到的已占用目标路径（plan 为纯函数，不 stat）。
    pub existing_targets: Vec<String>,
    pub files: Vec<PlanF01File>,
}

/// F02 输入（按行数分片，复用同一分片文件结构）。
#[derive(Clone, Debug)]
pub struct PlanF02Input {
    pub plan_id: String,
    /// 每片行数（用户输入，> 0；方案 §5.15）。
    pub lines_per_part: u64,
    pub output_dir: String,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub existing_targets: Vec<String>,
    pub files: Vec<PlanF01File>,
}

/// 生成 F01 计划：每个分片一条 `split` 条目，按 `groupId` 归为 1:N。
pub fn plan_f01(input: PlanF01Input) -> Result<PlanDocument, KernelError> {
    validate_plan_header(&input.plan_id)?;
    if input.target_bytes == 0 {
        return Err(KernelError::invalid_params(
            "targetBytes must be greater than zero",
        ));
    }
    validate_output(&input.output_dir, &input.files)?;
    let mut params = std::collections::BTreeMap::new();
    params.insert(
        "targetBytes".to_string(),
        PlanParamValue::Number(input.target_bytes),
    );
    params.insert(
        "outputDir".to_string(),
        PlanParamValue::String(input.output_dir.clone()),
    );
    params.insert(
        "encoding".to_string(),
        PlanParamValue::String("source-bytes".to_string()),
    );
    build_split_document(&SplitSpec::from_f01(&input), F01_KIND, params)
}

/// 生成 F02 计划：每满 `linesPerPart` 行一片，尾片保留（方案 §5.15）。
pub fn plan_f02(input: PlanF02Input) -> Result<PlanDocument, KernelError> {
    validate_plan_header(&input.plan_id)?;
    if input.lines_per_part == 0 {
        return Err(KernelError::invalid_params(
            "linesPerPart must be greater than zero",
        ));
    }
    validate_output(&input.output_dir, &input.files)?;
    let mut params = std::collections::BTreeMap::new();
    params.insert(
        "linesPerPart".to_string(),
        PlanParamValue::Number(input.lines_per_part),
    );
    params.insert(
        "outputDir".to_string(),
        PlanParamValue::String(input.output_dir.clone()),
    );
    params.insert(
        "encoding".to_string(),
        PlanParamValue::String("source-bytes".to_string()),
    );
    build_split_document(&SplitSpec::from_f02(&input), F02_KIND, params)
}

/// F01/F02 共用的计划字段（两者只有计量参数不同）。
struct SplitSpec<'a> {
    plan_id: &'a str,
    output_dir: &'a str,
    protected_dirs: &'a [ProtectedDir],
    case_sense: CaseSense,
    existing_targets: &'a [String],
    files: &'a [PlanF01File],
}

impl<'a> SplitSpec<'a> {
    fn from_f01(input: &'a PlanF01Input) -> Self {
        Self {
            plan_id: &input.plan_id,
            output_dir: &input.output_dir,
            protected_dirs: &input.protected_dirs,
            case_sense: input.case_sense,
            existing_targets: &input.existing_targets,
            files: &input.files,
        }
    }

    fn from_f02(input: &'a PlanF02Input) -> Self {
        Self {
            plan_id: &input.plan_id,
            output_dir: &input.output_dir,
            protected_dirs: &input.protected_dirs,
            case_sense: input.case_sense,
            existing_targets: &input.existing_targets,
            files: &input.files,
        }
    }
}

fn validate_plan_header(plan_id: &str) -> Result<(), KernelError> {
    if plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    Ok(())
}

fn validate_output(output_dir: &str, files: &[PlanF01File]) -> Result<(), KernelError> {
    if output_dir.trim().is_empty() {
        return Err(KernelError::missing_param("outputDir is required"));
    }
    if files.is_empty() {
        return Err(KernelError::missing_param(
            "split plans require at least one file",
        ));
    }
    Ok(())
}

fn build_split_document(
    spec: &SplitSpec<'_>,
    kind: &str,
    params: std::collections::BTreeMap<String, PlanParamValue>,
) -> Result<PlanDocument, KernelError> {
    let existing: BTreeSet<String> = spec
        .existing_targets
        .iter()
        .map(|path| conflict_key(path, spec.case_sense))
        .collect();
    let mut batch: BTreeSet<String> = BTreeSet::new();

    let mut entries = Vec::new();
    let mut warnings = Vec::new();

    for file in spec.files {
        if file.group_id.trim().is_empty() {
            return Err(KernelError::missing_param("groupId is required"));
        }
        if !is_ulid(&file.group_id) {
            return Err(KernelError::invalid_params(
                "groupId must be a 26-character ULID",
            ));
        }
        validate_layout(&file.layout)?;

        if file.layout.spans.is_empty() {
            warnings.push(PlanWarning {
                kind: "empty-file".to_string(),
                suggestion: Some(format!("{} is empty; no parts produced", file.path)),
                risk_level: RiskLevel::Safe,
            });
            continue;
        }
        if file.layout.spans.iter().any(|span| span.overlong_line) {
            warnings.push(PlanWarning {
                kind: "overlong-line-single-part".to_string(),
                suggestion: Some(format!(
                    "{} has a line longer than targetBytes; it occupies its own part",
                    file.path
                )),
                risk_level: RiskLevel::Caution,
            });
        }
        if file.layout.spans.iter().any(|span| span.hard_cut) {
            warnings.push(PlanWarning {
                kind: "no-newline-hard-cut".to_string(),
                suggestion: Some(format!(
                    "{} has no newline; parts are hard-cut on byte boundaries",
                    file.path
                )),
                risk_level: RiskLevel::Danger,
            });
        }

        let total = file.layout.spans.len() as u32;
        let file_name = crate::conflict::file_name_of(&file.path);
        let source_block = source_block(
            &file.path,
            file.canonical_path.as_deref(),
            spec.protected_dirs,
            spec.case_sense,
        );

        for span in &file.layout.spans {
            let target = join_dir(spec.output_dir, &shard_name(file_name, span.index, total));
            let target_block = target_block(&target, spec.protected_dirs, spec.case_sense);
            let key = conflict_key(&target, spec.case_sense);
            let duplicate_in_batch = !batch.insert(key.clone());
            let occupied = existing.contains(&key);
            let conflict = if duplicate_in_batch {
                Some(PlanConflict {
                    kind: "batch-duplicate".to_string(),
                    risk_level: RiskLevel::Danger,
                })
            } else if occupied {
                Some(PlanConflict {
                    kind: "existing-target".to_string(),
                    risk_level: RiskLevel::Danger,
                })
            } else {
                None
            };

            let mut reason = format!(
                "part {}/{} of {}: source bytes [{}..{}), {} bytes, {} lines",
                span.index,
                total,
                file_name,
                span.start,
                span.end,
                span.len(),
                span.lines
            );
            if span.overlong_line {
                reason.push_str("; overlong line (own part)");
            }
            if span.hard_cut {
                reason.push_str("; hard byte cut (no newline)");
            }

            entries.push(PlanEntry {
                entry_id: format!("{}-{:04}", spec.plan_id, entries.len() + 1),
                source: file.path.clone(),
                target: Some(target),
                action: PlanAction::Split,
                relation: Some(PlanRelation {
                    group_id: file.group_id.clone(),
                    role: PlanRole::Output,
                    part_index: Some(span.index),
                    part_total: Some(total),
                }),
                reason,
                conflict: conflict.clone(),
                resolution: conflict.as_ref().map(|_| PlanResolution::Sequence),
                blocked: source_block.clone().or(target_block),
                expected: Some(PlanExpected {
                    size: span.len(),
                    algorithm: "byte-range".to_string(),
                    hash: None,
                }),
                range: Some(PlanRange {
                    start: span.start,
                    end: span.end,
                }),
            });
        }
    }

    // 全部输入为空时仍返回计划：0 条条目＋empty-file 警告（T05 单独分组/跳过并报告），
    // apply 无事可做，不得静默当成功产物。
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
        plan_id: spec.plan_id.to_string(),
        kind: kind.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

fn validate_layout(layout: &SplitLayout) -> Result<(), KernelError> {
    let mut cursor = 0u64;
    for span in &layout.spans {
        if span.start != cursor || span.end <= span.start {
            return Err(KernelError::internal(
                "split layout is not contiguous",
                format!("span {} starts at {}", span.index, span.start),
            ));
        }
        cursor = span.end;
    }
    if cursor != layout.file_size {
        return Err(KernelError::internal(
            "split layout does not cover the whole file",
            format!("covered {cursor} of {}", layout.file_size),
        ));
    }
    Ok(())
}

fn source_block(
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

fn target_block(path: &str, protected: &[ProtectedDir], sense: CaseSense) -> Option<PlanBlocked> {
    match try_normalize(path) {
        Ok(normalized) => {
            protected_by(&normalized, None, protected, sense).map(|hit| PlanBlocked {
                code: "E2009".to_string(),
                reason: format!("target is under protected directory {hit}"),
                hint: Some("choose another output directory".to_string()),
            })
        }
        Err(error) => Some(PlanBlocked {
            code: error.code,
            reason: error.message,
            hint: None,
        }),
    }
}

fn join_dir(dir: &str, name: &str) -> String {
    let dir = dir.trim_end_matches(['/', '\\']);
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

fn conflict_key(path: &str, sense: CaseSense) -> String {
    let normalized = normalize(path);
    match sense {
        CaseSense::Sensitive => normalized,
        CaseSense::CaseInsensitive => normalized.to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const GROUP_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";

    fn layout_from_lines(lines: &[(u64, bool)], target: u64, file_size: u64) -> SplitLayout {
        let mut accumulator = SplitAccumulator::new(target).unwrap();
        for (bytes, newline) in lines {
            accumulator.push_line(*bytes, *newline);
        }
        accumulator.finish(file_size)
    }

    fn file(path: &str, layout: SplitLayout) -> PlanF01File {
        PlanF01File {
            path: path.to_string(),
            canonical_path: None,
            group_id: GROUP_ID.to_string(),
            layout,
        }
    }

    fn input(files: Vec<PlanF01File>) -> PlanF01Input {
        PlanF01Input {
            plan_id: PLAN_ID.to_string(),
            target_bytes: 64,
            output_dir: "out".to_string(),
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            existing_targets: Vec::new(),
            files,
        }
    }

    fn layout_from_line_count(
        lines: &[(u64, bool)],
        lines_per_part: u64,
        file_size: u64,
    ) -> SplitLayout {
        let mut accumulator = SplitAccumulator::new_lines(lines_per_part).unwrap();
        for (bytes, newline) in lines {
            accumulator.push_line(*bytes, *newline);
        }
        accumulator.finish(file_size)
    }

    fn input_f02(files: Vec<PlanF01File>, lines_per_part: u64) -> PlanF02Input {
        PlanF02Input {
            plan_id: PLAN_ID.to_string(),
            lines_per_part,
            output_dir: "out".to_string(),
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            existing_targets: Vec::new(),
            files,
        }
    }

    #[test]
    fn closes_parts_on_line_boundaries() {
        let layout = layout_from_lines(&[(40, true), (40, true), (40, true), (40, true)], 64, 160);
        let spans = &layout.spans;
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].start, spans[0].end), (0, 80));
        assert_eq!((spans[1].start, spans[1].end), (80, 160));
        assert!(spans.iter().all(|span| !span.overlong_line));
    }

    #[test]
    fn overlong_line_occupies_its_own_part() {
        let layout = layout_from_lines(&[(10, true), (80, true), (10, true)], 64, 100);
        assert_eq!(layout.spans.len(), 3);
        assert!(layout.spans[1].overlong_line);
        assert_eq!((layout.spans[1].start, layout.spans[1].end), (10, 90));
        assert_eq!(layout.spans[1].lines, 1);
    }

    #[test]
    fn hard_cuts_when_no_newline() {
        let layout = layout_from_lines(&[(150, false)], 64, 150);
        assert!(!layout.had_newline);
        assert_eq!(layout.spans.len(), 3);
        assert!(layout.spans.iter().all(|span| span.hard_cut));
        assert_eq!(
            layout.spans.iter().map(ShardSpan::len).collect::<Vec<_>>(),
            vec![64, 64, 22]
        );
    }

    #[test]
    fn empty_file_has_no_spans() {
        let layout = layout_from_lines(&[], 64, 0);
        assert!(layout.spans.is_empty());
    }

    #[test]
    fn names_parts_with_padded_sequence() {
        assert_eq!(shard_name("report.txt", 1, 3), "report.1.txt");
        assert_eq!(shard_name("report.txt", 2, 12), "report.02.txt");
        assert_eq!(shard_name("archive.tar", 1, 1), "archive.1.tar");
        assert_eq!(shard_name("plain", 1, 1), "plain.1");
    }

    #[test]
    fn plan_builds_one_entry_per_part_with_relations() {
        let layout = layout_from_lines(&[(40, true), (40, true), (40, true)], 64, 120);
        let plan = plan_f01(input(vec![file("big.txt", layout)])).unwrap();
        assert_eq!(plan.kind, F01_KIND);
        assert_eq!(plan.entries.len(), 2);
        for (index, entry) in plan.entries.iter().enumerate() {
            assert_eq!(entry.action, PlanAction::Split);
            let relation = entry.relation.as_ref().unwrap();
            assert_eq!(relation.group_id, GROUP_ID);
            assert_eq!(relation.role, PlanRole::Output);
            assert_eq!(relation.part_index, Some(index as u32 + 1));
            assert_eq!(relation.part_total, Some(2));
            assert!(entry.range.is_some());
        }
        assert_eq!(plan.entries[0].target.as_deref(), Some("out/big.1.txt"));
        assert_eq!(plan.entries[1].target.as_deref(), Some("out/big.2.txt"));
    }

    #[test]
    fn empty_input_returns_plan_with_warning_and_no_entries() {
        let plan = plan_f01(input(vec![file("empty.txt", SplitLayout::default())])).unwrap();
        assert!(plan.entries.is_empty());
        assert_eq!(plan.stats.total, 0);
        assert_eq!(plan.warnings.len(), 1);
        assert_eq!(plan.warnings[0].kind, "empty-file");
    }

    #[test]
    fn plan_warns_on_overlong_and_no_newline() {
        let overlong = layout_from_lines(&[(10, true), (80, true)], 64, 90);
        let hard = layout_from_lines(&[(150, false)], 64, 150);
        let plan = plan_f01(input(vec![file("a.txt", overlong), file("b.txt", hard)])).unwrap();
        let kinds: Vec<&str> = plan.warnings.iter().map(|w| w.kind.as_str()).collect();
        assert!(kinds.contains(&"overlong-line-single-part"));
        assert!(kinds.contains(&"no-newline-hard-cut"));
    }

    #[test]
    fn plan_marks_existing_and_batch_duplicate_targets() {
        let layout = layout_from_lines(&[(10, true)], 64, 10);
        let mut first = input(vec![
            file("a.txt", layout.clone()),
            file("a.txt", layout.clone()),
        ]);
        first.existing_targets = vec!["out/a.1.txt".to_string()];
        let plan = plan_f01(first).unwrap();
        assert_eq!(plan.entries.len(), 2);
        assert!(plan.entries.iter().all(|entry| entry.conflict.is_some()));
        assert!(plan
            .entries
            .iter()
            .all(|entry| entry.resolution == Some(PlanResolution::Sequence)));
    }

    #[test]
    fn plan_blocks_protected_source() {
        let layout = layout_from_lines(&[(10, true)], 64, 10);
        let mut protected = input(vec![file("etc/passwd.txt", layout)]);
        protected.protected_dirs = vec![ProtectedDir::new("etc")];
        let plan = plan_f01(protected).unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E2009");
    }

    #[test]
    fn plan_rejects_zero_target_and_non_ulid() {
        let layout = layout_from_lines(&[(10, true)], 64, 10);
        let mut zero = input(vec![file("a.txt", layout.clone())]);
        zero.target_bytes = 0;
        assert!(plan_f01(zero).is_err());

        let mut bad_id = input(vec![file("a.txt", layout)]);
        bad_id.plan_id = "not-a-ulid".to_string();
        assert!(plan_f01(bad_id).is_err());
    }

    #[test]
    fn plan_rejects_layout_that_skips_bytes() {
        let layout = SplitLayout {
            file_size: 10,
            spans: vec![ShardSpan {
                index: 1,
                start: 2,
                end: 10,
                lines: 1,
                overlong_line: false,
                hard_cut: false,
            }],
            max_line_bytes: 10,
            had_newline: true,
        };
        assert!(plan_f01(input(vec![file("a.txt", layout)])).is_err());
    }

    #[test]
    fn f02_closes_every_n_lines_and_keeps_tail_part() {
        let lines = [(10, true); 5];
        let layout = layout_from_line_count(&lines, 2, 50);
        assert_eq!(layout.spans.len(), 3);
        assert_eq!(
            layout.spans.iter().map(ShardSpan::len).collect::<Vec<_>>(),
            vec![20, 20, 10]
        );
        assert_eq!(
            layout
                .spans
                .iter()
                .map(|span| span.lines)
                .collect::<Vec<_>>(),
            vec![2, 2, 1]
        );
        assert!(layout.spans.iter().all(|span| !span.hard_cut));
    }

    #[test]
    fn f02_single_part_when_n_exceeds_total_lines() {
        let lines = [(10, true), (10, true), (10, true)];
        let layout = layout_from_line_count(&lines, 10, 30);
        assert_eq!(layout.spans.len(), 1);
        assert_eq!(layout.spans[0].lines, 3);
        assert_eq!((layout.spans[0].start, layout.spans[0].end), (0, 30));
    }

    #[test]
    fn f02_n_equals_one_puts_each_line_in_its_own_part() {
        let lines = [(10, true), (12, true), (14, true), (16, true)];
        let layout = layout_from_line_count(&lines, 1, 52);
        assert_eq!(layout.spans.len(), 4);
        assert!(layout.spans.iter().all(|span| span.lines == 1));
        assert_eq!(
            layout.spans.iter().map(ShardSpan::len).collect::<Vec<_>>(),
            vec![10, 12, 14, 16]
        );
    }

    #[test]
    fn f02_no_newline_file_stays_single_part_without_hard_cut() {
        let layout = layout_from_line_count(&[(150, false)], 2, 150);
        assert!(!layout.had_newline);
        assert_eq!(layout.spans.len(), 1);
        assert!(!layout.spans[0].hard_cut);
        assert_eq!((layout.spans[0].start, layout.spans[0].end), (0, 150));
    }

    #[test]
    fn f02_empty_file_has_no_spans() {
        let layout = layout_from_line_count(&[], 2, 0);
        assert!(layout.spans.is_empty());
    }

    #[test]
    fn f02_plan_uses_lines_params_and_split_entries() {
        let lines = [(10, true), (10, true), (10, true), (10, true), (10, true)];
        let layout = layout_from_line_count(&lines, 2, 50);
        let plan = plan_f02(input_f02(vec![file("big.txt", layout)], 2)).unwrap();
        assert_eq!(plan.kind, F02_KIND);
        assert_eq!(
            plan.params.get("linesPerPart"),
            Some(&PlanParamValue::Number(2))
        );
        assert_eq!(
            plan.params.get("encoding"),
            Some(&PlanParamValue::String("source-bytes".to_string()))
        );
        assert_eq!(plan.entries.len(), 3);
        for (index, entry) in plan.entries.iter().enumerate() {
            assert_eq!(entry.action, PlanAction::Split);
            let relation = entry.relation.as_ref().unwrap();
            assert_eq!(relation.group_id, GROUP_ID);
            assert_eq!(relation.part_index, Some(index as u32 + 1));
            assert_eq!(relation.part_total, Some(3));
            assert!(entry.range.is_some());
        }
        assert_eq!(plan.entries[2].target.as_deref(), Some("out/big.3.txt"));
    }

    #[test]
    fn f02_plan_does_not_warn_hard_cut_for_no_newline_file() {
        let layout = layout_from_line_count(&[(150, false)], 2, 150);
        let plan = plan_f02(input_f02(vec![file("blob.txt", layout)], 2)).unwrap();
        assert_eq!(plan.entries.len(), 1);
        assert!(plan
            .warnings
            .iter()
            .all(|warning| warning.kind != "no-newline-hard-cut"));
    }

    #[test]
    fn f02_plan_rejects_zero_lines_per_part() {
        let layout = layout_from_line_count(&[(10, true)], 2, 10);
        let plan = plan_f02(input_f02(vec![file("a.txt", layout)], 0));
        assert!(plan.is_err());
    }
}
