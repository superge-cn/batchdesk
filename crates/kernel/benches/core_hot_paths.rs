//! 内核纯函数热路径基准（criterion）。
//!
//! 与 `fixtures/bench/measure.py` 的分工：那套是**端到端场景**基准（C-1…C-8 数据集、
//! 含磁盘与进程开销，人工按手册执行、结果入 `fixtures/bench/results.md`）；本文件是
//! **纯函数微基准**（无磁盘/进程开销，只测变换器层），用于日常回归与 CI bench job。
//!
//! 阈值口径属方案 §9.1（端到端指标）；微基准不设达标线，只看跨提交的相对漂移
//! （对比脚本待实现，见执行手册 §5）。

use criterion::{criterion_group, criterion_main, Criterion};
use kernel::conflict::{scan_conflicts, NameRules};
use kernel::csv::CsvRecordScanner;
use kernel::encoding::{
    detect, transcode, InvalidSequencePolicy, PlatformPrior, SourceEncoding, TargetEncoding,
};
use kernel::fs::CaseSense;
use kernel::rename::{plan_f12, PlanF12Input, PlanF12Item, PlanF12Rule, RenameScope};

/// 1 MiB 量级的 GB18030 中文样本：经内核自身 UTF-8→GB18030 转码生成（不引额外依赖）。
fn gb18030_sample() -> Vec<u8> {
    let text = "中文测试文本，含标点、数字 12345 与英文 abc。\n".repeat(20_000);
    transcode(
        text.as_bytes(),
        SourceEncoding::Utf8,
        TargetEncoding::Gb18030,
        InvalidSequencePolicy::ReplaceAndCount,
    )
    .expect("utf-8 -> gb18030")
    .bytes
}

fn bench_encoding_detect(c: &mut Criterion) {
    let sample = gb18030_sample();
    c.bench_function("encoding/detect gb18030 1MiB", |b| {
        b.iter(|| detect(&sample, PlatformPrior::Other))
    });
}

fn bench_encoding_transcode(c: &mut Criterion) {
    let sample = gb18030_sample();
    c.bench_function("encoding/transcode gb18030->utf8 1MiB", |b| {
        b.iter(|| {
            transcode(
                &sample,
                SourceEncoding::Gb18030,
                TargetEncoding::Utf8 { bom: false },
                InvalidSequencePolicy::ReplaceAndCount,
            )
            .expect("gb18030 -> utf-8")
        })
    });
}

fn bench_csv_record_scanner(c: &mut Criterion) {
    // 含引号内换行与转义引号的 RFC 4180 记录（约 1 MiB）
    let mut sample = Vec::with_capacity(1 << 20);
    for i in 0..20_000 {
        sample
            .extend_from_slice(format!("{i},\"带,逗号与\"\"转义\"\"的字段,第二列\"\n").as_bytes());
    }
    c.bench_function("csv/record_scanner 1MiB", |b| {
        b.iter(|| {
            let mut scanner = CsvRecordScanner::new(b',');
            let mut records = 0u64;
            scanner.feed(&sample, |_| records += 1);
            let tail = scanner.finish();
            records + u64::from(tail.tail.is_some())
        })
    });
}

/// 10,000 个条目的改名计划（含前后缀与字面替换）。
fn plan_input(items: Vec<PlanF12Item>) -> PlanF12Input {
    PlanF12Input {
        plan_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
        items,
        existing_targets: Vec::new(),
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        long_path_supported: true,
        access_facts: Vec::new(),
        name_rules: NameRules::windows(),
        rule: PlanF12Rule {
            prefix: Some("doc_".to_string()),
            suffix: None,
            find: None,
            replace: String::new(),
            scope: RenameScope::Full,
        },
    }
}

fn bench_rename_plan_10k(c: &mut Criterion) {
    let items: Vec<PlanF12Item> = (0..10_000)
        .map(|i| PlanF12Item {
            path: format!("/data/dir{:03}/file{i:05}.txt", i % 100),
            name: format!("file{i:05}.txt"),
        })
        .collect();
    let input = plan_input(items);
    c.bench_function("rename/plan_f12 10k", |b| {
        b.iter_batched(
            || input.clone(),
            |input| plan_f12(input).expect("plan"),
            criterion::BatchSize::SmallInput,
        )
    });
}

fn bench_conflict_scan_10k(c: &mut Criterion) {
    let entries: Vec<(String, String)> = (0..10_000)
        .map(|i| {
            (
                format!("/data/file{i:05}.txt"),
                format!("/data/file{:05}.txt", (i + 1) % 10_000),
            )
        })
        .collect();
    c.bench_function("conflict/scan_conflicts 10k", |b| {
        b.iter(|| {
            scan_conflicts(
                black_box_entries(&entries),
                &Default::default(),
                CaseSense::Sensitive,
                NameRules::windows(),
            )
        })
    });
}

// criterion 的 black_box：避免被优化掉的输入构造
fn black_box_entries(entries: &[(String, String)]) -> &[(String, String)] {
    criterion::black_box(entries)
}

criterion_group!(
    benches,
    bench_encoding_detect,
    bench_encoding_transcode,
    bench_csv_record_scanner,
    bench_rename_plan_10k,
    bench_conflict_scan_10k
);
criterion_main!(benches);
