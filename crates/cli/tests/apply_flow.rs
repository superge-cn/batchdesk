use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

const PLAN_ID: &str = "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8";

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        // Windows 的时钟分辨率较粗，并行测试可能取到同一时间戳；进程内序号 + PID
        // 保证路径唯一，避免一个测试的 Drop 删掉另一个测试的目录（同 config_store 修复）。
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "batchdesk-apply-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn batchdesk(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_batchdesk"))
        .args(args)
        .output()
        .expect("run batchdesk")
}

fn batchdesk_str(args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_batchdesk"))
        .args(args)
        .output()
        .expect("run batchdesk")
}

/// 带 stdin 的调用（`apply --password-stdin` 用）：密码只走管道，不进 argv。
fn batchdesk_with_stdin(args: &[&str], input: &str) -> Output {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_batchdesk"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn batchdesk");
    child
        .stdin
        .as_mut()
        .expect("stdin is piped")
        .write_all(input.as_bytes())
        .expect("write password to stdin");
    child.wait_with_output().expect("wait batchdesk")
}

/// 加密夹具（第三方工具产出，见 `fixtures/archive/README.md`）。
fn password_zip(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/archive/password")
        .join(name)
}

fn path_text(path: &Path) -> &str {
    path.to_str().expect("test paths are UTF-8")
}

#[test]
fn temp_dirs_are_unique_for_parallel_tests() {
    let dirs: Vec<TempDir> = (0..64).map(|_| TempDir::new()).collect();
    let paths: std::collections::BTreeSet<_> = dirs.iter().map(|dir| dir.0.clone()).collect();
    assert_eq!(paths.len(), dirs.len(), "并行测试的临时目录不得重名");
}

#[test]
fn apply_and_undo_restore_files() {
    let dir = TempDir::new();
    let data = dir.0.join("data");
    let quarantine = dir.0.join("quarantine");
    let plan_path = dir.0.join("plan.json");
    let journal_path = dir.0.join("journal.jsonl");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("a.txt"), b"same-content").unwrap();
    fs::write(data.join("b.txt"), b"same-content").unwrap();
    fs::write(data.join("c.txt"), b"same-content").unwrap();
    fs::write(data.join("d.txt"), b"different").unwrap();

    let plan = json!({
        "planId": PLAN_ID,
        "kind": "c09",
        "params": {
            "hash": "blake3",
            "includeZeroByte": false,
            "keep": "earliest",
            "mode": "full",
            "quickWindowBytes": 4
        },
        "entries": [
            {
                "entryId": format!("{PLAN_ID}-0001"),
                "source": path_text(&data.join("b.txt")),
                "action": "delete",
                "reason": "duplicate of a.txt"
            },
            {
                "entryId": format!("{PLAN_ID}-0002"),
                "source": path_text(&data.join("c.txt")),
                "action": "delete",
                "reason": "duplicate of a.txt"
            }
        ],
        "stats": {"total": 2, "conflict": 0, "warning": 0, "blocked": 0},
        "warnings": []
    });
    fs::write(&plan_path, serde_json::to_string_pretty(&plan).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&apply.stdout).unwrap();
    assert_eq!(report["state"], "completed");
    assert_eq!(report["report"]["entries"][0]["status"], "ok");
    assert_eq!(report["report"]["entries"][1]["status"], "ok");
    assert!(data.join("a.txt").exists());
    assert!(!data.join("b.txt").exists());
    assert!(!data.join("c.txt").exists());
    assert!(data.join("d.txt").exists());
    assert_eq!(fs::read_dir(&quarantine).unwrap().count(), 2);
    // D5 追加式 Journal：每条记录先落 pending、再追加 applied；两条记录对应
    // 两条 applied 状态行（文件总行数为 4，不再是"每记录一行"的重写形态）。
    let journal_text = fs::read_to_string(&journal_path).unwrap();
    assert_eq!(
        journal_text.matches("\"status\":\"applied\"").count(),
        2,
        "两条记录都必须确认成 applied：\n{journal_text}"
    );

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        undo.status.success(),
        "undo failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&undo.stdout),
        String::from_utf8_lossy(&undo.stderr)
    );
    let undo_report: serde_json::Value = serde_json::from_slice(&undo.stdout).unwrap();
    assert_eq!(undo_report["reverted"], 2);
    assert!(undo_report["failed"].as_array().unwrap().is_empty());
    assert!(data.join("a.txt").exists());
    assert!(data.join("b.txt").exists());
    assert!(data.join("c.txt").exists());
    assert!(data.join("d.txt").exists());
    assert_eq!(fs::read_dir(&quarantine).unwrap().count(), 0);

    let journal = fs::read_to_string(&journal_path).unwrap();
    assert_eq!(journal.matches("\"status\":\"reverted\"").count(), 2);
}

#[test]
fn apply_skips_entries_changed_between_plan_and_apply() {
    let dir = TempDir::new();
    let data = dir.0.join("data");
    let quarantine = dir.0.join("quarantine");
    let plan_path = dir.0.join("plan.json");
    let journal_path = dir.0.join("journal.jsonl");
    fs::create_dir_all(&data).unwrap();
    // 同内容三份：earliest 保留最先写入的 a.txt，b/c 进入清理计划
    fs::write(data.join("a.txt"), b"same-content").unwrap();
    fs::write(data.join("b.txt"), b"same-content").unwrap();
    fs::write(data.join("c.txt"), b"same-content").unwrap();

    // 用真实 CLI 生成计划：plan_c09 会写入 expected 内容指纹（设计 01 §5，E2010 的前提）
    let plan_run = batchdesk(&[
        "c09",
        path_text(&data),
        "--quick-window-bytes",
        "4",
        "--plan",
        "--json",
    ]);
    assert!(
        plan_run.status.success(),
        "plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan_run.stdout),
        String::from_utf8_lossy(&plan_run.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&plan_run.stdout).unwrap();
    assert_eq!(plan["stats"]["total"], 2);
    for entry in plan["entries"].as_array().unwrap() {
        assert!(
            entry["expected"]["hash"].as_str().is_some(),
            "plan entry must carry a content fingerprint"
        );
        assert_eq!(entry["expected"]["algorithm"], "blake3");
    }
    fs::write(&plan_path, plan_run.stdout).unwrap();

    // plan→apply 之间篡改两份待清理文件：b.txt 改大小，c.txt 同尺寸改内容
    fs::write(data.join("b.txt"), b"same-content-longer").unwrap();
    fs::write(data.join("c.txt"), b"same-contenf").unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&apply.stdout).unwrap();
    assert_eq!(report["state"], "completed");
    for entry in report["report"]["entries"].as_array().unwrap() {
        assert_eq!(entry["status"], "skipped");
        assert_eq!(entry["error"]["code"], "E2010");
    }
    // 变更条目保留原位；隔离目录与 Journal 均不产生任何动作
    assert!(data.join("a.txt").exists());
    assert!(data.join("b.txt").exists());
    assert!(data.join("c.txt").exists());
    if journal_path.exists() {
        assert_eq!(
            fs::read_to_string(&journal_path).unwrap().lines().count(),
            0
        );
    }
    if quarantine.exists() {
        assert_eq!(fs::read_dir(&quarantine).unwrap().count(), 0);
    }
}

#[test]
fn cleanup_and_undo_respect_retention_window() {
    let dir = TempDir::new();
    let quarantine = dir.0.join("quarantine");
    let stale = quarantine.join("stale.txt");
    let journal_path = dir.0.join("journal.jsonl");
    fs::create_dir_all(&quarantine).unwrap();
    fs::write(&stale, b"stale").unwrap();
    let journal = json!({
        "seq": 1,
        "planId": PLAN_ID,
        "entryId": format!("{PLAN_ID}-0001"),
        "ts": "2000-01-01T00:00:00Z",
        "forward": {
            "op": "quarantine",
            "from": path_text(&dir.0.join("original.txt")),
            "to": path_text(&stale)
        },
        "reverse": {
            "op": "rename",
            "from": path_text(&stale),
            "to": path_text(&dir.0.join("original.txt"))
        },
        "status": "applied"
    });
    fs::write(
        &journal_path,
        format!("{}\n", serde_json::to_string(&journal).unwrap()),
    )
    .unwrap();

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--retention-days",
        "1",
        "--json",
    ]);
    assert!(!undo.status.success());
    assert!(String::from_utf8_lossy(&undo.stderr).contains("undo window expired"));
    assert!(stale.exists());

    let dry_run = batchdesk(&[
        "cleanup",
        "--journal",
        path_text(&journal_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--retention-days",
        "1",
        "--json",
    ]);
    assert!(
        dry_run.status.success(),
        "cleanup dry-run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&dry_run.stdout),
        String::from_utf8_lossy(&dry_run.stderr)
    );
    let dry_report: serde_json::Value = serde_json::from_slice(&dry_run.stdout).unwrap();
    assert_eq!(dry_report["dryRun"], true);
    assert_eq!(dry_report["expiredEntries"], 1);
    assert_eq!(dry_report["removedEntries"], 0);
    assert!(stale.exists());
    assert_eq!(
        fs::read_to_string(&journal_path).unwrap().lines().count(),
        1
    );

    let cleanup = batchdesk(&[
        "cleanup",
        "--journal",
        path_text(&journal_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--retention-days",
        "1",
        "--apply",
        "--json",
    ]);
    assert!(
        cleanup.status.success(),
        "cleanup apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&cleanup.stdout),
        String::from_utf8_lossy(&cleanup.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&cleanup.stdout).unwrap();
    assert_eq!(report["dryRun"], false);
    assert_eq!(report["removedEntries"], 1);
    assert_eq!(report["removedFiles"], 1);
    assert!(!stale.exists());
    assert_eq!(
        fs::read_to_string(&journal_path).unwrap().lines().count(),
        0
    );
}

#[test]
fn c01_reports_invalid_sequence_replacement_counts_per_entry() {
    // 方案 §5.2"默认替换并计数上报"的执行侧验收：转码遇到的非法序列计数必须以**条目级 detail**
    // 出现在执行报告里（JSON 与人读输出都要有），且合法文件仍上报 0（不是缺字段）。
    let dir = TempDir::new();
    let dirty = dir.0.join("dirty.txt");
    let clean = dir.0.join("clean.txt");
    fs::write(&dirty, b"ab\xff\xffcd".as_slice()).unwrap();
    fs::write(&clean, b"plain ascii line\n").unwrap();
    let dirty_original = fs::read(&dirty).unwrap();
    let plan_path = dir.0.join("plan.json");
    let journal = dir.0.join("journal.jsonl");
    let quarantine = dir.0.join("q");

    let plan = batchdesk(&[
        "c01",
        "--file",
        path_text(&dirty),
        "--file",
        path_text(&clean),
        "--target",
        "utf-8",
        "--source",
        "gb18030",
        "--sample-window",
        "64",
        "--sample-blocks",
        "0",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c01 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let plan_doc: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    fs::write(&plan_path, &plan.stdout).unwrap();

    let applied = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal),
        "--json",
    ]);
    assert!(
        applied.status.success(),
        "c01 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&applied.stdout),
        String::from_utf8_lossy(&applied.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&applied.stdout).unwrap();
    assert_eq!(report["state"], "completed");
    let detail_for = |file: &Path| -> String {
        let wanted = path_text(file);
        let entry = plan_doc["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["source"] == wanted)
            .unwrap_or_else(|| panic!("计划里缺少 {wanted} 的条目"));
        let entry_id = entry["entryId"].as_str().unwrap();
        let reported = report["report"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["entryId"] == entry_id)
            .unwrap_or_else(|| panic!("执行报告缺少条目 {entry_id}"));
        reported["detail"]
            .as_str()
            .unwrap_or("<detail 字段缺失>")
            .to_string()
    };
    assert_eq!(detail_for(&dirty), "invalid sequences replaced: 2");
    assert_eq!(
        detail_for(&clean),
        "invalid sequences replaced: 0",
        "合法文件也要上报计数，不能缺字段"
    );

    // 人读输出同样要看得见：把脏文件还原后再跑一次不带 --json 的 apply。
    fs::write(&dirty, &dirty_original).unwrap();
    let human = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&dir.0.join("q2")),
        "--journal",
        path_text(&dir.0.join("journal2.jsonl")),
    ]);
    assert!(
        human.status.success(),
        "human apply failed\nstderr:\n{}",
        String::from_utf8_lossy(&human.stderr)
    );
    let stdout = String::from_utf8_lossy(&human.stdout).into_owned();
    assert!(
        stdout.contains("invalid sequences replaced: 2"),
        "人读输出未显示条目级计数：{stdout}"
    );
}

#[test]
fn c01_plan_apply_and_undo_round_trip() {
    let dir = TempDir::new();
    let quarantine = dir.0.join("quarantine");
    let plan_path = dir.0.join("plan.json");
    let journal_path = dir.0.join("journal.jsonl");
    let source = dir.0.join("legacy.txt");
    let utf8_text = "中文内容测试\n第二行\n";

    // 用 kernel 纯函数把 UTF-8 文本转成 GB18030 字节，作为历史遗留输入。
    let transcoded = kernel::encoding::transcode(
        utf8_text.as_bytes(),
        kernel::encoding::SourceEncoding::Utf8,
        kernel::encoding::TargetEncoding::Gb18030,
        kernel::encoding::InvalidSequencePolicy::ReplaceAndCount,
    )
    .expect("encode GB18030 fixture");
    fs::write(&source, &transcoded.bytes).unwrap();
    let original = fs::read(&source).unwrap();
    assert_ne!(original, utf8_text.as_bytes());

    let plan_run = batchdesk(&[
        "c01",
        "--file",
        path_text(&source),
        "--target",
        "utf-8",
        "--source",
        "gb18030",
        "--sample-window",
        "8",
        "--sample-blocks",
        "1",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan_run.status.success(),
        "c01 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan_run.stdout),
        String::from_utf8_lossy(&plan_run.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&plan_run.stdout).unwrap();
    assert_eq!(plan["kind"], "c01");
    assert_eq!(plan["stats"]["total"], 1);
    assert_eq!(plan["entries"][0]["action"], "edit-content");
    assert_eq!(plan["entries"][0]["source"], path_text(&source));
    assert_eq!(plan["params"]["target"], "utf-8");
    assert_eq!(plan["params"]["source"], "gb18030");
    fs::write(&plan_path, &plan_run.stdout).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c01 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&apply.stdout).unwrap();
    assert_eq!(report["state"], "completed");
    assert_eq!(report["report"]["entries"][0]["status"], "ok");
    assert_eq!(fs::read(&source).unwrap(), utf8_text.as_bytes());
    assert_eq!(fs::read_dir(&quarantine).unwrap().count(), 1);

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        undo.status.success(),
        "c01 undo failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&undo.stdout),
        String::from_utf8_lossy(&undo.stderr)
    );
    let undo_report: serde_json::Value = serde_json::from_slice(&undo.stdout).unwrap();
    assert_eq!(undo_report["reverted"], 1);
    assert_eq!(
        fs::read(&source).unwrap(),
        original,
        "undo 必须还原原始 GB18030 字节"
    );
    // 覆盖写的备份是复制而非移动：撤销后仍留在隔离目录，由保留期清理（设计 01 §8）。
    assert_eq!(fs::read_dir(&quarantine).unwrap().count(), 1);
}

