//! F05 CSV 分割（方案 §5.3：表头策略 + 记录边界）。
//!
//! 纯函数层：记录边界、表头识别、列数漂移计数与分片布局全部由"字节流 + 分隔符"
//! 决定，本模块不读盘、不取时钟、不做编码嗅探。壳层负责按块读盘并驱动
//! [`CsvRecordScanner`]，再把结果交给 [`CsvSplitAccumulator`] 与 [`plan_f05`]。
//!
//! 切分口径（方案 §5.3）：
//! - 只在**记录边界**断开，禁止字节硬切（引号内换行属于同一条记录）；
//! - 默认**每片都保留表头**，v1 仅支持单行表头（多行表头阻断，不猜测）；
//! - 列数与表头不一致的记录只**计数上报**，不静默修正；
//! - 引号不闭合至文件尾 → 剩余内容按末条记录保留并告警（T01）；
//! - UTF-16 源需先经 C01 转码（字节级解析无法处理双字节字符），计划条目阻断。
//!
//! 本切片只做**源字节直通**（不转码）；产物编码跟随源文件。

use std::collections::{BTreeMap, BTreeSet};

use crate::encoding::SourceEncoding;
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::{normalize, protected_by, try_normalize, ProtectedDir};
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanConflict, PlanCsvSplit, PlanDocument, PlanEntry,
    PlanExpected, PlanParamValue, PlanRange, PlanRelation, PlanResolution, PlanRole, PlanStats,
    PlanWarning, RiskLevel,
};

pub const F05_KIND: &str = "f05";

/// CSV 分隔符（方案 §5.3 默认嗅探 `, ; Tab |`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delimiter {
    Comma,
    Semicolon,
    Tab,
    Pipe,
}

impl Delimiter {
    pub const ALL: [Delimiter; 4] = [
        Delimiter::Comma,
        Delimiter::Semicolon,
        Delimiter::Tab,
        Delimiter::Pipe,
    ];

    pub const fn as_byte(self) -> u8 {
        match self {
            Self::Comma => b',',
            Self::Semicolon => b';',
            Self::Tab => b'\t',
            Self::Pipe => b'|',
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Comma => ",",
            Self::Semicolon => ";",
            Self::Tab => "\\t",
            Self::Pipe => "|",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "," | "comma" => Some(Self::Comma),
            ";" | "semicolon" => Some(Self::Semicolon),
            "\\t" | "\t" | "tab" => Some(Self::Tab),
            "|" | "pipe" => Some(Self::Pipe),
            _ => None,
        }
    }
}

/// 扫描到的一条 CSV 记录（含行尾换行符的字节数）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CsvRecord {
    pub len: u64,
    pub columns: u64,
    /// 记录内含引号内换行（跨物理行）。
    pub multiline: bool,
}

/// RFC 4180 记录扫描器（纯状态机：按块喂字节，跨块保持引号状态）。
#[derive(Clone, Debug)]
pub struct CsvRecordScanner {
    delimiter: u8,
    in_quotes: bool,
    /// 刚读到引号内的 `"`，下一字节决定它是转义还是闭合。
    after_quote: bool,
    field_start: bool,
    record_len: u64,
    columns: u64,
    multiline: bool,
}

impl CsvRecordScanner {
    pub fn new(delimiter: u8) -> Self {
        Self {
            delimiter,
            in_quotes: false,
            after_quote: false,
            field_start: true,
            record_len: 0,
            columns: 1,
            multiline: false,
        }
    }

    /// 喂入一段原始字节；每完成一条记录（以 `\n` 结束）回调一次。
    pub fn feed<F>(&mut self, bytes: &[u8], mut on_record: F)
    where
        F: FnMut(CsvRecord),
    {
        for &byte in bytes {
            if self.in_quotes {
                if self.after_quote {
                    if byte == b'"' {
                        // `""`：引号转义，仍在引号内。
                        self.after_quote = false;
                        self.record_len += 1;
                        continue;
                    }
                    // 闭合引号后的普通字节：退出引号，按引号外规则处理本字节。
                    self.in_quotes = false;
                    self.after_quote = false;
                } else {
                    if byte == b'"' {
                        self.after_quote = true;
                    } else if byte == b'\n' {
                        self.multiline = true;
                    }
                    self.record_len += 1;
                    continue;
                }
            }

            self.record_len += 1;
            if self.field_start && byte == b'"' {
                self.in_quotes = true;
                self.field_start = false;
            } else if byte == self.delimiter {
                self.columns += 1;
                self.field_start = true;
            } else if byte == b'\n' {
                on_record(CsvRecord {
                    len: self.record_len,
                    columns: self.columns,
                    multiline: self.multiline,
                });
                self.record_len = 0;
                self.columns = 1;
                self.field_start = true;
                self.multiline = false;
            } else {
                self.field_start = false;
            }
        }
    }

