//!
//! C03 批量查找替换计划与内容变换（方案 §5.14）。
//!
//! 本切片支持 TXT/CSV 的纯文本与正则替换、CSV 单元格级默认保护、匹配计数与
//! 预览上下文。执行走 `ContentRewriter`：plain 与 CSV 单元格模式流式（尾部重叠窗口／
//! 记录级增量，ADR-0057），regex 模式仍整段（最大匹配长度不可知）。

use std::collections::BTreeMap;
use std::io::{Read, Write};

use regex::Regex;

use crate::binary;
use crate::csv::{sniff_delimiter, CsvRecordScanner, Delimiter};
use crate::csv_merge::{parse_records, write_record};
use crate::encoding::{detect, PlatformPrior, SourceEncoding};
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::ProtectedDir;
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanDocument, PlanEntry, PlanExpected, PlanParamValue,
    PlanResolution, PlanStats, PlanWarning, RiskLevel,
};
use crate::regex_rename::{compile_regex, validate_replacement};

pub const C03_KIND: &str = "c03";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplaceMode {
    Plain,
    Regex,
}

impl ReplaceMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Regex => "regex",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "plain" => Some(Self::Plain),
            "regex" => Some(Self::Regex),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsvMode {
    Cell,
    Text,
}

impl CsvMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cell => "cell",
            Self::Text => "text",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "cell" => Some(Self::Cell),
            "text" => Some(Self::Text),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContentStructure {
    #[default]
    Text,
    Csv,
}

