#!/usr/bin/env python3
"""T06 语料生成脚本：无任何换行的超大单行文本。

判定权威：T v1.12 §3.2 的 T06 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：按字节硬切并给"无换行可切"提示（方案 §5.1）。

**体积策略**（fixtures/bench 与 README 的"大文件按需生成"口径）：
- 默认生成 16 MiB 单行样本：结构上等价（无换行），并让样本落入方案 §4.8 的流式档区间，
  足以覆盖"按字节硬切"的功能正确性；体积参数是构造参数，非产品阈值；
- `--full` 按 §4.8 的超大文件档生成（> 2 GB），仅供本地性能场景使用，**不进 CI**。

幂等：内容为固定字节重复；内容相同时不重写。
自校验：必须不含任何换行符（\n 与 \r）；长度必须与请求值严格一致。
"""

from __future__ import annotations

import argparse
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "T06.txt"

DEFAULT_BYTES = 16 << 20          # 16 MiB（构造值，非产品阈值）
FULL_BYTES = (2 << 30) + (1 << 20)  # > 2 GB，对应方案 §4.8 的超大文件档边界

# 填充块：不含换行、可读、确定性
CHUNK = ("batchdesk T06 no-newline sample " * 64).encode("ascii")


def build(total_bytes: int) -> bytes:
    repeats, remainder = divmod(total_bytes, len(CHUNK))
    return CHUNK * repeats + CHUNK[:remainder]


def verify(data: bytes, total_bytes: int) -> None:
    if b"\n" in data or b"\r" in data:
        raise SystemExit("T06 语料失效：检测到换行符，与'无换行'定义不符")
    if len(data) != total_bytes:
        raise SystemExit(f"T06 语料失效：长度 {len(data)} ≠ 预期 {total_bytes}")


def main() -> None:
    parser = argparse.ArgumentParser(description="生成 T06 无换行单行样本")
    parser.add_argument(
        "--full",
        action="store_true",
        help="按方案 §4.8 的超大文件档生成（> 2 GB，仅供本地性能场景，勿在 CI 使用）",
    )
    args = parser.parse_args()

    total_bytes = FULL_BYTES if args.full else DEFAULT_BYTES

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build(total_bytes)
    verify(data, total_bytes)

    if OUT_FILE.exists() and OUT_FILE.stat().st_size == total_bytes:
        print(f"T06 已是目标体积，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(f"T06 已生成：{OUT_FILE}（{total_bytes} 字节，无任何换行）")


if __name__ == "__main__":
    main()