    /// 扫描结束：返回未以换行结束的末条记录与"引号未闭合"信号。
    pub fn finish(self) -> CsvScanTail {
        let tail = (self.record_len > 0).then_some(CsvRecord {
            len: self.record_len,
            columns: self.columns,
            multiline: self.multiline,
        });
        CsvScanTail {
            tail,
            unclosed_quote: self.in_quotes && !self.after_quote,
        }
    }
}

/// 扫描结束的收尾结果（不闭合引号按末条记录保留，方案 §5.3 异常处理）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CsvScanTail {
    pub tail: Option<CsvRecord>,
    pub unclosed_quote: bool,
}

/// 由样本（头部窗口）嗅探分隔符：取"列数稳定且多列记录最多"的候选。
///
/// 样本中未以换行结束的末尾记录不参与判定；全部候选都只有一列时返回 `None`
/// （壳层按"嗅探失败"告警并允许手动指定，方案 §5.3）。
pub fn sniff_delimiter(sample: &[u8]) -> Option<Delimiter> {
    let mut best: Option<(u32, u32, Delimiter)> = None;
    for delimiter in Delimiter::ALL {
        let mut scanner = CsvRecordScanner::new(delimiter.as_byte());
        let mut counts = Vec::new();
        scanner.feed(sample, |record| counts.push(record.columns));
        if counts.is_empty() {
            continue;
        }
        let multi = counts.iter().filter(|columns| **columns >= 2).count() as u32;
        if multi == 0 {
            continue;
        }
        let modal = modal_columns(&counts);
        let consistent = counts.iter().filter(|columns| **columns == modal).count() as u32;
        match best {
            Some((best_consistent, best_multi, _))
                if (consistent, multi) <= (best_consistent, best_multi) => {}
            _ => best = Some((consistent, multi, delimiter)),
        }
    }
    best.map(|(_, _, delimiter)| delimiter)
}

fn modal_columns(counts: &[u64]) -> u64 {
    let mut sorted = counts.to_vec();
    sorted.sort_unstable();
    let mut best = (0u64, 0usize);
    let mut index = 0;
    while index < sorted.len() {
        let mut end = index;
        while end < sorted.len() && sorted[end] == sorted[index] {
            end += 1;
        }
        let frequency = end - index;
        if frequency > best.1 || (frequency == best.1 && sorted[index] > best.0) {
            best = (sorted[index], frequency);
        }
        index = end;
    }
    best.0
}

/// 分片计量基准：按字节或按记录数（F05 方案 §5.3 "按大小/按行数"）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsvSplitBasis {
    Bytes(u64),
    Rows(u64),
}

/// 表头在源文件中的字节区间（左闭右开）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CsvHeaderSpan {
    pub start: u64,
    pub end: u64,
}

impl CsvHeaderSpan {
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// 一个分片的**记录区间**（不含表头；表头由计划载荷另行给出并每片重复写入）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CsvShard {
    pub index: u32,
    pub start: u64,
    pub end: u64,
    pub rows: u64,
    pub dirty_rows: u64,
}

impl CsvShard {
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// CSV 分片布局（壳层扫描后传入，kernel 只消费）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CsvSplitLayout {
    pub file_size: u64,
    pub delimiter: Delimiter,
    /// 分隔符是否由嗅探得到（false 表示嗅探失败，壳层按调用方参数或逗号兜底）。
    pub delimiter_detected: bool,
    pub header: Option<CsvHeaderSpan>,
    pub header_columns: u64,
    pub shards: Vec<CsvShard>,
    pub records: u64,
    pub dirty_rows: u64,
    pub multi_line_header: bool,
    pub unclosed_quote: bool,
    /// 源编码（仅用于阻断 UTF-16：字节级解析无法正确处理双字节字符）。
    pub source_encoding: Option<SourceEncoding>,
}

