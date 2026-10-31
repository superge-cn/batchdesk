//!
//! C03 批量查找替换计划与内容变换（方案 §5.14）。
//!
//! 本切片支持 TXT/CSV 的纯文本与正则替换、CSV 单元格级默认保护、匹配计数与
//! 预览上下文。执行走 `ContentRewriter`（整段读写；大文件流式窗口后续补齐）。

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
}

impl crate::generic_apply::ContentRewriter for ReplaceRewriter {
    fn rewrite(
        &self,
        entry: &PlanEntry,
        input: &mut dyn Read,
        output: &mut dyn Write,
    ) -> Result<(), KernelError> {
        let mut data = Vec::new();
        input.read_to_end(&mut data).map_err(|error| {
            KernelError::new(
                "E3005",
                "content rewrite failed",
                Some(error.to_string()),
                false,
            )
        })?;
        let structure = ContentStructure::resolve(&entry.source);
        let replaced = replace_bytes(
            &data,
            self.mode,
            &self.find,
            &self.replacement,
            self.csv_mode,
            structure,
            self.delimiter,
        )?;
        output.write_all(&replaced).map_err(|error| {
            KernelError::new(
                "E3005",
                "content rewrite failed",
                Some(error.to_string()),
                false,
            )
        })
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
}
