#!/usr/bin/env python3
"""文档一致性门禁：链接/锚点、版本锚点、expected.md、错误码白名单、用例覆盖映射表、表格列数。

只使用标准库；CI 的 lint job 必跑。规则以“当前锚点”为准，不检查历史修订记录
（CHANGELOG、基线修订表、ADR 正文中的历史版本引用允许保留）。

用法：
    python scripts/check_docs.py
"""

from __future__ import annotations

import csv
import hashlib
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
IGNORED_DIRS = {".git", "target", "node_modules"}
BASELINE_MANIFEST_RE = re.compile(
    r"^\|\s*《[^》]+》\s*\|\s*[^|]+\|\s*`([^`]+)`\s*\|\s*`([0-9a-fA-F]{64})`\s*\|"
)
REQUIRED_DOCS = [
    "docs/README.md",
    "docs/design/README.md",
    "docs/design/TRACEABILITY.md",
    "docs/design/14-AI助手与工具调用契约.md",
    "docs/design/proposals/F15-AI助手-方案变更提案.md",
    "docs/design/proposals/F15-方案第14章草案.md",
    "docs/testing/README.md",
    "docs/testing/性能基准执行手册.md",
    "docs/testing/AI助手测试分册-AI-v0.1.md",
    "fixtures/ai/README.md",
    "docs/design/assets/README.md",
    "docs/baseline/MANIFEST.md",
    "docs/baseline/PENDING-DECISIONS.md",
    "docs/OPEN-QUESTIONS.md",
    "docs/engineering-decisions.md",
]

