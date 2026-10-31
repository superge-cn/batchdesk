//! 24 条自动化语料的逐条断言 harness（OQ-08 起步）。
//!
//! 判定权威：T v1.12 §3.2；检测/转码实现见 `kernel::encoding`（ADR-0014）。
//! 语料样本不入库，测试前必须先生成：
//! `python fixtures/corpus/check_scope.py && python fixtures/corpus/verify_corpus.py --rebuild --hashes`
//!
//! 已接判定的条目由本文件直接断言；未接的条目登记在 `assertions.tsv`（状态
//! `pending`/`partial`），并由覆盖性测试守住"全部自动化语料都有台账行、人工项不进台账"。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use kernel::conflict::{scan_conflicts, ConflictKind};
use kernel::csv::{
    plan_f05, CsvRecord, CsvRecordScanner, CsvSplitAccumulator, CsvSplitBasis, CsvSplitLayout,
    Delimiter, PlanF05File, PlanF05Input,
};
use kernel::csv_merge::{
    build_union, column_keys, normalized_columns, parse_records, plan_f06, union_names,
    PlanF06File, PlanF06Input,
};
use kernel::encoding::{
    detect, transcode, Bom, Confidence, InvalidSequencePolicy, PlatformPrior, SourceEncoding,
    TargetEncoding,
};
use kernel::fs::CaseSense;
use kernel::line_endings::{
    plan_c02, LineEnding, LineEndingCounts, LineEndingStructure, LineEndingStructureMode,
    PlanC02File, PlanC02Input,
};
use kernel::path::{normalize, try_normalize};
use kernel::plan::{
    plan_c01, PlanC01File, PlanC01Input, PlanDocument, PlanParamValue, PlanResolution,
};
use kernel::rename::{plan_f12, PlanF12Input, PlanF12Item, PlanF12Rule, RenameScope};
use kernel::split::{plan_f01, PlanF01File, PlanF01Input, SplitAccumulator, SplitLayout};

/// 台账：`assertions.tsv` 是"哪条语料断言到什么程度"的唯一登记处。
const TABLE: &str = include_str!("assertions.tsv");
/// ULID 形态的计划 ID（仅满足 `plan_c01` 的参数校验）。
const PLAN_ID: &str = "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8";
const UTF8_NO_BOM: TargetEncoding = TargetEncoding::Utf8 { bom: false };

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus")
}

fn sample(corpus: &str, file: &str) -> Vec<u8> {
    let path = corpus_root().join(corpus).join("out").join(file);
    std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "读取语料 {corpus} 失败（{path:?}）：{error}\n\
             先运行 python fixtures/corpus/check_scope.py && \
             python fixtures/corpus/verify_corpus.py --rebuild --hashes 生成样本"
        )
    })
}

fn sample_text(corpus: &str, file: &str) -> String {
    String::from_utf8(sample(corpus, file)).expect("语料样本应为 UTF-8")
}

fn csv_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                current.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => fields.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    fields.push(current);
    fields
}

fn csv_rows(corpus: &str, file: &str) -> Vec<Vec<String>> {
    sample_text(corpus, file)
        .lines()
        .skip(1)
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.trim().is_empty())
        .map(csv_fields)
        .collect()
}

fn split_parent(path: &str) -> (&str, &str) {
    match path.rfind(['/', '\\']) {
        Some(index) => (&path[..=index], &path[index + 1..]),
        None => ("", path),
    }
}

fn rename_plan_for_row(
    source_path: &str,
    source_name: &str,
    target_name: &str,
    long_path_supported: bool,
) -> PlanDocument {
    plan_f12(PlanF12Input {
        plan_id: PLAN_ID.to_string(),
        items: vec![PlanF12Item {
            path: source_path.to_string(),
            name: source_name.to_string(),
        }],
        existing_targets: vec![source_path.to_string()],
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        long_path_supported,
        rule: PlanF12Rule {
            prefix: None,
            suffix: None,
            find: Some(source_name.to_string()),
            replace: target_name.to_string(),
            scope: RenameScope::Full,
        },
    })
    .expect("plan_f12 生成计划")
}