/// 逐记录累积分片边界（纯计算，内存 O(分片数)）。
#[derive(Clone, Debug)]
pub struct CsvSplitAccumulator {
    basis: CsvSplitBasis,
    keep_header: bool,
    header_seen: bool,
    header: Option<CsvHeaderSpan>,
    header_columns: u64,
    multi_line_header: bool,
    current_start: u64,
    current_bytes: u64,
    current_rows: u64,
    current_dirty: u64,
    shards: Vec<CsvShard>,
    records: u64,
    dirty_rows: u64,
}

impl CsvSplitAccumulator {
    pub fn new_bytes(target_bytes: u64, keep_header: bool) -> Result<Self, KernelError> {
        if target_bytes == 0 {
            return Err(KernelError::invalid_params(
                "targetBytes must be greater than zero",
            ));
        }
        Ok(Self::with_basis(
            CsvSplitBasis::Bytes(target_bytes),
            keep_header,
        ))
    }

    pub fn new_rows(rows_per_part: u64, keep_header: bool) -> Result<Self, KernelError> {
        if rows_per_part == 0 {
            return Err(KernelError::invalid_params(
                "rowsPerPart must be greater than zero",
            ));
        }
        Ok(Self::with_basis(
            CsvSplitBasis::Rows(rows_per_part),
            keep_header,
        ))
    }

    fn with_basis(basis: CsvSplitBasis, keep_header: bool) -> Self {
        Self {
            basis,
            keep_header,
            header_seen: false,
            header: None,
            header_columns: 0,
            multi_line_header: false,
            current_start: 0,
            current_bytes: 0,
            current_rows: 0,
            current_dirty: 0,
            shards: Vec::new(),
            records: 0,
            dirty_rows: 0,
        }
    }

    /// 推入一条记录；首条记录按 `keep_header` 决定是否为表头。
    pub fn push_record(&mut self, record: CsvRecord) {
        self.records += 1;
        if !self.header_seen {
            self.header_seen = true;
            self.header_columns = record.columns;
            self.multi_line_header = record.multiline;
            if self.keep_header {
                self.header = Some(CsvHeaderSpan {
                    start: 0,
                    end: record.len,
                });
                self.current_start = record.len;
                return;
            }
        }

        let dirty = u64::from(self.header_columns > 0 && record.columns != self.header_columns);
        self.current_dirty += dirty;
        self.dirty_rows += dirty;

        if let CsvSplitBasis::Bytes(target) = self.basis {
            if record.len > target {
                // 单条记录超过目标值：独占一片（每片至少一条记录）。
                self.close_current();
                self.push_shard(record.len, 1, dirty);
                return;
            }
        }

        self.current_bytes += record.len;
        self.current_rows += 1;
        let full = match self.basis {
            CsvSplitBasis::Bytes(target) => self.current_bytes >= target,
            CsvSplitBasis::Rows(target) => self.current_rows >= target,
        };
        if full {
            self.close_current();
        }
    }

    pub fn finish(
        mut self,
        file_size: u64,
        delimiter: Delimiter,
        delimiter_detected: bool,
        unclosed_quote: bool,
        source_encoding: Option<SourceEncoding>,
    ) -> CsvSplitLayout {
        self.close_current();
        CsvSplitLayout {
            file_size,
            delimiter,
            delimiter_detected,
            header: self.header,
            header_columns: self.header_columns,
            shards: self.shards,
            records: self.records,
            dirty_rows: self.dirty_rows,
            multi_line_header: self.multi_line_header,
            unclosed_quote,
            source_encoding,
        }
    }

    fn close_current(&mut self) {
        if self.current_rows == 0 {
            return;
        }
        let bytes = self.current_bytes;
        let rows = self.current_rows;
        let dirty = self.current_dirty;
        self.push_shard(bytes, rows, dirty);
    }

    fn push_shard(&mut self, bytes: u64, rows: u64, dirty_rows: u64) {
        let start = self.current_start;
        let end = start + bytes;
        self.shards.push(CsvShard {
            index: self.shards.len() as u32 + 1,
            start,
            end,
            rows,
            dirty_rows,
        });
        self.current_start = end;
        self.current_bytes = 0;
        self.current_rows = 0;
        self.current_dirty = 0;
    }
}

