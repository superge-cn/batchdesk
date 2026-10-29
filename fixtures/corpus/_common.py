#!/usr/bin/env python3
"""语料生成公共件：幂等写盘与输出目录管理。

约定来源：fixtures/README.md §3（幂等、自校验、合成数据、输出到 out/）。
本模块只负责"把字节写进 out/ 并统计新写/跳过"，**不做任何语料判定**——
判定与自校验留在各 gen_*.py 内，保证每条语料可独立读懂。
"""

from __future__ import annotations

from pathlib import Path


def emit(here: Path, files: dict[str, bytes]) -> tuple[int, int]:
    """幂等写盘：内容相同则跳过；返回 (新写数, 跳过数)。

    files 的键为相对 out/ 的路径，支持子目录（如 "d00/e00/f.dat"）。
    """
    out = here / "out"
    out.mkdir(parents=True, exist_ok=True)
    written = skipped = 0
    for name, data in files.items():
        target = out / name
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists() and target.read_bytes() == data:
            skipped += 1
            continue
        target.write_bytes(data)
        written += 1
    return written, skipped
