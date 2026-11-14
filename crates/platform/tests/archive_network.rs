// D1：C10 归档执行器的断线重试接线。用真实 ZipArchiveService 读本地夹具，
// 但让 inspect 前 4 枪返回 E2005（网络断线）；目标目录写成不可达 UNC，
// 从而让执行器把整单判为网络路径并走 NetworkRetry。
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use platform::archive::ZipArchiveService;
use platform::fs::CliFsAdapter;
use platform::journal::FileJournal;

use kernel::apply::{ApplyOptions, ApplyState};
use kernel::archive::{
    apply_archive_plan, plan_c10, ArchiveApplyContext, ArchiveCompression, ArchiveInspect,
    ArchiveService, CompressedArchive, PlanC10Input,
};
use kernel::error::KernelError;
use kernel::fs::{CaseSense, FsAdapter};
use kernel::license::AlwaysAllowLicense;
use kernel::retry::{NetworkRetry, PauseAction};

fn fixture_zip() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus/Z01/out/gbk_name.zip")
}

fn scratch(name: &str) -> PathBuf {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).expect("create scratch dir");
    base
}

struct FailingArchive {
    inner: ZipArchiveService,
    fail_inspect: AtomicU64,
    inspect_calls: AtomicU64,
}

impl FailingArchive {
    fn new(fail_inspect: u64) -> Self {
        Self {
            inner: ZipArchiveService,
            fail_inspect: AtomicU64::new(fail_inspect),
            inspect_calls: AtomicU64::new(0),
        }
    }
}

impl ArchiveService for FailingArchive {
    fn inspect(&self, archive: &str) -> Result<ArchiveInspect, KernelError> {
        self.inspect_calls.fetch_add(1, Ordering::SeqCst);
        if self
            .fail_inspect
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                value.checked_sub(1)
            })
            .is_ok()
        {
            return Err(KernelError::new(
                "E2005",
                "network path is unreachable",
                Some(archive.to_string()),
                true,
            ));
        }
        self.inner.inspect(archive)
    }

    fn extract_entry(
        &self,
        archive: &str,
        index: u64,
        password: Option<&str>,
        writer: &mut dyn std::io::Write,
    ) -> Result<(), KernelError> {
        self.inner.extract_entry(archive, index, password, writer)
    }

    fn compress(
        &self,
        sources: &[String],
        compression: ArchiveCompression,
    ) -> Result<CompressedArchive, KernelError> {
        self.inner.compress(sources, compression)
    }
}

#[derive(Default)]
struct RecordingRetry {
    waits: Mutex<Vec<Duration>>,
    pauses: AtomicU64,
}

impl NetworkRetry for RecordingRetry {
    fn wait(&self, delay: Duration) -> bool {
        self.waits.lock().unwrap().push(delay);
        true
    }

    fn pause(&self, _error: &KernelError) -> PauseAction {
        self.pauses.fetch_add(1, Ordering::SeqCst);
        PauseAction::Resumed
    }
}

#[test]
fn c10_extract_retries_network_inspect_three_times_then_pauses() {
    let root = scratch("c10-network-retry");
    let output = r"\\127.0.0.1\batchdesk-c10-network\out".to_string();
    let quarantine = root.join("quarantine");
    fs::create_dir_all(&quarantine).unwrap();
    let archive = fixture_zip();

    let real = ZipArchiveService;
    let inspect = real
        .inspect(archive.to_str().expect("utf8 path"))
        .expect("inspect local fixture");
    let plan = plan_c10(PlanC10Input {
        plan_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
        archive: archive.to_string_lossy().into_owned(),
        archive_size: inspect.archive_size,
        entries: inspect.entries,
        output_dir: output,
        max_total_bytes: 1_000_000,
        max_ratio: 1_000.0,
        existing_targets: Vec::new(),
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        name_rules: kernel::conflict::NameRules::posix(),
        password_provided: false,
    })
    .expect("plan c10");

    let adapter = CliFsAdapter::new(Some(quarantine));
    let license = AlwaysAllowLicense;
    let mut journal = FileJournal::open(root.join("journal.jsonl")).expect("open journal");
    let failing = FailingArchive::new(4);
    let retry = RecordingRetry::default();
    let mut context = ArchiveApplyContext {
        fs: &adapter,
        license: &license,
        journal: &mut journal,
        archive: &failing,
        task_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_string(),
        now_rfc3339: "2026-11-08T00:00:00Z".to_string(),
        protected_dirs: Vec::new(),
        case_sense: adapter.case_sensitivity(),
        name_rules: kernel::conflict::NameRules::posix(),
        password: None,
        progress: None,
        cancel: None,
        network_retry: Some(&retry),
    };

    let outcome =
        apply_archive_plan(&plan, &ApplyOptions::default(), &mut context).expect("apply c10");
    // 目标 UNC 不可达，条目最终失败；但 inspect 的重试/退避/暂停必须真实发生。
    assert_eq!(
        failing.inspect_calls.load(Ordering::SeqCst),
        5,
        "首次＋3 次退避重试＋暂停恢复后 1 次"
    );
    assert_eq!(
        *retry.waits.lock().unwrap(),
        vec![
            Duration::from_secs(2),
            Duration::from_secs(4),
            Duration::from_secs(8)
        ]
    );
    assert_eq!(retry.pauses.load(Ordering::SeqCst), 1);
    assert!(
        matches!(
            outcome.state,
            ApplyState::PartialFailed | ApplyState::Failed
        ),
        "目标不可达时不能报完成：{:?}",
        outcome.state
    );
}
