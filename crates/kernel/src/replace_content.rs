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

fn context_snippet(text: &str, start: usize, length: usize) -> String {
    let mut begin = start.saturating_sub(20);
    while begin > 0 && !text.is_char_boundary(begin) {
        begin -= 1;
    }
    let mut end = (start + length + 20).min(text.len());
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    let snippet = text[begin..end].replace(['\r', '\n'], " ");
    if snippet.chars().count() > 60 {
        let truncated: String = snippet.chars().take(60).collect();
        format!("{truncated}…")
    } else {
        snippet
    }
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