fn c01_input(corpus: &str, sample: &[u8]) -> PlanC01Input {
    PlanC01Input {
        plan_id: PLAN_ID.to_string(),
        target: UTF8_NO_BOM,
        invalid: InvalidSequencePolicy::Abort,
        prior: PlatformPrior::ChineseWindows,
        confirmed_source: None,
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        files: vec![PlanC01File {
            path: format!("/corpus/{corpus}"),
            canonical_path: None,
            file_size: u64::try_from(sample.len()).expect("样本长度"),
            sample: sample.to_vec(),
        }],
    }
}

fn blocked_code(corpus: &str, sample: &[u8]) -> Option<String> {
    let plan = plan_c01(c01_input(corpus, sample)).expect("plan_c01 生成计划");
    plan.entries
        .first()
        .and_then(|entry| entry.blocked.as_ref())
        .map(|blocked| blocked.code.clone())
}

/// 按 `\n` 逐行喂入纯分片累加器（测试侧 I/O，kernel 不读盘）。
fn split_layout_of(data: &[u8], target: u64) -> SplitLayout {
    let mut accumulator = SplitAccumulator::new(target).expect("target bytes");
    let mut line_start = 0usize;
    for (index, byte) in data.iter().enumerate() {
        if *byte == b'\n' {
            accumulator.push_line((index + 1 - line_start) as u64, true);
            line_start = index + 1;
        }
    }
    if line_start < data.len() {
        accumulator.push_line((data.len() - line_start) as u64, false);
    }
    accumulator.finish(data.len() as u64)
}

fn f01_plan(data: &[u8], target: u64, path: &str) -> PlanDocument {
    plan_f01(PlanF01Input {
        plan_id: PLAN_ID.to_string(),
        target_bytes: target,
        output_dir: "/corpus/out".to_string(),
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        existing_targets: Vec::new(),
        files: vec![PlanF01File {
            path: path.to_string(),
            canonical_path: None,
            group_id: "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q9".to_string(),
            layout: split_layout_of(data, target),
        }],
    })
    .expect("plan_f01 生成计划")
}

/// 按记录扫描样本并生成 F05 布局（每记录一个分片，便于核对边界）。
fn csv_layout(
    data: &[u8],
    delimiter: Delimiter,
    keep_header: bool,
) -> (CsvSplitLayout, Vec<CsvRecord>, Option<CsvRecord>, bool) {
    let mut scanner = CsvRecordScanner::new(delimiter.as_byte());
    let mut records = Vec::new();
    scanner.feed(data, |record| records.push(record));
    let tail = scanner.finish();
    let mut accumulator = CsvSplitAccumulator::new_rows(1, keep_header).expect("row split basis");
    for record in &records {
        accumulator.push_record(*record);
    }
    if let Some(record) = tail.tail {
        accumulator.push_record(record);
    }
    let layout = accumulator.finish(
        data.len() as u64,
        delimiter,
        true,
        tail.unclosed_quote,
        Some(SourceEncoding::Utf8),
    );
    (layout, records, tail.tail, tail.unclosed_quote)
}

fn f05_plan(
    data: &[u8],
    path: &str,
    delimiter: Delimiter,
    keep_header: bool,
) -> (
    PlanDocument,
    CsvSplitLayout,
    Vec<CsvRecord>,
    Option<CsvRecord>,
    bool,
) {
    let (layout, records, tail, unclosed_quote) = csv_layout(data, delimiter, keep_header);
    let plan = plan_f05(PlanF05Input {
        plan_id: PLAN_ID.to_string(),
        basis: CsvSplitBasis::Rows(1),
        keep_header,
        delimiter: None,
        output_dir: "/corpus/out".to_string(),
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        existing_targets: Vec::new(),
        files: vec![PlanF05File {
            path: path.to_string(),
            canonical_path: None,
            group_id: "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q9".to_string(),
            layout: layout.clone(),
        }],
    })
    .expect("plan_f05 生成计划");
    (plan, layout, records, tail, unclosed_quote)
}

