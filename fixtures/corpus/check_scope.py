#!/usr/bin/env python3
"""语料回归范围核对：显式区分「自动化语料」与「人工跟踪语料」。

背景
----
T v1.12 §3.2 的 28 条语料中，有 4 条的「构造方式」依赖人工操作或真实环境
（R05 断线、E01 Excel 占用、E02 OneDrive 释放空间、E04 配额受限卷），无法脚本化。
本脚本把这一区分**显式化**并作为 CI 门禁，避免三类静默问题：

  A. 新增语料但忘记登记脚本状态 → 该语料永远不会被跑到，且不报错；
  B. 某条语料既有脚本又在人工清单 → 「到底跑不跑」出现歧义；
  C. 磁盘上多了目录但清单里没有 → 语料逃出管理。

单一事实来源
------------
- `manifest.md` 的「脚本状态」列：定义每条语料是 就绪 / 待补 / 人工；
- `manual-only.txt`：定义 CI 显式排除的编号。
两者必须**双向一致**（规则 2），任何一侧单独改动都会失败。

规则
----
1. manifest 行数 = EXPECTED_TOTAL（28，对齐 T v1.12 §3.2）；
2. 「人工」集合（manifest）与 manual-only.txt 双向一致；
3. 状态=就绪 → 必须有 gen_*.py、对应目录与 expected.md，且 expected.md 回指 T v1.12 §3.2；
4. 状态=人工 → 不得有 gen_*.py；
5. 磁盘上的语料目录必须在 manifest 中列出；
6. 状态=待补 → 不要求目录（构建期允许尚未落地）。

用法
----
    python fixtures/corpus/check_scope.py

退出码：0 = 范围自洽；1 = 存在缺口或冲突。
在 GitHub Actions 中会把统计写入 $GITHUB_STEP_SUMMARY。
"""

from __future__ import annotations

import os
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
MANIFEST = HERE / "manifest.md"
MANUAL_LIST = HERE / "manual-only.txt"
EXPECTED_TOTAL = 28

STATUS_READY = "就绪"
STATUS_TODO = "待补"
STATUS_MANUAL = "人工"

ROW_RE = re.compile(r"^\|\s*([KTERZ]\d{2})\s*\|")


def read_manual_ids() -> set[str]:
    """读取人工跟踪清单（允许空行与 `#` 注释）。"""
    if not MANUAL_LIST.exists():
        raise SystemExit(f"缺少人工跟踪清单：{MANUAL_LIST}")
    ids: set[str] = set()
    for raw in MANUAL_LIST.read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0].strip()
        if line:
            ids.add(line)
    return ids


def read_manifest() -> dict[str, str]:
    """从 manifest.md 解析 编号 → 脚本状态。"""
    if not MANIFEST.exists():
        raise SystemExit(f"缺少语料清单：{MANIFEST}")

    table: dict[str, str] = {}
    for raw in MANIFEST.read_text(encoding="utf-8").splitlines():
        if not ROW_RE.match(raw):
            continue
        cells = [c.strip() for c in raw.strip().strip("|").split("|")]
        if len(cells) < 4:
            continue
        fid, status = cells[0], cells[3]
        label = (
            STATUS_READY
            if STATUS_READY in status
            else STATUS_MANUAL
            if STATUS_MANUAL in status
            else STATUS_TODO
        )
        table[fid] = label
    return table


def collect_corpus_dirs() -> set[str]:
    """磁盘上实际存在的语料目录（排除文件与以下划线开头的保留目录）。"""
    return {p.name for p in HERE.iterdir() if p.is_dir() and not p.name.startswith("_")}


def has_script(fixture_id: str) -> bool:
    return any((HERE / fixture_id).glob("gen_*.py"))


def main() -> int:
    errors: list[str] = []

    table = read_manifest()
    manual_ids = read_manual_ids()
    dirs = collect_corpus_dirs()

    ready = {f for f, s in table.items() if s == STATUS_READY}
    todo = {f for f, s in table.items() if s == STATUS_TODO}
    manual = {f for f, s in table.items() if s == STATUS_MANUAL}

    # 规则 1：行数
    if len(table) != EXPECTED_TOTAL:
        errors.append(
            f"manifest 列出 {len(table)} 条，与 T v1.12 §3.2 的 {EXPECTED_TOTAL} 条不符"
        )

    # 规则 2：人工集合双向一致
    for fid in sorted(manual - manual_ids):
        errors.append(f"{fid} 在 manifest 标为「人工」，但未列入 manual-only.txt")
    for fid in sorted(manual_ids - manual):
        errors.append(f"{fid} 在 manual-only.txt 中，但 manifest 未标为「人工」")

    # 规则 3：就绪项必须有脚本、目录与 expected.md
    for fid in sorted(ready):
        if not (HERE / fid).is_dir():
            errors.append(f"{fid} 标为「{STATUS_READY}」但缺少 corpus/{fid}/ 目录")
        if not has_script(fid):
            errors.append(f"{fid} 标为「{STATUS_READY}」但缺少 gen_*.py")
        expected = HERE / fid / "expected.md"
        if not expected.is_file():
            errors.append(f"{fid} 标为「{STATUS_READY}」但缺少 expected.md")
        elif "T v1.12 §3.2" not in expected.read_text(encoding="utf-8"):
            errors.append(f"{fid} 的 expected.md 未回指 T v1.12 §3.2")

    # 规则 4：人工项不得有脚本（二选一，杜绝歧义）
    for fid in sorted(manual):
        if has_script(fid):
            errors.append(
                f"{fid} 标为「{STATUS_MANUAL}」却存在 gen_*.py：请二选一"
                f"（可从脚本跑就改标「{STATUS_READY}」；必须人工就删掉脚本）"
            )

    # 规则 5：磁盘目录必须在清单内
    for fid in sorted(dirs - set(table)):
        errors.append(f"磁盘存在 corpus/{fid}/，但 manifest 未列出（语料逃出管理）")

    # 报告
    auto_ids = ready - manual
    report = "\n".join(
        [
            "## 语料回归范围",
            "",
            f"- 清单条目：**{len(table)}** / 声明 {EXPECTED_TOTAL}",
            f"- 自动化（脚本进 CI）：**{len(auto_ids)}** —— {', '.join(sorted(auto_ids)) or '（无）'}",
            f"- 人工跟踪（CI 显式排除）：**{len(manual)}** —— {', '.join(sorted(manual)) or '（无）'}",
            f"- 尚未落地（待补）：**{len(todo)}**",
            "",
            "> 说明：CI 的语料步骤只处理「自动化」子集；人工项单独跟踪，见 ",
            "> `fixtures/corpus/manual-results.md`。当前只做生成与范围核对，逐条断言尚未实现。",
        ]
    )

    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if errors:
        print(report)
        print("\n### 范围核对失败\n")
        for e in errors:
            print(f"- {e}")
        if summary:
            with open(summary, "a", encoding="utf-8") as fh:
                fh.write(report + "\n\n### 范围核对失败\n\n")
                fh.writelines(f"- {e}\n" for e in errors)
        return 1

    print(report)
    print("\n范围核对通过：自动化与人工集合互斥、清单与磁盘一致。")
    if summary:
        with open(summary, "a", encoding="utf-8") as fh:
            fh.write(report + "\n\n范围核对通过：自动化与人工集合互斥、清单与磁盘一致。\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())