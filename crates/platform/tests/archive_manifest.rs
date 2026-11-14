//! C10 归档专用执行器的**逐文件报告**（方案 §5.11「压缩完成后自动生成文件清单（可含哈希）」）。
//!
//! 用例覆盖映射表 `TC-C10-EXE-05` 一直挂着「逐文件报告未做」——ADR-0029 写明
//! 「每压缩包一条报告（不逐文件报告）」、ADR-0030 把它列进已知缺口。此前 C10 的
//! `ApplyEntryReport` 只有一条整包 summary，报告回答不了"这个包里/解出来的是哪些文件"。
//!
//! 本文件补齐这一臂：压缩与解压两侧成功后，条目 `detail` 必须携带**逐文件清单**
//! （名称＋未压缩体积），且与实际产物一致。

use std::fs;
use std::path::{Path, PathBuf};

use platform::archive::ZipArchiveService;
use platform::fs::CliFsAdapter;
use platform::journal::FileJournal;

use kernel::apply::ApplyOptions;
use kernel::archive::{
    apply_archive_plan, plan_c10, plan_c10_compress, ArchiveApplyContext, ArchiveCompression,
    ArchiveService, PlanC10CompressInput, PlanC10Input,
};
use kernel::conflict::NameRules;
use kernel::fs::CaseSense;
use kernel::license::AlwaysAllowLicense;

fn scratch(name: &str) -> PathBuf {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).expect("create scratch dir");
    base
}

fn context<'a>(
    adapter: &'a CliFsAdapter,
    journal: &'a mut FileJournal,
    service: &'a ZipArchiveService,
    quarantine: PathBuf,
) -> ArchiveApplyContext<'a> {
    let _ = quarantine;
    ArchiveApplyContext {
        fs: adapter,
        license: &AlwaysAllowLicense,
        journal,
        archive: service,
        task_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_string(),
        now_rfc3339: "2026-11-13T00:00:00Z".to_string(),
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        name_rules: NameRules::windows(),
        password: None,
        progress: None,
        cancel: None,
        network_retry: None,
    }
}

/// 压缩侧：报告条目必须逐文件列出包内容。
#[test]
fn compress_report_carries_a_per_file_manifest() {
    let root = scratch("archive-manifest-compress");
    let first = root.join("first.txt");
    let second = root.join("second.txt");
    fs::write(&first, b"alpha\n").expect("write first");
    fs::write(&second, b"beta\n").expect("write second");
    let output = root.join("bundle.zip");

    let plan = plan_c10_compress(PlanC10CompressInput {
        plan_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
        output: output.display().to_string(),
        sources: vec![first.display().to_string(), second.display().to_string()],
        compression: ArchiveCompression::Store,
        target_exists: false,
        source_file_count: 2,
        source_bytes: 11,
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
    })
    .expect("plan c10 compress");

    let adapter = CliFsAdapter::new(Some(root.join("quarantine")));
    let mut journal = FileJournal::open(root.join("journal.jsonl")).expect("open journal");
    let service = ZipArchiveService;
    let mut ctx = context(&adapter, &mut journal, &service, root.join("quarantine"));
    let outcome = apply_archive_plan(&plan, &ApplyOptions::default(), &mut ctx).expect("apply");

    assert_eq!(outcome.report.entries.len(), 1);
    let detail = outcome.report.entries[0]
        .detail
        .as_deref()
        .expect("压缩成功条目必须带逐文件清单");
    assert!(detail.contains("2 个文件"), "清单要给出文件总数：{detail}");
    assert!(detail.contains("first.txt"), "清单要点名每个文件：{detail}");
    assert!(
        detail.contains("second.txt"),
        "清单要点名每个文件：{detail}"
    );
    assert!(
        detail.contains("first.txt 6"),
        "清单要给出每个文件的未压缩体积：{detail}"
    );
    // 产物确实存在且可读——清单不是空话。
    assert!(output.exists(), "压缩产物应落盘");
}

/// 解压侧：报告条目必须逐文件列出写出的文件（目录条目不算）。
#[test]
fn extract_report_carries_a_per_file_manifest() {
    let root = scratch("archive-manifest-extract");
    let src = root.join("src");
    fs::create_dir_all(&src).expect("create src");
    let first = src.join("first.txt");
    let second = src.join("second.txt");
    fs::write(&first, b"alpha\n").expect("write first");
    fs::write(&second, b"beta\n").expect("write second");

    let service = ZipArchiveService;
    let archive = root.join("bundle.zip");
    let bytes = service
        .compress(
            &[first.display().to_string(), second.display().to_string()],
            ArchiveCompression::Store,
        )
        .expect("compress fixture")
        .bytes;
    fs::write(&archive, &bytes).expect("write archive");

    let inspect = service
        .inspect(&archive.display().to_string())
        .expect("inspect fixture");
    let output = root.join("out");
    fs::create_dir_all(&output).expect("create out");
    let plan = plan_c10(PlanC10Input {
        plan_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
        archive: archive.display().to_string(),
        archive_size: inspect.archive_size,
        entries: inspect.entries,
        output_dir: output.display().to_string(),
        max_total_bytes: 1_000_000,
        max_ratio: 1_000.0,
        existing_targets: Vec::new(),
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        name_rules: NameRules::windows(),
        password_provided: false,
    })
    .expect("plan c10");

    let adapter = CliFsAdapter::new(Some(root.join("quarantine")));
    let mut journal = FileJournal::open(root.join("journal.jsonl")).expect("open journal");
    let mut ctx = context(&adapter, &mut journal, &service, root.join("quarantine"));
    let outcome = apply_archive_plan(&plan, &ApplyOptions::default(), &mut ctx).expect("apply");

    let detail = outcome.report.entries[0]
        .detail
        .as_deref()
        .expect("解压成功条目必须带逐文件清单");
    assert!(detail.contains("2 个文件"), "清单要给出文件总数：{detail}");
    assert!(detail.contains("first.txt"), "清单要点名每个文件：{detail}");
    assert!(
        detail.contains("second.txt"),
        "清单要点名每个文件：{detail}"
    );
    assert_eq!(
        fs::read(output.join("first.txt")).expect("first"),
        b"alpha\n"
    );
    assert_eq!(
        fs::read(output.join("second.txt")).expect("second"),
        b"beta\n"
    );
}