fn f06_plan(data: &[u8], path: &str, has_header: bool) -> PlanDocument {
    let records = parse_records(data, b',');
    let columns = if has_header {
        records.first().cloned().unwrap_or_default()
    } else {
        Vec::new()
    };
    let column_count = if has_header {
        records.first().map_or(0, Vec::len) as u64
    } else {
        records.iter().map(Vec::len).max().unwrap_or(0) as u64
    };
    let data_rows = if has_header {
        records.len().saturating_sub(1)
    } else {
        records.len()
    } as u64;
    plan_f06(PlanF06Input {
        plan_id: PLAN_ID.to_string(),
        target: "/corpus/out/merged.csv".to_string(),
        target_encoding: UTF8_NO_BOM,
        invalid: InvalidSequencePolicy::Abort,
        output_delimiter: None,
        add_source_column: false,
        source_column_name: "来源文件".to_string(),
        strict: false,
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        target_occupied: false,
        files: vec![PlanF06File {
            path: path.to_string(),
            canonical_path: None,
            file_size: data.len() as u64,
            source_encoding: Some(SourceEncoding::Utf8),
            delimiter: Delimiter::Comma,
            has_header,
            columns,
            column_count,
            data_rows,
            dirty_rows: 0,
            source_label: None,
        }],
    })
    .expect("plan_f06 生成计划")
}

fn assert_utf8_lines(text: &str, expected_lines: usize, columns: usize) {
    assert!(!text.contains('\u{FFFD}'), "转码后不得出现替换字符");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), expected_lines, "行数（含表头）");
    for (index, line) in lines.iter().enumerate() {
        assert_eq!(line.split(',').count(), columns, "第 {} 行列数", index + 1);
    }
}

#[test]
fn t03_overlong_line_occupies_its_own_part_and_is_flagged() {
    let data = sample("T03", "T03.csv");
    let plan = f01_plan(&data, 1024 * 1024, "/corpus/T03.csv");
    assert!(
        plan.warnings
            .iter()
            .any(|warning| warning.kind == "overlong-line-single-part"),
        "超长行必须显著标注"
    );
    let overlong = plan
        .entries
        .iter()
        .find(|entry| entry.reason.contains("overlong line"))
        .expect("必须存在超长行独占的分片");
    assert_eq!(overlong.relation.as_ref().unwrap().part_index, Some(2));
    assert!(overlong.expected.as_ref().unwrap().size > 1024 * 1024);
}

#[test]
fn t06_no_newline_file_is_hard_cut_with_warning() {
    let data = sample("T06", "T06.txt");
    let target = 1024 * 1024u64;
    let plan = f01_plan(&data, target, "/corpus/T06.txt");
    assert!(
        plan.warnings
            .iter()
            .any(|warning| warning.kind == "no-newline-hard-cut"),
        "无换行必须告警"
    );
    let expected_parts =
        data.len() as u64 / target + u64::from(!(data.len() as u64).is_multiple_of(target));
    assert_eq!(plan.entries.len() as u64, expected_parts);
    assert!(plan
        .entries
        .iter()
        .all(|entry| entry.reason.contains("hard byte cut")));
}

#[test]
fn t05_empty_file_is_reported_without_parts() {
    let data = sample("T05", "T05.csv");
    assert!(data.is_empty());
    let plan = f01_plan(&data, 1024 * 1024, "/corpus/T05.csv");
    assert!(plan.entries.is_empty(), "空文件不得产生分片");
    assert_eq!(plan.warnings.len(), 1);
    assert_eq!(plan.warnings[0].kind, "empty-file");
}

