//! C10 密码包：密码是**执行期秘密**（ADR-0074）。
//!
//! 三条断言钉住契约，一条钉住算法覆盖：
//!
//! 1. `inspect` **不需要密码**——加密包也能列出条目（旧的 `by_index` 路径会在这里就
//!    报 "Password required to decrypt file"，连预览都出不来）；
//! 2. 对密码 + 对密码 → 正常解出，逐个文件进 Journal，可撤销；
//! 3. 错密码 → `E3009`，一个文件都不落地、Journal 零条目；
//! 4. 计划要密码而执行期没给 → **整单 `E3008`**（不是逐条失败、不是"解一半"）。
//!
//! 夹具由**第三方实现**产出（Info-ZIP 的 `zip -P` 与 7-Zip 的 `-mem=AES256`），
//! 见 `fixtures/archive/README.md`：用自家压缩器造样本会让"写"和"读"共用同一处缺陷。

use std::fs;
use std::path::{Path, PathBuf};

use platform::archive::ZipArchiveService;
use platform::fs::CliFsAdapter;
use platform::journal::FileJournal;

use kernel::apply::{undo_plan, ApplyOptions, ApplyState, ApplyStatus};
use kernel::archive::{
    apply_archive_plan, plan_c10, ArchiveApplyContext, ArchiveService, PlanC10Input,
};
use kernel::fs::{CaseSense, FsAdapter};
use kernel::license::AlwaysAllowLicense;

const PASSWORD: &str = "secret123";

fn fixture_zip(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/archive/password")
        .join(name)
}

fn scratch(name: &str) -> PathBuf {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).expect("create scratch dir");
    base
}

struct Case {
    root: PathBuf,
    archive: PathBuf,
    output: PathBuf,
    quarantine: PathBuf,
    journal: PathBuf,
}

fn case(name: &str, zip: &str) -> Case {
    let root = scratch(name);
    let output = root.join("out");
    fs::create_dir_all(&output).expect("create out");
    let quarantine = root.join("quarantine");
    fs::create_dir_all(&quarantine).expect("create quarantine");
    Case {
        archive: fixture_zip(zip),
        output,
        quarantine,
        journal: root.join("journal.jsonl"),
        root,
    }
}

fn plan(case: &Case, password_provided: bool) -> kernel::plan::PlanDocument {
    let service = ZipArchiveService;
    let inspect = service
        .inspect(case.archive.to_str().expect("utf8 path"))
        .expect("inspect encrypted archive without a password");
    plan_c10(PlanC10Input {
        plan_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
        archive: case.archive.to_string_lossy().into_owned(),
        archive_size: inspect.archive_size,
        entries: inspect.entries,
        output_dir: case.output.to_string_lossy().into_owned(),
        max_total_bytes: 1_000_000,
        max_ratio: 1_000.0,
        existing_targets: Vec::new(),
        protected_dirs: Vec::new(),
        case_sense: CaseSense::Sensitive,
        name_rules: kernel::conflict::NameRules::posix(),
        password_provided,
    })
    .expect("plan c10")
}

fn apply(
    case: &Case,
    plan: &kernel::plan::PlanDocument,
    password: Option<&str>,
) -> Result<kernel::apply::ApplyOutcome, kernel::error::KernelError> {
    let adapter = CliFsAdapter::new(Some(case.quarantine.clone()));
    let license = AlwaysAllowLicense;
    let mut journal = FileJournal::open(case.journal.clone()).expect("open journal");
    let service = ZipArchiveService;
    let mut context = ArchiveApplyContext {
        fs: &adapter,
        license: &license,
        journal: &mut journal,
        archive: &service,
        task_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_string(),
        now_rfc3339: "2026-11-05T00:00:00Z".to_string(),
        protected_dirs: Vec::new(),
        case_sense: adapter.case_sensitivity(),
        name_rules: kernel::conflict::NameRules::posix(),
        password,
        progress: None,
        cancel: None,
        network_retry: None,
    };
    apply_archive_plan(plan, &ApplyOptions::default(), &mut context)
}

fn left_overs(case: &Case) -> Vec<String> {
    let mut found = Vec::new();
    for entry in fs::read_dir(&case.output).expect("read out") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let kind = entry.file_type().expect("file type");
        let name = path
            .strip_prefix(&case.output)
            .expect("inside out")
            .to_string_lossy()
            .into_owned();
        if kind.is_dir() {
            for child in fs::read_dir(&path).expect("read sub") {
                let child = child.expect("sub entry");
                found.push(format!("{name}/{}", child.file_name().to_string_lossy()));
            }
        } else {
            found.push(name);
        }
    }
    found.sort();
    found
}

fn journal_lines(case: &Case) -> usize {
    match fs::read_to_string(&case.journal) {
        Ok(text) => text.lines().filter(|line| !line.trim().is_empty()).count(),
        Err(_) => 0,
    }
}

