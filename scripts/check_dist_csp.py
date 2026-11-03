#!/usr/bin/env python3
"""发布产物层的 CSP 前置断言（设计 12 §1 加固、ADR-0060 §4 的"产物层"那半）。

为什么需要它：`tauri.conf.json` 的 `csp` 是**运行时**策略，它假设打包产物"没有内联脚本、
不引远端资源"。这个假设一旦被打破，症状是**桌面端白屏而浏览器预览一切正常**——CI 里
没有任何东西会红。本脚本在 `pnpm build` 之后读 `dist/index.html`，把该假设钉成检查：

1. `dist/index.html` 存在且是 HTML；
2. 没有**不带 src 的** `<script>`（内联脚本会被 `script-src 'self'` 拦掉）；
3. 没有内联 `<style>` 块与 `style="..."` 属性（样式允许 `'unsafe-inline'`，但出现即说明
   有人在 HTML 里塞样式，值得显式看一眼而不是默默放过）；
4. 没有 `http(s)://` 的 src/href 远端引用（生产 CSP 不放开任何远端来源）。

只使用标准库；CI 的 web job 在构建之后运行。

用法：
    python3 scripts/check_dist_csp.py            # 默认查 web/dist
    python3 scripts/check_dist_csp.py --dist DIR # 指定产物目录（自测用）
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_DIST = ROOT / "web" / "dist"

# 不带 src 的 <script>：注意 `<script type="module" src=...>` 不算内联。
INLINE_SCRIPT_RE = re.compile(r"<script\b(?![^>]*\bsrc\s*=)[^>]*>", re.IGNORECASE)
INLINE_STYLE_BLOCK_RE = re.compile(r"<style\b", re.IGNORECASE)
STYLE_ATTRIBUTE_RE = re.compile(r"\bstyle\s*=\s*\"", re.IGNORECASE)
REMOTE_REF_RE = re.compile(r"\b(?:src|href)\s*=\s*\"(https?://[^\"]+)\"", re.IGNORECASE)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--dist", default=str(DEFAULT_DIST), help="产物目录（默认 web/dist）")
    args = parser.parse_args()

    dist = Path(args.dist)
    index = dist / "index.html"
    if not index.is_file():
        print(
            f"产物不存在：{index}\n先跑 `pnpm build`（CI 中该步骤在本检查之前）。",
            file=sys.stderr,
        )
        return 2

    html = index.read_text(encoding="utf-8")
    failures: list[str] = []

    inline_scripts = INLINE_SCRIPT_RE.findall(html)
    if inline_scripts:
        failures.append(
            f"发现 {len(inline_scripts)} 处内联 <script>（会被 CSP script-src 'self' 拦掉）："
            f"{inline_scripts[0][:80]}"
        )

    if INLINE_STYLE_BLOCK_RE.search(html):
        failures.append("发现内联 <style> 块：样式应来自外链 CSS，不在 HTML 里塞")
    if STYLE_ATTRIBUTE_RE.search(html):
        failures.append("发现 style=\"...\" 属性：同上，HTML 不应携带行内样式")

    remote = REMOTE_REF_RE.findall(html)
    if remote:
        failures.append(
            f"发现 {len(remote)} 处远端引用（生产 CSP 不放开远端来源）：{', '.join(remote[:3])}"
        )

    # 至少要有入口脚本，否则"没有内联脚本"只是因为它什么都没引。
    if not re.search(r"<script\b[^>]*\bsrc\s*=", html, re.IGNORECASE):
        failures.append("产物没有入口 <script src>：构建结果不完整，先确认 dist 是否被清空")

    if failures:
        print("产物 CSP 断言未通过：", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1

    print(
        "产物 CSP 断言通过：index.html 无内联脚本/无内联样式/无远端引用，"
        "入口脚本与样式均为同源外链（与 tauri.conf.json 的生产 csp 相容）。"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