#[test]
fn t01_unclosed_quote_is_kept_as_last_record_with_warning() {
    let data = sample("T01", "T01.csv");
    let (plan, layout, records, tail, unclosed_quote) =
        f05_plan(&data, "/corpus/T01.csv", Delimiter::Comma, true);
    assert!(unclosed_quote, "T01 必须检出引号未闭合");
    assert!(layout.unclosed_quote);
    assert!(tail.is_some(), "未闭合引号后的剩余内容必须作为末条记录保留");
    assert_eq!(layout.records, records.len() as u64 + 1);
    assert_eq!(layout.shards.last().unwrap().end, data.len() as u64);
    let physical_lines = data.iter().filter(|byte| **byte == b'\n').count()
        + usize::from(!matches!(data.last(), Some(&b'\n')));
    assert!(
        physical_lines > layout.records as usize,
        "不闭合引号应吞并后续物理行，成为末条记录"
    );
    assert!(plan
        .warnings
        .iter()
        .any(|warning| warning.kind == "unclosed-quote"));
}

#[test]
fn t02_quoted_newlines_do_not_split_records() {
    let data = sample("T02", "T02.csv");
    let (plan, layout, records, tail, unclosed_quote) =
        f05_plan(&data, "/corpus/T02.csv", Delimiter::Comma, true);
    assert!(!unclosed_quote);
    let all_records: Vec<CsvRecord> = records.iter().copied().chain(tail).collect();
    assert_eq!(all_records.len(), 9, "表头 + 8 条数据记录");
    assert_eq!(layout.records, 9);
    assert!(
        all_records.iter().any(|record| record.multiline),
        "至少一条记录跨物理行"
    );
    let parsed = parse_records(&data, b',');
    assert_eq!(parsed.len(), 9);
    assert!(parsed.iter().skip(1).any(|row| row
        .iter()
        .any(|field| field.contains('\n') || field.contains('\r'))));
    let physical_lines = data.iter().filter(|byte| **byte == b'\n').count();
    assert!(physical_lines > parsed.len(), "引号内换行不增加记录数");
    let header = layout.header.expect("T02 有表头");
    assert_eq!(plan.entries.len(), 8);
    assert!(plan
        .entries
        .iter()
        .all(|entry| entry.csv.as_ref().unwrap().rows == 1));
    let covered: u64 = plan
        .entries
        .iter()
        .map(|entry| {
            let range = entry.range.unwrap();
            range.end - range.start
        })
        .sum();
    assert_eq!(covered, data.len() as u64 - header.len());
}

#[test]
fn t04_column_drift_is_counted_without_correction() {
    let data = sample("T04", "T04.csv");
    let (plan, layout, _records, _tail, unclosed_quote) =
        f05_plan(&data, "/corpus/T04.csv", Delimiter::Comma, true);
    assert!(!unclosed_quote);
    let parsed = parse_records(&data, b',');
    assert_eq!(parsed.len(), 6);
    assert_eq!(parsed[0].len(), 3);
    let widths: Vec<usize> = parsed.iter().skip(1).map(Vec::len).collect();
    assert_eq!(widths, vec![2, 3, 4, 1, 3]);
    assert_eq!(layout.header_columns, 3);
    assert_eq!(layout.dirty_rows, 3);
    assert!(plan
        .warnings
        .iter()
        .any(|warning| warning.kind == "column-drift"));
}

#[test]
fn t07_headerless_sample_aligns_by_column_position() {
    let data = sample("T07", "T07.csv");
    let parsed = parse_records(&data, b',');
    assert_eq!(parsed.len(), 4);
    assert!(parsed.iter().all(|row| row.len() == 3));
    assert!(
        parsed[0][0].parse::<u64>().is_ok(),
        "T07 首列是数字编号，不是表头"
    );
    let plan = f06_plan(&data, "/corpus/T07.csv", false);
    let entry = &plan.entries[0];
    assert!(entry.blocked.is_none());
    let payload = entry.csv_merge.as_ref().expect("F06 载荷");
    assert!(!payload.has_header);
    assert!(payload.union.is_empty());
    assert_eq!(payload.positional_columns, 3);
    assert_eq!(
        plan.params.get("hasHeader"),
        Some(&PlanParamValue::Bool(false))
    );
    assert!(plan
        .warnings
        .iter()
        .any(|warning| warning.kind == "no-header-positional"));
}