LINK_RE = re.compile(r"\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
BASELINE_ROW_RE = re.compile(
    r"^\|\s*《([^》]+)》\s*\|\s*([^|]+?)\s*\|\s*`([^`]+)`\s*\|"
)
README_ROW_RE = re.compile(r"^\|\s*《([^》]+)》\s*\|\s*([^|]+?)\s*\|")
CODE_RE = re.compile(r"\bE\d{4}\b")
# E4（任务控制）✅ 行必须引用的取消类锚点（kernel 臂级回滚／壳层队列状态机／真 WebView E2E）。
CANCEL_ANCHOR_KEYS = (
    "cancel_rolls_back",
    "cancel_mid_batch",
    "task_state_events_cover_pause_resume_and_queued_cancel",
    "e2e-tauri",
)
# 设计索引行（`| [01](./01-….md) | 标题 | 状态… |`）与版本号 token（允许 `v1` 这种主版本写法）。
DESIGN_ROW_RE = re.compile(r"^\|\s*\[(\d\d)\]\(\./([^)]+)\)\s*\|")
DESIGN_VERSION_RE = re.compile(r"v(\d+(?:\.\d+)?)")


def md_files() -> list[Path]:
    return [
        p
        for p in sorted(ROOT.rglob("*.md"))
        if not any(part in IGNORED_DIRS for part in p.parts)
    ]


def slugify(heading: str) -> str:
    slug = heading.strip().lower()
    slug = re.sub(r"[^\w\u4e00-\u9fff\- ]", "", slug)
    return slug.replace(" ", "-")


def heading_slugs(text: str) -> set[str]:
    seen: dict[str, int] = {}
    result: set[str] = set()
    for line in text.splitlines():
        if not line.startswith("#"):
            continue
        base = slugify(line.lstrip("#").strip())
        count = seen.get(base, 0)
        result.add(base if count == 0 else f"{base}-{count}")
        seen[base] = count + 1
    return result


def check_links(errors: list[str]) -> None:
    cache: dict[Path, set[str]] = {}
    for path in md_files():
        text = path.read_text(encoding="utf-8")
        for match in LINK_RE.finditer(text):
            target = match.group(1).strip()
            if target.startswith(("http://", "https://", "mailto:")):
                continue
            if target.startswith("#"):
                dest, fragment = path, target[1:]
            else:
                path_part, _, fragment = target.partition("#")
                dest = (path.parent / path_part).resolve()
                if not dest.exists():
                    errors.append(
                        f"{path.relative_to(ROOT)}: 链接目标不存在 -> {target}"
                    )
                    continue
            if not fragment or dest.suffix.lower() != ".md":
                continue
            if dest not in cache:
                cache[dest] = heading_slugs(dest.read_text(encoding="utf-8"))
            if fragment not in cache[dest]:
                errors.append(
                    f"{path.relative_to(ROOT)}: 锚点不存在 -> {target}"
                )


def parse_baseline_versions() -> dict[str, tuple[str, str]]:
    path = ROOT / "docs/baseline/README.md"
    text = path.read_text(encoding="utf-8")
    versions: dict[str, tuple[str, str]] = {}
    for line in text.splitlines():
        match = BASELINE_ROW_RE.match(line)
        if match:
            title, version, filename = match.groups()
            versions[title.strip()] = (version.strip(), filename.strip())
    return versions


def parse_readme_versions() -> dict[str, str]:
    path = ROOT / "README.md"
    text = path.read_text(encoding="utf-8")
    versions: dict[str, str] = {}
    for line in text.splitlines():
        match = README_ROW_RE.match(line)
        if match:
            title, version = match.groups()
            versions[title.strip()] = version.strip()
    return versions


def check_versions(errors: list[str]) -> dict[str, str]:
    baseline = parse_baseline_versions()
    readme = parse_readme_versions()

    if not baseline:
        errors.append("docs/baseline/README.md: 未解析到基线版本表")
    for title, (version, filename) in baseline.items():
        target = ROOT / "docs/baseline" / filename
        if not target.is_file():
            errors.append(f"docs/baseline/README.md: 基线文件不存在 -> {filename}")
        if title not in readme:
            errors.append(f"README.md: 基线文档地图缺少《{title}》")
        elif readme[title] != version:
            errors.append(
                f"README.md: 《{title}》版本 {readme[title]} 与 docs/baseline/README.md 的 {version} 不一致"
            )

    product = baseline.get("办公辅助工具产品方案", ("", ""))[0]
    test = baseline.get("功能测试方案与验收标准", ("", ""))[0]

    testing_docs = list((ROOT / "docs/testing").glob("*.md"))
    for path in testing_docs:
        text = path.read_text(encoding="utf-8")
        if test and test not in text:
            errors.append(f"{path.relative_to(ROOT)}: 未锚定当前测试基线 {test}")
        if product and product not in text:
            errors.append(f"{path.relative_to(ROOT)}: 未锚定当前产品基线 {product}")

    for rel in [
        "fixtures/README.md",
        "fixtures/corpus/manifest.md",
        "fixtures/corpus/manual-only.txt",
    ]:
        path = ROOT / rel
        if path.is_file() and test and test not in path.read_text(encoding="utf-8"):
            errors.append(f"{rel}: 未锚定当前测试基线 {test}")

    return {"product": product, "test": test}


def check_corpus_expected(errors: list[str], test_version: str) -> None:
    for path in sorted((ROOT / "fixtures/corpus").glob("*/expected.md")):
        text = path.read_text(encoding="utf-8")
        rel = path.relative_to(ROOT)
        if "预期行为" not in text:
            errors.append(f"{rel}: 缺少“预期行为”摘要")
        if "判定权威" not in text:
            errors.append(f"{rel}: 缺少“判定权威”字段")
        if test_version and f"{test_version} §3.2" not in text:
            errors.append(f"{rel}: 未回指 {test_version} §3.2")


def defined_error_codes() -> set[str]:
    codes: set[str] = set()
    for path in sorted((ROOT / "docs/design").glob("*.md")):
        codes.update(CODE_RE.findall(path.read_text(encoding="utf-8")))
    return codes


def check_error_codes(errors: list[str]) -> None:
    defined = defined_error_codes()
    if not defined:
        errors.append("docs/design/: 未解析到任何错误码，检查错误码表是否缺失")
        return
    targets = list((ROOT / "fixtures/corpus").glob("*/expected.md"))
    targets += list((ROOT / "fixtures/corpus").glob("*/gen_*.py"))
    targets += list((ROOT / "docs/testing").glob("*.md"))
    for path in targets:
        for code in sorted(set(CODE_RE.findall(path.read_text(encoding="utf-8")))):
            if code not in defined:
                errors.append(
                    f"{path.relative_to(ROOT)}: 使用了未在 docs/design/ 定义的错误码 {code}"
                )


def check_required_docs(errors: list[str]) -> None:
    for rel in REQUIRED_DOCS:
        if not (ROOT / rel).is_file():
            errors.append(f"缺少文档索引/治理文件：{rel}")


def check_baseline_hashes(errors: list[str]) -> None:
    manifest = ROOT / "docs/baseline/MANIFEST.md"
    if not manifest.is_file():
        return
    rows = 0
    for line in manifest.read_text(encoding="utf-8").splitlines():
        match = BASELINE_MANIFEST_RE.match(line)
        if not match:
            continue
        rows += 1
        filename, expected = match.groups()
        path = ROOT / "docs/baseline" / filename
        if not path.is_file():
            errors.append(f"docs/baseline/MANIFEST.md: 基线文件不存在 -> {filename}")
            continue
        actual = hashlib.sha256(path.read_bytes()).hexdigest()
        if actual.lower() != expected.lower():
            errors.append(
                f"docs/baseline/MANIFEST.md: {filename} 哈希不符（快照 {expected[:16]}… / 实际 {actual[:16]}…）"
            )
    if rows == 0:
        errors.append("docs/baseline/MANIFEST.md: 未解析到基线哈希表")


def check_case_map(errors: list[str]) -> None:
    """用例覆盖映射表 CSV 的自校验：条数对账、锚点实名、状态与缺口/载体一致。"""
    path = ROOT / "docs/testing/用例覆盖映射表-v0.2.csv"
    if not path.is_file():
        errors.append("缺少用例覆盖映射表 CSV：docs/testing/用例覆盖映射表-v0.2.csv")
        return
    rows = list(csv.DictReader(path.open(encoding="utf-8")))
    if len(rows) != 296:
        errors.append(f"用例覆盖映射表 CSV：行数 {len(rows)}≠TC §3.1 的 296")
    ids = [row["实例ID"] for row in rows]
    if len(set(ids)) != len(ids):
        errors.append("用例覆盖映射表 CSV：实例 ID 有重复")

    def fns(rel: str) -> set[str]:
        source = (ROOT / rel).read_text(encoding="utf-8")
        return set(re.findall(r"(?m)^\s*(?:#\[[^\]]*\]\s*)?fn ([a-z0-9_]+)\(\)", source))

    tables = {
        "generic_apply::tests": fns("crates/kernel/src/generic_apply.rs"),
        "apply_flow": fns("crates/cli/tests/apply_flow.rs"),
        "corpus.rs": fns("crates/kernel/tests/corpus.rs"),
        # 壳层与 platform 的命名空间也在内：锚点写了 tasks::tests::x 就必须真存在
        "tasks::tests": fns("src-tauri/src/tasks.rs"),
        "platform::preflight::tests": fns("crates/platform/src/preflight.rs"),
        "kernel::preflight::tests": fns("crates/kernel/src/preflight.rs"),
    }
    for row in rows:
        instance, cell = row["实例ID"], row["自动化锚点"]
        if "…" in cell:
            errors.append(f"{instance}: 自动化锚点含省略式写法，须写实名测试")
        for prefix, names in tables.items():
            for name in re.findall(rf"{re.escape(prefix)}::([a-z0-9_]+)", cell):
                if name not in names:
                    errors.append(f"{instance}: 锚点 {prefix}::{name} 在源码中不存在")
        if row["状态"] == "🔶" and "WebDriver" not in row["载体"]:
            errors.append(f"{instance}: 🔶 行的载体列须注明 WebDriver")
        if row["状态"] == "⛔" and row["载体"].startswith("CI"):
            errors.append(f"{instance}: ⛔（人工/受控/待裁决）行不得标 CI 载体")
        if row["状态"] in ("✅", "◐") and "真人工" in row["载体"]:
            errors.append(f"{instance}: 自动化行（✅／◐）的载体不得记真人工，须记已有锚点的运行位置")
        if row["状态"] == "✅" and row["缺口 / 待办"]:
            errors.append(f"{instance}: ✅ 表示自动化闭环，不得同时挂缺口（应降为 ◐）")
        if row["配方"] == "E4" and row["状态"] == "✅" and not any(
            key in cell for key in CANCEL_ANCHOR_KEYS
        ):
            errors.append(f"{instance}: E4（任务控制）✅ 行须引用取消/暂停回滚断言")


TABLE_SEP_RE = re.compile(r"^\s*\|(\s*:?-{2,}:?\s*\|)+\s*$")
BARE_PIPE_RE = re.compile(r"(?<!\\)\|")


LEDGER_VERDICTS = {
    "实测通过", "锚点需精确化", "锚点不可机器定位", "锚点缺失",
    "计数不符", "名不符", "实测失败", "待人工执行",
}
# 这三类是"证据链已断"的硬缺陷：出现即 CI 红，必须当场改锚点或改判定，不许留过夜。
LEDGER_BLOCKING = {"计数不符", "名不符", "实测失败"}


def check_case_ledger(errors: list[str]) -> None:
    """用例执行台账 CSV 的自校验：与映射表逐行对齐、判定词法合法、硬缺陷为零。

    台账由 scripts/run_case_ledger.py 产出（先跑四套载体，再把每条锚点解析到实测清单）。
    它回答的是"证据是否真跑过、名字是否还在"，不是"用例判定是否通过"——后者仍由 TC 判定点承担。
    """
    path = ROOT / "docs/testing/用例执行台账-v0.1.csv"
    if not path.is_file():
        errors.append("缺少用例执行台账 CSV：docs/testing/用例执行台账-v0.1.csv（先跑 scripts/run_case_ledger.py）")
        return
    rows = list(csv.DictReader(path.open(encoding="utf-8")))
    if len(rows) != 296:
        errors.append(f"用例执行台账应为 296 行，实际 {len(rows)} 行")
    map_path = ROOT / "docs/testing/用例覆盖映射表-v0.2.csv"
    mapped = [r["实例ID"] for r in csv.DictReader(map_path.open(encoding="utf-8"))]
    ledger_ids = [r["实例ID"] for r in rows]
    if len(set(ledger_ids)) != len(ledger_ids):
        errors.append("用例执行台账的实例ID 不唯一")
    missing = set(mapped) - set(ledger_ids)
    extra = set(ledger_ids) - set(mapped)
    if missing:
        errors.append(f"用例执行台账缺少映射表中的实例：{sorted(missing)[:5]}")
    if extra:
        errors.append(f"用例执行台账出现映射表没有的实例：{sorted(extra)[:5]}")
    counts: dict[str, int] = {}
    for row in rows:
        verdict = row["判定"]
        counts[verdict] = counts.get(verdict, 0) + 1
        if verdict not in LEDGER_VERDICTS:
            errors.append(f"{row['实例ID']}：判定值 {verdict!r} 不在词法表内")
        manual_carrier = "真人工" in row["载体"]
        if manual_carrier != (verdict == "待人工执行"):
            errors.append(
                f"{row['实例ID']}：载体与判定不一致（载体真人工={manual_carrier}、判定={verdict}）——"
                "真人工行只能是待人工执行，反之待人工行必须有真人工/受控环境载体"
            )
        if verdict in LEDGER_BLOCKING:
            errors.append(
                f"{row['实例ID']}：判定 {verdict}（{row['解析明细'][:80]}）——证据链已断，需改锚点或补测试"
            )
        inventory = ROOT / "docs/testing/执行记录" / row["清单文件"]
        if not inventory.is_file():
            errors.append(f"{row['实例ID']}：实测清单文件不存在 {row['清单文件']}")
    for verdict in sorted(LEDGER_BLOCKING):
        if counts.get(verdict):
            errors.append(f"台账存在 {counts.get(verdict)} 行 {verdict}（见上）")
    # 摘要 md 的 §2 表与 CSV 必须同源：手改过 md（或 md 没跟着重跑）就报错，
    # 否则"数字不手写"只在生成侧成立、读的人仍可能拿到过期统计。
    md = ROOT / "docs/testing/用例执行台账-v0.1.md"
    if md.is_file():
        text = md.read_text(encoding="utf-8")
        for verdict in LEDGER_VERDICTS:
            found = re.findall(rf"^\| {re.escape(verdict)} \| (\d+) \|", text, flags=re.MULTILINE)
            if not found:
                errors.append(f"台账摘要 md 缺少「{verdict}」的统计表行")
            elif found[0] != str(counts.get(verdict, 0)):
                errors.append(
                    f"台账摘要 md 的「{verdict}」写作 {found[0]}，CSV 实为 {counts.get(verdict, 0)}——md 未随 CSV 重跑"
                )


# 用例覆盖统计的第二份数字：源只认映射表与执行台账两份 CSV。
# 日期表行（`| 2026-.. |` 开头的修订记录）是历史，按文件头约定不检查。
CASE_STATS_FILES = (
    "docs/DEVELOPMENT-PLAN.md",
    "docs/design/TRACEABILITY.md",
    "docs/testing/README.md",
    "docs/README.md",
    "AGENTS.md",
)
CASE_STATS_MAP_CSV = "docs/testing/用例覆盖映射表-v0.2.csv"
CASE_STATS_MAP_MD = "docs/testing/用例覆盖映射表-v0.2.md"
CASE_STATS_LEDGER_CSV = "docs/testing/用例执行台账-v0.1.csv"
CASE_STATS_DIST_RE = re.compile(r"✅\s*(\d+)\s*／\s*◐\s*(\d+)\s*／\s*⛔\s*(\d+)")
CASE_STATS_RATE_RE = re.compile(r"自动化可达（✅＋◐）\*\*(\d+) 条＝([\d.]+)%\*\*")
CASE_STATS_MANUAL_RE = re.compile(r"(?<![A-Za-z0-9])(\d+)\s*条待人工")
CASE_STATS_TOTAL = 296


def check_case_stats(errors: list[str]) -> None:
    """用例覆盖统计（✅／◐／⛔ 分布、自动化可达率、待人工条数）必须等于两份 CSV 的实测值。

    动因（2026-11-05，K08 收口当天）：`assertions.tsv` 翻成 24 asserted／0 partial、
    映射表重算成 ✅175／◐88／⛔33 之后，四份"活口径"文档仍写着旧值——
    `DEVELOPMENT-PLAN` §3.3 写「只剩 1 条 partial（K08）」、FAQ 行写「✅174／◐89／
    ⛔33」与「34 条待人工」、`TRACEABILITY` 测试行同款、映射表 md 的 §2 汇总
    自称「由 CSV 计算，非手填」而实际是手填。既有门禁核了语料断言分布（⑫，但只扫
    §3.1 的「验证」行）、设计索引版本（⑬）、映射表与台账的逐行自校，唯独没人核
    "文档正文里手抄的那份统计"——于是过期数字可以一直挂着，而它正是排期依据。

    口径：
    - 分布与可达率以映射表 CSV 的状态列为准（✅／◐／⛔／🔶 四个计数，合计须为 296）；
    - 待人工条数以执行台账 CSV「判定＝待人工执行」的行数为准，不用 ⛔ 反推
      （⛔ 含"功能未实现／待裁决"，与"要人上机"不是一回事，现在恰好相等是巧合）；
    - 映射表 md §2 四行、可达率行必须与 CSV 同源（该节标题已声明"由 CSV 计算，非手填"）；
    - 其余文档只核"写出来的分布／待人工数"，历史修订行跳过。
    """
    map_path = ROOT / CASE_STATS_MAP_CSV
    ledger_path = ROOT / CASE_STATS_LEDGER_CSV
    if not map_path.is_file() or not ledger_path.is_file():
        errors.append("用例覆盖统计核对无法进行：缺少映射表或执行台账 CSV")
        return
    states: dict[str, int] = {}
    for row in csv.DictReader(map_path.open(encoding="utf-8")):
        states[row["状态"]] = states.get(row["状态"], 0) + 1
    if sum(states.values()) != CASE_STATS_TOTAL:
        errors.append(
            f"用例覆盖统计自校失败：映射表状态分布合计 {sum(states.values())}≠{CASE_STATS_TOTAL}"
        )
        return
    verdicts: dict[str, int] = {}
    for row in csv.DictReader(ledger_path.open(encoding="utf-8")):
        verdicts[row["判定"]] = verdicts.get(row["判定"], 0) + 1
    manual = verdicts.get("待人工执行", 0)
    automated = states.get("✅", 0) + states.get("◐", 0)
    rate = f"{automated * 100 / CASE_STATS_TOTAL:.1f}"

    md = ROOT / CASE_STATS_MAP_MD
    if not md.is_file():
        errors.append(f"找不到 {CASE_STATS_MAP_MD}：用例覆盖统计核对无法进行")
    else:
        text = md.read_text(encoding="utf-8")
        for glyph in ("✅", "◐", "⛔", "🔶"):
            found = re.findall(rf"^\| {re.escape(glyph)}[^|]*\| (\d+) \|", text, flags=re.MULTILINE)
            if not found:
                errors.append(f"{CASE_STATS_MAP_MD} §2 汇总缺少「{glyph}」的统计表行")
            elif int(found[0]) != states.get(glyph, 0):
                errors.append(
                    f"{CASE_STATS_MAP_MD} §2 汇总的「{glyph}」写作 {found[0]}，"
                    f"CSV 实为 {states.get(glyph, 0)}——该节声明由 CSV 计算，须同步重算"
                )
        matched = CASE_STATS_RATE_RE.search(text)
        if not matched:
            errors.append(f"{CASE_STATS_MAP_MD} §2 汇总结论行写法变了（找不到「自动化可达（✅＋◐）…」）")
        elif (int(matched.group(1)), matched.group(2)) != (automated, rate):
            errors.append(
                f"{CASE_STATS_MAP_MD} §2 汇总结论写作 {matched.group(1)} 条＝{matched.group(2)}%，"
                f"CSV 实为 {automated} 条＝{rate}%"
            )

    seen_dist = seen_manual = 0
    want = (states.get("✅", 0), states.get("◐", 0), states.get("⛔", 0))
    for rel in CASE_STATS_FILES:
        path = ROOT / rel
        if not path.is_file():
            errors.append(f"找不到 {rel}：用例覆盖统计核对无法进行")
            continue
        for lineno, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if line.startswith("| 20"):
                continue
            for matched in CASE_STATS_DIST_RE.finditer(line):
                seen_dist += 1
                got = tuple(int(matched.group(i)) for i in (1, 2, 3))
                if got != want:
                    errors.append(
                        f"{rel}:{lineno}: 用例覆盖统计漂移——写 ✅{got[0]}／◐{got[1]}／⛔{got[2]}，"
                        f"CSV 实为 ✅{want[0]}／◐{want[1]}／⛔{want[2]}"
                    )
            for matched in CASE_STATS_MANUAL_RE.finditer(line):
                seen_manual += 1
                if int(matched.group(1)) != manual:
                    errors.append(
                        f"{rel}:{lineno}: 待人工条数漂移——写 {matched.group(1)} 条，"
                        f"执行台账实为 {manual} 条（判定＝待人工执行）"
                    )
    # 探测器自校：正例太少说明锚点或写法变了，检查会静默永真
    if seen_dist < 3 or seen_manual < 2:
        errors.append(
            f"用例覆盖统计核对自校失败：只核到 {seen_dist} 处分布、{seen_manual} 处待人工"
            "——锚点或写法变了，探测器失效＝没有正例"
        )


def check_table_arity(errors: list[str]) -> None:
    """Markdown 表格的列数必须与分隔行一致：代码段里的裸竖线会串列（渲染时列会整体错位）。

    docs/baseline 跳过——基线快照按 MANIFEST 哈希校验，改一字即破链，其缺陷由人类基线文档修。
    fixtures 纳入：环境台账与结果台账（C-4/env.md、bench/results.md）都是手写表格，与 docs 同风险。
    """
    targets = sorted((ROOT / "docs").rglob("*.md")) + sorted(ROOT.glob("*.md"))
    targets += [p for p in sorted((ROOT / "fixtures").rglob("*.md")) if "out" not in p.parts]
    for path in targets:
        if "baseline" in path.parts:
            continue
        lines = path.read_text(encoding="utf-8").splitlines()
        index = 0
        while index < len(lines):
            if TABLE_SEP_RE.match(lines[index]) and index > 0:
                expect = len(BARE_PIPE_RE.findall(lines[index]))
                row = index + 1
                while row < len(lines) and lines[row].lstrip().startswith("|") and not TABLE_SEP_RE.match(lines[row]):
                    actual = len(BARE_PIPE_RE.findall(lines[row]))
                    if actual != expect:
                        errors.append(
                            f"{path.relative_to(ROOT)}:{row + 1}: 表格列数 {actual - 1}≠分隔行的 {expect - 1}"
                            "（代码段内的竖线须写成 \\|）"
                        )
                    if not lines[row].rstrip().endswith("|"):
                        errors.append(
                            f"{path.relative_to(ROOT)}:{row + 1}: 表格行未以竖线收尾——单元格内不得有物理换行"
                        )
                    row += 1
                index = row
            else:
                index += 1


def _one_line_block(text: str, anchor: str) -> str:
    """取 anchor 起、到该行末的内容（用于 `export type X = "a" | "b";` 这类单行联合）。"""
    i = text.find(anchor)
    if i < 0:
        return ""
    j = text.find("\n", i)
    return text[i : (j if j >= 0 else len(text))]


def _paired_block(text: str, anchor: str, opener: str = "{") -> str | None:
    """取 anchor 之后第一个配对完整的 `\u2026opener X closer\u2026` 块（含边界字符）。

    注意：TS 的类型注解里也有 `{}`，所以数组要用 opener="["，别拿第一个花括号。
    """
    closer = {"{": "}", "[": "]"}[opener]
    i = text.find(anchor)
    if i < 0:
        return None
    open_at = text.find(opener, i)
    if open_at < 0:
        return None
    depth, j = 0, open_at
    while j < len(text):
        if text[j] == opener:
            depth += 1
        elif text[j] == closer:
            depth -= 1
            if depth == 0:
                return text[open_at : j + 1]
        j += 1
    return None


def check_kind_routing(errors: list[str]) -> None:
    """功能码三处清单必须相等：壳层 planOperation 路由 ↔ 导航表 ↔ FeatureView 联合。

    动因：三处此前各抄一份，加一个功能漏一处不会有任何东西变红——
    路由漏了则用户点页面得到 E1002，导航漏了则功能有路由却没有入口。
    （c09 工作台不在功能视图里，单独排除；F13 只有内核与 CLI，两侧都不该出现。）
    """
    lib = ROOT / "src-tauri" / "src" / "lib.rs"
    catalog = ROOT / "web" / "src" / "navCatalog.ts"
    if not lib.exists() or not catalog.exists():
        errors.append("找不到 src-tauri/src/lib.rs 或 web/src/navCatalog.ts：功能码路由核对无法进行")
        return
    lib_text = lib.read_text(encoding="utf-8")
    cat_text = catalog.read_text(encoding="utf-8")

    fn_start = lib_text.find("fn planOperation(")
    block = _paired_block(lib_text[fn_start:], "match kind.as_str()") if fn_start >= 0 else None
    routed = set(re.findall(r'"([fc]\d\d)"', block)) if block else set()

    nav_block = _paired_block(cat_text, "export const NAV_ENTRIES", "[")
    nav_ids = set(re.findall(r'\{ id: "([fc]\d\d)", label:', nav_block)) if nav_block else set()
    top_level = _one_line_block(cat_text, "export type TopLevelView =")
    # 顶层视图（c09 工作台等）不是"功能视图"，比对前先摘掉
    nav_feature_ids = nav_ids - set(re.findall(r'"([fc]\d\d)"', top_level))

    union_text = cat_text[cat_text.find("export type FeatureView =") : cat_text.find("export type TopLevelView")]
    view_union = set(re.findall(r'"([fc]\d\d)"', union_text))

    # 探测器自校：任一侧解析为空（或明显缩水）说明锚点/写法变了，检查会静默永真
    if not (routed and nav_feature_ids and view_union) or len(view_union) < 20:
        errors.append(
            f"功能码路由核对自校失败：解析结果异常（路由 {len(routed)}／侧栏功能项 {len(nav_feature_ids)}"
            f"／FeatureView 联合 {len(view_union)}）——锚点或写法变了，探测器失效＝没有正例"
        )
        return
    routed -= {"c09"}
    only_shell = sorted(routed - nav_feature_ids)
    only_nav = sorted(nav_feature_ids - routed)
    union_vs_nav = view_union ^ nav_feature_ids
    if only_shell:
        errors.append(
            f"壳层 planOperation 路由了 {len(only_shell)} 个功能码但侧栏没有对应视图：{', '.join(only_shell)}"
            "（有路由无入口＝用户点不到，或该补页面）"
        )
    if only_nav:
        errors.append(
            f"侧栏有 {len(only_nav)} 个功能视图但壳层未路由：{', '.join(only_nav)}"
            "（有入口无路由＝用户一点就吃 E1002）"
        )
    if union_vs_nav:
        errors.append(
            f"FeatureView 联合与 NAV_ENTRIES 的功能项不一致：{', '.join(sorted(union_vs_nav))}"
            "（同一件事的两份写法，必须同步）"
        )


ASSERTION_TSV = "crates/kernel/tests/assertions.tsv"
# 只检查「当前陈述」文件；CHANGELOG、ADR 正文、DEVELOPMENT-PLAN §6.1 状态表、
# manifest 落地记录、映射表批次留痕都是历史，按本文件头约定不检查。
ASSERTION_CLAIM_FILES = (
    "AGENTS.md",
    "docs/README.md",
    "fixtures/README.md",
    "fixtures/corpus/manifest.md",
    "fixtures/corpus/manual-results.md",
    "docs/testing/README.md",
    "docs/testing/测试用例库-TC-v0.4.md",
    "docs/design/TRACEABILITY.md",
    "docs/DEVELOPMENT-PLAN.md",
    ".github/workflows/ci.yml",
)
ASSERTION_ANCHORS = {
    # 这两个文件历史状态行极多，只取明确写「当前口径」的锚点行。
    # §3.3 与 FAQ 行是 2026-11-05 补的：K08 收口时这两处仍写「只剩 1 条 partial（K08）」，
    # 而锚点只扫 `| 验证 |` 一行，过期陈述整段漏检。
    "docs/DEVELOPMENT-PLAN.md": ("| 验证 |", "- **阶段 0 功能面**", "| 现在最该做什么？ |"),
    "docs/testing/README.md": ("| 语料生成 |", "| 代码与测试总览 |", "> **CI 口径**"),
}
ASSERTION_SKIP_PREFIXES = ("> **落地记录",)
# 不带分布数字、但同样已经失效的旧措辞（数字规则抓不到它们）。
ASSERTION_STALE_PHRASES = (
    "其余 pending 登记在",
    "标 pending",
    "E03/Z01/Z02 待接",
    "E03 待实现",
    "E03 仍只有生成与范围核对",
    "逐条断言 harness 尚未实现",
    "E03 一条自动化语料的逐条断言",
    "E03 语料断言；",
    "其余 16 条自动化语料",
)
# 数字前不得紧邻字母/数字，否则 `E03 pending` 会被读成「3 pending」。
ASSERTION_STATE_RE = re.compile(r"(?<![A-Za-z0-9])(\d+)\s*条?\s*(asserted|partial|pending)")


def check_assertion_claims(errors: list[str]) -> None:
    """语料断言口径以 assertions.tsv 为单一来源，看住各文档里的「当前陈述」。

    动因：2026-11-04 语料收口后，CI 注释、AGENTS、fixtures/README、manual-results、
    docs/testing/README、TRACEABILITY 等仍写「16 条 pending」「20 asserted／4 partial」
    「E03 待接」。既有门禁只核 tsv 与 corpus.rs，文档里手抄的第二份数字没人看。
    """
    tsv = ROOT / ASSERTION_TSV
    if not tsv.exists():
        errors.append(f"找不到 {ASSERTION_TSV}：语料断言口径核对无法进行")
        return
    counts = {"asserted": 0, "partial": 0, "pending": 0}
    rows = 0
    for raw in tsv.read_text(encoding="utf-8").splitlines():
        if not raw.strip() or raw.startswith("#"):
            continue
        cols = raw.split("\t")
        if len(cols) < 2 or cols[1] not in counts:
            errors.append(f"{ASSERTION_TSV}: 状态列无法识别：{raw[:80]}")
            return
        counts[cols[1]] += 1
        rows += 1
    if rows != 24 or sum(counts.values()) != 24:
        errors.append(
            f"语料断言口径自校失败：{ASSERTION_TSV} 解析出 {rows} 条（应为 24）"
        )
        return

    positive_files: set[str] = set()
    for rel in ASSERTION_CLAIM_FILES:
        path = ROOT / rel
        if not path.exists():
            errors.append(f"找不到 {rel}：语料断言口径核对无法进行")
            continue
        anchors = ASSERTION_ANCHORS.get(rel)
        for lineno, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if line.startswith(ASSERTION_SKIP_PREFIXES):
                continue
            if anchors and not line.startswith(anchors):
                continue
            for match in ASSERTION_STATE_RE.finditer(line):
                written, state = int(match.group(1)), match.group(2)
                if written != counts[state]:
                    errors.append(
                        f"{rel}:{lineno}: 语料断言口径漂移——写 {written} {state}，"
                        f"{ASSERTION_TSV} 实为 {counts[state]} {state}"
                    )
            for phrase in ASSERTION_STALE_PHRASES:
                if phrase in line:
                    errors.append(
                        f"{rel}:{lineno}: 语料断言口径仍是过期表述「{phrase}」"
                    )
            if re.search(rf"(?<![A-Za-z0-9]){counts['asserted']}\s*条?\s*asserted", line):
                positive_files.add(rel)
    # 探测器自校：没有任何文件写出当前数字，说明锚点/写法变了，检查会静默永真
    if len(positive_files) < 3:
        errors.append(
            f"语料断言口径自校失败：只有 {len(positive_files)} 个文件写出当前 asserted 数"
            "——锚点或写法变了，探测器失效＝没有正例"
        )


def _version_tuple(raw: str) -> tuple[int, int]:
    major, _, minor = raw.partition(".")
    return int(major), int(minor or 0)


def check_design_index_versions(errors: list[str]) -> None:
    """设计索引每行写的版本，必须是该契约正文的「当前版本」。

    动因：设计 01/06/09/12 在最近几批里都升过版，索引那行却停在旧号——读者第一站
    （docs/design/README.md）看到 v1.48／v1.4／v0.3／v0.19，正文已是 v1.49／v1.5／
    v0.7／v0.21。索引是「这个仓库现在按哪一版契约做事」的第一个答案，它错了比正文
    错更贵：没人会去正文对一遍。既有门禁核过基线版本，但设计契约的索引版本没人看。

    口径：当前版本＝max（状态行版本，修订表中最大版本号）；索引行、状态行都必须等于它。
    """
    index_path = ROOT / "docs/design/README.md"
    if not index_path.is_file():
        errors.append("docs/design/README.md: 不存在——设计索引版本核对无法进行")
        return
    checked = 0
    for lineno, line in enumerate(index_path.read_text(encoding="utf-8").splitlines(), 1):
        row = DESIGN_ROW_RE.match(line)
        if not row:
            continue
        number, file_name = row.group(1), row.group(2)
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) < 3:
            errors.append(f"docs/design/README.md:{lineno}: 第 {number} 号行不足三列")
            continue
        index_match = DESIGN_VERSION_RE.search(cells[2])
        doc_path = Path("docs/design") / file_name
        if not (ROOT / doc_path).is_file():
            errors.append(f"docs/design/README.md:{lineno}: 契约文件不存在 -> {file_name}")
            continue
        doc_lines = (ROOT / doc_path).read_text(encoding="utf-8").splitlines()
        status_line = next(
            (item for item in doc_lines[:6] if item.lstrip().startswith("> 状态")), None
        )
        status_match = DESIGN_VERSION_RE.search(status_line) if status_line else None
        revision = max(
            (
                _version_tuple(match.group(1))
                for item in doc_lines
                if re.match(r"^\|\s*v\d", item)
                if (match := DESIGN_VERSION_RE.search(item))
            ),
            default=None,
        )
        if index_match is None or status_match is None:
            errors.append(
                f"docs/design/README.md:{lineno}: 第 {number} 号版本核对失效"
                f"（索引{'有' if index_match else '无'}版本、"
                f"状态行{'有' if status_match else '无'}版本）——锚点变了，不静默通过"
            )
            continue
        current = max(_version_tuple(status_match.group(1)), revision or (0, 0))
        if _version_tuple(index_match.group(1)) != current:
            errors.append(
                f"docs/design/README.md:{lineno}: 第 {number} 号索引写 v{index_match.group(1)}，"
                f"正文当前 v{current[0]}.{current[1]}"
            )
        elif _version_tuple(status_match.group(1)) != current:
            errors.append(
                f"{doc_path}: 状态行写 v{status_match.group(1)}，修订表已到 "
                f"v{current[0]}.{current[1]}——升版时漏改状态行"
            )
        checked += 1
    if checked < 14:
        errors.append(
            f"设计索引版本核对自校失败：只解析到 {checked} 行（应为 14）"
            "——行格式变了，核对会静默永真"
        )


def main() -> int:
    errors: list[str] = []
    check_required_docs(errors)
    check_links(errors)
    versions = check_versions(errors)
    check_corpus_expected(errors, versions["test"])
    check_error_codes(errors)
    check_baseline_hashes(errors)
    check_case_map(errors)
    check_case_ledger(errors)
    check_case_stats(errors)
    check_table_arity(errors)
    check_kind_routing(errors)
    check_assertion_claims(errors)
    check_design_index_versions(errors)

    if errors:
        print("文档一致性检查失败：")
        for err in errors:
            print(f"- {err}")
        return 1

    print(
        "文档一致性检查通过：索引文件、链接/锚点、基线版本与哈希、语料 expected.md、"
        "错误码白名单、功能码路由↔侧栏↔FeatureView 三处相等、语料断言口径以 assertions.tsv 为单一来源、设计索引版本＝正文当前版本、用例覆盖映射表（296 条对账＋锚点实名＋状态一致性）、用例执行台账（296 行对齐＋判定词法＋硬缺陷为零）、用例覆盖统计≡两份 CSV 实测、Markdown 表格列数。"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
