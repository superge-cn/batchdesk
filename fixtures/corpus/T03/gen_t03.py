#!/usr/bin/env python3
"""T03 语料生成脚本：单行长度超过"目标分片值"的超长行。

判定权威：T v1.12 §3.2 的 T03 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：该超长行独占一片并显著标注（方案 §5.1；T v1.12 §3.2 T03 行）。

**不写死产品阈值**：§5.1 的目标分片值是用户可配置参数（原文以示例形式给出），
因此本脚本把它作为构造参数 `--shard-bytes`（CI 用小值放大同类场景），
产品口径一律以方案 §5.1／§9.1 为准，本脚本不复述任何产品默认值。

幂等：固定内容；内容相同时不重写。
自校验：最长行长度必须严格大于 `--shard-bytes`；其他行必须远短于它（保证只有一行超长）。
"""

from __future__ import annotations

import argparse
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "T03.csv"

HEADER = "id,payload"
NORMAL_ROWS: tuple[tuple[str, str], ...] = (
    ("1", "normal-line-one"),
    ("2", "normal-line-two"),
    ("3", "normal-line-three"),
)

# 构造参数默认值（非产品阈值）：放大"单行超分片"场景，使 CI 可在秒级构造与校验。
DEFAULT_SHARD_BYTES = 1 << 20  # 1 MiB

# 超出分片值的余量（构造参数）：保证"严格大于"，留出表头与换行的开销
MARGIN_BYTES = 4096


def build(shard_bytes: int) -> bytes:
    long_line = "9," + "A" * (shard_bytes + MARGIN_BYTES)
    lines = [HEADER, *(f"{i},{payload}" for i, payload in NORMAL_ROWS), long_line]
    return ("\r\n".join(lines) + "\r\n").encode("ascii")


def verify(data: bytes, shard_bytes: int) -> None:
    lines = data.decode("ascii").split("\r\n")
    lengths = [len(ln.encode("ascii")) for ln in lines if ln]

    longest = max(lengths)
    if longest <= shard_bytes:
        raise SystemExit(
            f"T03 语料失效：最长行 {longest} 字节未超过构造分片值 {shard_bytes} 字节"
        )

    others = sorted(lengths)[:-1]
    if others and max(others) >= shard_bytes:
        raise SystemExit("T03 语料失效：存在多行超过分片值，场景不再聚焦于'单行超长'")


def main() -> None:
    parser = argparse.ArgumentParser(description="生成 T03 单行超分片样本")
    parser.add_argument(
        "--shard-bytes",
        type=int,
        default=DEFAULT_SHARD_BYTES,
        help="构造用目标分片字节数（非产品阈值；产品口径见方案 §5.1／§9.1）",
    )
    args = parser.parse_args()
    if args.shard_bytes <= 0:
        raise SystemExit("T03 生成失败：--shard-bytes 必须为正整数")

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build(args.shard_bytes)
    verify(data, args.shard_bytes)

    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"T03 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(
        f"T03 已生成：{OUT_FILE}（{len(data)} 字节；单行 > 构造分片值 {args.shard_bytes} 字节）"
    )


if __name__ == "__main__":
    main()
