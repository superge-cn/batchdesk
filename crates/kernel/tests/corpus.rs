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

use kernel::encoding::{
    detect, transcode, Bom, Confidence, InvalidSequencePolicy, PlatformPrior, SourceEncoding,
    TargetEncoding,
};
use kernel::fs::CaseSense;
use kernel::plan::{plan_c01, PlanC01File, PlanC01Input, PlanDocument};
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
