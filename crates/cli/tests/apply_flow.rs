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
