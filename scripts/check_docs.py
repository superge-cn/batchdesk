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
    check_table_arity(errors)

    if errors:
        print("文档一致性检查失败：")
        for err in errors:
            print(f"- {err}")
        return 1

    print(
        "文档一致性检查通过：索引文件、链接/锚点、基线版本与哈希、语料 expected.md、"
        "错误码白名单、用例覆盖映射表（296 条对账＋锚点实名＋状态一致性）、用例执行台账（296 行对齐＋判定词法＋硬缺陷为零）、Markdown 表格列数。"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