#[test]
fn f01_plan_apply_and_undo_write_line_aligned_parts() {
    let dir = TempDir::new();
    let source = dir.0.join("big.txt");
    let parts = dir.0.join("parts");
    let quarantine = dir.0.join("quarantine");
    let plan_path = dir.0.join("plan.json");
    let journal_path = dir.0.join("journal.jsonl");
    fs::write(&source, b"aaa\nbbb\nccc\n").unwrap();

    let plan = batchdesk(&[
        "f01",
        "--file",
        path_text(&source),
        "--target-bytes",
        "6",
        "--output-dir",
        path_text(&parts),
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f01 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "f01");
    assert_eq!(document["entries"].as_array().unwrap().len(), 2);
    assert_eq!(document["entries"][0]["action"], "split");
    assert_eq!(document["entries"][0]["range"]["start"], 0);
    assert_eq!(document["entries"][0]["range"]["end"], 8);
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "f01 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&apply.stdout).unwrap();
    assert_eq!(report["state"], "completed");
    assert_eq!(fs::read(parts.join("big.1.txt")).unwrap(), b"aaa\nbbb\n");
    assert_eq!(fs::read(parts.join("big.2.txt")).unwrap(), b"ccc\n");
    assert_eq!(fs::read(&source).unwrap(), b"aaa\nbbb\nccc\n");

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        undo.status.success(),
        "f01 undo failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&undo.stdout),
        String::from_utf8_lossy(&undo.stderr)
    );
    let undo_report: serde_json::Value = serde_json::from_slice(&undo.stdout).unwrap();
    assert_eq!(undo_report["reverted"], 2);
    assert!(!parts.join("big.1.txt").exists(), "undo quarantines part 1");
    assert!(!parts.join("big.2.txt").exists(), "undo quarantines part 2");
}

#[test]
fn f02_plan_apply_and_undo_split_every_n_lines() {
    let dir = TempDir::new();
    let source = dir.0.join("rows.txt");
    let parts = dir.0.join("parts");
    let quarantine = dir.0.join("quarantine");
    let plan_path = dir.0.join("plan.json");
    let journal_path = dir.0.join("journal.jsonl");
    fs::write(&source, b"aaa\nbbb\nccc\nddd\neee\n").unwrap();

    let plan = batchdesk(&[
        "f02",
        "--file",
        path_text(&source),
        "--lines",
        "2",
        "--output-dir",
        path_text(&parts),
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f02 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "f02");
    assert_eq!(document["params"]["linesPerPart"], 2);
    assert_eq!(document["entries"].as_array().unwrap().len(), 3);
    assert_eq!(document["entries"][0]["action"], "split");
    assert_eq!(document["entries"][2]["relation"]["partTotal"], 3);
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "f02 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&apply.stdout).unwrap();
    assert_eq!(report["state"], "completed");
    assert_eq!(fs::read(parts.join("rows.1.txt")).unwrap(), b"aaa\nbbb\n");
    assert_eq!(fs::read(parts.join("rows.2.txt")).unwrap(), b"ccc\nddd\n");
    assert_eq!(
        fs::read(parts.join("rows.3.txt")).unwrap(),
        b"eee\n",
        "尾片保留（方案 §5.15）"
    );
    assert_eq!(fs::read(&source).unwrap(), b"aaa\nbbb\nccc\nddd\neee\n");

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        undo.status.success(),
        "f02 undo failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&undo.stdout),
        String::from_utf8_lossy(&undo.stderr)
    );
    let undo_report: serde_json::Value = serde_json::from_slice(&undo.stdout).unwrap();
    assert_eq!(undo_report["reverted"], 3);
    assert!(
        !parts.join("rows.1.txt").exists(),
        "undo quarantines part 1"
    );
    assert!(
        !parts.join("rows.3.txt").exists(),
        "undo quarantines part 3"
    );
}

#[test]
fn f03_plan_apply_and_undo_split_by_part_count() {
    let dir = TempDir::new();
    let source = dir.0.join("ten.txt");
    let parts = dir.0.join("parts");
    let quarantine = dir.0.join("quarantine");
    let plan_path = dir.0.join("plan.json");
    let journal_path = dir.0.join("journal.jsonl");
    let body: String = (1..=10).map(|line| format!("{line:02}\n")).collect();
    fs::write(&source, body.as_bytes()).unwrap();

    let plan = batchdesk(&[
        "f03",
        "--file",
        path_text(&source),
        "--parts",
        "3",
        "--output-dir",
        path_text(&parts),
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f03 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "f03");
    assert_eq!(document["params"]["parts"], 3);
    assert_eq!(document["entries"].as_array().unwrap().len(), 3);
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "f03 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&apply.stdout).unwrap();
    assert_eq!(report["state"], "completed");
    assert_eq!(
        fs::read(parts.join("ten.1.txt")).unwrap(),
        b"01\n02\n03\n04\n",
        "10 行分 3 份，余数进前几片（方案 §5.15）"
    );
    assert_eq!(fs::read(parts.join("ten.2.txt")).unwrap(), b"05\n06\n07\n");
    assert_eq!(fs::read(parts.join("ten.3.txt")).unwrap(), b"08\n09\n10\n");

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        undo.status.success(),
        "f03 undo failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&undo.stdout),
        String::from_utf8_lossy(&undo.stderr)
    );
    let undo_report: serde_json::Value = serde_json::from_slice(&undo.stdout).unwrap();
    assert_eq!(undo_report["reverted"], 3);
    assert!(!parts.join("ten.1.txt").exists(), "undo quarantines part 1");
}