impl ContentStructure {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Csv => "csv",
        }
    }

    pub fn resolve(path: &str) -> Self {
        let name = crate::conflict::file_name_of(path);
        match name.rsplit_once('.').map(|(_, extension)| extension) {
            Some(extension)
                if extension.eq_ignore_ascii_case("csv")
                    || extension.eq_ignore_ascii_case("tsv") =>
            {
                Self::Csv
            }
            _ => Self::Text,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ReplaceInspect {
    pub structure: ContentStructure,
    pub delimiter: Option<Delimiter>,
    pub match_count: u64,
    pub context: Option<String>,
    pub unclosed_quote: bool,
    pub column_drift: bool,
    pub utf8_error: bool,
}

enum Matcher {
    Plain(String),
    Regex(Regex),
}

impl Matcher {
    fn compile(mode: ReplaceMode, find: &str, replacement: &str) -> Result<Self, KernelError> {
        if find.is_empty() {
            return Err(KernelError::invalid_params("find text must not be empty"));
        }
        match mode {
            ReplaceMode::Plain => Ok(Self::Plain(find.to_string())),
            ReplaceMode::Regex => {
                let regex = compile_regex(find)?;
                validate_replacement(replacement, &regex)?;
                Ok(Self::Regex(regex))
            }
        }
    }

    fn count_and_context(&self, text: &str) -> (u64, Option<String>) {
        match self {
            Self::Plain(find) => {
                let mut count = 0;
                let mut context = None;
                for (start, matched) in text.match_indices(find) {
                    count += 1;
                    if context.is_none() {
                        context = Some(context_snippet(text, start, matched.len()));
                    }
                }
                (count, context)
            }
            Self::Regex(regex) => {
                let mut count = 0;
                let mut context = None;
                for matched in regex.find_iter(text) {
                    count += 1;
                    if context.is_none() {
                        context = Some(context_snippet(text, matched.start(), matched.len()));
                    }
                }
                (count, context)
            }
        }
    }

    fn replace(&self, text: &str, replacement: &str) -> String {
        match self {
            Self::Plain(find) => text.replace(find, replacement),
            Self::Regex(regex) => regex.replace_all(text, replacement).into_owned(),
        }
    }
}

/// 上下文半径（字节）：预览取匹配前后各 20 字节；壳层按「半径＋边界余量」回读窗口。
pub const CONTEXT_RADIUS_BYTES: usize = 20;
/// UTF-8 单字符最长 4 字节，因此向两侧走到字符边界最多各退 3 字节；窗口按这个余量多取。
pub const CONTEXT_SLACK_BYTES: usize = 3;

/// 首个匹配在全文中的字节偏移。流式采集只记偏移，上下文由壳层按偏移做一次
/// **有界**回读后交给 [`context_from_window`]——这样内存与文件体积无关，
/// 而预览文本与整段采集逐字一致。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirstMatch {
    pub start: usize,
    pub length: usize,
}

/// 预览上下文只有一条规则：±20 字节、落到字符边界、`\r`/`\n` 折成空格、60 字符截尾。
/// 整段版与流式版共用这里（`window` 取全文时窗口偏移为 0、`text_len` 即全文长度）。
pub fn context_from_window(
    window: &[u8],
    window_start: usize,
    text_len: usize,
    matched: FirstMatch,
) -> String {
    let want_begin = matched.start.saturating_sub(CONTEXT_RADIUS_BYTES);
    let want_end = (matched.start + matched.length + CONTEXT_RADIUS_BYTES).min(text_len);
    let mut begin = want_begin.saturating_sub(window_start);
    while (window_start + begin) > 0 && !is_utf8_boundary(window, begin) {
        begin -= 1;
    }
    let mut end = want_end - window_start;
    while end < window.len() && !is_utf8_boundary(window, end) {
        end += 1;
    }
    let snippet = String::from_utf8_lossy(&window[begin..end]).replace(['\r', '\n'], " ");
    if snippet.chars().count() > 60 {
        let truncated: String = snippet.chars().take(60).collect();
        format!("{truncated}…")
    } else {
        snippet
    }
}

fn is_utf8_boundary(bytes: &[u8], index: usize) -> bool {
    !matches!(bytes.get(index), Some(byte) if (0x80..=0xBF).contains(byte))
}

fn context_snippet(text: &str, start: usize, length: usize) -> String {
    context_from_window(text.as_bytes(), 0, text.len(), FirstMatch { start, length })
}

pub fn validate_rule(mode: ReplaceMode, find: &str, replacement: &str) -> Result<(), KernelError> {
    Matcher::compile(mode, find, replacement).map(|_| ())
}

/// 检查内容：匹配计数、上下文、CSV 结构信号。读取由壳层完成。
pub fn inspect_bytes(
    data: &[u8],
    mode: ReplaceMode,
    find: &str,
    replacement: &str,
    csv_mode: CsvMode,
    structure: ContentStructure,
    delimiter: Option<Delimiter>,
) -> Result<ReplaceInspect, KernelError> {
    let matcher = Matcher::compile(mode, find, replacement)?;
    let mut inspect = ReplaceInspect {
        structure,
        ..ReplaceInspect::default()
    };
    if structure == ContentStructure::Csv && csv_mode == CsvMode::Cell {
        let Some(delimiter) = delimiter.or_else(|| sniff_delimiter(data)) else {
            return Ok(inspect);
        };
        inspect.delimiter = Some(delimiter);
        let mut scanner = CsvRecordScanner::new(delimiter.as_byte());
        scanner.feed(data, |_| {});
        inspect.unclosed_quote = scanner.finish().unclosed_quote;
        if inspect.unclosed_quote {
            return Ok(inspect);
        }
        if std::str::from_utf8(data).is_err() {
            inspect.utf8_error = true;
            return Ok(inspect);
        }
        let records = parse_records(data, delimiter.as_byte());
        if let Some(first) = records.first() {
            inspect.column_drift = records.iter().any(|record| record.len() != first.len());
        }
        for record in &records {
            for field in record {
                let (count, context) = matcher.count_and_context(field);
                inspect.match_count += count;
                if inspect.context.is_none() {
                    inspect.context = context;
                }
            }
        }
        return Ok(inspect);
    }
    let text = match std::str::from_utf8(data) {
        Ok(text) => text,
        Err(_) => {
            inspect.utf8_error = true;
            return Ok(inspect);
        }
    };
    let (count, context) = matcher.count_and_context(text);
    inspect.match_count = count;
    inspect.context = context;
    Ok(inspect)
}

/// 计划段流式采集的通道选择（C03 事实采集，方案 §5.14／§4.8）。
///
/// 划界与改写段用的是同一把尺子（ADR-0057／ADR-0064）：能算出匹配长度上界才允许窗口化。
/// 因此**不能**把本模块写成「C03 计划段已全流式」。
pub enum ReplaceScan {
    /// 内存 O(块＋窗口)：plain 文本、静态可界定上界的正则、CSV 单元格三臂。
    Streaming(ReplaceStreamInspector),
    /// 单元格臂但分隔符未定：壳层先用 `csv::DelimiterSniffer` 流式嗅探，再带着结果重来。
    /// （整段版是把全文喂给 `sniff_delimiter`，判定口径不变，只是不再整读。）
    SniffFirst,
    /// 界定不出的正则（无界量词／锚点／`\b`／分组量化／零宽）：计划段仍整段采集，
    /// 内存 O(文件)——行为与本 ADR 之前逐字节相同。
    WholeFile,
}

/// 流式采集的产物。文本臂的 `inspect.context` 留空：上下文由壳层按 `context_probe`
/// 做一次**有界**回读（±23 字节）后交给 [`context_from_window`]，规则与整段版同一条函数。
pub struct StreamInspect {
    pub inspect: ReplaceInspect,
    pub context_probe: Option<FirstMatch>,
    /// 全文字节数，供回读窗口判断末端是否已到文件尾。
    pub text_len: usize,
}

/// C03 计划段的流式事实采集器：调用方逐块喂字节，本结构只保留尾巴窗口与首个匹配的偏移，
/// 内存与文件体积无关。判定（左到右、非重叠、±20 字节上下文、未闭合引号、列数漂移、
/// 非法 UTF-8 不计数）与 [`inspect_bytes`] 逐字一致，由等价性测试把住。
pub struct ReplaceStreamInspector {
    structure: ContentStructure,
    scan: StreamScan,
}

enum StreamScan {
    Text(TextScan),
    Cell(Box<CellScan>),
}

enum TextKind {
    Plain(String),
    Regex { regex: Regex, window: usize },
}

impl TextKind {
    /// 尾巴保留长度：字面串是 `len-1`，正则用的是算得出的字节上界（同改写段）。
    fn overlap(&self) -> usize {
        match self {
            Self::Plain(find) => find.len().saturating_sub(1),
            Self::Regex { window, .. } => *window,
        }
    }

    fn next_match(&self, text: &str, search: usize) -> Option<(usize, usize)> {
        match self {
            Self::Plain(find) => text[search..]
                .find(find.as_str())
                .map(|offset| (search + offset, find.len())),
            Self::Regex { regex, .. } => regex
                .find(&text[search..])
                .map(|matched| (search + matched.start(), matched.end() - matched.start())),
        }
    }
}

/// 文本臂（plain／可界定 regex）：逐块缓冲，只判定起点落在安全边界之前的匹配。
/// 结构与 `ReplaceRewriter::rewrite_plain`／`rewrite_regex_windowed` 同源，
/// 区别只在不写出内容。
struct TextScan {
    kind: TextKind,
    buffer: Vec<u8>,
    /// `buffer[0]` 在全文中的字节偏移。
    base: usize,
    total: usize,
    count: u64,
    first: Option<FirstMatch>,
    utf8_error: bool,
}

impl TextScan {
    fn new(kind: TextKind) -> Self {
        Self {
            kind,
            buffer: Vec::new(),
            base: 0,
            total: 0,
            count: 0,
            first: None,
            utf8_error: false,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len();
        if self.utf8_error {
            return;
        }
        self.buffer.extend_from_slice(bytes);
        self.scan(false);
    }

    fn finish(&mut self) {
        if !self.utf8_error {
            self.scan(true);
        }
    }

    fn scan(&mut self, eof: bool) {
        // 跨块被切开的多字节字符不算错：不完整尾巴留到下一块，EOF 才算错（同改写段）。
        let valid = match std::str::from_utf8(&self.buffer) {
            Ok(_) => self.buffer.len(),
            Err(error) if error.error_len().is_none() && !eof => error.valid_up_to(),
            Err(_) => {
                self.utf8_error = true;
                self.count = 0;
                self.first = None;
                self.buffer.clear();
                return;
            }
        };
        let mut safe = if eof {
            self.buffer.len()
        } else {
            self.buffer.len().saturating_sub(self.kind.overlap())
        };
        safe = safe.min(valid);
        safe = char_boundary_floor(&self.buffer[..valid], safe);
        let text = std::str::from_utf8(&self.buffer[..valid]).expect("just validated");
        let mut search = 0usize;
        while let Some((start, length)) = self.kind.next_match(text, search) {
            if start >= safe {
                // 起点还在保留窗口里：下一块补齐后才是最终判定，否则会漏跨块匹配。
                break;
            }
            self.count += 1;
            if self.first.is_none() {
                self.first = Some(FirstMatch {
                    start: self.base + start,
                    length,
                });
            }
            search = start + length;
        }
        let consumed = search.max(safe);
        self.buffer.drain(..consumed);
        self.base += consumed;
    }
}

/// 单元格臂：`RecordFeeder` 逐条产记录、逐字段计数，内存 O(单条记录)。
/// 未闭合引号仍由 `CsvRecordScanner` 给出（与整段版同一个机器）。
struct CellScan {
    feeder: crate::csv_merge::RecordFeeder,
    /// 未闭合引号仍由整段版用的同一台扫描器给出，收尾时取出 `finish()`。
    scanner: Option<CsvRecordScanner>,
    unclosed_quote: bool,
    total: usize,
    delimiter: Delimiter,
    matcher: Matcher,
    audit: Utf8Audit,
    first_columns: Option<usize>,
    column_drift: bool,
    count: u64,
    context: Option<String>,
}

impl CellScan {
    fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len();
        self.audit.push(bytes, false);
        if let Some(scanner) = self.scanner.as_mut() {
            scanner.feed(bytes, |_| {});
        }
        let records = self.feeder.push(bytes);
        self.consume(records);
    }

    fn finish(&mut self) {
        self.audit.push(&[], true);
        if let Some(scanner) = self.scanner.take() {
            self.unclosed_quote = scanner.finish().unclosed_quote;
        }
        let records = self.feeder.drain_finish();
        self.consume(records);
    }

    fn consume(&mut self, records: Vec<Vec<String>>) {
        for record in records {
            match self.first_columns {
                Some(columns) if record.len() != columns => self.column_drift = true,
                Some(_) => {}
                None => self.first_columns = Some(record.len()),
            }
            for field in &record {
                let (hits, context) = self.matcher.count_and_context(field);
                self.count += hits;
                if self.context.is_none() {
                    self.context = context;
                }
            }
        }
    }
}

/// 增量 UTF-8 校验：与整段版 `from_utf8(data).is_err()` 同判定，尾巴不完整只在 EOF 才算错。
#[derive(Default)]
struct Utf8Audit {
    pending: Vec<u8>,
    invalid: bool,
}

impl Utf8Audit {
    fn push(&mut self, bytes: &[u8], eof: bool) {
        if self.invalid {
            return;
        }
        let mut combined = std::mem::take(&mut self.pending);
        combined.extend_from_slice(bytes);
        match std::str::from_utf8(&combined) {
            Ok(_) => self.pending.clear(),
            Err(error) if error.error_len().is_none() && !eof => {
                self.pending = combined[error.valid_up_to()..].to_vec();
            }
            Err(_) => {
                self.invalid = true;
                self.pending.clear();
            }
        }
    }
}

impl ReplaceStreamInspector {
    /// 按臂选择采集通道。`delimiter` 为 `None` 且走单元格臂时返回
    /// [`ReplaceScan::SniffFirst`]：壳层嗅探完再带着结果重来。
    pub fn start(
        structure: ContentStructure,
        csv_mode: CsvMode,
        mode: ReplaceMode,
        find: &str,
        replacement: &str,
        delimiter: Option<Delimiter>,
    ) -> Result<ReplaceScan, KernelError> {
        let matcher = Matcher::compile(mode, find, replacement)?;
        if structure == ContentStructure::Csv && csv_mode == CsvMode::Cell {
            let Some(delimiter) = delimiter else {
                return Ok(ReplaceScan::SniffFirst);
            };
            return Ok(ReplaceScan::Streaming(Self {
                structure,
                scan: StreamScan::Cell(Box::new(CellScan {
                    feeder: crate::csv_merge::RecordFeeder::new(delimiter.as_byte()),
                    scanner: Some(CsvRecordScanner::new(delimiter.as_byte())),
                    unclosed_quote: false,
                    total: 0,
                    delimiter,
                    matcher,
                    audit: Utf8Audit::default(),
                    first_columns: None,
                    column_drift: false,
                    count: 0,
                    context: None,
                })),
            }));
        }
        let kind = match matcher {
            Matcher::Plain(find) => TextKind::Plain(find),
            Matcher::Regex(regex) => match regex_window_bytes(regex.as_str()) {
                Some(window) => TextKind::Regex { regex, window },
                // 界定不出上界的正则：连计数都保持整段，内存口径与改写段一致。
                None => return Ok(ReplaceScan::WholeFile),
            },
        };
        Ok(ReplaceScan::Streaming(Self {
            structure,
            scan: StreamScan::Text(TextScan::new(kind)),
        }))
    }

    pub fn push(&mut self, bytes: &[u8]) {
        match &mut self.scan {
            StreamScan::Text(scan) => scan.push(bytes),
            StreamScan::Cell(scan) => scan.push(bytes),
        }
    }

    pub fn finish(self) -> StreamInspect {
        match self.scan {
            StreamScan::Text(mut scan) => {
                scan.finish();
                let mut inspect = ReplaceInspect {
                    structure: self.structure,
                    ..ReplaceInspect::default()
                };
                inspect.utf8_error = scan.utf8_error;
                if scan.utf8_error {
                    return StreamInspect {
                        inspect,
                        context_probe: None,
                        text_len: scan.total,
                    };
                }
                inspect.match_count = scan.count;
                StreamInspect {
                    inspect,
                    context_probe: scan.first,
                    text_len: scan.total,
                }
            }
            StreamScan::Cell(mut scan) => {
                scan.finish();
                let mut inspect = ReplaceInspect {
                    structure: self.structure,
                    delimiter: Some(scan.delimiter),
                    ..ReplaceInspect::default()
                };
                // 短路顺序与整段版一致：未闭合引号优先于 UTF-8 判定，两者都不计数。
                if scan.unclosed_quote {
                    inspect.unclosed_quote = true;
                    return StreamInspect {
                        inspect,
                        context_probe: None,
                        text_len: scan.total,
                    };
                }
                if scan.audit.invalid {
                    inspect.utf8_error = true;
                    return StreamInspect {
                        inspect,
                        context_probe: None,
                        text_len: scan.total,
                    };
                }
                inspect.match_count = scan.count;
                inspect.context = scan.context.take();
                inspect.column_drift = scan.column_drift;
                StreamInspect {
                    inspect,
                    context_probe: None,
                    text_len: scan.total,
                }
            }
        }
    }
}

/// 执行内容变换（C03 重写器调用；整段读写）。
pub fn replace_bytes(
    data: &[u8],
    mode: ReplaceMode,
    find: &str,
    replacement: &str,
    csv_mode: CsvMode,
    structure: ContentStructure,
    delimiter: Option<Delimiter>,
) -> Result<Vec<u8>, KernelError> {
    let matcher = Matcher::compile(mode, find, replacement)?;
    if structure == ContentStructure::Csv && csv_mode == CsvMode::Cell {
        let delimiter = delimiter.or_else(|| sniff_delimiter(data)).ok_or_else(|| {
            KernelError::invalid_params("CSV delimiter could not be detected; pass --delimiter")
        })?;
        let mut scanner = CsvRecordScanner::new(delimiter.as_byte());
        scanner.feed(data, |_| {});
        if scanner.finish().unclosed_quote {
            return Err(KernelError::new(
                "E3004",
                "CSV contains an unclosed quote; cell-level replacement is refused",
                None,
                false,
            ));
        }
        let records = parse_records(data, delimiter.as_byte());
        let mut output = String::new();
        for record in &records {
            let replaced: Vec<String> = record
                .iter()
                .map(|field| matcher.replace(field, replacement))
                .collect();
            write_record(&replaced, delimiter.as_byte(), &mut output);
        }
        return Ok(output.into_bytes());
    }
    let text = std::str::from_utf8(data).map_err(|error| {
        KernelError::new(
            "E3001",
            "source is not valid UTF-8; convert it via C01 first",
            Some(error.to_string()),
            false,
        )
    })?;
    Ok(matcher.replace(text, replacement).into_bytes())
}

#[derive(Clone, Debug)]
pub struct PlanC03File {
    pub path: String,
    pub canonical_path: Option<String>,
    pub file_size: u64,
    pub sample: Vec<u8>,
    pub structure: ContentStructure,
    pub inspect: ReplaceInspect,
    /// 壳层采集的写入权限事实（`platform::preflight::path_check`）；C03 原地改写，
    /// 源文件即写入目标。只用于预览期点名（方案 §5.14 的"跳过并报告"之标记半程）。
    pub readonly: bool,
    pub locked: bool,
}

#[derive(Clone, Debug)]
pub struct PlanC03Input {
    pub plan_id: String,
    pub mode: ReplaceMode,
    pub find: String,
    pub replacement: String,
    pub csv_mode: CsvMode,
    pub files: Vec<PlanC03File>,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub confirmed_source: Option<SourceEncoding>,
    pub prior: PlatformPrior,
    pub delimiter: Option<Delimiter>,
}

pub fn plan_c03(input: PlanC03Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.files.is_empty() {
        return Err(KernelError::missing_param("c03 requires at least one file"));
    }
    validate_rule(input.mode, &input.find, &input.replacement)?;

    let mut params = BTreeMap::new();
    params.insert(
        "mode".to_string(),
        PlanParamValue::String(input.mode.as_str().to_string()),
    );
    params.insert(
        "find".to_string(),
        PlanParamValue::String(input.find.clone()),
    );
    params.insert(
        "replace".to_string(),
        PlanParamValue::String(input.replacement.clone()),
    );
    params.insert(
        "csvMode".to_string(),
        PlanParamValue::String(input.csv_mode.as_str().to_string()),
    );
    params.insert(
        "delimiter".to_string(),
        PlanParamValue::String(input.delimiter.map_or_else(
            || "auto".to_string(),
            |delimiter| delimiter.as_str().to_string(),
        )),
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
    let mut binary_skipped = false;
    let mut csv_text_mode = false;
    let mut column_drift = false;

    for file in &input.files {
        let mut blocked = crate::csv_merge::source_block(
            &file.path,
            file.canonical_path.as_deref(),
            &input.protected_dirs,
            input.case_sense,
        );
        if blocked.is_none() {
            if let Some(magic) = binary::magic_name(&file.sample) {
                binary_skipped = true;
                blocked = Some(PlanBlocked {
                    code: "E3001".to_string(),
                    reason: format!(
                        "binary file signature detected ({magic}); text pipeline refuses it"
                    ),
                    hint: Some("remove the file from the text pipeline".to_string()),
                });
            }
        }
        if blocked.is_none() {
            let detection = detect(&file.sample, input.prior);
            let selected = input.confirmed_source.or(if detection.needs_manual {
                None
            } else {
                detection.encoding
            });
            if selected != Some(SourceEncoding::Utf8) {
                blocked = Some(PlanBlocked {
                    code: "E3001".to_string(),
                    reason: "only UTF-8 sources are supported by this C03 slice".to_string(),
                    hint: Some("convert the file to UTF-8 first (C01)".to_string()),
                });
            } else if file.inspect.utf8_error {
                blocked = Some(PlanBlocked {
                    code: "E3001".to_string(),
                    reason: "source is not valid UTF-8; convert it via C01 first".to_string(),
                    hint: Some("convert the file to UTF-8 first (C01)".to_string()),
                });
            }
        }
        if blocked.is_none()
            && file.structure == ContentStructure::Csv
            && input.csv_mode == CsvMode::Cell
        {
            if file.inspect.unclosed_quote {
                blocked = Some(PlanBlocked {
                    code: "E3004".to_string(),
                    reason: "CSV contains an unclosed quote; cell-level replacement is refused"
                        .to_string(),
                    hint: Some("fix the CSV or use --csv-mode text".to_string()),
                });
            } else if file.inspect.delimiter.is_none() {
                blocked = Some(PlanBlocked {
                    code: "E3004".to_string(),
                    reason: "CSV delimiter could not be detected".to_string(),
                    hint: Some("pass --delimiter ,|;|tab|pipe".to_string()),
                });
            }
        }
        if file.structure == ContentStructure::Csv && input.csv_mode == CsvMode::Text {
            csv_text_mode = true;
        }
        if file.inspect.column_drift {
            column_drift = true;
        }

        let (reason, resolution) = match &blocked {
            Some(block) => (block.reason.clone(), None),
            None if file.inspect.match_count == 0 => {
                ("no matches".to_string(), Some(PlanResolution::Skip))
            }
            None => (
                format!(
                    "{} {} matches={}; context: {}",
                    input.mode.as_str(),
                    file.structure.as_str(),
                    file.inspect.match_count,
                    file.inspect.context.as_deref().unwrap_or("")
                ),
                None,
            ),
        };

        entries.push(PlanEntry {
            entry_id: format!("{}-{:04}", input.plan_id, entries.len() + 1),
            source: file.path.clone(),
            target: None,
            action: PlanAction::EditContent,
            relation: None,
            reason,
            conflict: None,
            resolution,
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
            csv_rows: None,
            csv_dedup: None,
            csv_filter: None,
            csv_columns: None,
            csv_sort: None,
        });
    }

    let mut warnings = Vec::new();
    if input.replacement.is_empty() {
        warnings.push(PlanWarning {
            kind: "empty-replacement".to_string(),
            suggestion: Some("replacement is empty; matched content will be deleted".to_string()),
            risk_level: RiskLevel::Danger,
        });
    }
    if csv_text_mode {
        warnings.push(PlanWarning {
            kind: "csv-text-mode".to_string(),
            suggestion: Some(
                "raw text replacement on CSV may break quotes; cell mode is safer".to_string(),
            ),
            risk_level: RiskLevel::Caution,
        });
    }
    if column_drift {
        warnings.push(PlanWarning {
            kind: "column-drift".to_string(),
            suggestion: Some("CSV rows have different column counts".to_string()),
            risk_level: RiskLevel::Caution,
        });
    }
    if binary_skipped {
        warnings.push(PlanWarning {
            kind: "binary-skipped".to_string(),
            suggestion: Some(
                "binary files are blocked and never enter the text pipeline".to_string(),
            ),
            risk_level: RiskLevel::Danger,
        });
    }

    // 方案 §5.14 的标记半程：只写会真正被改写的文件，被阻断的条目不再重复点名。
    let writable_sources: std::collections::BTreeSet<&str> = entries
        .iter()
        .filter(|entry| entry.blocked.is_none())
        .map(|entry| entry.source.as_str())
        .collect();
    let readonly_paths: Vec<String> = input
        .files
        .iter()
        .filter(|file| file.readonly && writable_sources.contains(file.path.as_str()))
        .map(|file| file.path.clone())
        .collect();
    let locked_paths: Vec<String> = input
        .files
        .iter()
        .filter(|file| file.locked && writable_sources.contains(file.path.as_str()))
        .map(|file| file.path.clone())
        .collect();
    if !readonly_paths.is_empty() {
        warnings.push(PlanWarning {
            kind: "readonly".to_string(),
            suggestion: Some(format!(
                "these files are read-only and rewriting them needs write access; \
                 clear the read-only attribute and preview again: {}",
                readonly_paths.join(", ")
            )),
            risk_level: RiskLevel::Caution,
        });
    }
    if !locked_paths.is_empty() {
        warnings.push(PlanWarning {
            kind: "locked".to_string(),
            suggestion: Some(format!(
                "these files are held open by another program; close it and preview again: {}",
                locked_paths.join(", ")
            )),
            risk_level: RiskLevel::Caution,
        });
    }

    let stats = PlanStats {
        total: entries.len(),
        conflict: 0,
        warning: warnings.len(),
        blocked: entries
            .iter()
            .filter(|entry| entry.blocked.is_some())
            .count(),
    };
    Ok(PlanDocument {
        plan_id: input.plan_id,
        kind: C03_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

pub struct ReplaceRewriter {
    mode: ReplaceMode,
    find: String,
    replacement: String,
    csv_mode: CsvMode,
    delimiter: Option<Delimiter>,
}

impl ReplaceRewriter {
    pub fn new(
        mode: ReplaceMode,
        find: String,
        replacement: String,
        csv_mode: CsvMode,
        delimiter: Option<Delimiter>,
    ) -> Self {
        Self {
            mode,
            find,
            replacement,
            csv_mode,
            delimiter,
        }
    }

    /// 从计划参数快照构造（与 `plan_c03` 写入的 params 同源）。
    pub fn from_plan(plan: &crate::plan::PlanDocument) -> Result<Self, KernelError> {
        let string_param = |name: &str| -> Result<&str, KernelError> {
            match plan.params.get(name) {
                Some(crate::plan::PlanParamValue::String(value)) => Ok(value.as_str()),
                _ => Err(KernelError::missing_param(format!(
                    "plan param {name} is required"
                ))),
            }
        };
        let mode = ReplaceMode::parse(string_param("mode")?).ok_or_else(|| {
            KernelError::invalid_params("plan param mode is not a supported mode")
        })?;
        let csv_mode = CsvMode::parse(string_param("csvMode")?).ok_or_else(|| {
            KernelError::invalid_params("plan param csvMode is not a supported mode")
        })?;
        let delimiter = match string_param("delimiter")? {
            "auto" => None,
            value => Some(Delimiter::parse(value).ok_or_else(|| {
                KernelError::invalid_params("plan param delimiter is not a supported delimiter")
            })?),
        };
        Ok(Self::new(
            mode,
            string_param("find")?.to_string(),
            string_param("replace")?.to_string(),
            csv_mode,
            delimiter,
        ))
    }
}

/// C03 流式替换的块大小（ADR-0057）：只决定内核侧内存上界与系统调用次数，
/// 与适配层的固定块读取（方案 §4.8 的 4 MB 建议值）无关。
const REPLACE_STREAM_CHUNK: usize = 64 * 1024;

/// 正则匹配长度的**字节上界**（ADR-0064）：窗口化只有在尾巴比任何可能的匹配都长时
/// 才不会漏替换。算得出上界才返回 `Some`；算不出**一律返回 `None` 退回整段**（＝现状），
/// 因为窗口偏小的后果是静默改坏内容，而退回整段的后果只是内存 O(文件)。
///
/// 三类必须拒绝的构造（各自的静默出错形态见 ADR-0064 §1）：
/// - 锚点与单词边界（`^` `$` `\A` `\z` `\Z` `\b` `\B`）：窗口将每块端点当成文本端点／丢掉左邻字节；
/// - 无界重复（`*` `+` `{m,}`）与带量词的分组 `(…){n}`：没有上界可算；
/// - 括号里的扩展语法（`(?…)`：标志组与非捕获组）：保守拒绝，不去推断其长度语义。
pub fn regex_window_bytes(pattern: &str) -> Option<usize> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut bound: usize = 0;
    // 最后一个可计数原子的长度（1 个字符）；量化符要改写它。
    let mut last: Option<usize> = None;
    let mut group_depth: usize = 0;
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '^' | '$' => return None,
            '\\' => {
                let next = *chars.get(index + 1)?;
                if matches!(next, 'b' | 'B' | 'A' | 'z' | 'Z') {
                    return None;
                }
                index += 2;
                bound += 1;
                last = Some(1);
            }
            '[' => {
                // 字符类整体记 1 个字符；`]` 出现在首位或 `^` 之后时是字面量。
                let mut cursor = index + 1;
                if chars.get(cursor) == Some(&'^') {
                    cursor += 1;
                }
                if chars.get(cursor) == Some(&']') {
                    cursor += 1;
                }
                let mut closed = false;
                while cursor < chars.len() {
                    match chars[cursor] {
                        '\\' => cursor += 2,
                        ']' => {
                            cursor += 1;
                            closed = true;
                            break;
                        }
                        _ => cursor += 1,
                    }
                }
                if !closed {
                    return None;
                }
                index = cursor;
                bound += 1;
                last = Some(1);
            }
            '.' => {
                index += 1;
                bound += 1;
                last = Some(1);
            }
            '(' => {
                if chars.get(index + 1) == Some(&'?') {
                    return None;
                }
                group_depth += 1;
                index += 1;
                last = None;
            }
            ')' => {
                if group_depth == 0 {
                    return None;
                }
                group_depth -= 1;
                index += 1;
                // 分组本身不计数；其后若跟量化符则无法定界（见 `*`/`+`/`{` 分支）。
                last = None;
            }
            '|' => {
                index += 1;
                last = None;
            }
            '*' | '+' => return None,
            '?' => {
                // `x?` 的上界仍是 x 本身；`x{1,4}?` 之类懒惰量化仍是有界的，交给 `{` 分支。
                last?;
                index += 1;
            }
            '{' => {
                // 量化对象必须是刚计过数的那个原子；分组与交替之后的量化一律拒绝。
                let counted = last?;
                let mut cursor = index + 1;
                let mut min_digits = String::new();
                while cursor < chars.len() && chars[cursor].is_ascii_digit() {
                    min_digits.push(chars[cursor]);
                    cursor += 1;
                }
                if min_digits.is_empty() {
                    return None;
                }
                let mut max_digits = String::new();
                if cursor < chars.len() && chars[cursor] == ',' {
                    cursor += 1;
                    while cursor < chars.len() && chars[cursor].is_ascii_digit() {
                        max_digits.push(chars[cursor]);
                        cursor += 1;
                    }
                    // `{m,}` 是上界无穷（含 `{m,0}` 之外的所有空数字情形）。
                    if max_digits.is_empty() {
                        return None;
                    }
                } else if cursor >= chars.len() || chars[cursor] != '}' {
                    return None;
                }
                if cursor >= chars.len() || chars[cursor] != '}' {
                    return None;
                }
                let max: usize = if max_digits.is_empty() {
                    min_digits.parse().ok()?
                } else {
                    max_digits.parse().ok()?
                };
                if max > 1024 {
                    // 上界能算，但乘出来的窗口比任何合理的文件还大：退回整段更诚实。
                    return None;
                }
                let added = counted.checked_mul(max)?;
                bound = bound - counted + added;
                last = Some(added);
                index = cursor + 1;
                // 懒惰标记 `?` 不改变上界。
                if chars.get(index) == Some(&'?') {
                    index += 1;
                }
            }
            other => {
                if other == ')' || other == '}' {
                    return None;
                }
                index += 1;
                bound += 1;
                last = Some(1);
            }
        }
    }
    if group_depth != 0 || bound == 0 {
        return None;
    }
    // 字符 → 字节：UTF-8 单字符最多 4 字节；上界取放大值，宁可尾巴留长。
    Some(bound.saturating_mul(4).max(4))
}

fn rewrite_io_error(error: std::io::Error) -> KernelError {
    KernelError::new(
        "E3005",
        "content rewrite failed",
        Some(error.to_string()),
        false,
    )
}

fn rewrite_utf8_error(error: std::str::Utf8Error) -> KernelError {
    KernelError::new(
        "E3001",
        "source is not valid UTF-8; convert it via C01 first",
        Some(error.to_string()),
        false,
    )
}

/// 安全边界必须回退到**字符边界**（ADR-0064）：按字节退 `overlap`/`window` 时，
/// 缓冲区整块合法的情形下边界正好可能落在多字节字符中间——半个字符写进输出、
/// 另外半个留在缓冲里，下一块的 UTF-8 校验会对一份完全合法的文件报 `E3001`。
fn char_boundary_floor(buffer: &[u8], limit: usize) -> usize {
    let mut limit = limit.min(buffer.len());
    // UTF-8 延续字节形如 0b10xx_xxxx：落在它上面就不是字符起点，逐字节回退到边界。
    while limit > 0 && matches!(buffer.get(limit), Some(byte) if (0x80..=0xBF).contains(byte)) {
        limit -= 1;
    }
    limit
}

impl ReplaceRewriter {
    /// plain 文本模式的流式替换（ADR-0057）：尾部重叠 `find.len()-1` 字节，
    /// 只判定「起点 < 安全边界」的匹配，命中即写出前缀与替换文本并跳过匹配。
    /// 语义与 `str::replace` 的非重叠、从左到右一致；内存 O(find + 块)。
    fn rewrite_plain(
        &self,
        input: &mut dyn Read,
        output: &mut dyn Write,
    ) -> Result<(), KernelError> {
        let find = self.find.as_bytes();
        let replacement = self.replacement.as_bytes();
        let overlap = find.len().saturating_sub(1);
        let mut buffer: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; REPLACE_STREAM_CHUNK];
        loop {
            let read = input.read(&mut chunk).map_err(rewrite_io_error)?;
            let eof = read == 0;
            buffer.extend_from_slice(&chunk[..read]);
            // 跨块被切开的多字节字符不算错：尾部不完整留到下一块再判，EOF 才算错。
            let valid = match std::str::from_utf8(&buffer) {
                Ok(_) => buffer.len(),
                Err(error) if error.error_len().is_none() && !eof => error.valid_up_to(),
                Err(error) => return Err(rewrite_utf8_error(error)),
            };
            let mut safe = if eof {
                buffer.len()
            } else {
                buffer.len().saturating_sub(overlap)
            };
            safe = safe.min(valid);
            safe = char_boundary_floor(&buffer[..valid], safe);
            let text = std::str::from_utf8(&buffer[..valid]).expect("just validated");
            let mut matches: Vec<(usize, usize)> = Vec::new();
            let mut search = 0;
            while let Some(offset) = text[search..].find(self.find.as_str()) {
                let start = search + offset;
                if start >= safe {
                    // 匹配起点落在重叠窗口里：等下一块补齐后再判，否则会漏跨块匹配。
                    break;
                }
                matches.push((start, start + find.len()));
                search = start + find.len();
            }
            let mut cursor = 0;
            for (start, end) in matches {
                if start > cursor {
                    output
                        .write_all(&buffer[cursor..start])
                        .map_err(rewrite_io_error)?;
                }
                output.write_all(replacement).map_err(rewrite_io_error)?;
                cursor = end;
            }
            let consumed = if cursor >= safe {
                cursor
            } else {
                output
                    .write_all(&buffer[cursor..safe])
                    .map_err(rewrite_io_error)?;
                safe
            };
            buffer.drain(..consumed);
            if eof {
                return Ok(());
            }
        }
    }

    /// 能不能走窗口化流式：算得出字节上界、且模式不会零宽匹配，才给 `(正则, 窗口)`。
    /// 任一条件不满足就返回 `None`——**退回整段**，绝不带着偏小的窗口去改用户文件。
    fn streamable_regex(&self) -> Option<(Regex, usize)> {
        let window = regex_window_bytes(&self.find)?;
        let regex = Regex::new(&self.find).ok()?;
        // 零宽匹配（`a|` 之类的空分支）让推进纪律变得含混：整段路径能正确处理的相邻空匹配，
        // 在这里要靠"起点 < safe"逐块推进，边界太多，不如不流式。
        if regex.is_match("") {
            return None;
        }
        Some((regex, window))
    }

    /// regex 文本模式的窗口化流式替换（ADR-0064）：`window` 是 `regex_window_bytes` 算出的
    /// **匹配长度字节上界**，尾巴比任何可能的匹配都长，所以「起点 < `len - window`」的判定
    /// 与整段 `replace_all` 逐字节等价；内存 O(window + 块)。
    fn rewrite_regex_windowed(
        &self,
        regex: &Regex,
        window: usize,
        input: &mut dyn Read,
        output: &mut dyn Write,
    ) -> Result<(), KernelError> {
        let mut buffer: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; REPLACE_STREAM_CHUNK];
        loop {
            let read = input.read(&mut chunk).map_err(rewrite_io_error)?;
            let eof = read == 0;
            buffer.extend_from_slice(&chunk[..read]);
            // 跨块被切开的多字节字符不算错：不完整尾部留到下一块，EOF 才算错（同 plain 臂）。
            let valid = match std::str::from_utf8(&buffer) {
                Ok(_) => buffer.len(),
                Err(error) if error.error_len().is_none() && !eof => error.valid_up_to(),
                Err(error) => return Err(rewrite_utf8_error(error)),
            };
            let mut safe = if eof {
                buffer.len()
            } else {
                buffer.len().saturating_sub(window)
            };
            safe = safe.min(valid);
            safe = char_boundary_floor(&buffer[..valid], safe);
            let text = std::str::from_utf8(&buffer[..valid]).expect("just validated");
            let mut cursor = 0usize;
            let mut search = 0usize;
            while search <= valid {
                let Some(m) = regex.find(&text[search..valid]) else {
                    break;
                };
                let start = search + m.start();
                let end = search + m.end();
                if start >= safe {
                    // 起点在保留窗口内：换成更长的匹配仍有可能，等下一块。
                    break;
                }
                if start > cursor {
                    output
                        .write_all(&buffer[cursor..start])
                        .map_err(rewrite_io_error)?;
                }
                let replaced = regex.replace(&text[start..end], self.replacement.as_str());
                output
                    .write_all(replaced.as_bytes())
                    .map_err(rewrite_io_error)?;
                cursor = end;
                search = end;
            }
            let consumed = if cursor >= safe {
                cursor
            } else {
                output
                    .write_all(&buffer[cursor..safe])
                    .map_err(rewrite_io_error)?;
                safe
            };
            buffer.drain(..consumed);
            if eof {
                return Ok(());
            }
        }
    }

    /// CSV 单元格模式的流式替换（ADR-0057）：`RecordFeeder` 增量产记录 → 逐字段
    /// 替换 → `write_record` 逐条写出。单元格替换是记录内局部操作，逐条序列化与
    /// 整段序列化逐字节一致；内存 O(单条记录 + 块)。
    fn rewrite_cells(
        &self,
        input: &mut dyn Read,
        output: &mut dyn Write,
    ) -> Result<(), KernelError> {
        let matcher = Matcher::compile(self.mode, &self.find, &self.replacement)?;
        let mut chunk = vec![0u8; REPLACE_STREAM_CHUNK];
        let first = input.read(&mut chunk).map_err(rewrite_io_error)?;
        let delimiter = match self.delimiter {
            Some(delimiter) => delimiter,
            None => sniff_delimiter(&chunk[..first]).ok_or_else(|| {
                KernelError::invalid_params("CSV delimiter could not be detected; pass --delimiter")
            })?,
        };
        let mut feeder = crate::csv_merge::RecordFeeder::new(delimiter.as_byte());
        let mut write_records = |records: Vec<Vec<String>>| -> Result<(), KernelError> {
            for record in records {
                let replaced: Vec<String> = record
                    .iter()
                    .map(|field| matcher.replace(field, &self.replacement))
                    .collect();
                let mut line = String::new();
                write_record(&replaced, delimiter.as_byte(), &mut line);
                output
                    .write_all(line.as_bytes())
                    .map_err(rewrite_io_error)?;
            }
            Ok(())
        };
        write_records(feeder.push(&chunk[..first]))?;
        loop {
            let read = input.read(&mut chunk).map_err(rewrite_io_error)?;
            if read == 0 {
                break;
            }
            write_records(feeder.push(&chunk[..read]))?;
        }
        if feeder.unclosed_quote() {
            return Err(KernelError::new(
                "E3004",
                "CSV contains an unclosed quote; cell-level replacement is refused",
                None,
                false,
            ));
        }
        write_records(feeder.drain_finish())?;
        Ok(())
    }
}

