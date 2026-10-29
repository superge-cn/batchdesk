#!/usr/bin/env python3
"""文档一致性门禁：链接/锚点、版本锚点、expected.md、错误码白名单。

只使用标准库；CI 的 lint job 必跑。规则以“当前锚点”为准，不检查历史修订记录
（CHANGELOG、基线修订表、ADR 正文中的历史版本引用允许保留）。

用法：
    python scripts/check_docs.py
"""

from __future__ import annotations

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


def main() -> int:
    errors: list[str] = []
    check_required_docs(errors)
    check_links(errors)
    versions = check_versions(errors)
    check_corpus_expected(errors, versions["test"])
    check_error_codes(errors)
    check_baseline_hashes(errors)

    if errors:
        print("文档一致性检查失败：")
        for err in errors:
            print(f"- {err}")
        return 1

    print(
        "文档一致性检查通过：索引文件、链接/锚点、基线版本与哈希、语料 expected.md、错误码白名单。"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
