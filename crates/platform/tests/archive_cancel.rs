//! C10 归档专用执行器的**条目级取消**（设计 01 §2.2「每个条目边界检查一次」＋ §7.7 取消处置）。
//!
//! 用例覆盖映射表 `TC-C10-EXE-05` 一直挂着「archive 臂（`apply_archive_plan` 专用执行器）
//! 的条目级取消回滚未断言」——因为取消断言都写在通用执行器（`generic_apply`）那边，
//! 而 C10 走的是专用执行器，没人验证过它的取消分支。本文件补齐这一臂：
//!
//! - 取消落在**条目边界**（第 2 个条目开始前），已应用的第 1 条留在磁盘与 Journal；
//! - 未派发的条目零落盘（不留半成品、也不产生报告行）；
//! - `undone_by` 为空——专用执行器没有执行中回滚通道，如实报告"未撤销"；
//! - 保留的条目仍可经 `undo_plan` 整单撤销（这正是桌面壳 `keepApplied` ＋ 问询对话框
//!   依赖的前提，ADR-0052）。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use platform::archive::ZipArchiveService;
use platform::fs::CliFsAdapter;
use platform::journal::FileJournal;

use kernel::apply::{undo_plan, ApplyOptions, ApplyState, ApplyStatus};
use kernel::archive::{
    apply_archive_plan, plan_c10, ArchiveApplyContext, ArchiveCompression, ArchiveService,
    PlanC10Input,
};
use kernel::fs::{CaseSense, FsAdapter};
use kernel::journal::JournalStatus;
use kernel::license::AlwaysAllowLicense;
use kernel::progress::{CancelToken, ProgressEvent, ProgressSink};

/// 在"第 `at` 个条目开始前"翻取消标志的端口。
///
/// 用 `ProgressSink` 而不是调用计数来定时：专用执行器在每条目开头先 `report_progress`、
/// 再查取消（设计 01 §2.2 的顺序），所以 `done == at` 的那一刻翻转标志，取消点落在条目
/// 边界上是确定的，不依赖"第几次调用 `is_cancelled`"这种脆弱假设。
struct CancelBeforeEntry {
    at: u64,
    flag: AtomicBool,
}

impl ProgressSink for CancelBeforeEntry {
    fn report(&self, event: ProgressEvent) {
        if event.done == self.at {
            self.flag.store(true, Ordering::SeqCst);
        }
    }
}

impl CancelToken for CancelBeforeEntry {
    fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// 在第 `after` 次 `is_cancelled` 询问之后翻标志的端口。
///
/// C10 的**一条计划对应一个压缩包**（`plan_c10` 只收一个 `archive`），所以"条目级取消"在
/// 这个执行器里唯一的落点是**条目内部、逐个文件之间**（`archive.rs` 的 `for item in
/// &selection.selected` 循环）。那里没有进度上报可用，只能按询问次数定时——询问顺序是
/// 「条目开头 1 次 → 每个文件 1 次」，因此 `after = 2` 表示"第 1 个文件之后、第 2 个文件
/// 之前"。若将来执行器改了询问位置，这条断言会立刻失败，不会静默放过。
struct CancelAfterAsks {
    after: usize,
    asks: AtomicUsize,
}

impl CancelToken for CancelAfterAsks {
    fn is_cancelled(&self) -> bool {
        self.asks.fetch_add(1, Ordering::SeqCst) >= self.after
    }
}

/// 一个干净的临时目录（落在 crate 的 `target/` 下，不随宿主 `/tmp` 容量误红）。
fn scratch(name: &str) -> PathBuf {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).expect("create scratch dir");
    base
}

struct Fixture {
    root: PathBuf,
    archive: PathBuf,
    output: PathBuf,
    quarantine: PathBuf,
    journal: PathBuf,
}

/// 造一个含两个文件的 ZIP（用平台自己的压缩实现，避免手写 ZIP 结构）。
fn fixture(name: &str) -> Fixture {
    let root = scratch(name);
    let src = root.join("src");
    fs::create_dir_all(&src).expect("create src");
    let first = src.join("first.txt");
    let second = src.join("second.txt");
    fs::write(&first, b"alpha\n").expect("write first");
    fs::write(&second, b"beta\n").expect("write second");

    let service = ZipArchiveService;
    let bytes = service
        .compress(
            &[first.display().to_string(), second.display().to_string()],
            ArchiveCompression::Store,
        )
        .expect("compress fixture")
        .bytes;
    let archive = root.join("bundle.zip");
    fs::write(&archive, &bytes).expect("write archive");

    let output = root.join("out");
    fs::create_dir_all(&output).expect("create out");
    let quarantine = root.join("quarantine");
    fs::create_dir_all(&quarantine).expect("create quarantine");
    let journal = root.join("journal.jsonl");

    Fixture {
        root,
        archive,
        output,
        quarantine,
        journal,
    }
}

fn plan_for(fixture: &Fixture) -> kernel::plan::PlanDocument {
    let service = ZipArchiveService;
    let inspect = service
        .inspect(&fixture.archive.display().to_string())
        .expect("inspect fixture archive");
    plan_c10(PlanC10Input {
        plan_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
        archive: fixture.archive.display().to_string(),
        archive_size: inspect.archive_size,
        entries: inspect.entries,
        output_dir: fixture.output.display().to_string(),
        max_total_bytes: 1_000_000,
        max_ratio: 1_000.0,
        existing_targets: Vec::new(),
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        name_rules: kernel::conflict::NameRules::windows(),
        password_provided: false,
    })
    .expect("plan c10")
}

