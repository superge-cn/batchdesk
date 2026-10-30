use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

const PLAN_ID: &str = "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8";

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("batchdesk-apply-{nanos}"));
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

fn path_text(path: &Path) -> &str {
    path.to_str().expect("test paths are UTF-8")
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
    assert_eq!(
        fs::read_to_string(&journal_path).unwrap().lines().count(),
        2
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
