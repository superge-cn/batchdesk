#!/usr/bin/env python3
"""C-6 的 C09 只读与计划预览断言：三段式哈希、快速模式候选、硬链接不判重。

数据集由同目录 gen_c6.py 生成（默认 20 对 1 MiB＋10 份 2 MiB＋1 源＋3 硬链接）。
本脚本只调用 `cargo run ... c09 --json`，不删除、不移动任何文件，并在运行前后
比对 `out/` 的文件清单与大小，确认 CLI 是只读的。计划预览只生成 PlanDocument，
不执行 apply。

快速哈希窗口 4096 字节是**测试用显式参数**，不是产品默认值：方案 §5.10 只写
"头部若干 KB＋尾部若干 KB"，产品窗口见 OQ-28，确认前 CLI 不设默认。

用法：
    python fixtures/bench/C-6-重复文件集/assert_c09.py --generate
    python fixtures/bench/C-6-重复文件集/assert_c09.py   # 要求 out/ 已存在
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
OUT = HERE / "out"
GEN = HERE / "gen_c6.py"

MIB = 1024 * 1024
PAIRS = 20
PAIR_SIZE = 1 * MIB
SHARED_SIZE = 2 * MIB
SHARED_COPIES = 10
LINK_ALIASES = 3
# 测试参数，不是产品默认窗口（OQ-28）；保留策略默认 earliest（OQ-29 已确认）。
QUICK_WINDOW = 4096
TEST_PLAN_ID = "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8"
EXPECTED_LOG = (
    f"same_size_pairs={PAIRS} pair_size={PAIR_SIZE} shared_size={SHARED_SIZE} "
    f"shared_copies={SHARED_COPIES} link_size={MIB} link_aliases={LINK_ALIASES} "
    "seed=2026"
)


class AssertionFailure(AssertionError):
    pass


def fail(message: str) -> None:
    raise AssertionFailure(message)


def run_c09(
    mode: str,
    quick_window: int | None,
    extra_args: tuple[str, ...] = (),
) -> subprocess.CompletedProcess[str]:
    command = [
        "cargo",
        "run",
        "--quiet",
        "-p",
        "batchdesk-cli",
        "--",
        "c09",
        str(OUT),
        "--mode",
        mode,
        "--json",
    ]
    if quick_window is not None:
        command.extend(["--quick-window-bytes", str(quick_window)])
    command.extend(extra_args)
    return subprocess.run(
        command,
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=600,
    )


def parse_report(process: subprocess.CompletedProcess[str], label: str) -> dict:
    if process.returncode != 0:
        fail(
            f"{label}: c09 exited {process.returncode}\n"
            f"stdout:\n{process.stdout}\nstderr:\n{process.stderr}"
        )
    try:
        return json.loads(process.stdout)
    except json.JSONDecodeError as error:
        fail(
            f"{label}: stdout is not JSON: {error}\n"
            f"stdout:\n{process.stdout}\nstderr:\n{process.stderr}"
        )
        raise AssertionError("unreachable") from error


def snapshot() -> list[tuple[str, int, int]]:
    return sorted(
        (
            str(path.relative_to(OUT)),
            path.stat().st_size,
            path.stat().st_mtime_ns,
        )
        for path in OUT.rglob("*")
        if path.is_file()
    )


def files_by_id(report: dict) -> dict[int, dict]:
    return {entry["id"]: entry for entry in report["files"]}


def assert_stats(
    report: dict,
    *,
    mode: str,
    quick_window: int | None,
    expected_dataset_files: int,
    expected_full_hashed: int,
    expected_quick_pruned: int,
) -> None:
    stats = report["stats"]
    if report["mode"] != mode:
        fail(f"mode: expected {mode}, got {report['mode']}")
    if report["quick_window_bytes"] != quick_window:
        fail(
            "quick_window_bytes: expected "
            f"{quick_window}, got {report['quick_window_bytes']}"
        )
    expected_quick_hashed = (
        expected_dataset_files if quick_window is not None else 0
    )
    checks = {
        "quick_hashed": (stats["quick_hashed"], expected_quick_hashed),
        "full_hashed": (stats["full_hashed"], expected_full_hashed),
        # out/gen.log is a real file with a unique size, so the size stage
        # prunes it; it is not part of the 54-file duplicate dataset.
        "size_pruned": (stats["size_pruned"], 1),
        "quick_pruned": (stats["quick_pruned"], expected_quick_pruned),
    }
    for name, (actual, expected) in checks.items():
        if actual != expected:
            fail(f"stats.{name}: expected {expected}, got {actual}")
    expected_total = expected_dataset_files + 1
    if len(report["files"]) != expected_total:
        fail(f"files: expected {expected_total}, got {len(report['files'])}")
    if report["skipped"]:
        fail(f"skipped: expected none, got {report['skipped']}")
    if report["zero_byte_ids"]:
        fail(f"zero_byte_ids: expected none, got {report['zero_byte_ids']}")
    if report["incomplete"]:
        fail(f"incomplete: expected none, got {report['incomplete']}")


def assert_shared_group(
    report: dict,
    *,
    expected_confidence: str,
    hardlinks_ok: bool,
) -> None:
    groups = report["groups"]
    if len(groups) != 1:
        fail(f"groups: expected exactly 1, got {len(groups)}: {groups}")
    group = groups[0]
    if group["size"] != SHARED_SIZE:
        fail(f"group size: expected {SHARED_SIZE}, got {group['size']}")
    if group["confidence"] != expected_confidence:
        fail(
            f"group confidence: expected {expected_confidence}, "
            f"got {group['confidence']}"
        )
    if len(group["members"]) != SHARED_COPIES:
        fail(
            f"group members: expected {SHARED_COPIES}, "
            f"got {len(group['members'])}"
        )
    if os.name == "posix" and group["needs_manual_review"]:
        fail("group needs_manual_review: expected false on a Unix host")
    if not group["hash"] or len(group["hash"]) != 64:
        fail(f"group hash: expected 64 hex chars, got {group['hash']!r}")

    by_id = files_by_id(report)
    parents = {
        Path(by_id[member]["path"]).parent.name for member in group["members"]
    }
    if parents != {"same_content"}:
        fail(f"group members: expected only same_content, got {parents}")


def assert_hash_presence(
    report: dict,
    *,
    quick_stage: bool,
    full_parents: set[str],
) -> None:
    for entry in report["files"]:
        parent = Path(entry["path"]).parent.name
        if parent not in {"same_size", "same_content", "hardlinks"}:
            continue
        if entry["quick_hash"] != quick_stage:
            fail(
                f"hash presence: {entry['path']} quick_hash expected "
                f"{quick_stage}, got {entry['quick_hash']}"
            )
        expected_full = parent in full_parents
        if entry["full_hash"] != expected_full:
            fail(
                f"hash presence: {entry['path']} full_hash expected "
                f"{expected_full}, got {entry['full_hash']}"
            )


def assert_hardlinks(report: dict, hardlinks_ok: bool) -> None:
    skipped = report["hardlink_skipped"]
    if not hardlinks_ok:
        if skipped:
            fail(f"hardlink_skipped: expected none, got {skipped}")
        print("warn: 文件系统不支持硬链接，硬链接断言已跳过（gen.log 已记录）")
        return
    if len(skipped) != LINK_ALIASES:
        fail(f"hardlink_skipped: expected {LINK_ALIASES}, got {skipped}")
    by_id = files_by_id(report)
    paths = [Path(by_id[member]["path"]) for member in skipped]
    if any(path.parent.name != "hardlinks" for path in paths):
        fail(f"hardlink_skipped: expected only hardlinks/, got {paths}")


def assert_plan_preview(plan: dict, hardlinks_ok: bool) -> None:
    if plan["planId"] != TEST_PLAN_ID:
        fail(f"planId: expected {TEST_PLAN_ID}, got {plan['planId']}")
    if plan["kind"] != "c09":
        fail(f"plan kind: expected c09, got {plan['kind']}")
    expected_params = {
        "hash": "blake3",
        "includeZeroByte": False,
        "keep": "shortest-path",
        "mode": "full",
        "quickWindowBytes": QUICK_WINDOW,
    }
    if plan["params"] != expected_params:
        fail(f"plan params: expected {expected_params}, got {plan['params']}")

    entries = plan["entries"]
    expected_entries = SHARED_COPIES - 1
    if len(entries) != expected_entries:
        fail(f"plan entries: expected {expected_entries}, got {len(entries)}")
    expected_names = {
        f"copy_{index:02d}.dat" for index in range(1, SHARED_COPIES)
    }
    planned_names: set[str] = set()
    entry_ids: set[str] = set()
    for entry in entries:
        if "target" in entry:
            fail(f"plan entry target: expected omitted, got {entry['target']}")
        if entry["action"] != "delete":
            fail(f"plan entry action: expected delete, got {entry['action']}")
        if not entry["entryId"].startswith(TEST_PLAN_ID):
            fail(f"plan entryId does not include planId: {entry['entryId']}")
        if entry["entryId"] in entry_ids:
            fail(f"duplicate plan entryId: {entry['entryId']}")
        entry_ids.add(entry["entryId"])
        source = Path(entry["source"])
        if source.parent.name != "same_content":
            fail(f"plan entry source is not under same_content: {source}")
        if source.name == "copy_00.dat":
            fail("plan preview planned to delete the keep file copy_00.dat")
        if "copy_00.dat" not in entry["reason"] or "quarantine" not in entry["reason"]:
            fail(f"plan entry reason is not actionable: {entry['reason']}")
        planned_names.add(source.name)

    if planned_names != expected_names:
        fail(f"plan entries: expected {expected_names}, got {planned_names}")
    expected_warning_count = 1 if hardlinks_ok else 0
    expected_stats = {
        "total": expected_entries,
        "conflict": 0,
        "warning": expected_warning_count,
        "blocked": 0,
    }
    if plan["stats"] != expected_stats:
        fail(f"plan stats: expected {expected_stats}, got {plan['stats']}")
    warnings = plan["warnings"]
    if hardlinks_ok:
        if len(warnings) != 1:
            fail(f"plan warnings: expected 1, got {warnings}")
        warning = warnings[0]
        if warning["kind"] != "hardlink-skipped":
            fail(f"plan warning kind: expected hardlink-skipped, got {warning}")
        if warning["riskLevel"] != "safe":
            fail(f"plan warning risk: expected safe, got {warning}")
        if "3 hardlink" not in warning.get("suggestion", ""):
            fail(f"plan warning suggestion: expected 3 entries, got {warning}")
    elif warnings:
        fail(f"plan warnings: expected none, got {warnings}")
    if plan["params"]["keep"] != "shortest-path":
        fail("plan keep policy was not preserved")


def main() -> int:
    parser = argparse.ArgumentParser(description="C-6 的 C09 只读与计划预览断言")
    parser.add_argument(
        "--generate",
        action="store_true",
        help="out/ 不存在时先运行 gen_c6.py",
    )
    args = parser.parse_args()

    if os.name != "posix":
        print(
            "C-6 的硬链接/文件标识断言需要 Unix 主机；"
            "Windows 需先落地 FsAdapter 文件 ID，暂不在本机执行。",
            file=sys.stderr,
        )
        return 2

    if not OUT.exists():
        if not args.generate:
            print(
                f"C-6 数据集不存在：{OUT}\n"
                f"请先运行 python {GEN}，或给本脚本加 --generate。",
                file=sys.stderr,
            )
            return 2
        subprocess.run([sys.executable, str(GEN)], check=True)

    log = (OUT / "gen.log").read_text(encoding="utf-8")
    if EXPECTED_LOG not in log:
        fail(f"C-6 参数与断言不一致，gen.log 内容：{log!r}")
    hardlinks_ok = "hardlinks=ok" in log
    expected_dataset_files = (
        PAIRS * 2 + SHARED_COPIES + 1 + (LINK_ALIASES if hardlinks_ok else 0)
    )
    expected_full_quick = SHARED_COPIES + (LINK_ALIASES + 1 if hardlinks_ok else 0)
    expected_quick_pruned = expected_dataset_files - expected_full_quick

    before = snapshot()

    full_two_stage = parse_report(run_c09("full", None), "full without quick window")
    assert_stats(
        full_two_stage,
        mode="full",
        quick_window=None,
        expected_dataset_files=expected_dataset_files,
        expected_full_hashed=expected_dataset_files,
        expected_quick_pruned=0,
    )
    assert_shared_group(
        full_two_stage, expected_confidence="confirmed", hardlinks_ok=hardlinks_ok
    )
    assert_hash_presence(
        full_two_stage,
        quick_stage=False,
        full_parents={"same_size", "same_content", "hardlinks"},
    )
    assert_hardlinks(full_two_stage, hardlinks_ok)
    print("ok: full（无快速窗口）大小→全量两阶段，同内容 10 份成组")

    full_three_stage = parse_report(
        run_c09("full", QUICK_WINDOW), "full with quick window"
    )
    assert_stats(
        full_three_stage,
        mode="full",
        quick_window=QUICK_WINDOW,
        expected_dataset_files=expected_dataset_files,
        expected_full_hashed=expected_full_quick,
        expected_quick_pruned=expected_quick_pruned,
    )
    assert_shared_group(
        full_three_stage, expected_confidence="confirmed", hardlinks_ok=hardlinks_ok
    )
    assert_hash_presence(
        full_three_stage,
        quick_stage=True,
        full_parents={
            "same_content",
            *({"hardlinks"} if hardlinks_ok else set()),
        },
    )
    assert_hardlinks(full_three_stage, hardlinks_ok)
    print(
        "ok: full（三阶段）快速哈希淘汰 "
        f"{expected_quick_pruned}/{expected_dataset_files}，"
        f"仅 {expected_full_quick} 份进入全量"
    )

    fast = parse_report(run_c09("fast", QUICK_WINDOW), "fast mode")
    assert_stats(
        fast,
        mode="fast",
        quick_window=QUICK_WINDOW,
        expected_dataset_files=expected_dataset_files,
        expected_full_hashed=0,
        expected_quick_pruned=expected_quick_pruned,
    )
    assert_shared_group(
        fast, expected_confidence="candidate", hardlinks_ok=hardlinks_ok
    )
    assert_hash_presence(fast, quick_stage=True, full_parents=set())
    assert_hardlinks(fast, hardlinks_ok)
    if any(group["confidence"] == "confirmed" for group in fast["groups"]):
        fail("fast mode: candidate groups must never be confirmed")
    print("ok: fast 仅输出 candidate 组，未读取全量哈希")

    no_window = run_c09("fast", None)
    if no_window.returncode == 0:
        fail("fast mode without --quick-window-bytes must fail")
    if "fast mode requires --quick-window-bytes" not in no_window.stderr:
        fail(f"fast mode error text missing, stderr：{no_window.stderr!r}")
    print("ok: fast 模式缺少显式窗口时拒绝运行")

    plan = parse_report(
        run_c09(
            "full",
            QUICK_WINDOW,
            (
                "--plan",
                "--keep",
                "shortest-path",
                "--plan-id",
                TEST_PLAN_ID,
            ),
        ),
        "plan preview",
    )
    assert_plan_preview(plan, hardlinks_ok)
    print(
        "ok: plan 预览仅生成 9 条 delete，保留 copy_00.dat，"
        "硬链接跳过告警，未执行 apply"
    )

    fast_plan = run_c09(
        "fast",
        QUICK_WINDOW,
        ("--plan", "--keep", "shortest-path"),
    )
    if fast_plan.returncode == 0:
        fail("fast mode must not be accepted for a cleanup plan")
    if "E1002" not in fast_plan.stderr:
        fail(f"fast plan error code missing, stderr：{fast_plan.stderr!r}")
    print("ok: fast 模式拒绝生成清理计划（E1002）")

    default_plan = parse_report(
        run_c09("full", QUICK_WINDOW, ("--plan",)),
        "default keep plan",
    )
    if default_plan["params"]["keep"] != "earliest":
        fail(
            "default plan keep: expected earliest, "
            f"got {default_plan['params']['keep']}"
        )
    if len(default_plan["entries"]) != SHARED_COPIES - 1:
        fail(
            "default plan entries: expected "
            f"{SHARED_COPIES - 1}, got {len(default_plan['entries'])}"
        )
    if default_plan["stats"]["warning"] != (1 if hardlinks_ok else 0):
        fail(f"default plan warning count: {default_plan['stats']}")
    print("ok: plan 省略 --keep 时默认 earliest（OQ-29）")

    after = snapshot()
    if before != after:
        fail("C-6 只读断言失败：运行前后 out/ 文件清单/大小/mtime 发生变化")
    print(f"ok: 只读性，{len(before)} 个文件未被修改")

    print(
        f"C-6 C09 断言通过：数据集 {expected_dataset_files} 个文件"
        f"（另 gen.log 1 个），"
        f"硬链接={'支持' if hardlinks_ok else '跳过'}，窗口={QUICK_WINDOW}（测试参数）"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AssertionFailure as error:
        print(f"C-6 C09 断言失败：{error}", file=sys.stderr)
        raise SystemExit(1)