#[test]
fn f04_plan_apply_and_undo_merge_with_encoding_unification() {
    let dir = TempDir::new();
    let first = dir.0.join("first.txt");
    let second = dir.0.join("second.txt");
    let output = dir.0.join("merged.txt");
    let quarantine = dir.0.join("quarantine");
    let plan_path = dir.0.join("plan.json");
    let journal_path = dir.0.join("journal.jsonl");
    // first 为 ASCII（GB18030 兼容），second 为 GB18030 的「中」；统一转 UTF-8 后按序拼接。
    fs::write(&first, b"A\n").unwrap();
    fs::write(&second, [0xD6, 0xD0, 0x0A]).unwrap();

    let plan = batchdesk(&[
        "f04",
        "--file",
        path_text(&first),
        "--file",
        path_text(&second),
        "--output",
        path_text(&output),
        "--target",
        "utf-8",
        "--source",
        "gb18030",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f04 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "f04");
    assert_eq!(document["params"]["fileCount"], 2);
    assert_eq!(
        document["entries"].as_array().unwrap().len(),
        1,
        "N:1 汇总为一条条目"
    );
    assert_eq!(document["entries"][0]["action"], "merge");
    assert_eq!(
        document["entries"][0]["merge"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "f04 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&apply.stdout).unwrap();
    assert_eq!(report["state"], "completed");
    assert_eq!(report["report"]["entries"][0]["status"], "ok");
    assert_eq!(
        fs::read(&output).unwrap(),
        "A\n中\n".as_bytes(),
        "输入按给定顺序拼接，且统一转码到目标编码"
    );
    assert!(first.exists());
    assert!(second.exists());

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        undo.status.success(),
        "f04 undo failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&undo.stdout),
        String::from_utf8_lossy(&undo.stderr)
    );
    let undo_report: serde_json::Value = serde_json::from_slice(&undo.stdout).unwrap();
    assert_eq!(undo_report["reverted"], 1);
    assert!(
        !output.exists(),
        "undo moves the merged output into quarantine"
    );
}

#[test]
fn f05_plan_apply_and_undo_repeats_the_csv_header() {
    let dir = TempDir::new();
    let source = dir.0.join("data.csv");
    let parts = dir.0.join("parts");
    let quarantine = dir.0.join("quarantine");
    let plan_path = dir.0.join("plan.json");
    let journal_path = dir.0.join("journal.jsonl");
    fs::write(&source, b"h1,h2\n1,2\n3,4\n5,6\n").unwrap();

    let plan = batchdesk(&[
        "f05",
        "--file",
        path_text(&source),
        "--rows",
        "2",
        "--output-dir",
        path_text(&parts),
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f05 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "f05");
    assert_eq!(document["entries"].as_array().unwrap().len(), 2);
    assert_eq!(document["entries"][0]["action"], "split");
    assert_eq!(document["entries"][0]["range"]["start"], 6);
    assert_eq!(document["entries"][0]["csv"]["header"]["end"], 6);
    assert_eq!(document["entries"][0]["csv"]["rows"], 2);
    assert_eq!(document["entries"][1]["range"]["start"], 14);
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "f05 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&apply.stdout).unwrap();
    assert_eq!(report["state"], "completed");
    assert_eq!(
        fs::read(parts.join("data.1.csv")).unwrap(),
        b"h1,h2\n1,2\n3,4\n",
        "every part repeats the header"
    );
    assert_eq!(fs::read(parts.join("data.2.csv")).unwrap(), b"h1,h2\n5,6\n");
    assert_eq!(fs::read(&source).unwrap(), b"h1,h2\n1,2\n3,4\n5,6\n");

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        undo.status.success(),
        "f05 undo failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&undo.stdout),
        String::from_utf8_lossy(&undo.stderr)
    );
    let undo_report: serde_json::Value = serde_json::from_slice(&undo.stdout).unwrap();
    assert_eq!(undo_report["reverted"], 2);
    assert!(!parts.join("data.1.csv").exists());
    assert!(!parts.join("data.2.csv").exists());
}

#[test]
fn k08_utf16_csv_parts_each_start_with_a_bom() {
    let dir = TempDir::new();
    let source = dir.0.join("k08.csv");
    let parts = dir.0.join("parts");
    let quarantine = dir.0.join("quarantine");
    let plan_path = dir.0.join("plan.json");
    let journal_path = dir.0.join("journal.jsonl");
    let sample = fs::read(corpus_file("K08", "K08.csv")).expect("先运行语料生成脚本");
    assert!(
        sample.starts_with(&[0xFF, 0xFE]),
        "K08 样本是 UTF-16LE + BOM"
    );
    fs::write(&source, &sample).unwrap();

    let plan = batchdesk(&[
        "f05",
        "--file",
        path_text(&source),
        "--rows",
        "2",
        "--output-dir",
        path_text(&parts),
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "k08 f05 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "f05");
    let entries = document["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3, "表头 + 5 条数据行按每片 2 行切 → 3 片");
    for entry in entries {
        assert!(entry["blocked"].is_null(), "带 BOM 的 UTF-16 不该被阻断");
        assert_eq!(
            entry["csv"]["header"]["start"], 0,
            "表头区间从 0 起、含 BOM"
        );
    }
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "k08 f05 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );

    // T v1.12 §3.2 K08：每片均写 BOM（方案 §5.1）。缺一片就无法被解码器识别。
    let first = fs::read(parts.join("k08.1.csv")).unwrap();
    let second = fs::read(parts.join("k08.2.csv")).unwrap();
    let third = fs::read(parts.join("k08.3.csv")).unwrap();
    for (name, bytes) in [
        ("k08.1.csv", &first),
        ("k08.2.csv", &second),
        ("k08.3.csv", &third),
    ] {
        assert!(
            bytes.starts_with(&[0xFF, 0xFE]),
            "{name} 必须以 UTF-16LE BOM 开头，实际前两字节 {:?}",
            &bytes[..2.min(bytes.len())]
        );
    }
    let first_text = decode_utf16le(&first);
    assert!(
        first_text.starts_with("姓名,部门,金额,备注"),
        "每片都保留表头"
    );
    assert_eq!(first_text.lines().count(), 3, "表头 + 2 条记录");
    let third_text = decode_utf16le(&third);
    assert!(third_text.starts_with("姓名,部门,金额,备注"));
    assert_eq!(third_text.lines().count(), 2, "表头 + 1 条记录");
}

/// 不保留表头时，第 2 片起的区间不再包含源文件开头的 BOM：
/// 必须由执行层补写前缀，否则这些分片是无 BOM 的 UTF-16（无法识别）。
#[test]
fn k08_utf16_csv_parts_without_header_still_start_with_a_bom() {
    let dir = TempDir::new();
    let source = dir.0.join("k08-noheader.csv");
    let parts = dir.0.join("parts");
    let quarantine = dir.0.join("quarantine");
    let plan_path = dir.0.join("plan.json");
    let journal_path = dir.0.join("journal.jsonl");
    fs::write(&source, fs::read(corpus_file("K08", "K08.csv")).unwrap()).unwrap();

    let plan = batchdesk(&[
        "f05",
        "--file",
        path_text(&source),
        "--rows",
        "2",
        "--no-keep-header",
        "--output-dir",
        path_text(&parts),
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "k08 f05 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    let entries = document["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3, "6 条记录按每片 2 行切 → 3 片");
    assert!(
        entries[0]["csv"]["shardBom"].is_null(),
        "首片区间从 0 起，源字节已含 BOM"
    );
    for entry in &entries[1..] {
        assert_eq!(entry["csv"]["shardBom"], "utf16-le", "第 2 片起补写 BOM");
        let start = entry["range"]["start"].as_u64().unwrap();
        let end = entry["range"]["end"].as_u64().unwrap();
        assert_eq!(
            entry["expected"]["size"].as_u64().unwrap(),
            2 + (end - start),
            "期望字节数必须把补写的 BOM 算进去"
        );
    }
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "k08 f05 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );

    for index in 1..=3 {
        let bytes = fs::read(parts.join(format!("k08-noheader.{index}.csv"))).unwrap();
        assert!(
            bytes.starts_with(&[0xFF, 0xFE]),
            "第 {index} 片缺少 BOM，Excel 打开会乱码"
        );
        let text = decode_utf16le(&bytes);
        assert_eq!(text.lines().count(), 2, "每片两条记录（不保留表头）");
    }
    // 第 2 片的内容必须正好是「BOM + 源文件里那两条记录」，不能多也不能少。
    let second = fs::read(parts.join("k08-noheader.2.csv")).unwrap();
    let range = &entries[1]["range"];
    let start = range["start"].as_u64().unwrap() as usize;
    let end = range["end"].as_u64().unwrap() as usize;
    let sample = fs::read(corpus_file("K08", "K08.csv")).unwrap();
    assert_eq!(&second[2..], &sample[start..end]);
}

fn decode_utf16le(bytes: &[u8]) -> String {
    assert!(bytes.starts_with(&[0xFF, 0xFE]), "缺少 UTF-16LE BOM");
    let units: Vec<u16> = bytes[2..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    String::from_utf16(&units).expect("分片内容是合法 UTF-16")
}

#[test]
fn f06_plan_apply_and_undo_aligns_columns_and_adds_source_column() {
    let dir = TempDir::new();
    let first = dir.0.join("a.csv");
    let second = dir.0.join("b.csv");
    let output = dir.0.join("all.csv");
    let quarantine = dir.0.join("quarantine");
    let plan_path = dir.0.join("plan.json");
    let journal_path = dir.0.join("journal.jsonl");
    fs::write(&first, b"id,name\n1,Alice\n").unwrap();
    fs::write(&second, b"name,extra\nBob,x\n").unwrap();

    let plan = batchdesk(&[
        "f06",
        "--file",
        path_text(&first),
        "--file",
        path_text(&second),
        "--output",
        path_text(&output),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--source-column",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f06 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "f06");
    assert_eq!(document["entries"].as_array().unwrap().len(), 1);
    assert_eq!(document["entries"][0]["action"], "merge");
    assert_eq!(
        document["entries"][0]["csvMerge"]["union"],
        json!(["id", "name", "extra"])
    );
    assert_eq!(
        document["entries"][0]["csvMerge"]["sourceColumn"],
        "来源文件"
    );
    assert_eq!(document["entries"][0]["csvMerge"]["hasHeader"], true);
    assert!(document["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["kind"] == "schema-missing-columns"));
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "f06 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&apply.stdout).unwrap();
    assert_eq!(report["state"], "completed");
    assert_eq!(report["report"]["entries"][0]["status"], "ok");
    assert_eq!(
        fs::read(&output).unwrap(),
        "来源文件,id,name,extra\na.csv,1,Alice,\nb.csv,,Bob,x\n".as_bytes(),
        "columns align by header name, missing values are empty, extras are appended"
    );
    assert!(first.exists());
    assert!(second.exists());

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        undo.status.success(),
        "f06 undo failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&undo.stdout),
        String::from_utf8_lossy(&undo.stderr)
    );
    let undo_report: serde_json::Value = serde_json::from_slice(&undo.stdout).unwrap();
    assert_eq!(undo_report["reverted"], 1);
    assert!(
        !output.exists(),
        "undo moves the merged output into quarantine"
    );
}

#[test]
fn f06_strict_mode_blocks_a_schema_mismatch() {
    let dir = TempDir::new();
    let first = dir.0.join("a.csv");
    let second = dir.0.join("b.csv");
    let output = dir.0.join("all.csv");
    fs::write(&first, b"id,name\n1,Alice\n").unwrap();
    fs::write(&second, b"name\nBob\n").unwrap();

    let plan = batchdesk(&[
        "f06",
        "--file",
        path_text(&first),
        "--file",
        path_text(&second),
        "--output",
        path_text(&output),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--strict",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f06 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["entries"][0]["blocked"]["code"], "E3004");
    assert!(!output.exists());
}

#[test]
fn f06_plan_surfaces_headerless_and_header_normalization_warnings() {
    let dir = TempDir::new();
    let headerless = dir.0.join("headerless.csv");
    let padded = dir.0.join("padded.csv");
    let out_headerless = dir.0.join("out-headerless.csv");
    let out_padded = dir.0.join("out-padded.csv");
    fs::write(&headerless, "1001,张伟,1200.00\n1002,李娜,3800.50\n").unwrap();
    fs::write(
        &padded,
        "姓名,姓名, 部门 ,金额 \n张伟,张伟（别名）,行政部,1200.00\n",
    )
    .unwrap();

    let headerless_plan = batchdesk(&[
        "f06",
        "--file",
        path_text(&headerless),
        "--output",
        path_text(&out_headerless),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--no-header",
    ]);
    assert!(
        headerless_plan.status.success(),
        "f06 headerless plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&headerless_plan.stdout),
        String::from_utf8_lossy(&headerless_plan.stderr)
    );
    assert!(
        String::from_utf8_lossy(&headerless_plan.stdout).contains("warning no-header-positional"),
        "无表头计划必须在 CLI 输出提示"
    );

    let padded_plan = batchdesk(&[
        "f06",
        "--file",
        path_text(&padded),
        "--output",
        path_text(&out_padded),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
    ]);
    assert!(
        padded_plan.status.success(),
        "f06 padded-header plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&padded_plan.stdout),
        String::from_utf8_lossy(&padded_plan.stderr)
    );
    let stdout = String::from_utf8_lossy(&padded_plan.stdout);
    assert!(
        stdout.contains("warning header-normalized"),
        "表头规范化必须在 CLI 输出提示"
    );
    assert!(
        stdout.contains("warning duplicate-column-names"),
        "重复列名必须在 CLI 输出提示"
    );
    assert!(!out_headerless.exists());
    assert!(!out_padded.exists());
}

#[test]
fn f04_detects_encodings_and_writes_one_bom() {
    let dir = TempDir::new();
    let first = dir.0.join("first.txt");
    let second = dir.0.join("second.txt");
    let output = dir.0.join("merged.txt");
    let quarantine = dir.0.join("quarantine");
    let plan_path = dir.0.join("plan.json");
    let journal_path = dir.0.join("journal.jsonl");
    fs::write(&first, "第一行\n").unwrap();
    fs::write(&second, "第二行\n").unwrap();

    let plan = batchdesk(&[
        "f04",
        "--file",
        path_text(&first),
        "--file",
        path_text(&second),
        "--output",
        path_text(&output),
        "--target",
        "utf-8-bom",
        "--sample-window",
        "64",
        "--sample-blocks",
        "0",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f04 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(
        document["entries"][0]["merge"]["targetEncoding"],
        "utf-8-bom"
    );
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "f04 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert_eq!(
        fs::read(&output).unwrap(),
        b"\xEF\xBB\xBF\xE7\xAC\xAC\xE4\xB8\x80\xE8\xA1\x8C\n\xE7\xAC\xAC\xE4\xBA\x8C\xE8\xA1\x8C\n",
        "BOM 只在合并产物开头出现一次"
    );
}

#[test]
fn f07_plan_apply_and_undo_extracts_columns_in_order() {
    let dir = TempDir::new();
    let first = dir.0.join("a.csv");
    let second = dir.0.join("b.csv");
    let out_dir = dir.0.join("out");
    let quarantine = dir.0.join("quarantine");
    let plan_path = dir.0.join("plan.json");
    let journal_path = dir.0.join("journal.jsonl");
    fs::create_dir_all(&out_dir).unwrap();
    fs::write(&first, b"id, name \n1,Alice\n").unwrap();
    fs::write(&second, b"name,id\nBob,2\n").unwrap();

    let plan = batchdesk(&[
        "f07",
        "--file",
        path_text(&first),
        "--file",
        path_text(&second),
        "--column",
        "name",
        "--column",
        "id",
        "--output-dir",
        path_text(&out_dir),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f07 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "f07");
    assert_eq!(document["entries"].as_array().unwrap().len(), 2);
    assert_eq!(document["entries"][0]["action"], "csv-extract");
    assert_eq!(
        document["entries"][0]["csvExtract"]["selections"],
        json!([
            {"index": 1, "name": "name"},
            {"index": 0, "name": "id"}
        ]),
        "header whitespace is normalized before matching"
    );
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "f07 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&apply.stdout).unwrap();
    assert_eq!(report["state"], "completed");
    assert_eq!(
        fs::read(out_dir.join("a.csv")).unwrap(),
        b"name,id\nAlice,1\n",
        "selected columns keep the requested order and row order"
    );
    assert_eq!(
        fs::read(out_dir.join("b.csv")).unwrap(),
        b"name,id\nBob,2\n"
    );
    assert_eq!(fs::read(&first).unwrap(), b"id, name \n1,Alice\n");
    assert_eq!(fs::read(&second).unwrap(), b"name,id\nBob,2\n");

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        undo.status.success(),
        "f07 undo failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&undo.stdout),
        String::from_utf8_lossy(&undo.stderr)
    );
    let undo_report: serde_json::Value = serde_json::from_slice(&undo.stdout).unwrap();
    assert_eq!(undo_report["reverted"], 2);
    assert!(!out_dir.join("a.csv").exists());
    assert!(!out_dir.join("b.csv").exists());
}

#[test]
fn f08_plan_apply_and_undo_takes_the_first_rows() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let output = dir.0.join("first.csv");
    let quarantine = dir.0.join("quarantine");
    let plan_path = dir.0.join("plan.json");
    let journal_path = dir.0.join("journal.jsonl");
    fs::write(&source, b"id,name\n1,Alice\n2,Bob\n3,Cara\n").unwrap();

    let plan = batchdesk(&[
        "f08",
        "--file",
        path_text(&source),
        "--first",
        "2",
        "--output",
        path_text(&output),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f08 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "f08");
    assert_eq!(document["entries"][0]["action"], "csv-rows");
    assert_eq!(document["entries"][0]["csvRows"]["window"], "first");
    assert_eq!(document["entries"][0]["csvRows"]["count"], 2);
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "f08 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert_eq!(
        fs::read(&output).unwrap(),
        b"id,name\n1,Alice\n2,Bob\n",
        "first N keeps the header and the first data rows"
    );

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(undo.status.success());
    let undo_report: serde_json::Value = serde_json::from_slice(&undo.stdout).unwrap();
    assert_eq!(undo_report["reverted"], 1);
    assert!(!output.exists());
    assert!(source.exists());
}

#[test]
fn f08_last_window_keeps_the_tail() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let output = dir.0.join("last.csv");
    fs::write(&source, b"id,name\n1,Alice\n2,Bob\n3,Cara\n").unwrap();

    let plan = batchdesk(&[
        "f08",
        "--file",
        path_text(&source),
        "--last",
        "2",
        "--output",
        path_text(&output),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    let payload = &document["entries"][0]["csvRows"];
    assert_eq!(payload["window"], "last");
    assert_eq!(payload["count"], 2);
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let quarantine = dir.0.join("quarantine");
    let journal = dir.0.join("journal.jsonl");
    let plan_path = dir.0.join("plan.json");
    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "f08 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert_eq!(
        fs::read(&output).unwrap(),
        b"id,name\n2,Bob\n3,Cara\n",
        "last N keeps the tail data rows"
    );
}

#[test]
fn f08_zero_rows_writes_only_the_header() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let output = dir.0.join("empty.csv");
    fs::write(&source, b"id,name\n1,Alice\n").unwrap();

    let plan = batchdesk(&[
        "f08",
        "--file",
        path_text(&source),
        "--first",
        "0",
        "--output",
        path_text(&output),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["entries"][0]["csvRows"]["count"], 0);
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(apply.status.success());
    assert_eq!(fs::read(&output).unwrap(), b"id,name\n");
}

#[test]
fn f09_plan_apply_and_undo_extracts_row_range_preserving_crlf() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let output = dir.0.join("range.csv");
    let data = b"id,note\r\n1,Alice\r\n2,Bob\r\n3,Cara\r\n4,Dan\r\n5,Eve\r\n";
    fs::write(&source, data).unwrap();

    let plan = batchdesk(&[
        "f09",
        "--file",
        path_text(&source),
        "--start",
        "2",
        "--end",
        "4",
        "--output",
        path_text(&output),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f09 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "f09");
    assert_eq!(document["entries"].as_array().unwrap().len(), 1);
    let payload = &document["entries"][0]["csvRows"];
    assert_eq!(payload["window"], "range");
    assert_eq!(payload["startRow"], 2);
    assert_eq!(payload["endRow"], 4);
    assert_eq!(payload["count"], 3);
    assert!(payload["headerRange"].is_object());
    assert!(payload["dataRange"].is_object());
    assert_eq!(payload["totalKnown"], false, "plan stops at endRow");
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "f09 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert_eq!(
        fs::read(&output).unwrap(),
        b"id,note\r\n2,Bob\r\n3,Cara\r\n4,Dan\r\n",
        "the selected range keeps the source CRLF line endings"
    );

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(undo.status.success());
    assert!(!output.exists());
}

#[test]
fn f09_range_beyond_rows_warns_and_clamps_the_output() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let output = dir.0.join("range.csv");
    fs::write(&source, b"id,name\n1,Alice\n2,Bob\n3,Cara\n").unwrap();

    let plan = batchdesk(&[
        "f09",
        "--file",
        path_text(&source),
        "--start",
        "2",
        "--end",
        "9",
        "--output",
        path_text(&output),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert!(document["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["kind"] == "range-exceeds-rows"));
    assert_eq!(document["entries"][0]["csvRows"]["count"], 2);
    assert_eq!(document["entries"][0]["csvRows"]["dataRows"], 3);
    assert_eq!(document["entries"][0]["csvRows"]["totalKnown"], true);
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(apply.status.success());
    assert_eq!(fs::read(&output).unwrap(), b"id,name\n2,Bob\n3,Cara\n");
}

#[test]
fn f09_rejects_an_invalid_range() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let output = dir.0.join("range.csv");
    fs::write(&source, b"id,name\n1,Alice\n").unwrap();

    let plan = batchdesk(&[
        "f09",
        "--file",
        path_text(&source),
        "--start",
        "4",
        "--end",
        "3",
        "--output",
        path_text(&output),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
    ]);
    assert!(!plan.status.success());
    assert!(String::from_utf8_lossy(&plan.stderr).contains("E1002"));
}

#[test]
fn f10_plan_apply_and_undo_writes_directory_listing() {
    let dir = TempDir::new();
    let root = dir.0.join("tree");
    let sub = root.join("sub");
    let output = dir.0.join("list.csv");
    fs::create_dir_all(&sub).unwrap();
    fs::write(root.join("a.txt"), b"abc").unwrap();
    fs::write(sub.join("b.txt"), b"hello").unwrap();

    let plan = batchdesk(&[
        "f10",
        "--root",
        path_text(&root),
        "--output",
        path_text(&output),
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f10 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "f10");
    assert_eq!(document["entries"][0]["action"], "listing");
    assert_eq!(document["params"]["format"], "csv");
    assert_eq!(document["params"]["includeFiles"], true);
    assert_eq!(document["params"]["includeDirs"], true);
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "f10 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let bytes = fs::read(&output).unwrap();
    assert_eq!(&bytes[..3], b"\xEF\xBB\xBF");
    let text = String::from_utf8(bytes[3..].to_vec()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "相对路径,名称,类型,大小,修改时间,层级");
    let a = lines
        .iter()
        .find(|line| line.starts_with("a.txt,"))
        .unwrap();
    let sub_line = lines.iter().find(|line| line.starts_with("sub,")).unwrap();
    let b = lines
        .iter()
        .find(|line| line.starts_with("sub/b.txt,"))
        .unwrap();
    assert!(a.contains(",文件,3,"));
    assert!(sub_line.contains(",目录,0,"));
    assert!(b.contains(",文件,5,"));
    assert!(a.ends_with(",1"));
    assert!(sub_line.ends_with(",1"));
    assert!(b.ends_with(",2"));
    let fields: Vec<&str> = a.split(',').collect();
    assert_eq!(fields.len(), 6);
    assert!(!fields[4].is_empty(), "mtime must be present");

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(undo.status.success());
    assert!(!output.exists());
}

#[test]
fn f10_empty_directory_and_target_inside_root_are_reported() {
    let dir = TempDir::new();
    let empty = dir.0.join("empty");
    fs::create_dir_all(&empty).unwrap();
    let output = dir.0.join("empty.csv");
    let plan = batchdesk(&[
        "f10",
        "--root",
        path_text(&empty),
        "--output",
        path_text(&output),
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert!(document["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["kind"] == "empty-directory"));

    let inside = empty.join("list.csv");
    let plan = batchdesk(&[
        "f10",
        "--root",
        path_text(&empty),
        "--output",
        path_text(&inside),
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["entries"][0]["blocked"]["code"], "E1002");
}

#[test]
fn f11_plan_apply_and_undo_creates_multi_level_directories() {
    let dir = TempDir::new();
    let root = dir.0.join("target");
    let list = dir.0.join("list.csv");
    fs::create_dir_all(&root).unwrap();
    fs::write(&list, "路径\na/b\nc\n".as_bytes()).unwrap();

    let plan = batchdesk(&[
        "f11",
        "--list",
        path_text(&list),
        "--root",
        path_text(&root),
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f11 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "f11");
    assert_eq!(document["entries"].as_array().unwrap().len(), 3);
    assert!(document["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry["action"] == "create-dir"));
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "f11 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert!(root.join("a").is_dir());
    assert!(root.join("a/b").is_dir());
    assert!(root.join("c").is_dir());

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        undo.status.success(),
        "f11 undo failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&undo.stdout),
        String::from_utf8_lossy(&undo.stderr)
    );
    assert!(!root.join("a").exists());
    assert!(!root.join("c").exists());
    assert!(root.exists(), "the target root itself is not removed");
}

#[test]
fn f11_undo_keeps_a_directory_that_became_non_empty_and_reports_the_failure() {
    let dir = TempDir::new();
    let root = dir.0.join("target");
    let list = dir.0.join("list.csv");
    fs::create_dir_all(&root).unwrap();
    fs::write(&list, "路径\na/b\nc\n".as_bytes()).unwrap();

    let plan = batchdesk(&[
        "f11",
        "--list",
        path_text(&list),
        "--root",
        path_text(&root),
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(plan.status.success(), "f11 plan failed");
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();
    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(apply.status.success(), "f11 apply failed");

    // 计划之后目录里出现了别人的文件：撤销只能删空目录，禁止递归删。
    fs::write(root.join("a/planted.txt"), b"keep me").unwrap();
    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    let report: serde_json::Value = serde_json::from_slice(&undo.stdout).unwrap();
    assert_eq!(
        report["failed"].as_array().unwrap().len(),
        1,
        "the non-empty directory must be reported as a failed undo entry\nstdout:\n{}",
        String::from_utf8_lossy(&undo.stdout)
    );
    assert_eq!(report["reverted"].as_u64().unwrap(), 2);
    assert_eq!(
        report["failed"][0]["error"]["code"].as_str().unwrap(),
        "E2008"
    );
    assert!(
        root.join("a").is_dir(),
        "a non-empty directory is not removed"
    );
    assert!(
        root.join("a/planted.txt").is_file(),
        "undo must never delete recursively"
    );
    assert!(
        !root.join("a/b").exists(),
        "the empty child is still reverted"
    );
    assert!(
        !root.join("c").exists(),
        "unaffected entries are still reverted"
    );
}

#[test]
fn f11_accepts_the_f10_listing_format_and_reports_invalid_rows() {
    let dir = TempDir::new();
    let root = dir.0.join("target");
    let list = dir.0.join("listing.csv");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &list,
        "相对路径,名称,类型,大小,修改时间,层级\n\
         sub,sub,目录,0,2026-10-30T00:00:00Z,1\n\
         sub/a.txt,a.txt,文件,3,2026-10-30T00:00:00Z,2\n\
         ../escape,escape,目录,0,2026-10-30T00:00:00Z,1\n",
    )
    .unwrap();

    let plan = batchdesk(&[
        "f11",
        "--list",
        path_text(&list),
        "--root",
        path_text(&root),
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["entries"].as_array().unwrap().len(), 1);
    assert_eq!(
        document["entries"][0]["target"].as_str().unwrap(),
        path_text(&root.join("sub"))
    );
    assert!(document["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["kind"] == "invalid-rows"));
}

#[test]
fn f11_existing_target_is_skipped() {
    let dir = TempDir::new();
    let root = dir.0.join("target");
    let list = dir.0.join("list.csv");
    fs::create_dir_all(root.join("already")).unwrap();
    fs::write(&list, "already\n").unwrap();

    let plan = batchdesk(&[
        "f11",
        "--list",
        path_text(&list),
        "--root",
        path_text(&root),
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(
        document["entries"][0]["conflict"]["kind"],
        "existing-target"
    );
    assert_eq!(document["entries"][0]["resolution"], "skip");
}

#[test]
fn f11_skips_branches_whose_parent_position_is_an_existing_file() {
    // TC-F11-PRE-03 的"父路径为文件"面：a 位置是文件时，a/b 与 a/b/c 必须在**计划期**就
    // 报出并跳过，apply 不得因此整体失败，也不得在无冲突的 d/e 分支上出错。
    let dir = TempDir::new();
    let root = dir.0.join("out");
    let list = dir.0.join("list.csv");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a"), b"not a directory").unwrap();
    fs::write(&list, "a/b/c\nd/e\n").unwrap();
    let text = |path: &Path| path.to_string_lossy().into_owned();

    let plan = batchdesk(&[
        "f11",
        "--list",
        path_text(&list),
        "--root",
        path_text(&root),
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f11 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    let conflict_of = |target: &str| {
        document["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["target"] == target)
            .and_then(|entry| entry["conflict"]["kind"].as_str())
            .map(str::to_string)
    };
    let a = text(&root.join("a"));
    let a_b = text(&root.join("a").join("b"));
    let a_b_c = text(&root.join("a").join("b").join("c"));
    let d = text(&root.join("d"));
    let d_e = text(&root.join("d").join("e"));
    // 文件自己那一行仍按既有语义报 existing-target（不借机改既有判定）。
    assert_eq!(conflict_of(&a).as_deref(), Some("existing-target"));
    assert_eq!(conflict_of(&a_b).as_deref(), Some("parent-is-file"));
    assert_eq!(conflict_of(&a_b_c).as_deref(), Some("parent-is-file"));
    assert_eq!(conflict_of(&d), None);
    assert_eq!(conflict_of(&d_e), None);
    assert_eq!(document["stats"]["conflict"], 3);
    assert!(
        document["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["kind"] == "parent-is-file"),
        "集中报告缺少 parent-is-file：{document}"
    );

    let plan_file = dir.0.join("plan.json");
    fs::write(&plan_file, &plan.stdout).unwrap();
    let applied = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_file),
        "--quarantine-dir",
        path_text(&dir.0.join("q")),
        "--journal",
        path_text(&dir.0.join("j.json")),
        "--json",
    ]);
    assert!(
        applied.status.success(),
        "跳过不得让任务整体失败\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&applied.stdout),
        String::from_utf8_lossy(&applied.stderr)
    );
    assert!(root.join("d").join("e").is_dir(), "无冲突分支应照常创建");
    assert!(!Path::new(&a_b).exists(), "被文件挡住的分支不得落盘");
    assert!(root.join("a").is_file(), "挡住路径的文件本身不受影响");
}

#[test]
fn empty_protected_list_blocks_nothing_while_a_hit_blocks_and_writes_nothing() {
    // TC-S02-PRE-01（默认空名单不误拦）与 TC-S02-EXE-02（默认名单态写入不误拦）此前只由
    // "各功能单测隐含"，没有一条显式断言。这里逐功能钉住三件事：
    // 空名单 → blocked=0；名单命中 → blocked≥1 且带 E2009；执行面 → 空名单真写入、命中零变更。
    let dir = TempDir::new();
    let tree = dir.0.join("tree");
    let out = dir.0.join("out");
    fs::create_dir_all(tree.join("sub")).unwrap();
    fs::create_dir_all(&out).unwrap();
    let source = tree.join("a.txt");
    fs::write(&source, b"one\n").unwrap();
    fs::write(tree.join("src1.txt"), b"x\n").unwrap();
    fs::write(tree.join("src2.txt"), b"y\n").unwrap();

    let text = |path: &Path| path.to_string_lossy().into_owned();
    // (功能, 计划旗标, 该计划的落点所在目录——保护它就该命中)
    let cases: Vec<(&str, Vec<String>, PathBuf)> = vec![
        (
            "f01",
            vec![
                "--file".into(),
                text(&source),
                "--target-bytes".into(),
                "2".into(),
                "--output-dir".into(),
                text(&tree.join("parts")),
            ],
            tree.clone(),
        ),
        (
            "f04",
            vec![
                "--file".into(),
                text(&tree.join("src1.txt")),
                "--file".into(),
                text(&tree.join("src2.txt")),
                "--output".into(),
                text(&tree.join("merged.txt")),
                "--target".into(),
                "utf-8".into(),
                "--sample-window".into(),
                "64".into(),
                "--sample-blocks".into(),
                "0".into(),
            ],
            tree.clone(),
        ),
        (
            "f10",
            vec![
                "--root".into(),
                text(&tree),
                "--output".into(),
                text(&out.join("list.csv")),
            ],
            out.clone(),
        ),
        (
            "f12",
            vec!["--dir".into(), text(&tree), "--prefix".into(), "x_".into()],
            tree.clone(),
        ),
        (
            "c01",
            vec![
                "--file".into(),
                text(&source),
                "--target".into(),
                "gb18030".into(),
                "--source".into(),
                "utf-8".into(),
                "--sample-window".into(),
                "64".into(),
                "--sample-blocks".into(),
                "0".into(),
            ],
            tree.clone(),
        ),
    ];

    for (kind, args, landing) in &cases {
        let plain = plan_for(kind, args, None);
        assert_eq!(
            plain["stats"]["blocked"], 0,
            "{kind} 空名单不得出现 blocked：{plain}"
        );
        assert!(
            plain["stats"]["total"].as_u64().unwrap_or(0) >= 1,
            "{kind} 夹具应至少产出一条条目：{plain}"
        );
        let hit = plan_for(kind, args, Some(landing));
        assert!(
            hit["stats"]["blocked"].as_u64().unwrap_or(0) >= 1,
            "{kind} 落点被保护时必须 blocked：{hit}"
        );
        assert!(
            hit["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["blocked"]["code"] == "E2009"),
            "{kind} 命中要带 06 码表的 E2009：{hit}"
        );
    }

    // 执行面：空名单的 c01 真的写；同一份计划把落点保护起来后执行零变更。
    // 源文本用中文——纯 ASCII 在 UTF-8 与 GB18030 下逐字节相同，验不出"真的写了"。
    let utf8_original = "中文一行\n".as_bytes().to_vec();
    fs::write(&source, &utf8_original).unwrap();
    let expected_gb18030 = kernel::encoding::transcode(
        &utf8_original,
        kernel::encoding::SourceEncoding::Utf8,
        kernel::encoding::TargetEncoding::Gb18030,
        kernel::encoding::InvalidSequencePolicy::ReplaceAndCount,
    )
    .expect("夹具应能转成 GB18030")
    .bytes;
    assert_ne!(
        expected_gb18030, utf8_original,
        "夹具必须让两种编码的字节不同"
    );
    let plan_path = dir.0.join("c01.json");
    let (_, c01_args, c01_landing) = cases
        .iter()
        .find(|(kind, _, _)| *kind == "c01")
        .expect("用例里应有 c01");
    let plain = plan_for("c01", c01_args, None);
    fs::write(&plan_path, serde_json::to_vec(&plain).unwrap()).unwrap();
    let applied = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&dir.0.join("q1")),
        "--journal",
        path_text(&dir.0.join("j1.jsonl")),
        "--json",
    ]);
    assert!(
        applied.status.success(),
        "c01 apply 失败：{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert_eq!(
        fs::read(&source).unwrap(),
        expected_gb18030,
        "空名单下转码要真的落盘"
    );
    fs::write(&source, &utf8_original).unwrap();

    let hit = plan_for("c01", c01_args, Some(c01_landing));
    let hit_path = dir.0.join("c01-protected.json");
    fs::write(&hit_path, serde_json::to_vec(&hit).unwrap()).unwrap();
    let protected_apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&hit_path),
        "--protected-dir",
        path_text(c01_landing),
        "--quarantine-dir",
        path_text(&dir.0.join("q2")),
        "--journal",
        path_text(&dir.0.join("j2.jsonl")),
        "--json",
    ]);
    assert!(
        protected_apply.status.success(),
        "命中保护的条目应被跳过而不是整单失败：{}",
        String::from_utf8_lossy(&protected_apply.stderr)
    );
    assert_eq!(
        fs::read(&source).unwrap(),
        utf8_original,
        "保护命中的条目一个字节都不许改"
    );
    let report: serde_json::Value = serde_json::from_slice(&protected_apply.stdout).unwrap();
    assert_eq!(report["report"]["entries"][0]["status"], "skipped");
    assert_eq!(
        report["report"]["entries"][0]["error"]["code"], "E2009",
        "跳过原因要带码位"
    );
}

/// 用同一组旗标跑一次 `plan`，可选带上 `--protected-dir`。
fn plan_for(kind: &str, args: &[String], protected: Option<&Path>) -> serde_json::Value {
    let mut argv: Vec<String> = vec![kind.to_string()];
    argv.extend(args.iter().cloned());
    if let Some(dir) = protected {
        argv.extend_from_slice(&[
            "--protected-dir".to_string(),
            dir.to_string_lossy().into_owned(),
        ]);
    }
    argv.extend_from_slice(&[
        "--plan-id".to_string(),
        PLAN_ID.to_string(),
        "--json".to_string(),
    ]);
    let out = batchdesk_str(&argv);
    assert!(
        out.status.success(),
        "{kind} 计划失败：{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("计划应是合法 JSON")
}

#[test]
fn f12_prefix_plan_apply_and_undo_renames_files_and_directories() {
    let dir = TempDir::new();
    let root = dir.0.join("rename");
    fs::create_dir_all(root.join("folder")).unwrap();
    fs::write(root.join("a.txt"), b"a").unwrap();
    fs::write(root.join("b.txt"), b"b").unwrap();

    let plan = batchdesk(&[
        "f12",
        "--dir",
        path_text(&root),
        "--prefix",
        "new_",
        "--scope",
        "stem",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f12 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "f12");
    assert_eq!(document["entries"].as_array().unwrap().len(), 3);
    assert_eq!(document["params"]["twoPhase"], false);
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "f12 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert!(root.join("new_a.txt").exists());
    assert!(root.join("new_b.txt").exists());
    assert!(root.join("new_folder").is_dir());

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(undo.status.success());
    assert!(root.join("a.txt").exists());
    assert!(root.join("b.txt").exists());
    assert!(root.join("folder").is_dir());
    assert!(!root.join("new_a.txt").exists());
}

/// E03 的执行期语义在**真实文件系统**上的证据。刻意限 POSIX：这里靠"目录不可写"
/// 制造 EACCES→E2002，而 Windows 的只读是文件属性位、不给目录用，同一段代码在那边
/// 会变成"改名本来就该成功"（Windows CI 上实测得到 `ok`，因此必须显式不跑，
/// 而不是假装它验证了 Windows 的只读行为）。Windows `FILE_ATTRIBUTE_READONLY` 的
/// 执行期表现仍属 Windows 实机日。
#[cfg(unix)]
#[test]
fn read_only_rename_target_is_skipped_without_failing_the_task() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new();
    let root = dir.0.join("rename");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.txt"), b"a").unwrap();
    fs::write(root.join("b.txt"), b"b").unwrap();

    let plan = batchdesk(&[
        "f12",
        "--dir",
        path_text(&root),
        "--prefix",
        "new_",
        "--scope",
        "stem",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(plan.status.success());
    let plan_path = dir.0.join("plan.json");
    fs::write(&plan_path, plan.stdout).unwrap();

    // 目录置为不可写：改名必然吃 EACCES，这是真实文件系统上的"写不进去"形态。
    fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();
    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    // 先恢复写位，任何断言失败都不该把临时目录留在不可清理的状态。
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(
        apply.status.success(),
        "apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&apply.stdout).unwrap();
    assert_eq!(
        report["state"], "completed",
        "只读写不进去不得把整任务判死：{report}"
    );
    let entries = report["report"]["entries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(entries.len(), 2);
    for entry in &entries {
        assert_eq!(entry["status"], "skipped", "只读条目应记跳过：{entry}");
        assert_eq!(entry["error"]["code"], "E2002");
    }
    assert!(root.join("a.txt").exists() && root.join("b.txt").exists());
    assert!(!root.join("new_a.txt").exists());
    assert!(
        !dir.0.join("journal.jsonl").exists(),
        "没写成就不该留下撤销记录"
    );
}

#[test]
fn f12_reports_existing_conflicts_and_invalid_targets() {
    let dir = TempDir::new();
    let root = dir.0.join("rename");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.txt"), b"a").unwrap();
    fs::write(root.join("x.txt"), b"existing").unwrap();

    let plan = batchdesk(&[
        "f12",
        "--dir",
        path_text(&root),
        "--find",
        "a",
        "--replace",
        "x",
        "--scope",
        "full",
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert!(document["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |entry| entry["conflict"]["kind"] == "existing-target" && entry["resolution"] == "skip"
        ));

    // 非法名分两半验，别把"Windows 规则"当成"所有平台"：
    // ① `/` 两边都不允许（POSIX 只禁 `/` 与 NUL），这条是与平台无关的断言；
    // ② `CON` 是 Windows 保留名、POSIX 上的普通文件名——按宿主断言（ADR-0073）。
    //    改前这里只跑 ①，② 一直假装成立，Linux 上其实早就是假绿。
    let invalid = batchdesk(&[
        "f12",
        "--dir",
        path_text(&root),
        "--find",
        "a",
        "--replace",
        "a/b",
        "--json",
    ]);
    assert!(invalid.status.success());
    let document: serde_json::Value = serde_json::from_slice(&invalid.stdout).unwrap();
    assert!(
        document["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["blocked"]["code"] == "E1002"),
        "a target name containing '/' is invalid on every platform"
    );

    let reserved = batchdesk(&[
        "f12",
        "--dir",
        path_text(&root),
        "--find",
        "a",
        "--replace",
        "CON",
        "--json",
    ]);
    assert!(reserved.status.success());
    let document: serde_json::Value = serde_json::from_slice(&reserved.stdout).unwrap();
    let blocked = document["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["blocked"]["code"] == "E1002");
    assert_eq!(
        blocked,
        cfg!(windows),
        "CON is a reserved name on Windows and an ordinary name on POSIX"
    );
}

#[test]
fn f12_and_c03_name_read_only_files_in_plan_warnings() {
    let dir = TempDir::new();
    let root = dir.0.join("readonly");
    fs::create_dir_all(&root).unwrap();
    let read_only = root.join("locked-notes.txt");
    fs::write(&read_only, b"alpha beta\n").unwrap();
    fs::write(root.join("plain.txt"), b"alpha\n").unwrap();
    let mut permissions = fs::metadata(&read_only).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&read_only, permissions).unwrap();

    let named_readonly = |output: &std::process::Output| {
        assert!(
            output.status.success(),
            "命令应成功出计划：{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let warnings = document["warnings"].as_array().cloned().unwrap_or_default();
        assert!(
            warnings.iter().any(|warning| {
                warning["kind"] == "readonly"
                    && warning["suggestion"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("locked-notes.txt")
            }),
            "预览必须点名只读文件，不得让它到执行期才撞墙：{warnings:?}"
        );
    };

    named_readonly(&batchdesk(&[
        "f12",
        "--dir",
        path_text(&root),
        "--suffix",
        "-v2",
        "--json",
    ]));
    named_readonly(&batchdesk(&[
        "c03",
        "--file",
        path_text(&read_only),
        "--find",
        "alpha",
        "--replace",
        "omega",
        "--sample-bytes",
        "4096",
        "--json",
    ]));

    // 还原写位，否则 Windows 上临时目录清理会失败。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&read_only).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&read_only, permissions).unwrap();
    }
    #[cfg(windows)]
    {
        let mut permissions = fs::metadata(&read_only).unwrap().permissions();
        // Windows 只读是属性位，这里就是恢复可写的正确做法；Unix 分支在上面用 set_mode。
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&read_only, permissions).unwrap();
    }
}

#[test]
fn f13_regex_capture_plan_apply_and_undo() {
    let dir = TempDir::new();
    let root = dir.0.join("regex");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("photo-001.jpg"), b"a").unwrap();
    fs::write(root.join("photo-002.jpg"), b"b").unwrap();

    let plan = batchdesk(&[
        "f13",
        "--dir",
        path_text(&root),
        "--regex",
        r"^photo-(\d+)\.jpg$",
        "--replace",
        "img_$1.jpg",
        "--scope",
        "full",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f13 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "f13");
    assert_eq!(document["entries"].as_array().unwrap().len(), 2);
    assert_eq!(document["params"]["twoPhase"], false);
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "f13 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert!(root.join("img_001.jpg").exists());
    assert!(root.join("img_002.jpg").exists());
    assert!(!root.join("photo-001.jpg").exists());

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(undo.status.success());
    assert!(root.join("photo-001.jpg").exists());
    assert!(root.join("photo-002.jpg").exists());
    assert!(!root.join("img_001.jpg").exists());
}

#[test]
fn f13_rejects_invalid_regex_and_missing_capture_references() {
    let dir = TempDir::new();
    let root = dir.0.join("regex");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.txt"), b"a").unwrap();

    let invalid = batchdesk(&[
        "f13",
        "--dir",
        path_text(&root),
        "--regex",
        "(",
        "--replace",
        "x",
        "--json",
    ]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("E1002"));

    let missing = batchdesk(&[
        "f13",
        "--dir",
        path_text(&root),
        "--regex",
        r"^(\w+)\.txt$",
        "--replace",
        "$2.txt",
        "--json",
    ]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("missing capture group"));
}

#[test]
fn f13_no_match_creates_no_entries_and_writes_nothing() {
    let dir = TempDir::new();
    let root = dir.0.join("regex");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.txt"), b"a").unwrap();

    let plan = batchdesk(&[
        "f13",
        "--dir",
        path_text(&root),
        "--regex",
        "zzz",
        "--replace",
        "x",
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert!(document["entries"].as_array().unwrap().is_empty());
    assert!(document["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["kind"] == "no-change"));
    assert!(root.join("a.txt").exists());
    assert_eq!(
        fs::read_dir(&root).unwrap().filter_map(Result::ok).count(),
        1,
        "no-match preview must not write"
    );
}

#[test]
fn c02_text_line_endings_plan_apply_and_undo() {
    let dir = TempDir::new();
    let source = dir.0.join("a.txt");
    fs::write(&source, b"a\r\nb\nc\r").unwrap();

    let plan = batchdesk(&[
        "c02",
        "--file",
        path_text(&source),
        "--to",
        "lf",
        "--sample-bytes",
        "64",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c02 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "c02");
    assert_eq!(document["entries"].as_array().unwrap().len(), 1);
    assert!(document["entries"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("crlf=1 lf=1 cr=1"));
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c02 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert_eq!(fs::read(&source).unwrap(), b"a\nb\nc\n");

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(undo.status.success());
    assert_eq!(fs::read(&source).unwrap(), b"a\r\nb\nc\r");
}

#[test]
fn c02_csv_keeps_quoted_newlines_when_converting_records() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let original = b"a,\"b\r\nc\",d\r\nx,y\r\n";
    fs::write(&source, original).unwrap();

    let plan = batchdesk(&[
        "c02",
        "--file",
        path_text(&source),
        "--to",
        "lf",
        "--structure",
        "auto",
        "--sample-bytes",
        "64",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c02 csv plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert!(document["entries"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("csv;"));
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(apply.status.success());
    assert_eq!(fs::read(&source).unwrap(), b"a,\"b\r\nc\",d\nx,y\n");

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(undo.status.success());
    assert_eq!(fs::read(&source).unwrap(), original);
}

#[test]
fn c02_blocks_binary_files_and_skips_already_target_files() {
    let dir = TempDir::new();
    let binary = dir.0.join("image.txt");
    fs::write(&binary, b"\x89PNG\r\n\x1a\nrest").unwrap();
    let already = dir.0.join("already.txt");
    fs::write(&already, b"a\nb\n").unwrap();

    let plan = batchdesk(&[
        "c02",
        "--file",
        path_text(&binary),
        "--file",
        path_text(&already),
        "--to",
        "lf",
        "--sample-bytes",
        "64",
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["entries"][0]["blocked"]["code"], "E3001");
    assert!(document["entries"][0]["blocked"]["reason"]
        .as_str()
        .unwrap()
        .contains("binary"));
    assert_eq!(document["entries"][1]["resolution"], "skip");
    assert!(document["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["kind"] == "binary-skipped"));
    assert_eq!(fs::read(&binary).unwrap(), b"\x89PNG\r\n\x1a\nrest");
    assert_eq!(fs::read(&already).unwrap(), b"a\nb\n");
}

#[test]
fn c03_plain_replace_plan_apply_and_undo() {
    let dir = TempDir::new();
    let source = dir.0.join("a.txt");
    fs::write(&source, b"alpha beta alpha").unwrap();

    let plan = batchdesk(&[
        "c03",
        "--file",
        path_text(&source),
        "--find",
        "alpha",
        "--replace",
        "gamma",
        "--sample-bytes",
        "64",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c03 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "c03");
    assert!(document["entries"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("matches=2"));
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c03 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert_eq!(fs::read(&source).unwrap(), b"gamma beta gamma");

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(undo.status.success());
    assert_eq!(fs::read(&source).unwrap(), b"alpha beta alpha");
}

#[test]
fn c03_regex_capture_replace_plan_apply_and_undo() {
    let dir = TempDir::new();
    let source = dir.0.join("a.txt");
    fs::write(&source, b"a1 b2").unwrap();

    let plan = batchdesk(&[
        "c03",
        "--file",
        path_text(&source),
        "--find",
        r"([ab])(\d)",
        "--replace",
        "$2$1",
        "--mode",
        "regex",
        "--sample-bytes",
        "64",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c03 regex plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(apply.status.success());
    assert_eq!(fs::read(&source).unwrap(), b"1a 2b");

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(undo.status.success());
    assert_eq!(fs::read(&source).unwrap(), b"a1 b2");
}

#[test]
fn c03_csv_cell_mode_preserves_quoted_newlines() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let original = b"name,note\nA,\"x y\"\nB,\"line\r\nx y\"\n";
    fs::write(&source, original).unwrap();

    let plan = batchdesk(&[
        "c03",
        "--file",
        path_text(&source),
        "--find",
        "x y",
        "--replace",
        "z",
        "--csv-mode",
        "cell",
        "--delimiter",
        ",",
        "--sample-bytes",
        "64",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c03 csv plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert!(document["entries"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("matches=2"));
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(apply.status.success());
    assert_eq!(
        fs::read(&source).unwrap(),
        b"name,note\nA,z\nB,\"line\r\nz\"\n"
    );

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(undo.status.success());
    assert_eq!(fs::read(&source).unwrap(), original);
}

#[test]
fn c03_blocks_binary_and_unclosed_csv() {
    let dir = TempDir::new();
    let binary = dir.0.join("image.txt");
    fs::write(&binary, b"\x89PNG\r\n\x1a\nrest").unwrap();
    let broken = dir.0.join("broken.csv");
    fs::write(&broken, b"a,\"b\n").unwrap();

    let plan = batchdesk(&[
        "c03",
        "--file",
        path_text(&binary),
        "--file",
        path_text(&broken),
        "--find",
        "b",
        "--replace",
        "c",
        "--delimiter",
        ",",
        "--sample-bytes",
        "64",
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["entries"][0]["blocked"]["code"], "E3001");
    assert_eq!(document["entries"][1]["blocked"]["code"], "E3004");
}

fn corpus_file(fid: &str, file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus")
        .join(fid)
        .join("out")
        .join(file)
}

fn corpus_zip(fid: &str, file: &str) -> PathBuf {
    corpus_file(fid, file)
}

#[test]
fn c10_z01_gbk_name_extracts_and_undo_removes_the_product() {
    let dir = TempDir::new();
    let output = dir.0.join("out");
    fs::create_dir_all(&output).unwrap();
    let archive = corpus_zip("Z01", "gbk_name.zip");

    let plan = batchdesk(&[
        "c10",
        "--extract",
        path_text(&archive),
        "--output-dir",
        path_text(&output),
        "--max-total-bytes",
        "10000",
        "--max-ratio",
        "100",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c10 z01 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "c10");
    assert!(document["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["kind"] == "gbk-filename-fallback"));
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c10 z01 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let extracted = output.join("中文资料.txt");
    assert!(extracted.exists());
    assert_eq!(fs::read(&extracted).unwrap().len(), 77);

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(undo.status.success());
    assert!(!extracted.exists());
}

#[test]
fn c10_z02_rejects_traversal_entries_but_extracts_the_safe_one() {
    let dir = TempDir::new();
    let output = dir.0.join("out");
    fs::create_dir_all(&output).unwrap();
    let archive = corpus_zip("Z02", "traversal.zip");

    let plan = batchdesk(&[
        "c10",
        "--extract",
        path_text(&archive),
        "--output-dir",
        path_text(&output),
        "--max-total-bytes",
        "10000",
        "--max-ratio",
        "100",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c10 z02 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert!(document["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["kind"] == "unsafe-entry-skipped"));
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c10 z02 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let safe = output.join("safe").join("ok.txt");
    assert!(safe.exists());
    assert_eq!(fs::read(&safe).unwrap().len(), 41);
    assert!(!output.join("escape.txt").exists());
    assert!(!dir.0.join("escape.txt").exists());

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(undo.status.success());
    assert!(!safe.exists());
    assert!(!output.join("safe").exists());
}

#[test]
fn c10_compress_extract_roundtrip_and_self_inclusion() {
    let dir = TempDir::new();
    let source = dir.0.join("src");
    fs::create_dir_all(source.join("sub")).unwrap();
    fs::write(source.join("a.txt"), b"alpha").unwrap();
    fs::write(source.join("sub").join("b.txt"), b"beta").unwrap();
    let zip_path = dir.0.join("out.zip");
    let output = dir.0.join("out");
    fs::create_dir_all(&output).unwrap();

    let compress_plan = batchdesk(&[
        "c10",
        "--compress",
        path_text(&source),
        "--output",
        path_text(&zip_path),
        "--compression",
        "deflate",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        compress_plan.status.success(),
        "c10 compress plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compress_plan.stdout),
        String::from_utf8_lossy(&compress_plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&compress_plan.stdout).unwrap();
    assert_eq!(document["kind"], "c10");
    assert_eq!(document["entries"][0]["action"], "archive");
    fs::write(
        dir.0.join("compress-plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();
    let compress_apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("compress-plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("compress-journal.jsonl")),
        "--json",
    ]);
    assert!(
        compress_apply.status.success(),
        "c10 compress apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compress_apply.stdout),
        String::from_utf8_lossy(&compress_apply.stderr)
    );
    assert!(zip_path.is_file());
    assert!(fs::metadata(&zip_path).unwrap().len() > 0);

    let extract_plan = batchdesk(&[
        "c10",
        "--extract",
        path_text(&zip_path),
        "--output-dir",
        path_text(&output),
        "--max-total-bytes",
        "10000",
        "--max-ratio",
        "100",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(extract_plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&extract_plan.stdout).unwrap();
    fs::write(
        dir.0.join("extract-plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();
    let extract_apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("extract-plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("extract-journal.jsonl")),
        "--json",
    ]);
    assert!(
        extract_apply.status.success(),
        "c10 extract apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&extract_apply.stdout),
        String::from_utf8_lossy(&extract_apply.stderr)
    );
    let root_name = source.file_name().unwrap().to_str().unwrap();
    assert_eq!(
        fs::read(output.join(root_name).join("a.txt")).unwrap(),
        b"alpha"
    );
    assert_eq!(
        fs::read(output.join(root_name).join("sub").join("b.txt")).unwrap(),
        b"beta"
    );

    let extract_undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("extract-journal.jsonl")),
        "--json",
    ]);
    assert!(extract_undo.status.success());
    assert!(!output.join(root_name).exists());
    let compress_undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("compress-journal.jsonl")),
        "--json",
    ]);
    assert!(compress_undo.status.success());
    assert!(!zip_path.exists());

    let self_plan = batchdesk(&[
        "c10",
        "--compress",
        path_text(&source),
        "--output",
        path_text(&source.join("inside.zip")),
        "--json",
    ]);
    assert!(self_plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&self_plan.stdout).unwrap();
    assert_eq!(document["entries"][0]["blocked"]["code"], "E1002");

    fs::write(&zip_path, b"existing").unwrap();
    let existing_plan = batchdesk(&[
        "c10",
        "--compress",
        path_text(&source),
        "--output",
        path_text(&zip_path),
        "--json",
    ]);
    assert!(existing_plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&existing_plan.stdout).unwrap();
    assert_eq!(document["entries"][0]["resolution"], "skip");
}

#[test]
fn f07_missing_column_blocks_with_e3004() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let output = dir.0.join("subset.csv");
    fs::write(&source, b"id,name\n1,Alice\n").unwrap();

    let plan = batchdesk(&[
        "f07",
        "--file",
        path_text(&source),
        "--column",
        "age",
        "--output",
        path_text(&output),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f07 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["entries"][0]["blocked"]["code"], "E3004");
    assert!(!output.exists());
}

#[test]
fn c11_full_row_dedup_plan_apply_and_undo() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let output = dir.0.join("a-dedup.csv");
    fs::write(&source, b"id,name\n1,A\n1,A\n2,B\n1,C\n").unwrap();

    let plan = batchdesk(&[
        "c11",
        "--file",
        path_text(&source),
        "--output",
        path_text(&output),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c11 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "c11");
    let payload = &document["entries"][0]["csvDedup"];
    assert_eq!(payload["key"], "row");
    assert_eq!(payload["dataRows"], 4);
    assert_eq!(payload["uniqueRows"], 3);
    assert_eq!(payload["duplicateRows"], 1);
    assert_eq!(document["stats"]["warning"], 1);
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c11 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert_eq!(fs::read(&output).unwrap(), b"id,name\n1,A\n2,B\n1,C\n");
    assert_eq!(fs::read(&source).unwrap(), b"id,name\n1,A\n1,A\n2,B\n1,C\n");

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        undo.status.success(),
        "c11 undo failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&undo.stdout),
        String::from_utf8_lossy(&undo.stderr)
    );
    assert!(!output.exists());
}

#[test]
fn c11_key_column_dedup_keeps_first_occurrence() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let output = dir.0.join("a-dedup.csv");
    fs::write(&source, b"id,name\n1,A\n1,A\n1,C\n2,B\n").unwrap();

    let plan = batchdesk(&[
        "c11",
        "--file",
        path_text(&source),
        "--output",
        path_text(&output),
        "--key-column",
        "id",
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c11 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    let payload = &document["entries"][0]["csvDedup"];
    assert_eq!(payload["key"], "columns");
    assert_eq!(payload["keyColumns"][0]["index"], 0);
    assert_eq!(payload["keyColumns"][0]["name"], "id");
    assert_eq!(payload["uniqueRows"], 2);
    assert_eq!(payload["duplicateRows"], 2);
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c11 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert_eq!(fs::read(&output).unwrap(), b"id,name\n1,A\n2,B\n");
}

#[test]
fn c11_existing_target_is_skipped_and_missing_key_column_blocks() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let output = dir.0.join("a-dedup.csv");
    fs::write(&source, b"id,name\n1,A\n1,A\n").unwrap();
    fs::write(&output, b"occupied\n").unwrap();

    let plan = batchdesk(&[
        "c11",
        "--file",
        path_text(&source),
        "--output",
        path_text(&output),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(
        document["entries"][0]["conflict"]["kind"],
        "existing-target"
    );
    assert_eq!(document["entries"][0]["resolution"], "skip");
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let missing = batchdesk(&[
        "c11",
        "--file",
        path_text(&source),
        "--output",
        path_text(&dir.0.join("other.csv")),
        "--key-column",
        "missing",
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(missing.status.success());
    let missing_document: serde_json::Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(missing_document["entries"][0]["blocked"]["code"], "E3004");

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c11 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert_eq!(fs::read(&output).unwrap(), b"occupied\n");
}

#[test]
fn c12_numeric_filter_keep_plan_apply_and_undo() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let output = dir.0.join("a-filtered.csv");
    fs::write(&source, b"id,amount\n1,10\n2,5\n3,20\n").unwrap();

    let plan = batchdesk(&[
        "c12",
        "--file",
        path_text(&source),
        "--output",
        path_text(&output),
        "--column",
        "amount",
        "--op",
        "ge",
        "--value",
        "10",
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c12 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "c12");
    let payload = &document["entries"][0]["csvFilter"];
    assert_eq!(payload["condition"]["op"], "ge");
    assert_eq!(payload["condition"]["value"], "10");
    assert_eq!(payload["mode"], "keep");
    assert_eq!(payload["matchedRows"], 2);
    assert_eq!(payload["keptRows"], 2);
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c12 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert_eq!(fs::read(&output).unwrap(), b"id,amount\n1,10\n3,20\n");
    assert_eq!(fs::read(&source).unwrap(), b"id,amount\n1,10\n2,5\n3,20\n");

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        undo.status.success(),
        "c12 undo failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&undo.stdout),
        String::from_utf8_lossy(&undo.stderr)
    );
    assert!(!output.exists());
}

#[test]
fn c12_contains_drop_mode_and_non_numeric_warning() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let output = dir.0.join("a-dropped.csv");
    fs::write(&source, b"id,note\n1,keep-me\n2,drop-me\n3,other\n").unwrap();

    let plan = batchdesk(&[
        "c12",
        "--file",
        path_text(&source),
        "--output",
        path_text(&output),
        "--column",
        "note",
        "--op",
        "contains",
        "--value",
        "drop",
        "--mode",
        "drop",
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c12 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    let payload = &document["entries"][0]["csvFilter"];
    assert_eq!(payload["mode"], "drop");
    assert_eq!(payload["matchedRows"], 1);
    assert_eq!(payload["keptRows"], 2);
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c12 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert_eq!(fs::read(&output).unwrap(), b"id,note\n1,keep-me\n3,other\n");
}

#[test]
fn c12_blocks_missing_column_and_skips_occupied_target() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let output = dir.0.join("a-filtered.csv");
    fs::write(&source, b"id,amount\n1,10\n1,10\n").unwrap();
    fs::write(&output, b"occupied\n").unwrap();

    let plan = batchdesk(&[
        "c12",
        "--file",
        path_text(&source),
        "--output",
        path_text(&output),
        "--column",
        "amount",
        "--op",
        "gt",
        "--value",
        "5",
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(
        document["entries"][0]["conflict"]["kind"],
        "existing-target"
    );
    assert_eq!(document["entries"][0]["resolution"], "skip");
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let missing = batchdesk(&[
        "c12",
        "--file",
        path_text(&source),
        "--output",
        path_text(&dir.0.join("other.csv")),
        "--column",
        "missing",
        "--op",
        "contains",
        "--value",
        "x",
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(missing.status.success());
    let missing_document: serde_json::Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(missing_document["entries"][0]["blocked"]["code"], "E3004");

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c12 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert_eq!(fs::read(&output).unwrap(), b"occupied\n");
}

#[test]
fn c04_plain_replace_plan_apply_and_undo() {
    let dir = TempDir::new();
    let work = dir.0.join("docs");
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join("report-2024.txt"), b"a").unwrap();
    fs::write(work.join("report-2023.txt"), b"b").unwrap();
    fs::write(work.join("notes.txt"), b"c").unwrap();

    let plan = batchdesk(&[
        "c04",
        "--dir",
        path_text(&work),
        "--find",
        "report",
        "--replace",
        "memo",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c04 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "c04");
    assert_eq!(document["entries"].as_array().unwrap().len(), 2);
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c04 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert!(work.join("memo-2024.txt").exists());
    assert!(work.join("memo-2023.txt").exists());
    assert!(work.join("notes.txt").exists());
    assert!(!work.join("report-2024.txt").exists());

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        undo.status.success(),
        "c04 undo failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&undo.stdout),
        String::from_utf8_lossy(&undo.stderr)
    );
    assert!(work.join("report-2024.txt").exists());
    assert!(!work.join("memo-2024.txt").exists());
}

#[test]
fn c04_regex_capture_replace_and_no_match() {
    let dir = TempDir::new();
    let work = dir.0.join("logs");
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join("2024-05-server.log"), b"a").unwrap();

    let plan = batchdesk(&[
        "c04",
        "--dir",
        path_text(&work),
        "--find",
        r"^(\d{4})-(\d{2})-(.*)\.log$",
        "--replace",
        "${2}_${1}_${3}.log",
        "--mode",
        "regex",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c04 regex plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "c04");
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c04 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert!(work.join("05_2024_server.log").exists());
}

#[test]
fn c04_invalid_regex_and_missing_capture_are_rejected() {
    let dir = TempDir::new();
    let work = dir.0.join("docs");
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join("a.txt"), b"a").unwrap();

    let invalid = batchdesk(&[
        "c04",
        "--dir",
        path_text(&work),
        "--find",
        "([a",
        "--mode",
        "regex",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(!invalid.status.success());
    let stderr = String::from_utf8_lossy(&invalid.stderr);
    assert!(stderr.contains("E1002"), "stderr: {stderr}");

    let missing = batchdesk(&[
        "c04",
        "--dir",
        path_text(&work),
        "--find",
        r"^(\d+)-(.*)$",
        "--replace",
        "$9",
        "--mode",
        "regex",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(!missing.status.success());
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(stderr.contains("E1002"), "stderr: {stderr}");

    let empty_find = batchdesk(&[
        "c04",
        "--dir",
        path_text(&work),
        "--find",
        "",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(!empty_find.status.success());
    // 边界要在 CLI 层也带码位（TC-C05-PRE-02）：只断"非零退出"会让码位换错也看不见。
    let stderr = String::from_utf8_lossy(&empty_find.stderr);
    assert!(stderr.contains("E1002"), "stderr: {stderr}");

    let unknown_scope = batchdesk(&[
        "c04",
        "--dir",
        path_text(&work),
        "--find",
        "a",
        "--replace",
        "b",
        "--scope",
        "bogus",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(!unknown_scope.status.success());
    let stderr = String::from_utf8_lossy(&unknown_scope.stderr);
    assert!(stderr.contains("E1002"), "stderr: {stderr}");

    let scope_without_value = batchdesk(&[
        "c04",
        "--dir",
        path_text(&work),
        "--find",
        "a",
        "--replace",
        "b",
        "--plan-id",
        PLAN_ID,
        "--scope",
    ]);
    assert!(!scope_without_value.status.success());
    let stderr = String::from_utf8_lossy(&scope_without_value.stderr);
    assert!(stderr.contains("E1001"), "stderr: {stderr}");
}

#[test]
fn c05_preview_reports_counts_labels_and_zero_side_effects() {
    let dir = TempDir::new();
    let root = dir.0.join("rename");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.txt"), b"a").unwrap();

    let plan = batchdesk(&[
        "f12",
        "--dir",
        path_text(&root),
        "--prefix",
        "new_",
        "--scope",
        "stem",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "f12 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    // c05 的输入是计划文件：给计划注入一条 existing 冲突（标注逻辑本身由
    // kernel preview 单测覆盖），验证 CLI→kernel 的报告链路。
    let mut document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    document["entries"][0]["conflict"] = serde_json::json!({
        "kind": "existing",
        "riskLevel": "caution"
    });
    document["entries"][0]["resolution"] = serde_json::json!("skip");
    let plan_path = dir.0.join("plan.json");
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let preview = batchdesk(&["c05", "--plan", path_text(&plan_path), "--json"]);
    assert!(
        preview.status.success(),
        "c05 preview failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&preview.stdout),
        String::from_utf8_lossy(&preview.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(report["kind"], "f12");
    assert_eq!(report["counts"]["toProcess"], 0);
    assert_eq!(report["counts"]["conflicts"], 1);
    let rows = report["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["status"], "conflict");
    assert_eq!(rows[0]["oldPath"], path_text(&root.join("a.txt")));
    assert!(
        report["highlights"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line.as_str().unwrap().contains("default policy is skip")),
        "highlights: {report}"
    );
    // 预览零副作用：文件与目录保持原状（§7.4 预览不落盘）。
    assert!(root.join("a.txt").exists());
    assert!(!root.join("new_a.txt").exists());
}

#[test]
fn c06_undo_dry_run_previews_rollback_and_occupied_targets_are_skipped() {
    let dir = TempDir::new();
    let root = dir.0.join("rename");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.txt"), b"a").unwrap();
    fs::write(root.join("b.txt"), b"b").unwrap();

    let plan = batchdesk(&[
        "f12",
        "--dir",
        path_text(&root),
        "--prefix",
        "new_",
        "--scope",
        "stem",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();
    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan.json")),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(apply.status.success());
    assert!(root.join("new_a.txt").exists());

    // 回滚预演：全部可还原、零副作用（方案 §7.8"回滚前重新做冲突检测"）。
    let dry = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--dry-run",
        "--json",
    ]);
    assert!(
        dry.status.success(),
        "undo dry-run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&dry.stdout),
        String::from_utf8_lossy(&dry.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&dry.stdout).unwrap();
    assert_eq!(report["conflictCount"], 0);
    assert_eq!(report["missingCount"], 0);
    assert_eq!(report["entries"].as_array().unwrap().len(), 2);
    assert!(root.join("new_a.txt").exists(), "dry run must not rename");
    assert!(!root.join("a.txt").exists(), "dry run must not rename");

    // 占用恢复点后：预演标 conflict，实际撤销逐条跳过并报告（状态不变）。
    fs::write(root.join("a.txt"), b"occupied").unwrap();
    let dry_conflict = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--dry-run",
        "--json",
    ]);
    assert!(dry_conflict.status.success());
    let report: serde_json::Value = serde_json::from_slice(&dry_conflict.stdout).unwrap();
    assert_eq!(report["conflictCount"], 1);

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(
        !undo.status.success(),
        "occupied restore targets must surface failures"
    );
    let report: serde_json::Value = serde_json::from_slice(&undo.stdout).unwrap();
    assert_eq!(report["reverted"], 1, "only the unoccupied entry reverts");
    assert_eq!(report["failed"].as_array().unwrap().len(), 1);
    // 被占用条目状态不变，其余条目已还原（方案 §7.8"逐条跳过并报告"）。
    assert!(root.join("new_a.txt").exists());
    assert!(root.join("a.txt").exists());
    assert!(root.join("b.txt").exists());
    assert!(!root.join("new_b.txt").exists());
}

#[test]
fn c06_undo_is_terminal_and_the_plan_can_be_reapplied() {
    // TC C06 RBK：回滚为终态不可二次回滚；撤销后可重新执行原计划（§7.8、11.6-22）。
    let dir = TempDir::new();
    let root = dir.0.join("rename");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.txt"), b"a").unwrap();

    let plan = batchdesk(&[
        "f12",
        "--dir",
        path_text(&root),
        "--prefix",
        "new_",
        "--scope",
        "stem",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    let plan_path = dir.0.join("plan.json");
    let quarantine_path = dir.0.join("quarantine");
    let journal_path = dir.0.join("journal.jsonl");
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();
    let apply_args = [
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine_path),
        "--journal",
        path_text(&journal_path),
        "--json",
    ];
    assert!(batchdesk(&apply_args).status.success());
    assert!(root.join("new_a.txt").exists());

    let undo_args = [
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--json",
    ];
    let undo = batchdesk(&undo_args);
    assert!(undo.status.success());
    assert!(root.join("a.txt").exists());

    // 二次撤销：applied 条目为零（幂等跳过），回滚为终态。
    let undo_again = batchdesk(&undo_args);
    assert!(undo_again.status.success());
    let report: serde_json::Value = serde_json::from_slice(&undo_again.stdout).unwrap();
    assert_eq!(report["reverted"], 0);
    assert_eq!(report["entries"].as_array().unwrap().len(), 0);

    // 撤销后再执行：原计划重新生效（§7.8"由用户重新预览并执行原操作"）。
    assert!(batchdesk(&apply_args).status.success());
    assert!(root.join("new_a.txt").exists());
}

#[test]
fn c06_unknown_plan_id_reports_missing_mapping() {
    // TC C06 PRE-○2：映射缺失给明确提示（E1001），而非静默"撤销 0 条"。
    let dir = TempDir::new();
    let journal_path = dir.0.join("journal.jsonl");
    fs::write(&journal_path, "").unwrap();
    let undo = batchdesk(&[
        "undo",
        "--plan",
        "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q9",
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(!undo.status.success());
    assert!(
        String::from_utf8_lossy(&undo.stderr).contains("no journal entries for this plan"),
        "stderr: {}",
        String::from_utf8_lossy(&undo.stderr)
    );
}

#[test]
fn c06_cross_batch_undos_are_independent() {
    // TC C06 RBK-04：两个批次的映射互不干扰——链式重命名后按批次逆序撤销。
    let dir = TempDir::new();
    let root = dir.0.join("rename");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.txt"), b"a").unwrap();

    let plan_a = batchdesk(&[
        "f12",
        "--dir",
        path_text(&root),
        "--prefix",
        "new_",
        "--scope",
        "stem",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(plan_a.status.success());
    let document_a: serde_json::Value = serde_json::from_slice(&plan_a.stdout).unwrap();
    let plan_a_path = dir.0.join("plan-a.json");
    fs::write(
        &plan_a_path,
        serde_json::to_string_pretty(&document_a).unwrap(),
    )
    .unwrap();
    let apply_a = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_a_path),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(apply_a.status.success());
    assert!(root.join("new_a.txt").exists());

    // 批次 B 建立在批次 A 的产物上：new_a → final_new_a。
    let plan_b = batchdesk(&[
        "f12",
        "--dir",
        path_text(&root),
        "--prefix",
        "final_",
        "--scope",
        "stem",
        "--plan-id",
        "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q9",
        "--json",
    ]);
    assert!(plan_b.status.success());
    let document_b: serde_json::Value = serde_json::from_slice(&plan_b.stdout).unwrap();
    let plan_b_path = dir.0.join("plan-b.json");
    fs::write(
        &plan_b_path,
        serde_json::to_string_pretty(&document_b).unwrap(),
    )
    .unwrap();
    let apply_b = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_b_path),
        "--quarantine-dir",
        path_text(&dir.0.join("quarantine")),
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(apply_b.status.success());
    assert!(root.join("final_new_a.txt").exists());

    // 撤销批次 B：只回它自己的映射，批次 A 的产物保持。
    let undo_b = batchdesk(&[
        "undo",
        "--plan",
        "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q9",
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(undo_b.status.success());
    let report: serde_json::Value = serde_json::from_slice(&undo_b.stdout).unwrap();
    assert_eq!(report["reverted"], 1);
    assert!(root.join("new_a.txt").exists());
    assert!(!root.join("final_new_a.txt").exists());
    assert!(
        !root.join("a.txt").exists(),
        "undo of batch B must not restore batch A's source"
    );

    // 撤销批次 A：回到最初状态。
    let undo_a = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&dir.0.join("journal.jsonl")),
        "--json",
    ]);
    assert!(undo_a.status.success());
    assert!(root.join("a.txt").exists());
    assert!(!root.join("new_a.txt").exists());
}

#[test]
fn c14_split_plan_apply_and_undo_inserts_columns_after_source() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let out_dir = dir.0.join("out");
    let plan_path = dir.0.join("plan.json");
    let quarantine = dir.0.join("quarantine");
    let journal_path = dir.0.join("journal.jsonl");
    fs::create_dir_all(&out_dir).unwrap();
    fs::write(&source, b"id,tags,note\n1,x;y;z,first\n2,only,second\n").unwrap();

    let plan = batchdesk(&[
        "c14",
        "--file",
        path_text(&source),
        "--split-column",
        "tags",
        "--split-separator",
        ";",
        "--output-dir",
        path_text(&out_dir),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c14 split plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "c14");
    assert_eq!(document["entries"][0]["action"], "csv-columns");
    let split = &document["entries"][0]["csvColumns"]["split"];
    assert_eq!(split["index"], 2);
    assert_eq!(split["parts"], 3);
    assert_eq!(
        split["newNames"],
        json!(["tags.1", "tags.2", "tags.3"]),
        "new columns are derived from the source name (方案 §5.15)"
    );
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c14 split apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let output = out_dir.join("a.csv");
    assert!(output.exists());
    let text = fs::read_to_string(&output).unwrap();
    let mut lines = text.lines();
    assert_eq!(lines.next().unwrap(), "id,tags,tags.1,tags.2,tags.3,note");
    assert_eq!(lines.next().unwrap(), "1,x;y;z,x,y,z,first");
    // 段数不足补空串（方案 §5.15 空值处理）：单段如实映射，其余新列为空。
    assert_eq!(lines.next().unwrap(), "2,only,only,,,second");

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(undo.status.success());
    assert!(!output.exists(), "undo removes the produced file");
}

#[test]
fn c14_split_regex_and_merge_plan_apply_and_undo() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let out_dir = dir.0.join("out");
    let quarantine = dir.0.join("quarantine");
    let journal_path = dir.0.join("journal.jsonl");
    fs::create_dir_all(&out_dir).unwrap();
    fs::write(&source, b"full,extra\nAlice; alice@example.com,ok\n").unwrap();

    // 正则拆分：按分号及其两侧空白切段。
    let plan = batchdesk(&[
        "c14",
        "--file",
        path_text(&source),
        "--split-column",
        "full",
        "--split-regex",
        r"\s*;\s*",
        "--output-dir",
        path_text(&out_dir),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    fs::write(
        dir.0.join("plan-split.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();
    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan-split.json")),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(apply.status.success());
    let text = fs::read_to_string(out_dir.join("a.csv")).unwrap();
    assert_eq!(text.lines().next().unwrap(), "full,full.1,full.2,extra");
    assert_eq!(
        text.lines().nth(1).unwrap(),
        "Alice; alice@example.com,Alice,alice@example.com,ok"
    );
    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(undo.status.success());
    assert!(!out_dir.join("a.csv").exists());

    // 合并：两列拼接为一个新列追加在表尾。
    let merge = batchdesk(&[
        "c14",
        "--file",
        path_text(&source),
        "--merge-column",
        "full",
        "--merge-column-index",
        "2",
        "--join-with",
        " | ",
        "--merge-name",
        "combined",
        "--output-dir",
        path_text(&out_dir),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(merge.status.success());
    let document: serde_json::Value = serde_json::from_slice(&merge.stdout).unwrap();
    fs::write(
        dir.0.join("plan-merge.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();
    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&dir.0.join("plan-merge.json")),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(apply.status.success());
    let text = fs::read_to_string(out_dir.join("a.csv")).unwrap();
    assert_eq!(text.lines().next().unwrap(), "full,extra,combined");
    assert_eq!(
        text.lines().nth(1).unwrap(),
        "Alice; alice@example.com,ok,Alice; alice@example.com | ok"
    );
    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(undo.status.success());
    assert!(!out_dir.join("a.csv").exists());
}

#[test]
fn c14_rejects_invalid_regex_and_conflicting_modes() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, b"id,tags\n1,x\n").unwrap();

    let invalid_regex = batchdesk(&[
        "c14",
        "--file",
        path_text(&source),
        "--split-column",
        "tags",
        "--split-regex",
        "(unclosed",
        "--output-dir",
        path_text(&dir.0.join("out")),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--json",
    ]);
    assert!(!invalid_regex.status.success());
    assert!(
        String::from_utf8_lossy(&invalid_regex.stderr).contains("E1005"),
        "stderr: {}",
        String::from_utf8_lossy(&invalid_regex.stderr)
    );

    let both_modes = batchdesk(&[
        "c14",
        "--file",
        path_text(&source),
        "--split-column",
        "tags",
        "--split-separator",
        ";",
        "--merge-column",
        "id",
        "--join-with",
        "-",
        "--output-dir",
        path_text(&dir.0.join("out")),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--json",
    ]);
    assert!(!both_modes.status.success());
    assert!(
        String::from_utf8_lossy(&both_modes.stderr).contains("either split or merge"),
        "stderr: {}",
        String::from_utf8_lossy(&both_modes.stderr)
    );
    assert!(!dir.0.join("out").join("a.csv").exists());
}

#[test]
fn c07_move_plan_apply_and_undo_via_manifest() {
    let dir = TempDir::new();
    let source_root = dir.0.join("src");
    let target_root = dir.0.join("dst");
    let quarantine = dir.0.join("quarantine");
    let journal_path = dir.0.join("journal.jsonl");
    fs::create_dir_all(&source_root).unwrap();
    fs::create_dir_all(&target_root).unwrap();
    fs::write(source_root.join("a.txt"), b"a").unwrap();
    fs::write(source_root.join("b.txt"), b"b").unwrap();
    let manifest_path = dir.0.join("manifest.csv");
    fs::write(&manifest_path, b"a.txt,archive/2026\nb.txt,\n").unwrap();

    let plan = batchdesk(&[
        "c07",
        "--manifest",
        path_text(&manifest_path),
        "--source-root",
        path_text(&source_root),
        "--target-root",
        path_text(&target_root),
        "--operation",
        "move",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c07 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "c07");
    assert_eq!(document["entries"].as_array().unwrap().len(), 2);
    assert_eq!(document["entries"][0]["action"], "move");
    // kernel 计划内路径一律正斜杠规范化；与平台 join 结果比较前先归一。
    let planned_target = document["entries"][0]["target"]
        .as_str()
        .unwrap()
        .replace('\\', "/");
    let expected_target = path_text(&target_root.join("archive/2026/a.txt")).replace('\\', "/");
    assert_eq!(
        planned_target, expected_target,
        "target sub-directory is resolved against the target root"
    );
    let plan_path = dir.0.join("plan.json");
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c07 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    // 目标子目录不存在 → 执行期自动创建（方案 §5.8"按配置自动创建"）。
    assert!(target_root.join("archive/2026/a.txt").exists());
    assert!(target_root.join("b.txt").exists());
    assert!(!source_root.join("a.txt").exists());
    assert!(!source_root.join("b.txt").exists());

    // 撤销＝反向移动（§5.8）。
    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(undo.status.success());
    assert!(source_root.join("a.txt").exists());
    assert!(source_root.join("b.txt").exists());
    assert!(!target_root.join("archive/2026/a.txt").exists());
}

#[test]
fn c07_missing_sources_skip_or_error_and_copy_keeps_source() {
    let dir = TempDir::new();
    let source_root = dir.0.join("src");
    let target_root = dir.0.join("dst");
    let quarantine = dir.0.join("quarantine");
    let journal_path = dir.0.join("journal.jsonl");
    fs::create_dir_all(&source_root).unwrap();
    fs::create_dir_all(&target_root).unwrap();
    fs::write(source_root.join("a.txt"), b"a").unwrap();
    let manifest_path = dir.0.join("manifest.csv");
    // gone.txt 缺失：skip 策略 → 条目标记 E2001 并跳过；copy 保留源。
    fs::write(&manifest_path, b"a.txt,keep\ngone.txt,keep\n").unwrap();

    let plan = batchdesk(&[
        "c07",
        "--manifest",
        path_text(&manifest_path),
        "--source-root",
        path_text(&source_root),
        "--target-root",
        path_text(&target_root),
        "--operation",
        "copy",
        "--missing",
        "skip",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["entries"][0]["action"], "copy");
    assert_eq!(document["entries"][1]["blocked"]["code"], "E2001");
    let plan_path = dir.0.join("plan.json");
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(apply.status.success());
    // 复制保留源（§5.8 复制类）。
    assert!(source_root.join("a.txt").exists());
    assert!(target_root.join("keep/a.txt").exists());

    // error 策略：任一缺失 → 整单 E2001 拒绝。
    let error_plan = batchdesk(&[
        "c07",
        "--manifest",
        path_text(&manifest_path),
        "--source-root",
        path_text(&source_root),
        "--target-root",
        path_text(&target_root),
        "--operation",
        "copy",
        "--missing",
        "error",
        "--json",
    ]);
    assert!(!error_plan.status.success());
    assert!(
        String::from_utf8_lossy(&error_plan.stderr).contains("E2001"),
        "stderr: {}",
        String::from_utf8_lossy(&error_plan.stderr)
    );
}

#[test]
fn c08_rule_archive_plan_apply_and_undo() {
    let dir = TempDir::new();
    let source_root = dir.0.join("src");
    let target_root = dir.0.join("dst");
    let quarantine = dir.0.join("quarantine");
    let journal_path = dir.0.join("journal.jsonl");
    fs::create_dir_all(&target_root).unwrap();
    fs::create_dir_all(source_root.join("nested")).unwrap();
    fs::write(source_root.join("photo.jpg"), b"jpg").unwrap();
    fs::write(source_root.join("nested").join("deep.jpg"), b"deep").unwrap();
    fs::write(source_root.join("note.txt"), b"txt").unwrap();

    let plan = batchdesk(&[
        "c08",
        "--source-root",
        path_text(&source_root),
        "--target-root",
        path_text(&target_root),
        "--rule",
        "photos|ext:jpg",
        "--recursive",
        "--unclassified-dir",
        "未分类",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c08 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "c08");
    assert_eq!(document["entries"].as_array().unwrap().len(), 3);
    // 首中生效：*.jpg → photos（递归含 nested）；note.txt 未命中 → 未分类目录。
    // 遍历顺序按目录项排序（nested 先于 photo），按 rel_path 定位而非下标。
    let targets: Vec<&str> = document["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["target"].as_str().unwrap())
        .collect();
    let prefix = format!("{}/", path_text(&target_root).replace('\\', "/"));
    assert!(targets
        .iter()
        .any(|target| *target == format!("{prefix}photos/photo.jpg")));
    assert!(targets
        .iter()
        .any(|target| *target == format!("{prefix}photos/deep.jpg")));
    assert!(targets
        .iter()
        .any(|target| *target == format!("{prefix}未分类/note.txt")));
    let plan_path = dir.0.join("plan.json");
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c08 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert!(target_root.join("photos/photo.jpg").exists());
    assert!(target_root.join("photos/deep.jpg").exists());
    assert!(target_root.join("未分类/note.txt").exists());
    assert!(!source_root.join("photo.jpg").exists());

    // 撤销＝反向移动（§5.8 移动引擎回滚）。
    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(undo.status.success());
    assert!(source_root.join("photo.jpg").exists());
    assert!(source_root.join("nested").join("deep.jpg").exists());
    assert!(source_root.join("note.txt").exists());
    assert!(!target_root.join("photos/photo.jpg").exists());
}

#[test]
fn c08_rejects_unsafe_rules_and_reports_hit_statistics() {
    let dir = TempDir::new();
    let source_root = dir.0.join("src");
    let target_root = dir.0.join("dst");
    fs::create_dir_all(&source_root).unwrap();
    fs::create_dir_all(&target_root).unwrap();
    fs::write(source_root.join("a.jpg"), b"jpg").unwrap();
    fs::write(source_root.join("b.jpg"), b"jpg").unwrap();
    fs::write(source_root.join("c.jpg"), b"jpg").unwrap();

    // 规则子目录越界 → E1003。
    let escape = batchdesk(&[
        "c08",
        "--source-root",
        path_text(&source_root),
        "--target-root",
        path_text(&dir.0.join("dst")),
        "--rule",
        "../escape|ext:jpg",
        "--json",
    ]);
    assert!(!escape.status.success());
    assert!(
        String::from_utf8_lossy(&escape.stderr).contains("E1003"),
        "stderr: {}",
        String::from_utf8_lossy(&escape.stderr)
    );

    // 全中规则 → 高危警告（§5.9 命中超 50%）。
    let plan = batchdesk(&[
        "c08",
        "--source-root",
        path_text(&source_root),
        "--target-root",
        path_text(&dir.0.join("dst")),
        "--rule",
        "all|ext:jpg",
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert!(
        document["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["kind"] == "high-hit-rule" && warning["riskLevel"] == "danger"),
        "warnings: {document}"
    );
}

#[test]
fn c13_numeric_sort_plan_apply_and_undo() {
    let dir = TempDir::new();
    let source = dir.0.join("a.csv");
    let out_dir = dir.0.join("out");
    let quarantine = dir.0.join("quarantine");
    let journal_path = dir.0.join("journal.jsonl");
    fs::create_dir_all(&out_dir).unwrap();
    fs::write(&source, b"name,value\nbeta,10\nalpha,2\ngamma,7\n").unwrap();

    let plan = batchdesk(&[
        "c13",
        "--file",
        path_text(&source),
        "--sort-column",
        "value",
        "--order",
        "asc",
        "--output-dir",
        path_text(&out_dir),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c13 plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stdout),
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["kind"], "c13");
    assert_eq!(document["entries"][0]["action"], "csv-sort");
    assert_eq!(
        document["entries"][0]["csvSort"]["keys"][0]["kind"], "numeric",
        "类型感知：全数值采样推断 numeric"
    );
    let plan_path = dir.0.join("plan.json");
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c13 apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let text = fs::read_to_string(out_dir.join("a.csv")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "name,value");
    // 数值序：2 < 7 < 10（字典序会得到 10 < 2 < 7）。
    assert_eq!(lines[1], "alpha,2");
    assert_eq!(lines[2], "gamma,7");
    assert_eq!(lines[3], "beta,10");

    let undo = batchdesk(&[
        "undo",
        "--plan",
        PLAN_ID,
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(undo.status.success());
    assert!(!out_dir.join("a.csv").exists());
}

#[test]
fn c13_external_merge_sort_with_small_chunks() {
    let dir = TempDir::new();
    let source = dir.0.join("big.csv");
    let out_dir = dir.0.join("out");
    let quarantine = dir.0.join("quarantine");
    let journal_path = dir.0.join("journal.jsonl");
    fs::create_dir_all(&out_dir).unwrap();
    let mut body = String::from("value\n");
    // 6 行数据、阈值 2 → 3 块外排归并。
    for value in [42, 7, 99, 1, 55, 3] {
        body.push_str(&format!("{value}\n"));
    }
    fs::write(&source, body).unwrap();

    let plan = batchdesk(&[
        "c13",
        "--file",
        path_text(&source),
        "--sort-column-index",
        "1",
        "--external-chunk-rows",
        "2",
        "--output-dir",
        path_text(&out_dir),
        "--target",
        "utf-8",
        "--sample-bytes",
        "4096",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(plan.status.success());
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(
        document["entries"][0]["csvSort"]["externalChunkRows"], 2,
        "外排阈值显式传入（OQ-28 同口径）"
    );
    assert!(
        document["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["kind"] == "external-sort"),
        "warnings: {document}"
    );
    let plan_path = dir.0.join("plan.json");
    fs::write(&plan_path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

    let apply = batchdesk(&[
        "apply",
        "--plan",
        path_text(&plan_path),
        "--quarantine-dir",
        path_text(&quarantine),
        "--journal",
        path_text(&journal_path),
        "--json",
    ]);
    assert!(
        apply.status.success(),
        "c13 external apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let text = fs::read_to_string(out_dir.join("big.csv")).unwrap();
    assert_eq!(
        text.lines().collect::<Vec<_>>(),
        vec!["value", "1", "3", "7", "42", "55", "99"],
        "multi-chunk merge produces globally sorted output"
    );
    // 临时分块文件已清理。
    let leftovers: Vec<_> = fs::read_dir(&out_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains(".sort-chunk-"))
        .collect();
    assert!(leftovers.is_empty(), "chunk temp files must be cleaned up");
}

/// C18 横切干跑（§3.2 C18、§5.15）：对全部功能的 plan 命令做横切收口——
/// ① 重复预览幂等（同一输入＋固定 plan-id 两次输出逐字节一致）；
/// ② 预览零落盘（源树文件集合与内容前后一致）。
/// 执行仍需显式 apply（不变量 2"plan 为纯函数绝不落盘"的横切验证）。
#[test]
fn c18_plan_dry_run_is_idempotent_and_side_effect_free_across_features() {
    let dir = TempDir::new();
    let root = dir.0.join("tree");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), b"alpha\nbeta\n").unwrap();
    fs::write(root.join("b.csv"), b"id,value\n2,10\n1,20\n").unwrap();
    fs::write(root.join("c.csv"), b"id,value\n1,x\n").unwrap();
    fs::write(root.join("photo.jpg"), b"jpg").unwrap();
    fs::write(root.join("dup.txt"), b"dup").unwrap();
    fs::write(root.join("sub").join("dup.txt"), b"dup").unwrap();
    fs::write(root.join("d.txt"), b"gamma\n").unwrap();

    // 源树快照（相对路径 + 内容哈希占位：内容长度 + 字节）。
    let snapshot = || -> Vec<(String, u64)> {
        let mut files: Vec<(String, u64)> = walk_source(&root);
        files.sort();
        files
    };
    let before = snapshot();

    // 各功能 plan 命令（固定 --plan-id 保证输出可复现）。
    let out_dir = dir.0.join("out");
    fs::create_dir_all(&out_dir).unwrap();
    let manifest = dir.0.join("manifest.csv");
    fs::write(&manifest, "a.txt,archived\n").unwrap();
    // F11 的清单与建档根：建档根必须已存在，清单里的多级路径由计划展开。
    let f11_list = dir.0.join("f11-list.csv");
    fs::write(&f11_list, b"a/b\nc\n").unwrap();
    let f11_root = dir.0.join("f11-root");
    fs::create_dir_all(&f11_root).unwrap();
    let plan_commands: Vec<Vec<String>> = vec![
        vec![
            "c01".into(),
            "--file".into(),
            path_text(&root.join("b.csv")).to_string(),
            "--target".into(),
            "utf-8".into(),
            "--sample-window".into(),
            "4096".into(),
            "--sample-blocks".into(),
            "1".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "c04".into(),
            "--dir".into(),
            path_text(&root).to_string(),
            "--find".into(),
            "dup".into(),
            "--replace".into(),
            "uniq".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "c07".into(),
            "--manifest".into(),
            path_text(&manifest).to_string(),
            "--source-root".into(),
            path_text(&root).to_string(),
            "--target-root".into(),
            path_text(&out_dir).to_string(),
            "--operation".into(),
            "move".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "c08".into(),
            "--source-root".into(),
            path_text(&root).to_string(),
            "--target-root".into(),
            path_text(&out_dir).to_string(),
            "--rule".into(),
            "jpgs|ext:jpg".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "c09".into(),
            path_text(&root).to_string(),
            "--plan".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "c13".into(),
            "--file".into(),
            path_text(&root.join("b.csv")).to_string(),
            "--sort-column".into(),
            "value".into(),
            "--output-dir".into(),
            path_text(&out_dir).to_string(),
            "--target".into(),
            "utf-8".into(),
            "--sample-bytes".into(),
            "4096".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "c14".into(),
            "--file".into(),
            path_text(&root.join("b.csv")).to_string(),
            "--split-column".into(),
            "value".into(),
            "--split-separator".into(),
            " ".into(),
            "--output-dir".into(),
            path_text(&out_dir).to_string(),
            "--target".into(),
            "utf-8".into(),
            "--sample-bytes".into(),
            "4096".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "f01".into(),
            "--file".into(),
            path_text(&root.join("a.txt")).to_string(),
            "--target-bytes".into(),
            "8".into(),
            "--output-dir".into(),
            path_text(&out_dir).to_string(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "f12".into(),
            "--dir".into(),
            path_text(&root).to_string(),
            "--prefix".into(),
            "r_".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        // ── 以下为 2026-11-02 补齐的 14 个 plan 命令（原 harness 只跑 9 个）──
        vec![
            "c02".into(),
            "--file".into(),
            path_text(&root.join("a.txt")).to_string(),
            "--to".into(),
            "lf".into(),
            "--sample-bytes".into(),
            "64".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "c03".into(),
            "--file".into(),
            path_text(&root.join("a.txt")).to_string(),
            "--find".into(),
            "alpha".into(),
            "--replace".into(),
            "gamma".into(),
            "--sample-bytes".into(),
            "64".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "c10".into(),
            "--extract".into(),
            path_text(&corpus_zip("Z01", "gbk_name.zip")).to_string(),
            "--output-dir".into(),
            path_text(&out_dir).to_string(),
            "--max-total-bytes".into(),
            "10000".into(),
            "--max-ratio".into(),
            "100".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        // C11/C12 属阶段 1，但同样是 plan 命令；横切 harness 一并覆盖零落盘与幂等。
        vec![
            "c11".into(),
            "--file".into(),
            path_text(&root.join("b.csv")).to_string(),
            "--output".into(),
            path_text(&out_dir.join("dedup.csv")).to_string(),
            "--target".into(),
            "utf-8".into(),
            "--sample-bytes".into(),
            "4096".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "c12".into(),
            "--file".into(),
            path_text(&root.join("b.csv")).to_string(),
            "--output".into(),
            path_text(&out_dir.join("filter.csv")).to_string(),
            "--column".into(),
            "value".into(),
            "--op".into(),
            "ge".into(),
            "--value".into(),
            "1".into(),
            "--target".into(),
            "utf-8".into(),
            "--sample-bytes".into(),
            "4096".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "f02".into(),
            "--file".into(),
            path_text(&root.join("a.txt")).to_string(),
            "--lines".into(),
            "1".into(),
            "--output-dir".into(),
            path_text(&out_dir).to_string(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "f03".into(),
            "--file".into(),
            path_text(&root.join("a.txt")).to_string(),
            "--parts".into(),
            "2".into(),
            "--output-dir".into(),
            path_text(&out_dir).to_string(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "f04".into(),
            "--file".into(),
            path_text(&root.join("a.txt")).to_string(),
            "--file".into(),
            path_text(&root.join("d.txt")).to_string(),
            "--output".into(),
            path_text(&out_dir.join("merged.txt")).to_string(),
            "--target".into(),
            "utf-8".into(),
            "--sample-window".into(),
            "4096".into(),
            "--sample-blocks".into(),
            "1".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "f05".into(),
            "--file".into(),
            path_text(&root.join("b.csv")).to_string(),
            "--rows".into(),
            "1".into(),
            "--output-dir".into(),
            path_text(&out_dir).to_string(),
            "--sample-bytes".into(),
            "4096".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "f06".into(),
            "--file".into(),
            path_text(&root.join("b.csv")).to_string(),
            "--file".into(),
            path_text(&root.join("c.csv")).to_string(),
            "--output".into(),
            path_text(&out_dir.join("merged.csv")).to_string(),
            "--target".into(),
            "utf-8".into(),
            "--sample-bytes".into(),
            "4096".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "f07".into(),
            "--file".into(),
            path_text(&root.join("b.csv")).to_string(),
            "--column".into(),
            "id".into(),
            "--output-dir".into(),
            path_text(&out_dir).to_string(),
            "--target".into(),
            "utf-8".into(),
            "--sample-bytes".into(),
            "4096".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "f08".into(),
            "--file".into(),
            path_text(&root.join("b.csv")).to_string(),
            "--first".into(),
            "1".into(),
            "--output".into(),
            path_text(&out_dir.join("head.csv")).to_string(),
            "--target".into(),
            "utf-8".into(),
            "--sample-bytes".into(),
            "4096".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "f09".into(),
            "--file".into(),
            path_text(&root.join("b.csv")).to_string(),
            "--start".into(),
            "1".into(),
            "--end".into(),
            "1".into(),
            "--output".into(),
            path_text(&out_dir.join("range.csv")).to_string(),
            "--target".into(),
            "utf-8".into(),
            "--sample-bytes".into(),
            "4096".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "f10".into(),
            "--root".into(),
            path_text(&root).to_string(),
            "--output".into(),
            path_text(&out_dir.join("list.csv")).to_string(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "f11".into(),
            "--list".into(),
            path_text(&f11_list).to_string(),
            "--root".into(),
            path_text(&f11_root).to_string(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
        vec![
            "f13".into(),
            "--dir".into(),
            path_text(&root).to_string(),
            "--regex".into(),
            "^a\\.txt$".into(),
            "--replace".into(),
            "z.txt".into(),
            "--scope".into(),
            "full".into(),
            "--plan-id".into(),
            PLAN_ID.into(),
            "--json".into(),
        ],
    ];

    for command in &plan_commands {
        let first = batchdesk_str(command);
        assert!(
            first.status.success(),
            "plan failed: {}\nstdout:\n{}\nstderr:\n{}",
            command[0],
            String::from_utf8_lossy(&first.stdout),
            String::from_utf8_lossy(&first.stderr)
        );
        let second = batchdesk_str(command);
        assert!(
            second.status.success(),
            "repeated plan failed: {}",
            command[0]
        );
        assert_eq!(
            first.stdout, second.stdout,
            "C18 重复预览必须幂等（feature: {}）",
            command[0]
        );
        let after = snapshot();
        assert_eq!(before, after, "C18 预览零落盘（feature: {}）", command[0]);
    }
}

/// 收集源树全部文件的（相对路径, 内容长度）；目录结构变化也能被察觉（缺文件
/// 会改变清单）。仅用于 C18 横切快照，不复制基线判定文本。
fn walk_source(root: &Path) -> Vec<(String, u64)> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                files.push((rel, fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0)));
            }
        }
    }
    files
}

// ── C10 密码包（ADR-0074）──────────────────────────────────────────────────
//
// 三条端到端断言：计划期只说"会不会给密码"、执行期密码走 stdin、错密码报 E3009。

#[test]
fn c10_encrypted_archive_is_blocked_until_the_plan_expects_a_password() {
    let dir = TempDir::new();
    let output = dir.0.join("out");
    fs::create_dir_all(&output).unwrap();
    let archive = password_zip("zipcrypto.zip");

    let plan = batchdesk(&[
        "c10",
        "--extract",
        path_text(&archive),
        "--output-dir",
        path_text(&output),
        "--max-total-bytes",
        "10000",
        "--max-ratio",
        "100",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "预览本身应当成功（blocked 是计划内容，不是命令失败）\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(document["entries"][0]["blocked"]["code"], "E3008");
    assert!(
        document["params"].get("passwordProvided").is_none(),
        "没承诺密码就不该记这个事实：{}",
        document["params"]
    );
}

#[test]
fn c10_encrypted_archive_extracts_with_the_password_from_stdin() {
    let dir = TempDir::new();
    let output = dir.0.join("out");
    fs::create_dir_all(&output).unwrap();
    let archive = password_zip("zipcrypto.zip");

    let plan = batchdesk(&[
        "c10",
        "--extract",
        path_text(&archive),
        "--output-dir",
        path_text(&output),
        "--max-total-bytes",
        "10000",
        "--max-ratio",
        "100",
        "--expect-password",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(
        plan.status.success(),
        "c10 加密包预览失败\nstderr:\n{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let text = String::from_utf8_lossy(&plan.stdout).into_owned();
    let document: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(document["entries"][0]["blocked"].is_null());
    assert_eq!(document["params"]["passwordProvided"], true);
    assert!(document["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["kind"] == "encrypted-archive"));
    fs::write(
        dir.0.join("plan.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    // 计划文件里不能出现密码——连"承诺过密码"的包也只有那个布尔。
    let plan_file = fs::read_to_string(dir.0.join("plan.json")).unwrap();
    assert!(!plan_file.contains("secret123"), "计划文件里出现了密码");

    let apply = batchdesk_with_stdin(
        &[
            "apply",
            "--plan",
            path_text(&dir.0.join("plan.json")),
            "--quarantine-dir",
            path_text(&dir.0.join("quarantine")),
            "--journal",
            path_text(&dir.0.join("journal.jsonl")),
            "--password-stdin",
            "--json",
        ],
        "secret123\n",
    );
    assert!(
        apply.status.success(),
        "加密包执行失败\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    assert_eq!(
        fs::read(output.join("a.txt")).unwrap(),
        b"hello zip\n",
        "解出来的内容应当与夹具一致"
    );
    assert_eq!(
        fs::read(output.join("sub").join("b.txt")).unwrap(),
        b"second\n"
    );
}

#[test]
fn c10_encrypted_archive_with_a_wrong_password_reports_e3009() {
    let dir = TempDir::new();
    let output = dir.0.join("out");
    fs::create_dir_all(&output).unwrap();
    let archive = password_zip("aes256.zip");

    let plan = batchdesk(&[
        "c10",
        "--extract",
        path_text(&archive),
        "--output-dir",
        path_text(&output),
        "--max-total-bytes",
        "10000",
        "--max-ratio",
        "100",
        "--expect-password",
        "--plan-id",
        PLAN_ID,
        "--json",
    ]);
    assert!(plan.status.success());
    fs::write(dir.0.join("plan.json"), &plan.stdout).unwrap();

    let apply = batchdesk_with_stdin(
        &[
            "apply",
            "--plan",
            path_text(&dir.0.join("plan.json")),
            "--quarantine-dir",
            path_text(&dir.0.join("quarantine")),
            "--journal",
            path_text(&dir.0.join("journal.jsonl")),
            "--password-stdin",
            "--json",
        ],
        "wrong-password\n",
    );
    assert!(
        !apply.status.success(),
        "错密码不该成功\nstdout:\n{}",
        String::from_utf8_lossy(&apply.stdout)
    );
    // `--json` 下报告走 stdout（一条 failed 报告），错误码在条目级。
    let outcome: serde_json::Value =
        serde_json::from_slice(&apply.stdout).expect("apply --json prints an outcome");
    assert_eq!(outcome["state"], "failed");
    assert_eq!(
        outcome["report"]["entries"][0]["error"]["code"], "E3009",
        "错密码应报 E3009，实际 {}",
        outcome["report"]["entries"][0]
    );
    assert!(!output.join("a.txt").exists(), "错密码不得留下任何产物");
}