#[test]
fn t08_duplicate_and_padded_headers_normalize_without_collapsing() {
    let data = sample("T08", "T08.csv");
    let parsed = parse_records(&data, b',');
    assert_eq!(
        parsed[0],
        vec![
            "姓名".to_string(),
            "姓名".to_string(),
            " 部门 ".to_string(),
            "金额 ".to_string()
        ]
    );
    let normalized = normalized_columns(&parsed[0]);
    assert_eq!(
        normalized,
        vec![
            "姓名".to_string(),
            "姓名".to_string(),
            "部门".to_string(),
            "金额".to_string()
        ]
    );
    assert_eq!(
        column_keys(&normalized),
        vec![
            ("姓名".to_string(), 0),
            ("姓名".to_string(), 1),
            ("部门".to_string(), 0),
            ("金额".to_string(), 0),
        ]
    );
    let union = build_union(std::slice::from_ref(&normalized));
    assert_eq!(union_names(&union), normalized);
    let plan = f06_plan(&data, "/corpus/T08.csv", true);
    let payload = plan.entries[0].csv_merge.as_ref().expect("F06 载荷");
    assert_eq!(
        payload.union,
        vec![
            "姓名".to_string(),
            "姓名".to_string(),
            "部门".to_string(),
            "金额".to_string()
        ]
    );
    assert_eq!(payload.positional_columns, 4);
    assert!(payload.has_header);
    assert!(plan
        .warnings
        .iter()
        .any(|warning| warning.kind == "header-normalized"));
    assert!(plan
        .warnings
        .iter()
        .any(|warning| warning.kind == "duplicate-column-names"));
}

#[test]
fn k01_gbk_sample_detects_as_gb18030_and_transcodes_clean() {
    let data = sample("K01", "K01.csv");
    let detection = detect(&data, PlatformPrior::ChineseWindows);
    assert_eq!(
        detection.encoding,
        Some(SourceEncoding::Gb18030),
        "K01 必须判定为 GB18030，不得判为 UTF-8"
    );
    assert_eq!(detection.confidence, Confidence::High);
    assert!(!detection.needs_manual);
    assert_eq!(detection.bom, None, "K01 样本无 BOM");

    let output = transcode(
        &data,
        SourceEncoding::Gb18030,
        UTF8_NO_BOM,
        InvalidSequencePolicy::Abort,
    )
    .expect("K01 转码为 UTF-8");
    assert_eq!(output.replacement_count, 0, "合法 GB18030 不应产生替换");
    let text = String::from_utf8(output.bytes).expect("输出必须是合法 UTF-8");
    assert_utf8_lines(&text, 9, 4);
}

#[test]
fn k02_utf8_sample_is_not_mistaken_for_gbk() {
    let data = sample("K02", "K02.csv");
    let detection = detect(&data, PlatformPrior::ChineseWindows);
    assert_eq!(detection.encoding, Some(SourceEncoding::Utf8));
    assert_eq!(detection.confidence, Confidence::High);
    assert!(!detection.needs_manual);
    assert_eq!(detection.bom, None, "K02 样本不写 BOM");
}

#[test]
fn k03_bom_wins_and_target_bom_follows_caller() {
    let data = sample("K03", "K03.csv");
    assert!(
        data.starts_with(&[0xEF, 0xBB, 0xBF]),
        "K03 样本必须以 UTF-8 BOM 开头"
    );
    let detection = detect(&data, PlatformPrior::Other);
    assert_eq!(detection.encoding, Some(SourceEncoding::Utf8));
    assert_eq!(detection.bom, Some(Bom::Utf8), "BOM 优先命中");
    assert_eq!(detection.confidence, Confidence::High);

    let output = transcode(
        &data,
        SourceEncoding::Utf8,
        TargetEncoding::Utf8 { bom: true },
        InvalidSequencePolicy::Abort,
    )
    .expect("K03 重写为 UTF-8 BOM");
    assert!(output.had_bom_input, "输入带 BOM 需如实上报");
    assert_eq!(output.bytes.get(..3), Some(&[0xEF, 0xBB, 0xBF][..]));
    assert_eq!(
        output.bytes.get(3..),
        data.get(3..),
        "去掉源 BOM 后内容应逐字节保持"
    );
}