/// 单个待分割 CSV：布局由壳层扫描得到。
#[derive(Clone, Debug)]
pub struct PlanF05File {
    pub path: String,
    pub canonical_path: Option<String>,
    pub group_id: String,
    pub layout: CsvSplitLayout,
}

#[derive(Clone, Debug)]
pub struct PlanF05Input {
    pub plan_id: String,
    pub basis: CsvSplitBasis,
    /// 是否每片保留表头（方案 §5.3 默认开启）。
    pub keep_header: bool,
    /// 调用方显式指定的分隔符；`None` 表示按文件嗅探结果。
    pub delimiter: Option<Delimiter>,
    pub output_dir: String,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub existing_targets: Vec<String>,
    pub files: Vec<PlanF05File>,
}

/// 生成 F05 计划：每个分片一条 `split` 条目，按 `groupId` 归为 1:N。
pub fn plan_f05(input: PlanF05Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    match input.basis {
        CsvSplitBasis::Bytes(0) => {
            return Err(KernelError::invalid_params(
                "targetBytes must be greater than zero",
            ))
        }
        CsvSplitBasis::Rows(0) => {
            return Err(KernelError::invalid_params(
                "rowsPerPart must be greater than zero",
            ))
        }
        _ => {}
    }
    if input.output_dir.trim().is_empty() {
        return Err(KernelError::missing_param("outputDir is required"));
    }
    if input.files.is_empty() {
        return Err(KernelError::missing_param(
            "split plans require at least one file",
        ));
    }

    let mut params = BTreeMap::new();
    match input.basis {
        CsvSplitBasis::Bytes(bytes) => {
            params.insert("targetBytes".to_string(), PlanParamValue::Number(bytes));
        }
        CsvSplitBasis::Rows(rows) => {
            params.insert("rowsPerPart".to_string(), PlanParamValue::Number(rows));
        }
    }
    params.insert(
        "outputDir".to_string(),
        PlanParamValue::String(input.output_dir.clone()),
    );
    params.insert(
        "keepHeader".to_string(),
        PlanParamValue::Bool(input.keep_header),
    );
    params.insert(
        "delimiter".to_string(),
        PlanParamValue::String(
            input
                .delimiter
                .map_or_else(|| "auto".to_string(), |value| value.as_str().to_string()),
        ),
    );
    params.insert(
        "encoding".to_string(),
        PlanParamValue::String("source-bytes".to_string()),
    );

    let existing: BTreeSet<String> = input
        .existing_targets
        .iter()
        .map(|path| conflict_key(path, input.case_sense))
        .collect();
    let mut batch: BTreeSet<String> = BTreeSet::new();
    let mut entries = Vec::new();
    let mut warnings = Vec::new();

    for file in &input.files {
        if file.group_id.trim().is_empty() {
            return Err(KernelError::missing_param("groupId is required"));
        }
        if !is_ulid(&file.group_id) {
            return Err(KernelError::invalid_params(
                "groupId must be a 26-character ULID",
            ));
        }
        validate_layout(&file.layout)?;

        let blocked = file_block(&file.layout);
        let file_warnings = layout_warnings(&file.layout, &file.path, input.delimiter);
        warnings.extend(file_warnings);
        if file.layout.shards.is_empty() {
            continue;
        }

        let total = file.layout.shards.len() as u32;
        let file_name = crate::conflict::file_name_of(&file.path);
        let source_block = source_block(
            &file.path,
            file.canonical_path.as_deref(),
            &input.protected_dirs,
            input.case_sense,
        );
        let delimiter = input.delimiter.unwrap_or(file.layout.delimiter);
        let header_size = file.layout.header.map_or(0, |header| header.len());

        for shard in &file.layout.shards {
            let target = join_dir(
                &input.output_dir,
                &crate::split::shard_name(file_name, shard.index, total),
            );
            let target_block = target_block(&target, &input.protected_dirs, input.case_sense);
            let key = conflict_key(&target, input.case_sense);
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
                "part {}/{} of {}: {} record(s), {} bytes [{}..{})",
                shard.index,
                total,
                file_name,
                shard.rows,
                shard.len(),
                shard.start,
                shard.end
            );
            if file.layout.header.is_some() {
                reason.push_str("; header repeated");
            }
            if shard.dirty_rows > 0 {
                reason.push_str(&format!(
                    "; {} row(s) drifted from header",
                    shard.dirty_rows
                ));
            }

            entries.push(PlanEntry {
                entry_id: format!("{}-{:04}", input.plan_id, entries.len() + 1),
                source: file.path.clone(),
                target: Some(target),
                action: PlanAction::Split,
                relation: Some(PlanRelation {
                    group_id: file.group_id.clone(),
                    role: PlanRole::Output,
                    part_index: Some(shard.index),
                    part_total: Some(total),
                }),
                reason,
                conflict: conflict.clone(),
                resolution: conflict.as_ref().map(|_| PlanResolution::Sequence),
                blocked: source_block.clone().or(blocked.clone()).or(target_block),
                expected: Some(PlanExpected {
                    size: header_size + shard.len(),
                    algorithm: "csv-byte-range".to_string(),
                    hash: None,
                }),
                range: Some(PlanRange {
                    start: shard.start,
                    end: shard.end,
                }),
                csv: Some(PlanCsvSplit {
                    header: file.layout.header.map(|header| PlanRange {
                        start: header.start,
                        end: header.end,
                    }),
                    delimiter: delimiter.as_str().to_string(),
                    rows: shard.rows,
                    dirty_rows: shard.dirty_rows,
                }),
                merge: None,
                csv_merge: None,
                csv_extract: None,
            });
        }
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
        kind: F05_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

fn file_block(layout: &CsvSplitLayout) -> Option<PlanBlocked> {
    if matches!(
        layout.source_encoding,
        Some(SourceEncoding::Utf16Le | SourceEncoding::Utf16Be)
    ) {
        return Some(PlanBlocked {
            code: "E3001".to_string(),
            reason: "UTF-16 CSV must be transcoded before splitting".to_string(),
            hint: Some(
                "run C01 (transcode to UTF-8/GB18030) first, then split the CSV".to_string(),
            ),
        });
    }
    if layout.multi_line_header {
        return Some(PlanBlocked {
            code: "E3004".to_string(),
            reason: "multi-line headers are not supported in v1".to_string(),
            hint: Some(
                "keep a single header row before splitting (方案 §5.3, 11.6-15)".to_string(),
            ),
        });
    }
    None
}

fn layout_warnings(
    layout: &CsvSplitLayout,
    path: &str,
    requested: Option<Delimiter>,
) -> Vec<PlanWarning> {
    let mut warnings = Vec::new();
    if layout.records == 0 {
        warnings.push(PlanWarning {
            kind: "empty-file".to_string(),
            suggestion: Some(format!("{path} is empty; no parts produced")),
            risk_level: RiskLevel::Safe,
        });
        return warnings;
    }
    if layout.shards.is_empty() {
        warnings.push(PlanWarning {
            kind: "header-only".to_string(),
            suggestion: Some(format!(
                "{path} has a header but no data rows; no parts produced"
            )),
            risk_level: RiskLevel::Safe,
        });
    }
    if layout.unclosed_quote {
        warnings.push(PlanWarning {
            kind: "unclosed-quote".to_string(),
            suggestion: Some(format!(
                "{path} has an unclosed quote; the remainder is kept as the last record"
            )),
            risk_level: RiskLevel::Danger,
        });
    }
    if layout.dirty_rows > 0 {
        warnings.push(PlanWarning {
            kind: "column-drift".to_string(),
            suggestion: Some(format!(
                "{path} has {} row(s) whose column count differs from the header; reported without correction",
                layout.dirty_rows
            )),
            risk_level: RiskLevel::Caution,
        });
    }
    if requested.is_none() && !layout.delimiter_detected {
        warnings.push(PlanWarning {
            kind: "delimiter-not-detected".to_string(),
            suggestion: Some(format!(
                "{path} delimiter could not be detected; comma is assumed, specify --delimiter to override"
            )),
            risk_level: RiskLevel::Caution,
        });
    }
    warnings
}

/// 校验布局：表头从 0 起，记录区间从表头之后连续覆盖整个文件。
pub fn validate_layout(layout: &CsvSplitLayout) -> Result<(), KernelError> {
    let mut cursor = match &layout.header {
        Some(header) => {
            if header.start != 0 || header.end < header.start {
                return Err(KernelError::internal(
                    "csv header span is invalid",
                    format!("header spans [{}..{})", header.start, header.end),
                ));
            }
            header.end
        }
        None => 0,
    };
    for shard in &layout.shards {
        if shard.start != cursor || shard.end <= shard.start {
            return Err(KernelError::internal(
                "csv shard layout is not contiguous",
                format!("shard {} starts at {}", shard.index, shard.start),
            ));
        }
        cursor = shard.end;
    }
    if cursor != layout.file_size {
        return Err(KernelError::internal(
            "csv shard layout does not cover the whole file",
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

    fn scan(
        bytes: &[u8],
        delimiter: Delimiter,
        keep_header: bool,
        basis: CsvSplitBasis,
    ) -> CsvSplitLayout {
        let mut scanner = CsvRecordScanner::new(delimiter.as_byte());
        let mut accumulator = match basis {
            CsvSplitBasis::Bytes(target) => {
                CsvSplitAccumulator::new_bytes(target, keep_header).unwrap()
            }
            CsvSplitBasis::Rows(rows) => CsvSplitAccumulator::new_rows(rows, keep_header).unwrap(),
        };
        scanner.feed(bytes, |record| accumulator.push_record(record));
        let tail = scanner.finish();
        if let Some(record) = tail.tail {
            accumulator.push_record(record);
        }
        accumulator.finish(
            bytes.len() as u64,
            delimiter,
            true,
            tail.unclosed_quote,
            Some(SourceEncoding::Utf8),
        )
    }

    fn file(path: &str, layout: CsvSplitLayout) -> PlanF05File {
        PlanF05File {
            path: path.to_string(),
            canonical_path: None,
            group_id: GROUP_ID.to_string(),
            layout,
        }
    }

    fn input(files: Vec<PlanF05File>, basis: CsvSplitBasis) -> PlanF05Input {
        PlanF05Input {
            plan_id: PLAN_ID.to_string(),
            basis,
            keep_header: true,
            delimiter: None,
            output_dir: "out".to_string(),
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            existing_targets: Vec::new(),
            files,
        }
    }

    #[test]
    fn scanner_keeps_quoted_newlines_inside_one_record() {
        let mut scanner = CsvRecordScanner::new(b',');
        let mut records = Vec::new();
        scanner.feed(b"a,b\n\"x\ny\",z\n", |record| records.push(record));
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].columns, 2);
        assert_eq!(records[1].columns, 2);
        assert!(records[1].multiline);
        assert_eq!(records[1].len, 8);
    }

    #[test]
    fn scanner_handles_escaped_quotes_and_crlf() {
        let mut scanner = CsvRecordScanner::new(b',');
        let mut records = Vec::new();
        scanner.feed(b"\"a\"\"b\",c\r\n", |record| records.push(record));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].columns, 2);
        assert!(!records[0].multiline);
        assert_eq!(records[0].len, 10);
    }

    #[test]
    fn scanner_reports_unclosed_quote_tail() {
        let mut scanner = CsvRecordScanner::new(b',');
        let mut records = Vec::new();
        scanner.feed(b"a,b\n\"x,", |record| records.push(record));
        let tail = scanner.finish();
        assert_eq!(records.len(), 1);
        let tail_record = tail.tail.unwrap();
        assert_eq!(tail_record.columns, 1);
        assert!(tail.unclosed_quote);
    }

    #[test]
    fn scanner_state_survives_chunk_boundaries() {
        let source = b"\"one\",\"two\"\n\"three\",\"four\"\n";
        let mut whole = CsvRecordScanner::new(b',');
        let mut whole_records = Vec::new();
        whole.feed(source, |record| whole_records.push(record));

        let mut split = CsvRecordScanner::new(b',');
        let mut split_records = Vec::new();
        for chunk in source.chunks(3) {
            split.feed(chunk, |record| split_records.push(record));
        }
        assert_eq!(whole_records, split_records);
    }

    #[test]
    fn sniff_picks_the_delimiter_with_stable_columns() {
        assert_eq!(
            sniff_delimiter(b"a,b,c\n1,2,3\n4,5,6\n"),
            Some(Delimiter::Comma)
        );
        assert_eq!(
            sniff_delimiter(b"a;b;c\n1;2;3\n"),
            Some(Delimiter::Semicolon)
        );
        assert_eq!(sniff_delimiter(b"a\tb\tc\n1\t2\t3\n"), Some(Delimiter::Tab));
        assert_eq!(sniff_delimiter(b"a|b|c\n1|2|3\n"), Some(Delimiter::Pipe));
    }

    #[test]
    fn sniff_returns_none_for_single_column_data() {
        assert_eq!(sniff_delimiter(b"a\nb\nc\n"), None);
        assert_eq!(sniff_delimiter(b""), None);
    }

    #[test]
    fn accumulator_repeats_header_and_splits_at_record_boundaries() {
        let layout = scan(
            b"h1,h2\n1,2\n3,4\n5,6\n",
            Delimiter::Comma,
            true,
            CsvSplitBasis::Bytes(8),
        );
        let header = layout.header.unwrap();
        assert_eq!((header.start, header.end), (0, 6));
        assert_eq!(layout.shards.len(), 2);
        assert_eq!((layout.shards[0].start, layout.shards[0].end), (6, 14));
        assert_eq!(layout.shards[0].rows, 2);
        assert_eq!((layout.shards[1].start, layout.shards[1].end), (14, 18));
        assert_eq!(layout.shards[1].rows, 1);
    }

    #[test]
    fn accumulator_splits_by_row_count() {
        let layout = scan(
            b"h1,h2\n1,2\n3,4\n5,6\n7,8\n",
            Delimiter::Comma,
            true,
            CsvSplitBasis::Rows(2),
        );
        assert_eq!(layout.shards.len(), 2);
        assert_eq!(layout.shards[0].rows, 2);
        assert_eq!(layout.shards[1].rows, 2);
    }

    #[test]
    fn accumulator_counts_column_drift_without_correcting() {
        let layout = scan(
            b"h1,h2,h3\n1,2\n3,4,5,6\n7,8,9\n",
            Delimiter::Comma,
            true,
            CsvSplitBasis::Rows(1),
        );
        assert_eq!(layout.header_columns, 3);
        assert_eq!(layout.dirty_rows, 2);
        assert_eq!(layout.shards[0].dirty_rows, 1);
        assert_eq!(layout.shards[1].dirty_rows, 1);
        assert_eq!(layout.shards[2].dirty_rows, 0);
        assert_eq!(
            layout.shards.iter().map(|shard| shard.len()).sum::<u64>(),
            layout.file_size - layout.header.unwrap().len()
        );
    }

    #[test]
    fn overlong_record_occupies_its_own_part() {
        let layout = scan(
            b"h\n1\n\"0123456789\"\n2\n",
            Delimiter::Comma,
            true,
            CsvSplitBasis::Bytes(4),
        );
        assert_eq!(layout.shards.len(), 3);
        assert_eq!(layout.shards[1].rows, 1);
        assert_eq!(layout.shards[1].dirty_rows, 0);
    }

    #[test]
    fn header_can_be_disabled() {
        let layout = scan(
            b"h1,h2\n1,2\n3,4\n",
            Delimiter::Comma,
            false,
            CsvSplitBasis::Rows(2),
        );
        assert!(layout.header.is_none());
        assert_eq!(layout.shards.len(), 2);
        assert_eq!(layout.shards[0].rows, 2);
        assert_eq!(layout.shards[1].rows, 1);
    }

    #[test]
    fn header_only_file_has_no_shards() {
        let layout = scan(b"h1,h2\n", Delimiter::Comma, true, CsvSplitBasis::Bytes(8));
        assert!(layout.shards.is_empty());
        assert_eq!(layout.header.unwrap().end, 6);
        assert_eq!(layout.file_size, 6);
    }

    #[test]
    fn plan_builds_csv_split_entries_with_repeated_header() {
        let layout = scan(
            b"h1,h2\n1,2\n3,4\n5,6\n",
            Delimiter::Comma,
            true,
            CsvSplitBasis::Rows(2),
        );
        let plan = plan_f05(input(
            vec![file("data.csv", layout)],
            CsvSplitBasis::Rows(2),
        ))
        .unwrap();
        assert_eq!(plan.kind, F05_KIND);
        assert_eq!(plan.entries.len(), 2);
        assert_eq!(
            plan.params.get("rowsPerPart"),
            Some(&PlanParamValue::Number(2))
        );
        assert_eq!(
            plan.params.get("keepHeader"),
            Some(&PlanParamValue::Bool(true))
        );
        for (index, entry) in plan.entries.iter().enumerate() {
            let csv = entry.csv.as_ref().unwrap();
            assert_eq!(csv.header, Some(PlanRange { start: 0, end: 6 }));
            assert_eq!(csv.delimiter, ",");
            assert_eq!(entry.range.unwrap().start, 6 + index as u64 * 8);
            assert_eq!(
                entry.expected.as_ref().unwrap().size,
                6 + entry.range.unwrap().end - entry.range.unwrap().start
            );
        }
        assert_eq!(plan.entries[1].target.as_deref(), Some("out/data.2.csv"));
    }

    #[test]
    fn plan_warns_on_column_drift_and_unclosed_quote() {
        let layout = scan(
            b"h1,h2\n1\n\"x,",
            Delimiter::Comma,
            true,
            CsvSplitBasis::Rows(1),
        );
        let plan = plan_f05(input(vec![file("bad.csv", layout)], CsvSplitBasis::Rows(1))).unwrap();
        let kinds: Vec<&str> = plan
            .warnings
            .iter()
            .map(|warning| warning.kind.as_str())
            .collect();
        assert!(kinds.contains(&"column-drift"));
        assert!(kinds.contains(&"unclosed-quote"));
    }

    #[test]
    fn plan_blocks_utf16_source_and_multi_line_header() {
        let mut utf16 = scan(
            b"h1,h2\n1,2\n",
            Delimiter::Comma,
            true,
            CsvSplitBasis::Rows(1),
        );
        utf16.source_encoding = Some(SourceEncoding::Utf16Le);
        let plan = plan_f05(input(vec![file("u.csv", utf16)], CsvSplitBasis::Rows(1))).unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3001");

        let multi = scan(
            b"\"h1\nh2\",x\n1,2\n",
            Delimiter::Comma,
            true,
            CsvSplitBasis::Rows(1),
        );
        assert!(multi.multi_line_header);
        let plan = plan_f05(input(vec![file("m.csv", multi)], CsvSplitBasis::Rows(1))).unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3004");
    }

    #[test]
    fn plan_warns_when_delimiter_was_not_detected() {
        let mut layout = scan(b"a\nb\n", Delimiter::Comma, true, CsvSplitBasis::Rows(1));
        layout.delimiter_detected = false;
        let plan = plan_f05(input(
            vec![file("plain.csv", layout)],
            CsvSplitBasis::Rows(1),
        ))
        .unwrap();
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "delimiter-not-detected"));
    }

    #[test]
    fn plan_skips_empty_and_header_only_files() {
        let empty = scan(b"", Delimiter::Comma, true, CsvSplitBasis::Rows(1));
        let header_only = scan(b"h1,h2\n", Delimiter::Comma, true, CsvSplitBasis::Rows(1));
        let plan = plan_f05(input(
            vec![file("empty.csv", empty), file("h.csv", header_only)],
            CsvSplitBasis::Rows(1),
        ))
        .unwrap();
        assert!(plan.entries.is_empty());
        let kinds: Vec<&str> = plan
            .warnings
            .iter()
            .map(|warning| warning.kind.as_str())
            .collect();
        assert!(kinds.contains(&"empty-file"));
        assert!(kinds.contains(&"header-only"));
    }

    #[test]
    fn plan_rejects_bad_parameter_and_layout() {
        let layout = scan(b"h\n1\n", Delimiter::Comma, true, CsvSplitBasis::Rows(1));
        assert!(plan_f05(input(
            vec![file("a.csv", layout.clone())],
            CsvSplitBasis::Rows(0)
        ))
        .is_err());
        assert!(plan_f05(input(vec![file("a.csv", layout)], CsvSplitBasis::Bytes(0))).is_err());

        let broken = CsvSplitLayout {
            file_size: 10,
            delimiter: Delimiter::Comma,
            delimiter_detected: true,
            header: Some(CsvHeaderSpan { start: 0, end: 5 }),
            header_columns: 1,
            shards: vec![CsvShard {
                index: 1,
                start: 7,
                end: 10,
                rows: 1,
                dirty_rows: 0,
            }],
            records: 2,
            dirty_rows: 0,
            multi_line_header: false,
            unclosed_quote: false,
            source_encoding: Some(SourceEncoding::Utf8),
        };
        assert!(plan_f05(input(vec![file("b.csv", broken)], CsvSplitBasis::Rows(1))).is_err());
    }
}
