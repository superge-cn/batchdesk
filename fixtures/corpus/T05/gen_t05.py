#!/usr/bin/env python3
"""T05 语料生成脚本：0 字节空文件。

判定权威：T v1.12 §3.2 的 T05 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：单独分组/跳过并报告（方案 §5.10 边界；T v1.12 §3.2 T05 行）。

幂等：空文件天然幂等；已存在且大小为 0 时跳过。
自校验：文件必须存在且大小严格为 0；sha256 必须是空串的固定值（供 manifest 冻结校验）。
"""

from __future__ import annotations

import hashlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "T05.csv"

EMPTY_SHA256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"


def build() -> bytes:
    return b""


def verify(data: bytes) -> None:
    if len(data) != 0:
        raise SystemExit(f"T05 语料失效：内容长度 {len(data)} 字节，与空文件定义不符")

    digest = hashlib.sha256(data).hexdigest()
    if digest != EMPTY_SHA256:
        raise SystemExit(f"T05 语料失效：sha256 与空串固定值不符（{digest}）")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build()
    verify(data)

    if OUT_FILE.exists() and OUT_FILE.stat().st_size == 0:
        print(f"T05 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(f"T05 已生成：{OUT_FILE}（0 字节，sha256={EMPTY_SHA256[:16]}…）")


if __name__ == "__main__":
    main()
