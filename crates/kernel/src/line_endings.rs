//!
//! C02 换行符转换计划与流式处理器（方案 §5.15）。
//!
//! 统一 CRLF/LF/CR；CSV 只转换引号外的记录终止符，引号内换行按数据保留。
//! 计划层纯函数；执行层经 `ContentRewriter` 注入流式读写，不整文件读入内存。

use std::collections::BTreeMap;
use std::io::{Read, Write};

use crate::binary;
use crate::csv_merge::source_block;
use crate::encoding::{detect, PlatformPrior, SourceEncoding};
use crate::error::KernelError;
use crate::fs::CaseSense;
use crate::path::ProtectedDir;
use crate::plan::{
    is_ulid, PlanAction, PlanBlocked, PlanDocument, PlanEntry, PlanExpected, PlanParamValue,
    PlanResolution, PlanStats, PlanWarning, RiskLevel,
};

pub const C02_KIND: &str = "c02";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Crlf,
    Lf,
    Cr,
}

impl LineEnding {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Crlf => "crlf",
            Self::Lf => "lf",
            Self::Cr => "cr",
        }
    }

    pub const fn bytes(self) -> &'static [u8] {
        match self {
            Self::Crlf => b"\r\n",
            Self::Lf => b"\n",
            Self::Cr => b"\r",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "crlf" => Some(Self::Crlf),
            "lf" => Some(Self::Lf),
            "cr" => Some(Self::Cr),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEndingStructure {
    Text,
    Csv,
}

impl LineEndingStructure {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Csv => "csv",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEndingStructureMode {
    Auto,
    Text,
    Csv,
}

impl LineEndingStructureMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Text => "text",
            Self::Csv => "csv",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "text" => Some(Self::Text),
            "csv" => Some(Self::Csv),
            _ => None,
        }
    }

    /// `auto` 按扩展名判断；`.csv`/`.tsv` 视为结构化 CSV。
    pub fn resolve(self, path: &str) -> LineEndingStructure {
        match self {
            Self::Text => LineEndingStructure::Text,
            Self::Csv => LineEndingStructure::Csv,
            Self::Auto => {
                let name = crate::conflict::file_name_of(path);
                let extension = name.rsplit_once('.').map(|(_, extension)| extension);
                match extension {
                    Some(extension) if extension.eq_ignore_ascii_case("csv") => {
                        LineEndingStructure::Csv
                    }
                    Some(extension) if extension.eq_ignore_ascii_case("tsv") => {
                        LineEndingStructure::Csv
                    }
                    _ => LineEndingStructure::Text,
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineEndingCounts {
    pub crlf: u64,
    pub lf: u64,
    pub cr: u64,
}

impl LineEndingCounts {
    pub const fn total(self) -> u64 {
        self.crlf + self.lf + self.cr
    }

    pub fn mixed(self) -> bool {
        [self.crlf, self.lf, self.cr]
            .iter()
            .filter(|count| **count > 0)
            .count()
            > 1
    }

    pub fn only(self, target: LineEnding) -> bool {
        self.total() > 0
            && match target {
                LineEnding::Crlf => self.crlf > 0 && self.lf == 0 && self.cr == 0,
                LineEnding::Lf => self.lf > 0 && self.crlf == 0 && self.cr == 0,
                LineEnding::Cr => self.cr > 0 && self.crlf == 0 && self.lf == 0,
            }
    }
}

/// 流式换行处理器：`target=None` 只统计，`target=Some` 同时转换。
pub struct LineEndingProcessor {
    structure: LineEndingStructure,
    target: Option<LineEnding>,
    in_quotes: bool,
    pending_cr: bool,
    pending_quote: bool,
    counts: LineEndingCounts,
}

impl LineEndingProcessor {
    pub fn new(structure: LineEndingStructure, target: Option<LineEnding>) -> Self {
        Self {
            structure,
            target,
            in_quotes: false,
            pending_cr: false,
            pending_quote: false,
            counts: LineEndingCounts::default(),
        }
    }

    pub const fn counts(&self) -> LineEndingCounts {
        self.counts
    }

    pub fn feed(&mut self, input: &[u8], out: &mut dyn Write) -> Result<(), KernelError> {
        let mut index = 0;
        if self.pending_quote {
            self.pending_quote = false;
            if input.first() == Some(&b'"') {
                write_out(out, b"\"\"")?;
                index += 1;
            } else {
                self.in_quotes = false;
                write_out(out, b"\"")?;
            }
        }
        while index < input.len() {
            let byte = input[index];
            if self.pending_cr {
                self.pending_cr = false;
                if byte == b'\n' {
                    self.emit_newline(LineEnding::Crlf, out)?;
                    index += 1;
                    continue;
                }
                self.emit_newline(LineEnding::Cr, out)?;
            }
            match byte {
                b'\r' => {
                    self.pending_cr = true;
                    index += 1;
                }
                b'\n' => {
                    self.emit_newline(LineEnding::Lf, out)?;
                    index += 1;
                }
                b'"' if self.structure == LineEndingStructure::Csv => {
                    index += self.handle_quote(input, index, out)?;
                }
                _ => {
                    let start = index;
                    let mut end = index + 1;
                    while end < input.len() && !self.is_special(input[end]) {
                        end += 1;
                    }
                    write_out(out, &input[start..end])?;
                    index = end;
                }
            }
        }
        Ok(())
    }

    pub fn finish(&mut self, out: &mut dyn Write) -> Result<(), KernelError> {
        if self.pending_cr {
            self.pending_cr = false;
            self.emit_newline(LineEnding::Cr, out)?;
        }
        if self.pending_quote {
            self.pending_quote = false;
            write_out(out, b"\"")?;
        }
        Ok(())
    }

    fn handle_quote(
        &mut self,
        input: &[u8],
        index: usize,
        out: &mut dyn Write,
    ) -> Result<usize, KernelError> {
        if self.in_quotes {
            if input.get(index + 1) == Some(&b'"') {
                write_out(out, b"\"\"")?;
                return Ok(2);
            }
            if index + 1 == input.len() {
                self.pending_quote = true;
                return Ok(1);
            }
            self.in_quotes = false;
            write_out(out, b"\"")?;
        } else {
            self.in_quotes = true;
            write_out(out, b"\"")?;
        }
        Ok(1)
    }

    fn is_special(&self, byte: u8) -> bool {
        byte == b'\r'
            || byte == b'\n'
            || (self.structure == LineEndingStructure::Csv && byte == b'"')
    }

    fn emit_newline(&mut self, kind: LineEnding, out: &mut dyn Write) -> Result<(), KernelError> {
        let inside_quotes = self.structure == LineEndingStructure::Csv && self.in_quotes;
        if !inside_quotes {
            match kind {
                LineEnding::Crlf => self.counts.crlf += 1,
                LineEnding::Lf => self.counts.lf += 1,
                LineEnding::Cr => self.counts.cr += 1,
            }
        }
        let bytes = if inside_quotes {
            kind.bytes()
        } else {
            self.target.unwrap_or(kind).bytes()
        };
        write_out(out, bytes)
    }
}

fn write_out(out: &mut dyn Write, bytes: &[u8]) -> Result<(), KernelError> {
    out.write_all(bytes).map_err(|error| {
        KernelError::new(
            "E3005",
            "content rewrite failed",
            Some(error.to_string()),
            false,
        )
    })
}

/// C02 的流式重写器：从计划参数构造，按条目路径解析 CSV/文本结构。
pub struct LineEndingRewriter {
    target: LineEnding,
    structure: LineEndingStructureMode,
}

impl LineEndingRewriter {
    pub const fn new(target: LineEnding, structure: LineEndingStructureMode) -> Self {
        Self { target, structure }
    }
}

impl crate::generic_apply::ContentRewriter for LineEndingRewriter {
    fn rewrite(
        &self,
        entry: &PlanEntry,
        input: &mut dyn Read,
        output: &mut dyn Write,
    ) -> Result<(), KernelError> {
        let structure = self.structure.resolve(&entry.source);
        let mut processor = LineEndingProcessor::new(structure, Some(self.target));
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = input.read(&mut buffer).map_err(|error| {
                KernelError::new(
                    "E3005",
                    "content rewrite failed",
                    Some(error.to_string()),
                    false,
                )
            })?;
            if read == 0 {
                break;
            }
            processor.feed(&buffer[..read], output)?;
        }
        processor.finish(output)
    }
}

#[derive(Clone, Debug)]
pub struct PlanC02File {
    pub path: String,
    pub canonical_path: Option<String>,
    pub file_size: u64,
    pub sample: Vec<u8>,
    pub counts: LineEndingCounts,
    pub structure: LineEndingStructure,
}

#[derive(Clone, Debug)]
pub struct PlanC02Input {
    pub plan_id: String,
    pub target: LineEnding,
    pub structure: LineEndingStructureMode,
    pub files: Vec<PlanC02File>,
    pub protected_dirs: Vec<ProtectedDir>,
    pub case_sense: CaseSense,
    pub confirmed_source: Option<SourceEncoding>,
    pub prior: PlatformPrior,
}

pub fn plan_c02(input: PlanC02Input) -> Result<PlanDocument, KernelError> {
    if input.plan_id.trim().is_empty() {
        return Err(KernelError::missing_param("planId is required"));
    }
    if !is_ulid(&input.plan_id) {
        return Err(KernelError::invalid_params(
            "planId must be a 26-character ULID",
        ));
    }
    if input.files.is_empty() {
        return Err(KernelError::missing_param("c02 requires at least one file"));
    }

    let mut params = BTreeMap::new();
    params.insert(
        "target".to_string(),
        PlanParamValue::String(input.target.as_str().to_string()),
    );
    params.insert(
        "structure".to_string(),
        PlanParamValue::String(input.structure.as_str().to_string()),
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
    let mut mixed = false;
    let mut no_line_endings = false;
    let mut binary_skipped = false;

    for file in &input.files {
        let mut blocked = source_block(
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
            match selected {
                Some(SourceEncoding::Utf16Le | SourceEncoding::Utf16Be) => {
                    blocked = Some(PlanBlocked {
                        code: "E3001".to_string(),
                        reason: "UTF-16 line-ending conversion is not supported in this slice"
                            .to_string(),
                        hint: Some("convert the file to UTF-8 first (C01)".to_string()),
                    });
                }
                Some(_) => {}
                None => {
                    let detected = detection.encoding.map_or("unknown", SourceEncoding::as_str);
                    blocked = Some(PlanBlocked {
                        code: "E3001".to_string(),
                        reason: format!("encoding is undetermined (detected {detected})"),
                        hint: Some("choose the source encoding explicitly".to_string()),
                    });
                }
            }
        }

        let (reason, resolution) = match &blocked {
            Some(block) => (block.reason.clone(), None),
            None if file.counts.total() == 0 => {
                no_line_endings = true;
                (
                    "no line endings found".to_string(),
                    Some(PlanResolution::Skip),
                )
            }
            None if file.counts.only(input.target) => (
                format!("already uses {} line endings", input.target.as_str()),
                Some(PlanResolution::Skip),
            ),
            None => {
                if file.counts.mixed() {
                    mixed = true;
                }
                (
                    format!(
                        "{}; crlf={} lf={} cr={} -> {}",
                        file.structure.as_str(),
                        file.counts.crlf,
                        file.counts.lf,
                        file.counts.cr,
                        input.target.as_str()
                    ),
                    None,
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
        });
    }

    let mut warnings = Vec::new();
    if mixed {
        warnings.push(PlanWarning {
            kind: "mixed-line-endings".to_string(),
            suggestion: Some("source files use more than one line-ending form".to_string()),
            risk_level: RiskLevel::Caution,
        });
    }
    if no_line_endings {
        warnings.push(PlanWarning {
            kind: "no-line-endings".to_string(),
            suggestion: Some("some files contain no line endings; they are skipped".to_string()),
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
        kind: C02_KIND.to_string(),
        params,
        entries,
        stats,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(input: &[u8], structure: LineEndingStructure) -> LineEndingCounts {
        let mut processor = LineEndingProcessor::new(structure, None);
        let mut sink = std::io::sink();
        processor.feed(input, &mut sink).unwrap();
        processor.finish(&mut sink).unwrap();
        processor.counts()
    }

    fn convert(input: &[u8], structure: LineEndingStructure, target: LineEnding) -> Vec<u8> {
        let mut processor = LineEndingProcessor::new(structure, Some(target));
        let mut output = Vec::new();
        processor.feed(input, &mut output).unwrap();
        processor.finish(&mut output).unwrap();
        output
    }

    #[test]
    fn text_mixed_newlines_are_counted_and_converted() {
        let input = b"a\r\nb\nc\rd";
        assert_eq!(
            counts(input, LineEndingStructure::Text),
            LineEndingCounts {
                crlf: 1,
                lf: 1,
                cr: 1
            }
        );
        assert_eq!(
            convert(input, LineEndingStructure::Text, LineEnding::Lf),
            b"a\nb\nc\nd"
        );
        assert_eq!(
            convert(input, LineEndingStructure::Text, LineEnding::Crlf),
            b"a\r\nb\r\nc\r\nd"
        );
        assert_eq!(
            convert(input, LineEndingStructure::Text, LineEnding::Cr),
            b"a\rb\rc\rd"
        );
    }

    #[test]
    fn crlf_split_across_chunks_is_converted_once() {
        let mut processor =
            LineEndingProcessor::new(LineEndingStructure::Text, Some(LineEnding::Lf));
        let mut output = Vec::new();
        processor.feed(b"a\r", &mut output).unwrap();
        processor.feed(b"\nb", &mut output).unwrap();
        processor.finish(&mut output).unwrap();
        assert_eq!(output, b"a\nb");
        assert_eq!(processor.counts().crlf, 1);
    }

    #[test]
    fn csv_quoted_newlines_are_preserved_and_record_endings_converted() {
        let input = b"a,\"b\r\nc\",d\r\nx,y\r\n";
        assert_eq!(
            counts(input, LineEndingStructure::Csv),
            LineEndingCounts {
                crlf: 2,
                lf: 0,
                cr: 0
            },
            "quoted CRLF is data, only record terminators count"
        );
        assert_eq!(
            convert(input, LineEndingStructure::Csv, LineEnding::Lf),
            b"a,\"b\r\nc\",d\nx,y\n"
        );
        assert_eq!(
            convert(input, LineEndingStructure::Csv, LineEnding::Crlf),
            input,
            "already-target record endings stay unchanged; quoted CRLF is preserved"
        );
    }

    #[test]
    fn csv_quoted_lf_is_preserved_when_target_is_crlf() {
        let input = b"a,\"b\nc\"\nx";
        assert_eq!(
            convert(input, LineEndingStructure::Csv, LineEnding::Crlf),
            b"a,\"b\nc\"\r\nx"
        );
    }

    #[test]
    fn csv_escaped_quotes_across_chunk_boundary_are_preserved() {
        let mut processor =
            LineEndingProcessor::new(LineEndingStructure::Csv, Some(LineEnding::Lf));
        let mut output = Vec::new();
        for byte in b"a,\"b\"\"c\"\r\nx" {
            processor.feed(&[*byte], &mut output).unwrap();
        }
        processor.finish(&mut output).unwrap();
        assert_eq!(output, b"a,\"b\"\"c\"\nx");
    }

    #[test]
    fn file_without_final_newline_keeps_that_property() {
        assert_eq!(
            convert(b"a\r\nb", LineEndingStructure::Text, LineEnding::Lf),
            b"a\nb"
        );
    }

    #[test]
    fn utf8_bom_is_preserved() {
        assert_eq!(
            convert(
                b"\xef\xbb\xbfa\r\n",
                LineEndingStructure::Text,
                LineEnding::Lf
            ),
            b"\xef\xbb\xbfa\n"
        );
    }

    #[test]
    fn structure_mode_resolves_by_extension() {
        assert_eq!(
            LineEndingStructureMode::Auto.resolve("/tmp/a.CSV"),
            LineEndingStructure::Csv
        );
        assert_eq!(
            LineEndingStructureMode::Auto.resolve("/tmp/a.tsv"),
            LineEndingStructure::Csv
        );
        assert_eq!(
            LineEndingStructureMode::Auto.resolve("/tmp/a.txt"),
            LineEndingStructure::Text
        );
        assert_eq!(
            LineEndingStructureMode::Text.resolve("/tmp/a.csv"),
            LineEndingStructure::Text
        );
    }

    fn file(path: &str, sample: &[u8], counts: LineEndingCounts) -> PlanC02File {
        PlanC02File {
            path: path.to_string(),
            canonical_path: None,
            file_size: sample.len() as u64,
            sample: sample.to_vec(),
            counts,
            structure: LineEndingStructure::Text,
        }
    }

    fn input(files: Vec<PlanC02File>) -> PlanC02Input {
        PlanC02Input {
            plan_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            target: LineEnding::Lf,
            structure: LineEndingStructureMode::Text,
            files,
            protected_dirs: Vec::new(),
            case_sense: CaseSense::Sensitive,
            confirmed_source: None,
            prior: PlatformPrior::Other,
        }
    }

    #[test]
    fn plan_converts_mixed_files_and_warns() {
        let plan = plan_c02(input(vec![file(
            "/data/a.txt",
            b"a\r\nb\n",
            LineEndingCounts {
                crlf: 1,
                lf: 1,
                cr: 0,
            },
        )]))
        .unwrap();
        assert_eq!(plan.kind, C02_KIND);
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].action, PlanAction::EditContent);
        assert!(plan.entries[0].target.is_none());
        assert!(plan.entries[0].resolution.is_none());
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "mixed-line-endings"));
        assert_eq!(
            plan.params["target"],
            PlanParamValue::String("lf".to_string())
        );
    }

    #[test]
    fn already_target_and_no_newline_files_are_skipped() {
        let plan = plan_c02(input(vec![
            file(
                "/data/a.txt",
                b"a\nb\n",
                LineEndingCounts {
                    crlf: 0,
                    lf: 2,
                    cr: 0,
                },
            ),
            file("/data/b.txt", b"single line", LineEndingCounts::default()),
        ]))
        .unwrap();
        assert_eq!(plan.entries.len(), 2);
        assert_eq!(plan.entries[0].resolution, Some(PlanResolution::Skip));
        assert_eq!(plan.entries[1].resolution, Some(PlanResolution::Skip));
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "no-line-endings"));
    }

    #[test]
    fn binary_magic_blocks_before_encoding_detection() {
        let plan = plan_c02(input(vec![file(
            "/data/image.txt",
            b"\x89PNG\r\n\x1a\nrest",
            LineEndingCounts::default(),
        )]))
        .unwrap();
        let blocked = plan.entries[0].blocked.as_ref().unwrap();
        assert_eq!(blocked.code, "E3001");
        assert!(blocked.reason.contains("binary file signature"));
        assert!(plan
            .warnings
            .iter()
            .any(|warning| warning.kind == "binary-skipped"));
    }

    #[test]
    fn utf16_and_ambiguous_encodings_are_blocked() {
        let plan = plan_c02(input(vec![
            file(
                "/data/utf16.txt",
                &[0xff, 0xfe, b'a', 0x00],
                LineEndingCounts::default(),
            ),
            file(
                "/data/binary.txt",
                &[0x01, 0x02, 0x03],
                LineEndingCounts::default(),
            ),
        ]))
        .unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E3001");
        assert!(plan.entries[0]
            .blocked
            .as_ref()
            .unwrap()
            .reason
            .contains("UTF-16"));
        assert_eq!(plan.entries[1].blocked.as_ref().unwrap().code, "E3001");
    }

    #[test]
    fn protected_source_is_blocked() {
        let mut input = input(vec![file(
            "/data/a.txt",
            b"a\r\n",
            LineEndingCounts {
                crlf: 1,
                lf: 0,
                cr: 0,
            },
        )]);
        input.protected_dirs = vec![ProtectedDir::new("/data")];
        let plan = plan_c02(input).unwrap();
        assert_eq!(plan.entries[0].blocked.as_ref().unwrap().code, "E2009");
    }

    #[test]
    fn confirmed_source_can_override_ambiguous_detection() {
        let mut input = input(vec![file(
            "/data/a.txt",
            &[0x01, 0x02, 0x03],
            LineEndingCounts {
                crlf: 1,
                lf: 0,
                cr: 0,
            },
        )]);
        input.confirmed_source = Some(SourceEncoding::Latin1);
        let plan = plan_c02(input).unwrap();
        assert!(plan.entries[0].blocked.is_none());
    }
}