#[test]
fn k05_ascii_sample_defaults_to_utf8_with_hint() {
    let data = sample("K05", "K05.csv");
    let detection = detect(&data, PlatformPrior::Other);
    assert_eq!(detection.encoding, Some(SourceEncoding::Utf8));
    let note = detection
        .note
        .as_deref()
        .expect("K05 必须给出默认 UTF-8 提示");
    assert!(
        note.contains("ASCII"),
        "提示应说明纯 ASCII/过短样本的默认判定，实际：{note}"
    );
}

#[test]
fn k04_and_k06_are_never_silently_converted() {
    for (corpus, file) in [("K04", "K04.csv"), ("K06", "photo.txt")] {
        let data = sample(corpus, file);
        let detection = detect(&data, PlatformPrior::ChineseWindows);
        assert!(
            detection.needs_manual,
            "{corpus} 必须降级为需人工确认，不得静默采用判定结果"
        );
        assert_eq!(
            blocked_code(corpus, &data),
            Some("E3001".to_string()),
            "{corpus} 的 C01 计划必须产出 blocked E3001"
        );
    }
}

#[test]
fn k07_mixed_encoding_is_flagged_not_repaired() {
    let data = sample("K07", "K07.txt");
    let detection = detect(&data, PlatformPrior::ChineseWindows);
    assert!(
        detection.needs_manual,
        "K07 混合编码不得被自动采用或静默修复"
    );
    assert_eq!(
        blocked_code("K07", &data),
        Some("E3001".to_string()),
        "K07 的 C01 计划必须产出 blocked E3001"
    );
}

#[test]
fn k08_utf16_bom_sample_detects_and_transcodes_clean() {
    let data = sample("K08", "K08.csv");
    let detection = detect(&data, PlatformPrior::Other);
    assert_eq!(detection.encoding, Some(SourceEncoding::Utf16Le));
    assert_eq!(detection.bom, Some(Bom::Utf16Le));
    assert!(!detection.needs_manual);

    let output = transcode(
        &data,
        SourceEncoding::Utf16Le,
        UTF8_NO_BOM,
        InvalidSequencePolicy::Abort,
    )
    .expect("K08 转码为 UTF-8");
    assert_eq!(output.replacement_count, 0);
    assert!(output.had_bom_input);
    let text = String::from_utf8(output.bytes).expect("输出必须是合法 UTF-8");
    assert_utf8_lines(&text, 6, 4);
}

#[test]
fn r01_long_path_samples_follow_path_length_tiers() {
    let rows = csv_rows("R01", "paths.csv");
    assert_eq!(rows.len(), 3, "R01 应提供三档边界路径");

    for row in rows {
        let path = &row[1];
        let char_len: usize = row[2].parse().expect("char_len 应为整数");
        assert_eq!(path.chars().count(), char_len, "样本长度与台账一致");

        let (parent, file_name) = split_parent(path);
        let source = format!("{parent}src.txt");
        let plan = rename_plan_for_row(&source, "src.txt", file_name, false);
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].target.as_deref(), Some(path.as_str()));

        match char_len {
            200 => {
                assert!(!plan
                    .warnings
                    .iter()
                    .any(|warning| warning.kind.starts_with("long-path")));
                assert_eq!(plan.entries[0].resolution, None);
            }
            259 => {
                assert!(plan
                    .warnings
                    .iter()
                    .any(|warning| warning.kind == "long-path-caution"));
                assert_eq!(plan.entries[0].resolution, None);
            }
            260 => {
                assert!(plan
                    .warnings
                    .iter()
                    .any(|warning| warning.kind == "long-path-disabled"));
                assert_eq!(plan.entries[0].resolution, Some(PlanResolution::Skip));

                let enabled = rename_plan_for_row(&source, "src.txt", file_name, true);
                assert!(!enabled
                    .warnings
                    .iter()
                    .any(|warning| warning.kind == "long-path-disabled"));
                assert_eq!(enabled.entries[0].resolution, None);
            }
            other => panic!("R01 出现未预期长度档：{other}"),
        }
    }
}