impl crate::generic_apply::ContentRewriter for ReplaceRewriter {
    fn rewrite(
        &self,
        entry: &PlanEntry,
        input: &mut dyn Read,
        output: &mut dyn Write,
    ) -> Result<crate::generic_apply::EditOutcome, KernelError> {
        let structure = ContentStructure::resolve(&entry.source);
        if structure == ContentStructure::Csv && self.csv_mode == CsvMode::Cell {
            self.rewrite_cells(input, output)?;
            return Ok(crate::generic_apply::EditOutcome::default());
        }
        if self.mode == ReplaceMode::Plain {
            self.rewrite_plain(input, output)?;
            return Ok(crate::generic_apply::EditOutcome::default());
        }
        // regex：能算出匹配长度字节上界、且不会零宽匹配时走窗口化流式（ADR-0064）。
        if structure == ContentStructure::Text {
            if let Some((regex, window)) = self.streamable_regex() {
                self.rewrite_regex_windowed(&regex, window, input, output)?;
                return Ok(crate::generic_apply::EditOutcome::default());
            }
        }
        // 界定不出的正则（`*`/`+`/锚点/单词边界/分组量化/零宽匹配）仍走整段：
        // 窗口偏小的代价是静默改坏内容，整段的代价只是内存 O(文件)。
        let mut data = Vec::new();
        input.read_to_end(&mut data).map_err(rewrite_io_error)?;
        let replaced = replace_bytes(
            &data,
            self.mode,
            &self.find,
            &self.replacement,
            self.csv_mode,
            structure,
            self.delimiter,
        )?;
        output.write_all(&replaced).map_err(rewrite_io_error)?;
        // C03 暂无条目级明细可报（匹配计数在计划期已上报，ADR-0050）。
        Ok(crate::generic_apply::EditOutcome::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    /// 一条待对账的用例：臂的入参＋输入字节。写成结构体是因为 clippy 不接受八个参数的
    /// 测试助手，而不是因为逻辑需要它。
    struct Case {
        name: &'static str,
        data: Vec<u8>,
        structure: ContentStructure,
        csv_mode: CsvMode,
        mode: ReplaceMode,
        find: &'static str,
        replacement: &'static str,
        delimiter: Option<Delimiter>,
    }

    impl Case {
        fn whole(&self) -> ReplaceInspect {
            inspect_bytes(
                &self.data,
                self.mode,
                self.find,
                self.replacement,
                self.csv_mode,
                self.structure,
                self.delimiter,
            )
            .unwrap_or_else(|error| panic!("{}: 整段基准失败 {error:?}", self.name))
        }
    }

    /// 流式采集＋「按偏移有界回读窗口填上下文」的完整链路，就是壳层 `inspect_file` 要做的事。
    /// 用它把逐块结果逼回整段结果：任何一条臂、任何一个分块尺寸都必须逐字段相等。
    fn streamed_inspect(case: &Case, chunk: usize) -> ReplaceInspect {
        let started = ReplaceStreamInspector::start(
            case.structure,
            case.csv_mode,
            case.mode,
            case.find,
            case.replacement,
            case.delimiter,
        )
        .unwrap();
        let ReplaceScan::Streaming(mut inspector) = started else {
            panic!("这条臂必须走流式：{}", case.name);
        };
        for piece in case.data.chunks(chunk.max(1)) {
            inspector.push(piece);
        }
        let mut outcome = inspector.finish();
        if let Some(probe) = outcome.context_probe {
            let pad = CONTEXT_RADIUS_BYTES + CONTEXT_SLACK_BYTES;
            let window_start = probe.start.saturating_sub(pad);
            let window_end = (probe.start + probe.length + pad).min(outcome.text_len);
            let window = &case.data[window_start..window_end];
            outcome.inspect.context = Some(context_from_window(
                window,
                window_start,
                outcome.text_len,
                probe,
            ));
        }
        outcome.inspect
    }

    /// 文本臂用例的简写：structure/csv_mode 固定 Text/Text、不指定分隔符。
    fn text_case(
        name: &'static str,
        data: Vec<u8>,
        mode: ReplaceMode,
        find: &'static str,
        replacement: &'static str,
    ) -> Case {
        Case {
            name,
            data,
            structure: ContentStructure::Text,
            csv_mode: CsvMode::Text,
            mode,
            find,
            replacement,
            delimiter: None,
        }
    }

    /// 单元格臂用例的简写：分隔符固定逗号（嗅探分支由 platform 的用例覆盖）。
    fn cell_case(
        name: &'static str,
        data: Vec<u8>,
        mode: ReplaceMode,
        find: &'static str,
        replacement: &'static str,
    ) -> Case {
        Case {
            structure: ContentStructure::Csv,
            csv_mode: CsvMode::Cell,
            delimiter: Some(Delimiter::Comma),
            ..text_case(name, data, mode, find, replacement)
        }
    }

    #[test]
    fn streaming_plan_inspect_matches_whole_file_for_every_arm_and_chunk_size() {
        let mut straddle = vec![b'z'; 70_000];
        straddle.extend_from_slice(b"normal-row tail normal-row");
        let mut multibyte = vec![b'a'; 65_534];
        multibyte.extend_from_slice("\u{1F600} normal-row".as_bytes());
        let mut cell_rows = String::from("h1,h2\n");
        for index in 0..6_000 {
            cell_rows.push_str(&format!("{index},normal-row\n"));
        }
        let mut invalid_text = b"head normal-row ".to_vec();
        invalid_text.extend_from_slice(&[0xff, 0xfe]);
        let cases = vec![
            // 文本臂：字面串恰好跨块、多字节、非法字节、末端不完整、零命中、空文件。
            text_case(
                "plain-basic",
                b"xxnormal-rowyy normal-row\nlast normal-row".to_vec(),
                ReplaceMode::Plain,
                "normal-row",
                "normal_row",
            ),
            text_case(
                "plain-single-byte",
                b"a a a".to_vec(),
                ReplaceMode::Plain,
                "a",
                "b",
            ),
            text_case(
                "plain-overlap",
                b"aaaa aa aaaa".to_vec(),
                ReplaceMode::Plain,
                "aa",
                "bb",
            ),
            text_case(
                "plain-multibyte",
                "汉字漢 normal-row \u{1F600} normal-row \u{1F389}"
                    .as_bytes()
                    .to_vec(),
                ReplaceMode::Plain,
                "normal-row",
                "normal_row",
            ),
            text_case(
                "plain-boundary-cut",
                b"0123456789\xc3\xa9 normal-row tail".to_vec(),
                ReplaceMode::Plain,
                "normal-row",
                "X",
            ),
            text_case(
                "plain-invalid-utf8",
                invalid_text,
                ReplaceMode::Plain,
                "normal-row",
                "X",
            ),
            text_case(
                "plain-incomplete-tail",
                b"normal-row \xc3".to_vec(),
                ReplaceMode::Plain,
                "normal-row",
                "X",
            ),
            text_case(
                "plain-no-match",
                b"nothing here at all".to_vec(),
                ReplaceMode::Plain,
                "zzz",
                "X",
            ),
            text_case("plain-empty", Vec::new(), ReplaceMode::Plain, "a", "b"),
            text_case(
                "plain-first-at-zero",
                b"ab ab ab".to_vec(),
                ReplaceMode::Plain,
                "ab",
                "cd",
            ),
            text_case(
                "plain-cr-lf-context",
                b"row1\r\nnormal-row\r\nrow3".to_vec(),
                ReplaceMode::Plain,
                "normal-row",
                "X",
            ),
            text_case(
                "plain-long-context",
                "prefix ".repeat(30).into_bytes(),
                ReplaceMode::Plain,
                "fi",
                "FI",
            ),
            text_case(
                "plain-straddles-big-chunk",
                straddle,
                ReplaceMode::Plain,
                "normal-row",
                "normal_row",
            ),
            text_case(
                "plain-multibyte-straddle-64k",
                multibyte,
                ReplaceMode::Plain,
                "normal-row",
                "normal_row",
            ),
            // 正则臂：可界定的走流式（定长字符类、有界量词＋捕获组）。
            text_case(
                "regex-bounded",
                b"id 123456789 and 42 and 987654321".to_vec(),
                ReplaceMode::Regex,
                "[0-9]{9}",
                "000000000",
            ),
            text_case(
                "regex-group",
                b"a1b a22 a333".to_vec(),
                ReplaceMode::Regex,
                "([0-9]{1,3})",
                "[$1]",
            ),
            text_case(
                "regex-bounded-ascii",
                b"cat cat cat".to_vec(),
                ReplaceMode::Regex,
                "c[at]{2}",
                "dog",
            ),
            // 单元格臂：引号内换行、列数漂移、未闭合引号、末条无换行、非法字节、仅表头。
            cell_case(
                "cell-basic",
                b"h1,h2\n1,normal-row\n2,3\n".to_vec(),
                ReplaceMode::Plain,
                "normal-row",
                "normal_row",
            ),
            cell_case(
                "cell-multi-chunk",
                cell_rows.into_bytes(),
                ReplaceMode::Plain,
                "normal-row",
                "normal_row",
            ),
            cell_case(
                "cell-quoted-newline",
                b"h1,h2\n\"a\nb\",normal-row\n".to_vec(),
                ReplaceMode::Plain,
                "normal-row",
                "X",
            ),
            cell_case(
                "cell-drift",
                b"h1,h2\n1,2,3\n4\n".to_vec(),
                ReplaceMode::Plain,
                "1",
                "X",
            ),
            cell_case(
                "cell-unclosed-quote",
                b"h1,h2\n\"oops,2\n".to_vec(),
                ReplaceMode::Plain,
                "oops",
                "X",
            ),
            cell_case(
                "cell-no-trailing-newline",
                b"h1,h2\n1,normal-row".to_vec(),
                ReplaceMode::Plain,
                "normal-row",
                "X",
            ),
            cell_case(
                "cell-invalid-utf8",
                b"h1,h2\n\"\xff\",2\n".to_vec(),
                ReplaceMode::Plain,
                "h1",
                "X",
            ),
            cell_case(
                "cell-header-only",
                b"h1,h2\n".to_vec(),
                ReplaceMode::Plain,
                "h1",
                "X",
            ),
            cell_case("cell-empty", Vec::new(), ReplaceMode::Plain, "a", "b"),
            cell_case(
                "cell-regex",
                b"a,b\n123456789,x\n".to_vec(),
                ReplaceMode::Regex,
                "[0-9]{3}",
                "Z",
            ),
            cell_case(
                "cell-first-empty-record",
                b"\n\nh1,h2\n1,2\n".to_vec(),
                ReplaceMode::Plain,
                "h1",
                "X",
            ),
            // .csv 但按文本处理（--csv-mode text）：走文本臂，仍要求与整段版一致。
            Case {
                csv_mode: CsvMode::Text,
                ..cell_case(
                    "csv-as-text",
                    b"a,b\nnormal-row,c\n".to_vec(),
                    ReplaceMode::Plain,
                    "normal-row",
                    "X",
                )
            },
        ];
        for case in &cases {
            let want = case.whole();
            for chunk in [1usize, 2, 3, 5, 7, 13, 64, 4096, case.data.len().max(1)] {
                let got = streamed_inspect(case, chunk);
                assert_eq!(
                    got.match_count, want.match_count,
                    "{} chunk={chunk} 计数不符",
                    case.name
                );
                assert_eq!(
                    got.context, want.context,
                    "{} chunk={chunk} 上下文不符：want={:?} got={:?}",
                    case.name, want.context, got.context
                );
                assert_eq!(
                    got.unclosed_quote, want.unclosed_quote,
                    "{} chunk={chunk} 未闭合引号",
                    case.name
                );
                assert_eq!(
                    got.column_drift, want.column_drift,
                    "{} chunk={chunk} 列数漂移",
                    case.name
                );
                assert_eq!(
                    got.utf8_error, want.utf8_error,
                    "{} chunk={chunk} 非法 UTF-8",
                    case.name
                );
                assert_eq!(
                    got.delimiter, want.delimiter,
                    "{} chunk={chunk} 分隔符",
                    case.name
                );
                assert_eq!(
                    got.structure, want.structure,
                    "{} chunk={chunk} 结构",
                    case.name
                );
            }
        }
    }

    #[test]
    fn unbounded_regex_keeps_the_whole_file_plan_channel() {
        // 界定不出上界的正则不能窗口化：漏替换的代价比多占内存大（ADR-0064 同一条理由）。
        for pattern in ["a.*b", "\\d+", "^ab", "ab$", "\\bcat\\b", "(a)+", "(?:a|)"] {
            let started = ReplaceStreamInspector::start(
                ContentStructure::Text,
                CsvMode::Text,
                ReplaceMode::Regex,
                pattern,
                "X",
                None,
            )
            .unwrap();
            assert!(
                matches!(started, ReplaceScan::WholeFile),
                "模式 {pattern} 应退回整段采集"
            );
        }
        for pattern in ["[0-9]{3}", "abc", "a|bcde", "[a-c]{2,4}"] {
            let started = ReplaceStreamInspector::start(
                ContentStructure::Text,
                CsvMode::Text,
                ReplaceMode::Regex,
                pattern,
                "X",
                None,
            )
            .unwrap();
            assert!(
                matches!(started, ReplaceScan::Streaming(_)),
                "模式 {pattern} 应可窗口化流式"
            );
        }
    }

    #[test]
    fn cell_arm_without_delimiter_asks_for_the_sniff_pass() {
        // 分隔符未定时计划段先嗅探（与整段版同样看全文），壳层嗅探完带着结果重来。
        let started = ReplaceStreamInspector::start(
            ContentStructure::Csv,
            CsvMode::Cell,
            ReplaceMode::Plain,
            "a",
            "b",
            None,
        )
        .unwrap();
        assert!(matches!(started, ReplaceScan::SniffFirst));
    }

    #[test]
    fn plain_replacement_counts_and_replaces_non_overlapping() {
        let data = b"a a a";
        let inspect = inspect_bytes(
            data,
            ReplaceMode::Plain,
            "a",
            "b",
            CsvMode::Cell,
            ContentStructure::Text,
            None,
        )
        .unwrap();
        assert_eq!(inspect.match_count, 3);
        assert_eq!(
            replace_bytes(
                data,
                ReplaceMode::Plain,
                "a",
                "b",
                CsvMode::Cell,
                ContentStructure::Text,
                None
            )
            .unwrap(),
            b"b b b"
        );
    }

    #[test]
    fn regex_capture_groups_replace_and_count() {
        let data = b"a1 b2";
        let inspect = inspect_bytes(
            data,
            ReplaceMode::Regex,
            r"([ab])(\d)",
            "$2$1",
            CsvMode::Cell,
            ContentStructure::Text,
            None,
        )
        .unwrap();
        assert_eq!(inspect.match_count, 2);
        assert_eq!(
            replace_bytes(
                data,
                ReplaceMode::Regex,
                r"([ab])(\d)",
                "$2$1",
                CsvMode::Cell,
                ContentStructure::Text,
                None
            )
            .unwrap(),
            b"1a 2b"
        );
    }

    #[test]
    fn csv_cell_mode_replaces_inside_quoted_fields_without_breaking_structure() {
        let data = b"name,note\nA,\"x y\"\nB,\"line\r\nx y\"\n";
        let inspect = inspect_bytes(
            data,
            ReplaceMode::Plain,
            "x y",
            "z",
            CsvMode::Cell,
            ContentStructure::Csv,
            None,
        )
        .unwrap();
        assert_eq!(inspect.match_count, 2);
        let replaced = replace_bytes(
            data,
            ReplaceMode::Plain,
            "x y",
            "z",
            CsvMode::Cell,
            ContentStructure::Csv,
            None,
        )
        .unwrap();
        assert_eq!(
            replaced, b"name,note\nA,z\nB,\"line\r\nz\"\n",
            "quoted field newline is preserved; only cell text changes"
        );
        let records = parse_records(&replaced, b',');
        assert_eq!(records[2][1], "line\r\nz");
    }

    #[test]
    fn csv_unclosed_quote_is_reported_and_refused() {
        let data = b"a,\"b\n";
        let inspect = inspect_bytes(
            data,
            ReplaceMode::Plain,
            "b",
            "c",
            CsvMode::Cell,
            ContentStructure::Csv,
            Some(Delimiter::Comma),
        )
        .unwrap();
        assert!(inspect.unclosed_quote);
        let error = replace_bytes(
            data,
            ReplaceMode::Plain,
            "b",
            "c",
            CsvMode::Cell,
            ContentStructure::Csv,
            Some(Delimiter::Comma),
        )
        .unwrap_err();
        assert_eq!(error.code, "E3004");
    }

    #[test]
    fn csv_text_mode_does_not_parse_cells() {
        let data = b"a,\"b\"\n";
        let replaced = replace_bytes(
            data,
            ReplaceMode::Plain,
            "\"b\"",
            "c",
            CsvMode::Text,
            ContentStructure::Csv,
            None,
        )
        .unwrap();
        assert_eq!(replaced, b"a,c\n");
    }

    #[test]
    fn regex_invalid_or_missing_capture_is_rejected() {
        assert_eq!(
            validate_rule(ReplaceMode::Regex, "(", "x")
                .unwrap_err()
                .code,
            "E1002"
        );
        assert_eq!(
            validate_rule(ReplaceMode::Regex, r"^(\w+)$", "$2")
                .unwrap_err()
                .code,
            "E1002"
        );
    }

    #[test]
    fn empty_find_is_rejected() {
        assert_eq!(
            validate_rule(ReplaceMode::Plain, "", "x").unwrap_err().code,
            "E1002"
        );
    }

    fn file(path: &str, sample: &[u8], inspect: ReplaceInspect) -> PlanC03File {
        PlanC03File {
            path: path.to_string(),
            canonical_path: None,
            file_size: sample.len() as u64,
            sample: sample.to_vec(),
            structure: inspect.structure,
            inspect,
            readonly: false,
            locked: false,
        }
    }

    fn input(files: Vec<PlanC03File>) -> PlanC03Input {
        PlanC03Input {
            plan_id: PLAN_ID.to_string(),
            mode: ReplaceMode::Plain,
            find: "a".to_string(),
            replacement: "b".to_string(),
            csv_mode: CsvMode::Cell,
            files,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            confirmed_source: None,
            prior: PlatformPrior::Other,
            delimiter: None,
        }
    }

    #[test]
    fn plan_skips_no_matches_and_creates_entries_for_matches() {
        let plan = plan_c03(input(vec![
            file(
                "/data/a.txt",
                b"a a",
                ReplaceInspect {
                    structure: ContentStructure::Text,
                    match_count: 2,
                    context: Some("a a".to_string()),
                    ..ReplaceInspect::default()
                },
            ),
            file(
                "/data/b.txt",
                b"zzz",
                ReplaceInspect {
                    structure: ContentStructure::Text,
                    match_count: 0,
                    ..ReplaceInspect::default()
                },
            ),
        ]))
        .unwrap();
        assert_eq!(plan.kind, C03_KIND);
        assert_eq!(plan.entries.len(), 2);
        assert!(plan.entries[0].resolution.is_none());
        assert_eq!(plan.entries[1].resolution, Some(PlanResolution::Skip));
    }

    #[test]
    fn readonly_and_locked_files_are_named_in_preview_warnings() {
        let matched = || ReplaceInspect {
            structure: ContentStructure::Text,
            match_count: 1,
            context: Some("a".to_string()),
            ..ReplaceInspect::default()
        };
        let read_only = PlanC03File {
            readonly: true,
            ..file("/data/a.txt", b"a", matched())
        };
        let held_open = PlanC03File {
            locked: true,
            ..file("/data/b.txt", b"a", matched())
        };
        let plan = plan_c03(input(vec![read_only, held_open])).unwrap();
        let named = |kind: &str| {
            plan.warnings
                .iter()
                .find(|warning| warning.kind == kind)
                .and_then(|warning| warning.suggestion.clone())
        };
        assert!(
            named("readonly")
                .as_deref()
                .is_some_and(|text| text.contains("/data/a.txt")),
            "只读文件必须在预览期点名：{:?}",
            plan.warnings
        );
        assert!(
            named("locked")
                .as_deref()
                .is_some_and(|text| text.contains("/data/b.txt")),
            "被占用文件必须在预览期点名：{:?}",
            plan.warnings
        );
        assert_eq!(plan.entries.len(), 2);
        assert!(plan.entries.iter().all(|entry| entry.blocked.is_none()));
    }

    #[test]
    fn blocked_files_are_not_named_as_readonly() {
        // 只读点名只覆盖"真的会被改写"的文件：二进制文件在 E3001 处已阻断，
        // 再点一次只读等于把两个不相干的结论混进同一条告警。
        let blocked_read_only = PlanC03File {
            readonly: true,
            ..file(
                "/data/image.txt",
                b"\x89PNG\r\n\x1a\nrest",
                ReplaceInspect {
                    structure: ContentStructure::Text,
                    ..ReplaceInspect::default()
                },
            )
        };
        let plan = plan_c03(input(vec![blocked_read_only])).unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3001");
        assert!(
            !plan
                .warnings
                .iter()
                .any(|warning| warning.kind == "readonly"),
            "被阻断的文件不得点名只读：{:?}",
            plan.warnings
        );
    }

    #[test]
    fn plan_blocks_binary_and_non_utf8_sources() {
        let plan = plan_c03(input(vec![
            file(
                "/data/image.txt",
                b"\x89PNG\r\n\x1a\nrest",
                ReplaceInspect {
                    structure: ContentStructure::Text,
                    ..ReplaceInspect::default()
                },
            ),
            file(
                "/data/gbk.txt",
                b"\xd6\xd0\xce\xc4",
                ReplaceInspect {
                    structure: ContentStructure::Text,
                    ..ReplaceInspect::default()
                },
            ),
        ]))
        .unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3001");
        assert_eq!(plan.entries[1].blocked.as_ref().unwrap().code, "E3001");
    }

    #[test]
    fn plan_blocks_unclosed_csv_quote_in_cell_mode() {
        let plan = plan_c03(input(vec![file(
            "/data/a.csv",
            b"a,\"b\n",
            ReplaceInspect {
                structure: ContentStructure::Csv,
                delimiter: Some(Delimiter::Comma),
                unclosed_quote: true,
                match_count: 1,
                ..ReplaceInspect::default()
            },
        )]))
        .unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3004");
    }

    #[test]
    fn empty_replacement_is_a_danger_warning() {
        let mut input = input(vec![file(
            "/data/a.txt",
            b"a",
            ReplaceInspect {
                structure: ContentStructure::Text,
                match_count: 1,
                ..ReplaceInspect::default()
            },
        )]);
        input.replacement = String::new();
        let plan = plan_c03(input).unwrap();
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "empty-replacement"));
    }
    /// 按固定步长吐字节的 `Read`：强制把匹配、替换文本与多字节字符切开，
    /// 用来验证流式路径与整段路径逐字节一致（ADR-0057 的护栏）。
    struct Dripping {
        data: Vec<u8>,
        step: usize,
        pos: usize,
    }