#[test]
fn cancel_between_items_keeps_extracted_files_and_stays_undoable() {
    let fixture = fixture("archive-cancel-keep");
    let plan = plan_for(&fixture);
    // C10 的一条计划对应一个压缩包：两个文件同在一条 entry 里。
    assert_eq!(plan.entries.len(), 1, "c10 计划按压缩包出条目");

    let adapter = CliFsAdapter::new(Some(fixture.quarantine.clone()));
    let license = AlwaysAllowLicense;
    let mut journal = FileJournal::open(fixture.journal.clone()).expect("open journal");
    let service = ZipArchiveService;
    // 第 1 个文件写完之后、第 2 个文件之前取消。
    let cancel = CancelAfterAsks {
        after: 2,
        asks: AtomicUsize::new(0),
    };

    let mut context = ArchiveApplyContext {
        fs: &adapter,
        license: &license,
        journal: &mut journal,
        archive: &service,
        task_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_string(),
        now_rfc3339: "2026-11-03T00:00:00Z".to_string(),
        protected_dirs: Vec::new(),
        case_sense: adapter.case_sensitivity(),
        name_rules: kernel::conflict::NameRules::windows(),
        password: None,
        progress: None,
        cancel: Some(&cancel),
        network_retry: None,
    };

    let outcome = apply_archive_plan(&plan, &ApplyOptions::default(), &mut context)
        .expect("apply cancelled archive plan");

    // 三态里的"取消"由 state 表达（设计 01 §2.2）。
    assert_eq!(outcome.state, ApplyState::Cancelled);
    // 专用执行器没有执行中回滚通道：取消一律保留已应用条目并如实留 `undone_by=None`。
    assert!(outcome.report.undone_by.is_none());
    // **口径不对称，故意断言**：`ApplyStatus` 没有 cancelled 取值（取消由 state 表达），
    // 所以被中途打断的这条 entry 仍报 `ok`——它描述的是"已派发的部分做成了什么"。
    // 读者不得把这条 ok 读成"整包已解完"：第二个文件确实没落盘（下面断言）。
    assert_eq!(outcome.report.entries.len(), 1);
    assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);

    let first = fixture.output.join("first.txt");
    let second = fixture.output.join("second.txt");
    assert_eq!(
        fs::read(&first).expect("first target"),
        b"alpha\n",
        "取消前已写出的文件应留在磁盘上（keepApplied 语义）"
    );
    assert!(!second.exists(), "取消点之后的文件不得落盘");

    // 已应用的部分必须真的进了 Journal，否则"取消后可撤销"只是说法。
    assert_eq!(journal.entries().len(), 1, "Journal 只应有 1 条 applied");
    assert_eq!(journal.entries()[0].plan_id, plan.plan_id);
    assert_eq!(journal.entries()[0].plan_kind.as_deref(), Some("c10"));
    assert_eq!(journal.entries()[0].status, JournalStatus::Applied);

    // 保留的条目仍可整单撤销：归入隔离目录、原位置清空、Journal 标 reverted。
    let undo = undo_plan(&plan.plan_id, &adapter, &license, &mut journal).expect("undo kept plan");
    assert_eq!(undo.reverted, 1, "取消后保留的条目应当能整单撤销");
    assert!(undo.failed.is_empty(), "撤销不应有失败条目");
    assert!(!first.exists(), "撤销后产物应离开原位置");
    assert_eq!(journal.entries()[0].status, JournalStatus::Reverted);

    let _ = fs::remove_dir_all(&fixture.root);
}

#[test]
fn cancel_before_the_first_entry_touches_nothing() {
    let fixture = fixture("archive-cancel-none");
    let plan = plan_for(&fixture);

    let adapter = CliFsAdapter::new(Some(fixture.quarantine.clone()));
    let license = AlwaysAllowLicense;
    let mut journal = FileJournal::open(fixture.journal.clone()).expect("open journal");
    let service = ZipArchiveService;
    // done == 0 时取消 ⇒ 一条都不派发（首条之前就停）。
    let cancel = CancelBeforeEntry {
        at: 0,
        flag: AtomicBool::new(false),
    };

    let mut context = ArchiveApplyContext {
        fs: &adapter,
        license: &license,
        journal: &mut journal,
        archive: &service,
        task_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX".to_string(),
        now_rfc3339: "2026-11-03T00:00:00Z".to_string(),
        protected_dirs: Vec::new(),
        case_sense: adapter.case_sensitivity(),
        name_rules: kernel::conflict::NameRules::windows(),
        password: None,
        progress: Some(&cancel),
        cancel: Some(&cancel),
        network_retry: None,
    };

    let outcome =
        apply_archive_plan(&plan, &ApplyOptions::default(), &mut context).expect("apply nothing");

    assert_eq!(outcome.state, ApplyState::Cancelled);
    assert!(
        outcome.report.entries.is_empty(),
        "零派发不应产生条目报告，实际 {:?}",
        outcome.report.entries
    );
    assert!(journal.entries().is_empty(), "零派发不得写 Journal");
    assert!(!fixture.output.join("first.txt").exists(), "零派发不得落盘");

    let _ = fs::remove_dir_all(&fixture.root);
}