#[test]
fn r02_case_only_targets_conflict_only_on_case_insensitive_hosts() {
    let rows = csv_rows("R02", "rename_map.csv");
    assert_eq!(rows.len(), 2);
    let entries: Vec<(String, String)> = rows
        .iter()
        .map(|row| {
            (
                format!("/corpus/R02/{}", row[0]),
                format!("/corpus/R02/{}", row[1]),
            )
        })
        .collect();
    assert_ne!(entries[0].1, entries[1].1, "样本目标名应不同");
    assert_eq!(
        entries[0].1.to_lowercase(),
        entries[1].1.to_lowercase(),
        "样本目标名应仅大小写不同"
    );

    let insensitive = scan_conflicts(&entries, &BTreeSet::new(), CaseSense::CaseInsensitive);
    assert_eq!(
        insensitive.conflicts.get(&1),
        Some(&ConflictKind::CaseOnly),
        "大小写不敏感卷上第二条目标应判 case-only"
    );

    let sensitive = scan_conflicts(&entries, &BTreeSet::new(), CaseSense::Sensitive);
    assert!(
        sensitive.conflicts.is_empty(),
        "大小写敏感卷上两个目标应视为不同名"
    );
}

#[test]
fn r03_reserved_names_are_blocked_in_preview() {
    let rows = csv_rows("R03", "rename_map.csv");
    assert_eq!(rows.len(), 6);
    for row in rows {
        let source = &row[0];
        let target = &row[1];
        let plan = rename_plan_for_row(&format!("/corpus/R03/{source}"), source, target, true);
        let blocked = plan.entries[0]
            .blocked
            .as_ref()
            .unwrap_or_else(|| panic!("保留名应被阻断：{target}"));
        assert_eq!(blocked.code, "E1002");
        assert!(
            blocked.reason.contains("reserved-name"),
            "{target}: {}",
            blocked.reason
        );
    }
}

#[test]
fn r04_trailing_space_and_dot_targets_are_blocked_in_preview() {
    let rows = csv_rows("R04", "rename_map.csv");
    assert_eq!(rows.len(), 3);
    let mut blocked = 0;
    for row in &rows {
        let source = &row[0];
        let target = &row[1];
        let plan = rename_plan_for_row(&format!("/corpus/R04/{source}"), source, target, true);
        if target.ends_with(' ') || target.ends_with('.') {
            let entry = plan.entries[0]
                .blocked
                .as_ref()
                .unwrap_or_else(|| panic!("尾部空格/点应被阻断：{target:?}"));
            assert!(
                entry.reason.contains("trailing-dot-space"),
                "{target:?}: {}",
                entry.reason
            );
            blocked += 1;
        } else {
            // §5.13 只拦截整名结尾空格/点；"首部空格"不在拦截清单。
            assert!(
                plan.entries[0].blocked.is_none(),
                "仅整名尾部空格/点进入拦截清单：{target:?}"
            );
        }
    }
    assert_eq!(blocked, 2, "样本应覆盖尾部空格与结尾点两条");
}