    impl Read for Dripping {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let take = self.step.min(out.len()).min(self.data.len() - self.pos);
            out[..take].copy_from_slice(&self.data[self.pos..self.pos + take]);
            self.pos += take;
            Ok(take)
        }
    }

    fn rewrite_entry(source: &str) -> PlanEntry {
        PlanEntry {
            entry_id: "e1".to_string(),
            source: source.to_string(),
            target: None,
            action: PlanAction::EditContent,
            relation: None,
            reason: "replace".to_string(),
            conflict: None,
            resolution: None,
            blocked: None,
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
        }
    }

    fn rewrite_with(
        rewriter: &ReplaceRewriter,
        entry: &PlanEntry,
        data: &[u8],
        step: usize,
    ) -> Result<Vec<u8>, KernelError> {
        let mut input = Dripping {
            data: data.to_vec(),
            step,
            pos: 0,
        };
        let mut output = Vec::new();
        crate::generic_apply::ContentRewriter::rewrite(rewriter, entry, &mut input, &mut output)?;
        Ok(output)
    }

    #[test]
    fn plain_streaming_matches_whole_file_replace_across_every_chunk_boundary() {
        // "中ab" 反复：find 跨块、且多字节字符会被 1 字节步长切开。
        let data: Vec<u8> = "中ab".repeat(400).into_bytes();
        let entry = rewrite_entry("/data/a.txt");
        let rewriter = ReplaceRewriter::new(
            ReplaceMode::Plain,
            "中ab".to_string(),
            "X".to_string(),
            CsvMode::Cell,
            None,
        );
        let whole = replace_bytes(
            &data,
            ReplaceMode::Plain,
            "中ab",
            "X",
            CsvMode::Cell,
            ContentStructure::Text,
            None,
        )
        .unwrap();
        for step in [1usize, 2, 3, 7, 64 * 1024] {
            let streamed = rewrite_with(&rewriter, &entry, &data, step).unwrap();
            assert_eq!(
                streamed, whole,
                "step {step} diverged from the whole-file path"
            );
        }
    }

    #[test]
    fn regex_window_bytes_only_accepts_patterns_with_a_computable_bound() {
        // 算得出上界：字面串、定长类、有界重复。
        for pattern in ["ab", "[0-9]{3}", "a{2,5}", "\\d{4}", "(ab|cdef)"] {
            assert!(
                regex_window_bytes(pattern).is_some(),
                "{pattern} 应当算得出字节上界"
            );
        }
        // 上界放大值必须真的 ≥ 任何可能匹配的字节长度。
        assert!(regex_window_bytes("[0-9]{3}").unwrap() >= 3 * 4);
        // 界定不出／语义依赖整段上下文：一律 None（＝退回整段）。
        for pattern in [
            "a.*b",      // 无界
            "\\d+",      // 无界
            "x{2,}",     // 开顶上界
            "foo$",      // 块尾会被当成文本尾
            "^\\s",      // 块首会被当成文本首
            "\\bcat\\b", // 左邻字节不在窗口内
            "(?i)abc",   // 内联标志组
            "(ab)+",     // 分组量化
            "[a-z",      // 非法/看不懂的构造
        ] {
            assert_eq!(
                regex_window_bytes(pattern),
                None,
                "{pattern} 不该被窗口化：窗口偏小的代价是静默改坏内容"
            );
        }
    }

    #[test]
    fn regex_streaming_matches_whole_file_replace_across_every_chunk_boundary() {
        // 捕获引用 ＋ 多字节字符 ＋ 匹配横跨块边界：窗口化路径必须与整段逐字节一致。
        let data: Vec<u8> = "价格12元;中ab;价789元".repeat(300).into_bytes();
        let entry = rewrite_entry("/data/a.txt");
        for (pattern, replacement) in [
            ("价格[0-9]{1,3}元", "COST"),
            ("[0-9]{3}", "999"),
            ("中a.", "★"),
        ] {
            let rewriter = ReplaceRewriter::new(
                ReplaceMode::Regex,
                pattern.to_string(),
                replacement.to_string(),
                CsvMode::Cell,
                None,
            );
            let whole = replace_bytes(
                &data,
                ReplaceMode::Regex,
                pattern,
                replacement,
                CsvMode::Cell,
                ContentStructure::Text,
                None,
            )
            .unwrap();
            for step in [1usize, 2, 5, 64 * 1024] {
                let streamed = rewrite_with(&rewriter, &entry, &data, step).unwrap();
                assert_eq!(
                    streamed, whole,
                    "pattern {pattern} 在 step {step} 与整段路径不一致"
                );
            }
        }
    }

    #[test]
    fn regex_with_capture_reference_streams_identically_to_whole_file() {
        // `$1` 展开在两条路径必须同源：窗口化臂复用 `regex.replace`，不另写展开逻辑。
        let data: Vec<u8> = "id=0123;".repeat(500).into_bytes();
        let entry = rewrite_entry("/data/a.txt");
        let rewriter = ReplaceRewriter::new(
            ReplaceMode::Regex,
            "id=([0-9]{4})".to_string(),
            "#$1#".to_string(),
            CsvMode::Cell,
            None,
        );
        let whole = replace_bytes(
            &data,
            ReplaceMode::Regex,
            "id=([0-9]{4})",
            "#$1#",
            CsvMode::Cell,
            ContentStructure::Text,
            None,
        )
        .unwrap();
        for step in [1usize, 3, 64 * 1024] {
            assert_eq!(
                rewrite_with(&rewriter, &entry, &data, step).unwrap(),
                whole,
                "捕获引用在 step {step} 与整段不一致"
            );
        }
    }

    #[test]
    fn anchored_and_unbounded_regex_still_produce_whole_file_results() {
        // 界定不出的模式退回整段后，结果与旧行为一致（这里断言的是"没被窗口化带跑"）。
        let data: Vec<u8> = "aaa\nbbb\naaa".as_bytes().to_vec();
        let entry = rewrite_entry("/data/a.txt");
        for pattern in ["a.*b", "^bbb$", "\\baaa\\b"] {
            let rewriter = ReplaceRewriter::new(
                ReplaceMode::Regex,
                pattern.to_string(),
                "X".to_string(),
                CsvMode::Cell,
                None,
            );
            let whole = replace_bytes(
                &data,
                ReplaceMode::Regex,
                pattern,
                "X",
                CsvMode::Cell,
                ContentStructure::Text,
                None,
            )
            .unwrap();
            assert_eq!(
                rewrite_with(&rewriter, &entry, &data, 2).unwrap(),
                whole,
                "{pattern} 退回整段后结果必须不变"
            );
        }
    }

    #[test]
    fn plain_streaming_must_not_split_a_multibyte_character_at_the_overlap_edge() {
        // 复现：find 是 6 字节的多字节串，overlap=5；整块读取时 `len - 5` 很可能落在
        // 「中」的第 2/3 个延续字节上——按字节消费会把一个字符劈成"半字符入输出、
        // 半字符留缓冲"，下一块再从 UTF-8 校验里冒出假的 E3001。
        let data: Vec<u8> = "中".repeat(300_000).into_bytes();
        let entry = rewrite_entry("/data/a.txt");
        let rewriter = ReplaceRewriter::new(
            ReplaceMode::Plain,
            "中中".to_string(),
            "X".to_string(),
            CsvMode::Cell,
            None,
        );
        let whole = replace_bytes(
            &data,
            ReplaceMode::Plain,
            "中中",
            "X",
            CsvMode::Cell,
            ContentStructure::Text,
            None,
        )
        .unwrap();
        // 块大小取 3 的倍数：缓冲区整块都是合法 UTF-8，此时 `len - overlap` 才会
        // 真的落在「中」的延续字节上（块大小取非倍数时，不完整的尾巴会被
        // `valid_up_to` 先兜住，看不见这个缺陷）。
        for step in [3072usize, 3 * 64 * 1024] {
            let streamed = rewrite_with(&rewriter, &entry, &data, step).unwrap();
            assert_eq!(
                streamed, whole,
                "step {step}：安全边界必须回退到字符边界，否则半个字符会被劈开"
            );
        }
    }

    #[test]
    fn plain_streaming_does_not_split_when_the_find_never_matches() {
        // 无匹配时 `cursor` 停在 0，写出的是 `buffer[0..safe]`——这条分支才是
        // "安全边界落在字符中间"最直接的暴露面。
        let data: Vec<u8> = "中".repeat(100_000).into_bytes();
        let entry = rewrite_entry("/data/a.txt");
        let rewriter = ReplaceRewriter::new(
            ReplaceMode::Plain,
            "qq".to_string(),
            "X".to_string(),
            CsvMode::Cell,
            None,
        );
        let streamed = rewrite_with(&rewriter, &entry, &data, 3072).unwrap();
        assert_eq!(streamed, data, "整块合法时按字节退 overlap 会劈开字符");
    }

    #[test]
    fn plain_streaming_keeps_e3001_on_invalid_utf8_and_handles_empty_input() {
        let entry = rewrite_entry("/data/a.txt");
        let rewriter = ReplaceRewriter::new(
            ReplaceMode::Plain,
            "ab".to_string(),
            "X".to_string(),
            CsvMode::Cell,
            None,
        );
        let error = rewrite_with(&rewriter, &entry, b"ab\xffab", 2).unwrap_err();
        assert_eq!(error.code, "E3001");
        assert_eq!(rewrite_with(&rewriter, &entry, b"", 3).unwrap(), b"");
    }

    #[test]
    fn cell_streaming_matches_whole_file_cell_replace_across_every_chunk_boundary() {
        // 引号内换行 + 逗号 + 中文：记录边界与多字节字符都被 1/2/3 字节步长切开。
        let data = "id,note\n1,\"多行\n中文,值\"\n2,plain\n"
            .as_bytes()
            .to_vec();
        let entry = rewrite_entry("/data/a.csv");
        let rewriter = ReplaceRewriter::new(
            ReplaceMode::Plain,
            "中".to_string(),
            "C".to_string(),
            CsvMode::Cell,
            Some(Delimiter::Comma),
        );
        let whole = replace_bytes(
            &data,
            ReplaceMode::Plain,
            "中",
            "C",
            CsvMode::Cell,
            ContentStructure::Csv,
            Some(Delimiter::Comma),
        )
        .unwrap();
        for step in [1usize, 2, 3, 5, 64 * 1024] {
            let streamed = rewrite_with(&rewriter, &entry, &data, step).unwrap();
            assert_eq!(
                streamed, whole,
                "step {step} diverged from the whole-file path"
            );
        }
    }

    #[test]
    fn cell_streaming_still_refuses_unclosed_quotes_with_e3004() {
        let entry = rewrite_entry("/data/a.csv");
        let rewriter = ReplaceRewriter::new(
            ReplaceMode::Plain,
            "a".to_string(),
            "b".to_string(),
            CsvMode::Cell,
            Some(Delimiter::Comma),
        );
        let error = rewrite_with(&rewriter, &entry, b"id,note\n1,\"open\n", 2).unwrap_err();
        assert_eq!(error.code, "E3004");
    }
}
