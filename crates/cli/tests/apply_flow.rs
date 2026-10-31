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

    let invalid = batchdesk(&[
        "f12",
        "--dir",
        path_text(&root),
        "--find",
        "a",
        "--replace",
        "CON",
        "--json",
    ]);
    assert!(invalid.status.success());
    let document: serde_json::Value = serde_json::from_slice(&invalid.stdout).unwrap();
    assert!(document["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["blocked"]["code"] == "E1002"));
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

fn corpus_zip(fid: &str, file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/corpus")
        .join(fid)
        .join("out")
        .join(file)
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
