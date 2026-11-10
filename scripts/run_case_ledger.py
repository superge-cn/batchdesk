#!/usr/bin/env python3
"""生成 docs/testing/用例执行台账-v0.1.csv：把映射表的每条锚点解析到"本次真跑过且通过的测试"。

为什么需要它：映射表的 ✅ 只说明"该实例有自动化锚点"，不证明那次执行发生过、
也不证明锚点写的测试名还存在。本脚本先跑四套载体（根 workspace／桌面壳／Vitest／
Playwright），再把 296 条实例逐条对到实测清单上，给出**可机器复核**的判定：

    实测通过（逐名）         锚点里的测试名在实测清单里且全部 ok
    实测通过（模块级 N 项）  锚点是"`模块` N 项"式描述，按模块前缀整组核对
    实测通过（文件级 N 条）  前端锚点按测试文件整体核对（Vitest／Playwright）
    实测失败                 任一解析到的实测用例 FAILED
    计数不符                 表内声称的条数与实测条数不一致（台账需修正）
    名不符                   锚点指向的测试名在源码里找不到（台账需修正）
    待人工执行               载体为真人工／受控环境，本脚本不代跑
    锚点不可机器定位         锚点是纯文字描述，需改写为精确测试名（缺陷清单）

判定权威仍是 TC §3 与映射表；本脚本只回答"证据是否真跑过、名字是否还在"。
用法：python3 scripts/run_case_ledger.py [--reuse-cargo <log>] [--offline]
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MAP_CSV = ROOT / "docs" / "testing" / "用例覆盖映射表-v0.2.csv"
LEDGER_CSV = ROOT / "docs" / "testing" / "用例执行台账-v0.1.csv"
RECORD_DIR = ROOT / "docs" / "testing" / "执行记录"

CARGO_LINE = re.compile(r"^test (\S+) \.\.\. (ok|FAILED|ignored)")
# cargo 的 Running 行有两种形态："Running unittests src/lib.rs (…)" 与
# "Running tests/corpus.rs (…)"，所以只取括号里的制品路径，不猜前面的段数。
RUNNING_LINE = re.compile(r"Running\b.*?\(([^)]*)\)$")

# cargo 测试制品名（去掉尾随哈希）→ 台账里用的载体标签
BINARY_LABEL = {
    "kernel": "kernel",
    "platform": "platform",
    "batchdesk": "cli",
    "apply_flow": "apply_flow",
    "corpus": "corpus",
    "batchdesk_desktop": "shell",
    "batchdesk_desktop_lib": "shell",
}


def run(cmd: list[str], cwd: Path) -> str:
    print(f"$ {' '.join(cmd)}", flush=True)
    # Windows 的 npm/pnpm 装的是 .cmd shim，CreateProcess 只追加 .exe——用 shutil.which
    # 一次性解析到真实可执行文件；POSIX 下 which 原样返回，行为不变。
    resolved = shutil.which(cmd[0]) or cmd[0]
    # cargo 的 "Running …" 行进 stderr、libtest 的结果行进 stdout；
    # 两者必须按顺序合并才能把测试名归到正确的测试目标上。
    proc = subprocess.run(
        [resolved] + cmd[1:], cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stdout[-4000:] + "\n")
        raise SystemExit(f"命令失败（退出码 {proc.returncode}）：{' '.join(cmd)}——失败样本不得进台账")
    return proc.stdout


def normalize_status(value: str) -> str:
    """把各测试框架的状态词归一到 ok / FAILED / ignored。"""
    value = (value or "").lower()
    if value in ("ok", "passed", "pass", "success"):
        return "ok"
    if value in ("failed", "failure", "fail"):
        return "FAILED"
    return "ignored"


def parse_cargo(text: str) -> dict[str, str]:
    """→ {"<载体>::<测试全路径>": "ok|FAILED|ignored"}"""
    results: dict[str, str] = {}
    label = "unknown"
    for line in text.splitlines():
        match = RUNNING_LINE.search(line)
        if match:
            # Windows 的括号内路径是反斜杠（且带盘符与 .exe），按两种分隔符统一取末段。
            artifact = re.split(r"[/\\]", match.group(1))[-1]
            stem = artifact.rsplit("-", 1)[0] if "-" in artifact else artifact
            label = BINARY_LABEL.get(stem, stem)
            continue
        hit = CARGO_LINE.match(line)
        if hit:
            results[f"{label}::{hit.group(1)}"] = normalize_status(hit.group(2))
    return results


def collect_rust(reuse: str | None) -> dict[str, str]:
    inventory: dict[str, str] = {}
    if reuse:
        workspace_text = Path(reuse).read_text(encoding="utf-8")
        shell_text = run(["cargo", "test", "--manifest-path", "src-tauri/Cargo.toml"], ROOT)
    else:
        workspace_text = run(["cargo", "test", "--workspace", "--all-features"], ROOT)
        shell_text = run(["cargo", "test", "--manifest-path", "src-tauri/Cargo.toml"], ROOT)
    inventory.update(parse_cargo(workspace_text))
    inventory.update(parse_cargo(shell_text))
    return inventory


def ledger_tmp(name: str) -> Path:
    """台账中间文件落在仓库 target/ 下。

    原实现写 `/tmp`：Windows 上会解析成当前盘符根目录（如 `E:\\tmp`），
    在受限环境里既不可写、也可能被安全策略拦下；target/ 与构建产物同卷且可丢弃。
    """
    directory = ROOT / "target" / "case-ledger"
    directory.mkdir(parents=True, exist_ok=True)
    return directory / name


def collect_vitest(offline: bool) -> list[dict]:
    out = ledger_tmp("vitest.json")
    if out.exists():
        out.unlink()
    if offline:
        return []
    run(["pnpm", "exec", "vitest", "run", "--reporter=json", f"--outputFile={out}"], ROOT / "web")
    data = json.loads(out.read_text(encoding="utf-8"))
    rows = []
    for suite in data.get("testResults", []):
        file = Path(suite.get("name", "")).name
        for assertion in suite.get("assertionResults", []):
            rows.append({
                "file": file,
                "title": assertion.get("fullName", ""),
                "status": normalize_status(assertion.get("status", "")),
            })
    return rows


def collect_playwright(offline: bool) -> list[dict]:
    out = ledger_tmp("playwright.json")
    if out.exists():
        out.unlink()
    if offline:
        return []
    env = dict(os.environ, PLAYWRIGHT_JSON_OUTPUT_NAME=str(out))
    print("$ pnpm exec playwright test --reporter=json", flush=True)
    # 与 run() 同口径：Windows 下 pnpm 是 .cmd shim，需 shutil.which 解析（CreateProcess 只追加 .exe）。
    pw_pnpm = shutil.which("pnpm") or "pnpm"
    proc = subprocess.run(
        [pw_pnpm, "exec", "playwright", "test", "--reporter=json"],
        cwd=ROOT / "web",
        env=env,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stdout[-3000:] + "\n" + proc.stderr[-2000:])
        raise SystemExit("Playwright 存在失败用例，台账不产出")
    data = json.loads(out.read_text(encoding="utf-8"))
    rows = []
    for suite in data.get("suites", []):
        walk_suite(suite, suite.get("title", ""), rows)
    return rows


def walk_suite(suite: dict, file_title: str, rows: list[dict]) -> None:
    title = suite.get("title", "")
    current = file_title or title
    for spec in suite.get("specs", []):
        for test in spec.get("tests", []):
            for result in test.get("results", [{}]):
                rows.append({
                    "file": (spec.get("file") or current),
                    "title": spec.get("title", ""),
                    "status": normalize_status(result.get("status", "")),
                })
                break
    for child in suite.get("suites", []):
        walk_suite(child, current, rows)


# 锚点里的中文限定词 → 测试名里对应的英文片段（用于把"kernel `generic_apply` 并发 6 项"
# 这类限定式锚点收窄到该组测试，而不是把整个模块 44 项都算进来）。
QUALIFIERS = {
    "跨盘": ("cross_volume", "cross"),
    "并发": ("concurren", "pooled", "quota", "entry_count_limit"),
    "取消": ("cancel",),
    "部分失败": ("failing_entry", "partial"),
    "冲突": ("conflict",),
    "幂等": ("idempot",),
    "长路径": ("long_path",),
    "剪枝": ("prune", "snapshot"),
}


def qualifier_tokens(anchor: str) -> tuple[str, ...]:
    for phrase, tokens in QUALIFIERS.items():
        if phrase in anchor:
            return tokens
    return ()


def load_map_rows() -> list[list[str]]:
    with MAP_CSV.open(encoding="utf-8") as handle:
        rows = list(csv.reader(handle))
    return rows[1:]


def resolve(anchor: str, carrier: str, rust: dict[str, str], vitest: list[dict],
            playwright: list[dict], out: list[str] | None = None):
    """→ (判定, 解析层, 解析明细)；out 非空时回收命中的清单键，用于按真实运行位置填「实测命令」"""
    if "真人工" in carrier:
        return ("待人工执行", "人工", "载体为真人工／受控环境，本脚本不代跑")

    hits: list[tuple[str, str]] = []      # (清单键, 状态)——锚点指名或用例名精确命中
    module_hits: list[tuple[str, str]] = []  # 模块级整组命中（仅在锚点自己就是模块级时才作判定）
    missing: list[str] = []               # 锚点写了但清单里找不到的名字
    notes: list[str] = []                 # 解析过程的说明（模块级、标题匹配、条数不符…）
    frontend: list[tuple[str, str]] = []  # 前端文件／标题级解析结果

    def lookup(name: str, prefixes: tuple[str, ...]) -> str | None:
        for prefix in prefixes:
            key = f"{prefix}::{name}"
            if key in rust:
                return key
        for prefix in prefixes:
            suffix = f"::{name}"
            for key in rust:
                if key.startswith(prefix + "::") and key.endswith(suffix):
                    return key
        return None

    # 1) 显式测试名
    for pattern, prefixes in (
        (re.compile(r"generic_apply::tests::([A-Za-z0-9_]+)"), ("kernel",)),
        (re.compile(r"apply_flow::([A-Za-z0-9_]+)"), ("apply_flow", "cli")),
        (re.compile(r"corpus\.rs::([a-z][A-Za-z0-9_]*)"), ("corpus",)),
    ):
        for name in pattern.findall(anchor):
            key = lookup(name, prefixes)
            if key is None:
                missing.append(f"{prefixes[0]}::{name}")
            else:
                hits.append((key, rust[key]))

    # 1b) 通配形态的壳层/内核锚点（"壳层 protected_snapshot_comparison_*"）
    for star in re.findall(r"\b([a-z][a-z0-9_]{5,})\*\b", anchor):
        keys = sorted(key for key in rust if key.split("::")[-1].startswith(star))
        if keys:
            hits.extend((key, rust[key]) for key in keys)
            notes.append(f"通配 {star}* 展开为 {len(keys)} 项")
        else:
            missing.append(f"{star}*")

    # 1c) 完整命名空间路径形态的锚点（"tasks::tests::x"、"platform::preflight::tests::x"、
    #     "kernel::preflight::tests::x"）：清单键以该路径结尾即可定位，覆盖全部 crate/壳层命名空间。
    for path in re.findall(r"\b(?:[a-z][a-z0-9_]*::){1,5}[a-z][a-z0-9_]+\b", anchor):
        if len(path.split("::")[-1]) < 12:
            continue
        matches = sorted(key for key in rust if key.endswith(path))
        if matches:
            hits.extend((key, rust[key]) for key in matches)
            notes.append(f"按命名空间路径定位：{path.rsplit('::', 1)[-1]}")

    # 1d) 锚点里"＋裸用例名"的写法（"…::tests::a＋b"）：整段精确查清单。
    #     过去这类第二段直接被忽略，锚点声称两个用例、实际只核一个也算绿——查不到就记名不符。
    for segment in re.split(r"[＋、]", anchor)[1:]:
        token = re.sub(r"\b(?:[a-z][a-z0-9_]*::){1,6}[a-z][a-z0-9_]+", "", segment).strip()
        if "_" not in token or not re.fullmatch(r"[a-z][a-z0-9_]{11,}", token):
            continue
        matches = sorted(key for key in rust if key.split("::")[-1] == token)
        if matches:
            hits.extend((key, rust[key]) for key in matches)
            notes.append(f"按裸用例名精确定位：{token}")
        else:
            missing.append(token)

    # 2) 壳层裸测试名（载体为壳层时，锚点常直接写名字）
    if not hits and ("壳层" in carrier or "壳层" in anchor):
        for token in re.findall(r"\b([a-z][a-z0-9_]{14,})\b", anchor):
            key = lookup(token, ("shell",))
            if key:
                hits.append((key, rust[key]))

    # 3) 模块级锚点：按子句（＋／；切分）解析「crate `模块` N 项」，逐子句核对整组
    loose_module = False
    count_mismatch = False
    for clause in re.split(r"[＋；;]", anchor):
        module_match = re.search(r"`([a-z][a-z0-9_]*)`", clause)
        if not module_match:
            continue
        module = module_match.group(1)
        claimed_match = re.search(r"(\d+)\s*项", clause)
        crate = ("platform" if "platform" in clause else
                 "kernel" if "kernel" in clause else
                 "shell" if "壳层" in clause else None)
        keys = sorted(key for key in rust
                      if f"::{module}::tests::" in key and (crate is None or key.startswith(f"{crate}::")))
        if not keys:
            continue
        if claimed_match is None:
            loose_module = True
            notes.append(f"{module}::tests::* 整组 {len(keys)} 项"
                         f"{'全绿' if all(rust[key] == 'ok' for key in keys) else '存在未通过'}，但锚点未声称条数也未指名用例")
            module_hits.extend((key, rust[key]) for key in keys)
            continue
        claimed = int(claimed_match.group(1))
        tokens = qualifier_tokens(clause) or qualifier_tokens(anchor)
        narrowed = sorted(key for key in keys if any(token in key.lower() for token in tokens)) if tokens else []
        group = narrowed if narrowed else keys
        if narrowed and len(narrowed) != claimed:
            count_mismatch = True
            notes.append(f"按限定词筛出 {module}::{len(narrowed)} 项、表内声称 {claimed} 项")
        elif len(group) != claimed:
            count_mismatch = True
            notes.append(f"表内声称 {claimed} 项、实测 {module}::tests::* 为 {len(group)} 项")
        else:
            notes.append(f"模块级 {module} 整组 {len(group)} 项（与表内声称一致）"
                         + ("，已按限定词收窄" if narrowed else ""))
        module_hits.extend((key, rust[key]) for key in group)

    # 3b) 语料 ID 形式的锚点（corpus.rs::K06 → corpus::k06_*）
    for cid in re.findall(r"corpus\.rs::([A-Z]\d{2})", anchor):
        prefix = f"corpus::{cid.lower()}_"
        keys = sorted(key for key in rust if key.startswith(prefix))
        if keys:
            hits.extend((key, rust[key]) for key in keys)
            notes.append(f"{cid} 按前缀定位到 {len(keys)} 项")
        else:
            missing.append(f"corpus::{cid.lower()}_*")

    # 4) 前端锚点：按「＋」切段，段内的文件引用只与同段的「…」标题配对，
    #    否则一条锚点里 Vitest 的标题会被错配到 Playwright 的文件上。
    frontend: list[tuple[str, str]] = []
    for segment in re.split("＋", anchor):
        quoted = re.findall(r"「([^」]{4,})」", segment)
        for file in re.findall(r"([A-Za-z0-9_.-]+\.test\.tsx)", segment):
            rows = [row for row in vitest if row["file"] == file]
            if not rows:
                missing.append(f"vitest::{file}")
                continue
            if quoted:
                rows = [row for row in rows if any(phrase[:5] in row["title"] for phrase in quoted)]
                if not rows:
                    missing.append(f"vitest::{file}「{'、'.join(quoted)}」")
                    continue
                notes.append(f"{file} 按标题匹配 {len(rows)} 条")
            frontend.extend((f"vitest::{file}::{row['title']}", row["status"]) for row in rows)
        for file in re.findall(r"([A-Za-z0-9_.-]+\.spec\.ts)", segment):
            rows = [row for row in playwright if row["file"].endswith(file)]
            if not rows:
                missing.append(f"playwright::{file}")
                continue
            if quoted:
                rows = [row for row in rows if any(phrase[:5] in row["title"] for phrase in quoted)]
                if not rows:
                    missing.append(f"playwright::{file}「{'、'.join(quoted)}」")
                    continue
                notes.append(f"{file} 按标题匹配 {len(rows)} 条")
            frontend.extend((f"playwright::{file}::{row['title']}", row["status"]) for row in rows)

    # 5) 名片段兜底：只认**用例名本身**含该片段的测试（避免把整个模块当成一条实例的证据）
    if not hits and not module_hits and not frontend and not loose_module:
        for raw in re.findall(r"\b([A-Za-z][A-Za-z0-9_-]{7,})\b", anchor):
            token = raw.replace("-", "_").lower()
            if not (("_" in token and len(token) >= 10) or len(token) >= 16):
                continue
            matches = sorted(key for key in rust if token in key.split("::")[-1].lower())
            if matches:
                hits.extend((key, rust[key]) for key in matches)
                notes.append(f"按用例名片段 {raw} 定位到 {len(matches)} 项")
                break

    resolved = hits + module_hits + frontend
    if out is not None:
        out.extend(name for name, _ in resolved)
    if not resolved:
        if missing:
            return ("名不符", "逐名", "锚点指向的测试名不在实测清单：" + "、".join(sorted(set(missing))))
        if anchor.strip() in {"✅", "◐", "⛔", "🔶"} or anchor.strip().startswith("无"):
            return ("锚点缺失", "未解析", "锚点列没有可核证据（状态符号或\"无\"），需补测试名或改判状态")
        if loose_module:
            return ("锚点需精确化", "模块级", "；".join(notes) + "——需改写为「crate `模块` N 项」或点名用例")
        return ("锚点不可机器定位", "未解析", "锚点是纯文字描述，实测清单里找不到对应模块或用例名")
    failed = sorted({name for name, status in resolved if status != "ok"})
    ignored = sorted({name for name, status in resolved if status == "ignored"})
    detail = f"{len(resolved)} 项实测通过" + (f"；ignored {len(ignored)}" if ignored else "")
    if notes:
        detail += "；" + "；".join(notes)
    if failed:
        return ("实测失败", "逐名" if hits else "模块级", "失败用例：" + "、".join(failed[:6]))
    if missing:
        return ("名不符", "逐名" if hits else "模块级",
                "部分锚点未命中：" + "、".join(sorted(set(missing))) + f"；其余 {detail}")
    # 锚点里有指名用例／前端用例时，就以指名为准；模块级的条数漂移只作备注（同一行里的
    # "kernel `csv` 18 项"若已过期，仍会在明细里显示出来，等下一轮把锚点改写）。
    if hits or frontend:
        tier = "逐名" if hits else "文件级"
        if count_mismatch:
            detail += "（同行模块级声称条数已过期，见明细）"
        return ("实测通过", tier, detail)
    if count_mismatch:
        return ("计数不符", "模块级", detail)
    if loose_module:
        return ("锚点需精确化", "模块级", detail)
    return ("实测通过", "模块级", detail)


def load_inventory(path: Path):
    """从已落盘的清单复原三套实测结果（审计/迭代用，避免重复跑）。"""
    rust: dict[str, str] = {}
    vitest: list[dict] = []
    playwright: list[dict] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        key, _, status = line.partition("\t")
        if key.startswith("vitest::"):
            rest = key[len("vitest::"):]
            file, _, title = rest.partition("::")
            vitest.append({"file": file, "title": title, "status": status})
        elif key.startswith("playwright::"):
            rest = key[len("playwright::"):]
            file, _, title = rest.partition("::")
            playwright.append({"file": file, "title": title, "status": status})
        else:
            rust[key] = status
    return rust, vitest, playwright


LEDGER_MD = ROOT / "docs" / "testing" / "用例执行台账-v0.1.md"


def write_summary(ledger_rows: list[list[str]], counts: dict[str, int], tiers: dict[str, int],
                  inventory_name: str, started: str, head: str, totals: dict[str, int]) -> None:
    """台账摘要与实跑同批产出，避免手写数字与清单漂移。"""
    order = ["实测通过", "锚点需精确化", "锚点不可机器定位", "锚点缺失", "计数不符", "名不符",
             "实测失败", "待人工执行"]
    unresolved = [row[0] for row in ledger_rows if row[5] == "锚点不可机器定位"]
    s01_count = sum(1 for case_id in unresolved if case_id.startswith("TC-S01-"))
    s02_count = sum(1 for case_id in unresolved if case_id.startswith("TC-S02-"))

    def ids(verdict: str) -> list[str]:
        return [row[0] for row in ledger_rows if row[5] == verdict]

    lines = [
        "# 阶段 0 用例执行台账 v0.1（锚点级实测复核）",
        "",
        f"> 状态：启用｜本轮实测：{started}｜提交 `{head}`｜实测清单：[`执行记录/{inventory_name}`](执行记录/{inventory_name})",
        ">",
        "> 权威：实例矩阵取 TC v0.4 §3；阶段归属与阶段 0 出口口径取 **T v1.13** §2.2／§4；"
        "预览与执行两段式、强制力位置取 **产品方案 v1.22** §7.1／§8.5。本文件不自立任何判定标准。",
        ">",
        "",
        "**这份台账回答一个问题**：映射表里每条实例写的自动化锚点，**这次真的跑过吗、名字还在吗**。",
        "它**不**替代 T/TC 的验收判定——"
        "「实测通过」＝该实例的证据（测试／文件／模块整组）在本次运行里存在且为绿；"
        "判定点是否被完整覆盖仍以《用例覆盖映射表》的状态与缺口列为准。",
        "",
        "## 1. 生成方式（可复跑）",
        "",
        "```bash",
        "python3 scripts/run_case_ledger.py     # 先跑四套载体，再逐条解析锚点，产出本文件与 CSV",
        "python3 scripts/check_docs.py          # 门禁：296 行对齐、判定词法、载体↔判定一致、硬缺陷为零",
        "```",
        "",
        f"采集：`cargo test --workspace --all-features`（{totals['rust']} 条 Rust 用例）＋"
        "`cargo test --manifest-path src-tauri/Cargo.toml`＋`pnpm test`（Vitest {vitest} 条）＋"
        "`pnpm test:e2e`（Playwright {playwright} 条）合并为实测清单。".replace(
            "{vitest}", str(totals["vitest"])).replace("{playwright}", str(totals["playwright"])),
        "",
        "解析层级（`解析层` 列）：**逐名**＝锚点写的是可查到的测试名；**模块级**＝锚点写"
        "「crate `模块` N 项」，按整组核对且 N 与实测条数一致；**文件级**＝前端按测试文件／用例标题配对；"
        "**人工**＝载体是真人工／受控环境，本脚本不代跑；**未解析**＝锚点是纯文字描述。",
        "",
        "## 2. 本轮结果",
        "",
        "| 判定 | 行数 | 含义 |",
        "|---|---|---|",
    ]
    meaning = {
        "实测通过": "锚点解析成功且本次全绿（逐名／模块级整组／前端文件级）",
        "锚点需精确化": "整组测试在跑且为绿，但锚点没指名用例、也没给可核对的「N 项」",
        "锚点不可机器定位": "锚点是纯文字描述，清单里找不到对应模块或用例名",
        "锚点缺失": "锚点列写的是\"无\"或状态符号——该行没有可核证据",
        "计数不符": "锚点声称的条数与实测条数不一致（台账需刷新）",
        "名不符": "锚点指向的测试名在清单里不存在（改名／删除未同步）",
        "实测失败": "解析到的用例本次未通过",
        "待人工执行": "真人工／受控环境载体（Windows 实机日等），本脚本不代跑",
    }
    for verdict in order:
        lines.append(f"| {verdict} | {counts.get(verdict, 0)} | {meaning[verdict]} |")
    lines += ["", f"合计 {len(ledger_rows)} 行；解析层分布：" +
              "／".join(f"{tier} {tiers.get(tier, 0)}" for tier in ("逐名", "模块级", "文件级", "人工", "未解析")), ""]

    loose = ids("锚点需精确化")
    unlocatable = ids("锚点不可机器定位")
    missing_anchor = ids("锚点缺失")
    lines += ["## 3. 待办：把不可机器核对的锚点改成可核对", ""]
    if loose:
        lines.append(
            f"- **锚点需精确化（{len(loose)} 条）**：改成「crate `模块` N 项」或直接点名用例——"
            "多数形如\"kernel `csv` 多行表头 E3004 单测\"，整组确实在跑，缺的是指名。"
            f" 实例：{'、'.join(loose[:12])} 等。")
    else:
        lines.append(
            "- **锚点需精确化（0 条）**：2026-11-02 把上一轮的 43 条整组式锚点逐条改写为具体用例名"
            "（`crate::模块::tests::用例`／`apply_flow::用例`／`tasks::tests::用例`）或"
            "「crate `模块` N 项」整组口径，本类清零。")
    if unlocatable:
        lines.append(
            f"- **锚点不可机器定位（{len(unlocatable)} 条）**：{'、'.join(unlocatable)}——"
            f"其中 {s01_count} 条是 S01 的预检／执行阻断面"
            "（`preflightPlan`／`applyPlan` 的 E2004 前置阻断与零变更），在壳层实测清单里没有对应用例，"
            f"属**真缺测试**而非文字问题；{s02_count} 条是 S02 的\"各功能隐含成立\"，"
            "要么补一条显式断言，要么改判状态。")
    else:
        lines.append(
            "- **锚点不可机器定位（0 条）**：2026-11-02 补齐了上一轮 5 条 S01 壳层断言"
            "（`tasks::tests::preflight_gate_*` 前置阻断 E2004／零变更／逐次重算、UNC 不可达只警告、"
            "通过后可执行并进隔离目录）与 1 条 S02 隐含面断言，本类清零。")
    if missing_anchor:
        lines.append(
            f"- **锚点缺失（{len(missing_anchor)} 条）**：{'、'.join(missing_anchor)}——"
            "并发运行中调参「下一批生效」既无 kernel 断言也无 UI，按 ⛔ 待实现处理更准确（改判需走方案 §11.6）。")
    else:
        lines.append("- **锚点缺失（0 条）**：映射表 296 行的锚点列均写有可核证据或已注明人工载体。")
    lines += ["", "## 4. 口径边界（不谎报）", "",
              "- 「实测通过」**不等于**用例验收通过：模块级／文件级判定的证据是\"整组在跑且绿\"，"
        "该实例的判定点是否被完整覆盖仍以映射表状态列（✅／◐）与缺口列为准。",
        "- 覆盖率不因本台账而改变：本表只增加\"证据是否真跑\"这一层可核性。",
        "- 34 条待人工执行行全部落在 Windows 实机日／受控环境（载体列已注明），Linux 预跑不代替。".replace(
            "34 条", f"{counts.get('待人工执行', 0)} 条"),
        "- 门禁只放行：判定词法合法、与映射表 296 行一一对应、载体与判定一致、清单文件存在，"
        "且**计数不符／名不符／实测失败为 0**（这三类是证据链已断，出现即 CI 红）；"
        "锚点需精确化／不可机器定位／锚点缺失三类**不阻断 CI**，但必须在 §3 逐条列出、不得静默。",
        "",
    ]
    LEDGER_MD.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"摘要落盘：{LEDGER_MD}")


def main() -> None:
    parser = argparse.ArgumentParser(description="逐条把 TC 阶段 0 实例的锚点对到实测清单并出台账")
    parser.add_argument("--reuse-cargo", help="复用已有的根 workspace 日志（仅迭代用，正式记录不得用）")
    parser.add_argument("--reuse-inventory", help="从已落盘清单复原实测结果（审计模式，不重跑）")
    parser.add_argument("--offline", action="store_true", help="不跑前端两套（仅迭代用）")
    args = parser.parse_args()

    started = time.strftime("%Y-%m-%dT%H:%M:%S%z")
    head = subprocess.run(
        ["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, capture_output=True, text=True
    ).stdout.strip()
    if args.reuse_inventory:
        rust, vitest, playwright = load_inventory(Path(args.reuse_inventory))
        started = f"{started}（审计模式：清单取自 {Path(args.reuse_inventory).name}）"
    else:
        rust = collect_rust(args.reuse_cargo)
        vitest = collect_vitest(args.offline)
        playwright = collect_playwright(args.offline)
    print(f"实测清单：Rust {len(rust)} 条、Vitest {len(vitest)} 条、Playwright {len(playwright)} 条")

    RECORD_DIR.mkdir(parents=True, exist_ok=True)
    inventory = RECORD_DIR / f"{time.strftime('%Y-%m-%d')}-inventory.tsv"
    with inventory.open("w", encoding="utf-8") as sink:
        for key, status in sorted(rust.items()):
            sink.write(f"{key}\t{status}\n")
        for row in vitest:
            sink.write(f"vitest::{row['file']}::{row['title']}\t{row['status']}\n")
        for row in playwright:
            sink.write(f"playwright::{row['file']}::{row['title']}\t{row['status']}\n")

    commands = {
        "kernel/platform/cli/corpus/apply_flow": "cargo test --workspace --all-features",
        "shell": "cargo test --manifest-path src-tauri/Cargo.toml",
        "vitest": "pnpm test",
        "playwright": "pnpm test:e2e",
    }
    header = [
        "实例ID", "功能", "类型", "表内状态", "载体", "判定", "解析层", "解析明细",
        "实测命令", "实测时间", "提交", "清单文件",
    ]
    ledger_rows = []
    stats: dict[str, int] = {}

    def pick_command(keys: list[str]) -> str:
        """「实测命令」取解析到的清单键所属载体——锚点写壳层但根 workspace 也跑过的行不再误标。"""
        crates = {key.split("::", 1)[0] for key in keys}
        chosen = []
        if crates - {"shell", "vitest", "playwright"}:
            chosen.append(commands["kernel/platform/cli/corpus/apply_flow"])
        for crate, label in (("shell", "shell"), ("vitest", "vitest"), ("playwright", "playwright")):
            if crate in crates:
                chosen.append(commands[label])
        return "＋".join(chosen) if chosen else commands["kernel/platform/cli/corpus/apply_flow"]

    for row in load_map_rows():
        fid, fname, _stage, _prio, case_id, kind, _seq, _recipe, _points, _src, status, carrier, anchor, _gap = row
        keys: list[str] = []
        verdict, tier, detail = resolve(anchor, carrier, rust, vitest, playwright, keys)
        stats[verdict] = stats.get(verdict, 0) + 1
        command = "—（人工／受控环境）" if verdict == "待人工执行" else pick_command(keys)
        ledger_rows.append([
            case_id, fname, kind, status, carrier, verdict, tier, detail,
            command, started, head, inventory.name,
        ])

    with LEDGER_CSV.open("w", encoding="utf-8", newline="") as sink:
        writer = csv.writer(sink, lineterminator="\n")
        writer.writerow(header)
        writer.writerows(ledger_rows)

    print(f"台账落盘：{LEDGER_CSV}（{len(ledger_rows)} 行）")
    for verdict in sorted(stats, key=stats.get, reverse=True):
        print(f"  {verdict}: {stats[verdict]}")
    tiers: dict[str, int] = {}
    for row in ledger_rows:
        tiers[row[6]] = tiers.get(row[6], 0) + 1
    write_summary(ledger_rows, stats, tiers, inventory.name, started, head,
                  {"rust": len(rust), "vitest": len(vitest), "playwright": len(playwright)})
    print(f"清单落盘：{inventory}（Rust {len(rust)}＋Vitest {len(vitest)}＋Playwright {len(playwright)}）")
    json.dump(
        {"started": started, "head": head, "rust": len(rust), "vitest": len(vitest),
         "playwright": len(playwright), "verdicts": stats},
        open(ledger_tmp("meta.json"), "w", encoding="utf-8"),
        ensure_ascii=False,
    )


if __name__ == "__main__":
    main()