#[test]
fn r06_extended_and_plain_paths_normalize_to_the_same_value() {
    let rows = csv_rows("R06", "paths.csv");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][1], "plain");
    assert_eq!(rows[1][1], "extended");
    let plain = &rows[0][2];
    let extended = &rows[1][2];

    assert_eq!(normalize(plain), normalize(extended));
    assert!(!normalize(extended).starts_with(r"\\?\"));
    assert_eq!(try_normalize(extended).unwrap(), normalize(plain));

    let (_parent, file_name) = split_parent(extended);
    let plan = plan_f12(PlanF12Input {
        plan_id: PLAN_ID.to_string(),
        items: vec![PlanF12Item {
            path: extended.clone(),
            name: file_name.to_string(),
        }],
        existing_targets: vec![plain.clone()],
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        long_path_supported: true,
        rule: PlanF12Rule {
            prefix: Some("new_".to_string()),
            suffix: None,
            find: None,
            replace: String::new(),
            scope: RenameScope::Stem,
        },
    })
    .expect("plan_f12 生成计划");
    let entry = &plan.entries[0];
    assert!(entry.conflict.is_none(), "扩展/普通写法不应互判重");
    assert!(entry.blocked.is_none());
}

#[test]
fn k06_binary_magic_is_rejected_by_c02_plan() {
    let data = sample("K06", "photo.txt");
    let plan = plan_c02(PlanC02Input {
        plan_id: PLAN_ID.to_string(),
        target: LineEnding::Lf,
        structure: LineEndingStructureMode::Text,
        files: vec![PlanC02File {
            path: "/corpus/K06/photo.txt".to_string(),
            canonical_path: None,
            file_size: data.len() as u64,
            sample: data,
            counts: LineEndingCounts::default(),
            structure: LineEndingStructure::Text,
        }],
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        confirmed_source: None,
        prior: PlatformPrior::Other,
    })
    .expect("plan_c02 生成计划");
    let blocked = plan.entries[0].blocked.as_ref().expect("二进制应被阻断");
    assert_eq!(blocked.code, "E3001");
    assert!(blocked.reason.contains("binary file signature"));
}

fn is_corpus_id(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(first) if "KTERZ".contains(first))
        && chars.clone().count() == 2
        && chars.all(|ch| ch.is_ascii_digit())
}

fn manual_only_ids(path: &Path) -> BTreeSet<String> {
    let text = std::fs::read_to_string(path).expect("读取 manual-only.txt");
    text.lines()
        .filter_map(|line| line.split('#').next())
        .map(str::trim)
        .filter(|candidate| is_corpus_id(candidate))
        .map(str::to_string)
        .collect()
}

/// 台账覆盖性：全部自动化语料必须有一行、人工项不得出现、状态与缺口说明自洽。
///
/// 该测试不执行 pending 行的产品行为，只守住"漏测不静默"——新增语料或转人工
/// 时若忘记登记台账，CI 会直接失败。
#[test]
fn assertions_table_covers_every_automated_corpus() {
    let root = corpus_root();
    let excluded = manual_only_ids(&root.join("manual-only.txt"));

    let mut on_disk: Vec<String> = std::fs::read_dir(&root)
        .expect("读取 fixtures/corpus")
        .map(|entry| {
            entry
                .expect("目录项")
                .file_name()
                .to_string_lossy()
                .to_string()
        })
        .filter(|name| is_corpus_id(name) && !excluded.contains(name))
        .collect();
    on_disk.sort();
    assert_eq!(
        on_disk.len(),
        24,
        "自动化语料应为 24 条（T v1.12 §3.2 共 28 条，人工 4 条见 manual-only.txt）"
    );

    let mut tracked = Vec::new();
    let mut asserted = 0usize;
    let mut partial = 0usize;
    let mut pending = 0usize;
    for line in TABLE.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cells: Vec<&str> = line.split('\t').map(str::trim).collect();
        assert_eq!(
            cells.len(),
            4,
            "assertions.tsv 每行应为 4 列（编号/状态/断言点/缺口）：{line}"
        );
        let (id, status, assertion, gap) = (cells[0], cells[1], cells[2], cells[3]);
        assert!(is_corpus_id(id), "非语料编号：{id}");
        assert!(!assertion.is_empty(), "{id} 必须写明已接断言点（未接写 —）");
        match status {
            "asserted" => asserted += 1,
            "partial" => {
                partial += 1;
                assert!(!gap.is_empty(), "{id} 标 partial 必须写明剩余缺口");
            }
            "pending" => {
                pending += 1;
                assert!(!gap.is_empty(), "{id} 标 pending 必须写明缺口与前置依赖");
            }
            other => panic!("{id} 状态非法：{other}（只允许 asserted/partial/pending）"),
        }
        tracked.push(id.to_string());
    }
    tracked.sort();
    assert_eq!(
        tracked, on_disk,
        "assertions.tsv 必须与自动化语料集合一一对应（人工项不得登记）"
    );
    println!("语料台账：asserted={asserted} partial={partial} pending={pending}");
}