/// 双算法共用同一条断言：ZipCrypto（Info-ZIP）与 AES-256（7-Zip）。
fn extracts_with_password_into_journal(name: &str, zip: &str) {
    let case = case(name, zip);
    let plan = plan(&case, true);
    assert!(
        plan.entries[0].blocked.is_none(),
        "{zip}: 承诺提供密码后不该 blocked"
    );
    let outcome = apply(&case, &plan, Some(PASSWORD)).expect("apply with password");
    assert_eq!(outcome.state, ApplyState::Completed, "{zip}");
    assert_eq!(outcome.report.entries[0].status, ApplyStatus::Ok);
    assert_eq!(
        left_overs(&case),
        vec!["a.txt".to_string(), "sub/b.txt".to_string()],
        "{zip}: 加密包解出来的文件清单"
    );
    assert_eq!(
        fs::read(case.output.join("a.txt")).expect("read a.txt"),
        b"hello zip\n"
    );
    assert_eq!(
        fs::read(case.output.join("sub").join("b.txt")).expect("read b.txt"),
        b"second\n"
    );
    // 逐文件 Journal：撤销链不因"是加密包"而少记一条。
    assert_eq!(journal_lines(&case), 3, "{zip}: 2 个文件 + 1 个目录");

    let _ = fs::remove_dir_all(&case.root);
}

#[test]
fn zipcrypto_archive_extracts_with_the_right_password() {
    extracts_with_password_into_journal("archive-password-zipcrypto", "zipcrypto.zip");
}

#[test]
fn aes256_archive_extracts_with_the_right_password() {
    extracts_with_password_into_journal("archive-password-aes256", "aes256.zip");
}

#[test]
fn inspect_lists_encrypted_entries_without_a_password() {
    let case = case("archive-password-inspect", "zipcrypto.zip");
    let inspect = ZipArchiveService
        .inspect(case.archive.to_str().expect("utf8 path"))
        .expect("inspect without password");
    assert_eq!(inspect.entries.len(), 2);
    assert!(inspect.entries.iter().all(|entry| entry.encrypted));
    assert!(inspect.entries.iter().all(|entry| entry.supported));
    assert_eq!(
        inspect.entries[0].name.as_deref(),
        Some("a.txt"),
        "条目名不需要解密即可读出"
    );
    let _ = fs::remove_dir_all(&case.root);
}

#[test]
fn wrong_password_fails_the_entry_with_e3009_and_writes_nothing() {
    let case = case("archive-password-wrong", "zipcrypto.zip");
    let plan = plan(&case, true);
    let outcome = apply(&case, &plan, Some("not-the-password")).expect("apply runs, entry fails");
    assert_eq!(outcome.state, ApplyState::Failed);
    assert_eq!(outcome.report.entries[0].status, ApplyStatus::Failed);
    let error = outcome.report.entries[0]
        .error
        .as_ref()
        .expect("entry level error");
    // 错密码可能在打开条目时被判（InvalidPassword），也可能在读到校验位时被判
    // （CRC／认证失败）——两条路都归 E3009（ADR-0074 §5.6）。
    assert_eq!(error.code, "E3009", "{}", error.message);
    assert!(
        left_overs(&case).is_empty(),
        "错密码不得留下任何产物，实际 {:?}",
        left_overs(&case)
    );
    // 写文件前先建父目录，所以错密码时可能留下一个**空目录**——它在 Journal 里，
    // 撤销必须能把它清回去（这是"没写成功但也没留下垃圾"的可验证版本）。
    let adapter = CliFsAdapter::new(Some(case.quarantine.clone()));
    let license = AlwaysAllowLicense;
    let mut journal = FileJournal::open(case.journal.clone()).expect("reopen journal");
    let undo = undo_plan(&plan.plan_id, &adapter, &license, &mut journal).expect("undo");
    assert!(undo.failed.is_empty(), "撤销不应失败：{:?}", undo.failed);
    assert!(
        fs::read_dir(&case.output)
            .expect("read out")
            .next()
            .is_none(),
        "撤销后输出目录应为空"
    );
    let _ = fs::remove_dir_all(&case.root);
}

#[test]
fn missing_password_refuses_the_whole_plan_before_touching_disk() {
    let case = case("archive-password-missing", "zipcrypto.zip");
    let plan = plan(&case, true);
    let error = apply(&case, &plan, None).expect_err("整单拒绝");
    assert_eq!(error.code, "E3008", "{}", error.message);
    assert!(
        left_overs(&case).is_empty(),
        "整单拒绝时零落盘，实际 {:?}",
        left_overs(&case)
    );
    assert_eq!(journal_lines(&case), 0);
    let _ = fs::remove_dir_all(&case.root);
}

#[test]
fn empty_password_counts_as_missing() {
    // `by_index_decrypt(b"")` 与"不给密码"是两回事，边界上必须统一成"没给"。
    let case = case("archive-password-empty", "zipcrypto.zip");
    let plan = plan(&case, true);
    let error = apply(&case, &plan, Some("")).expect_err("空串按未提供处理");
    assert_eq!(error.code, "E3008");
    assert!(left_overs(&case).is_empty());
    let _ = fs::remove_dir_all(&case.root);
}
