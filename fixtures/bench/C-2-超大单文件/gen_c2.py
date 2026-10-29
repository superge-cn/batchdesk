#!/usr/bin/env python3
"""C-2 基准数据集生成脚本：超大单文件（> 2 GB 档）。

判定权威：T v1.12 附录 C 的 C-2 行与本目录 README（本脚本只负责产出数据集，不承载阈值）。
对应指标：方案 §9.1 的"大文件吞吐"与"内存占用恒定"；对应策略分档见 §4.8（> 2 GB 走"流式＋预扫描"）。

参数（固定，写入生成日志——README 要求）
- 主文件体积：默认 2 GiB ＋ 64 MiB（跨过 §4.8 的 "> 2 GB" 档界，构造参数，非产品阈值；--size-mib 可调）；
- 行长：常规行约 60 字节 + 每 1 GiB 插入 1 条极端长行（默认 1 MiB，--long-line-bytes 可调），
  用于覆盖 §5.1"有效下限由最长单行动态决定"的边界；长行长度是构造参数，不表示产品下限；
- 随机源：random.Random(2026) 固定种子 → 重复运行逐字节一致（幂等）。

设计要点
- 分块写入（4 MiB/块，与 §4.8 的示例块大小同量级，示例非阈值），避免一次性占内存——本身也是"内存恒定"的对照；
- 自校验：字节数精确、sha256 记录、长行确实存在且长度正确；
- 体积大，**不入库**：产物落 `out/`，CI 不默认生成，由性能测试按需调用；
- 合成数据：全部为虚构内容。
"""

from __future__ import annotations

import argparse
import hashlib
import random
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "big_file.txt"

SEED = 2026
CHUNK = 4 * 1024 * 1024  # 4 MiB：与 §4.8 示例块大小同量级（示例，非阈值）
MIB = 1024 * 1024
GIB = 1024 * MIB

DEFAULT_SIZE = 2 * GIB + 64 * MIB  # 跨过 "> 2 GB" 档界
DEFAULT_LONG_LINE = 1 * MIB


def build_stream(total_bytes: int, long_line_bytes: int, rng: random.Random):
    """按块产出字节：常规行 + 周期性极端长行。产出总字节数恰为 total_bytes。"""
    written = 0
    next_long_at = GIB  # 每 1 GiB 插一条长行
    buf = bytearray()
    while written < total_bytes:
        remain = total_bytes - written
        if written >= next_long_at:
            line = ("L" * long_line_bytes + "\n").encode("ascii")
            next_long_at += GIB
        else:
            line = (f"{rng.randrange(10**9):09d},{rng.randrange(10**6):06d},normal-row\n").encode("ascii")
        take = line[:remain]  # 尾块截断，保证总字节精确
        buf.extend(take)
        written += len(take)
        if len(buf) >= CHUNK:
            yield bytes(buf[:CHUNK])
            del buf[:CHUNK]
    if buf:
        yield bytes(buf)


def generate(total_bytes: int, long_line_bytes: int) -> str:
    rng = random.Random(SEED)
    digest = hashlib.sha256()
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    with OUT_FILE.open("wb") as fh:
        for chunk in build_stream(total_bytes, long_line_bytes, rng):
            digest.update(chunk)
            fh.write(chunk)
    return digest.hexdigest()


def verify(total_bytes: int, expected_sha: str, long_line_bytes: int) -> None:
    actual = OUT_FILE.stat().st_size
    if actual != total_bytes:
        raise SystemExit(f"C-2 数据集失效：字节数 {actual} ≠ 预期 {total_bytes}")
    digest = hashlib.sha256()
    with OUT_FILE.open("rb") as fh:
        for chunk in iter(lambda: fh.read(CHUNK), b""):
            digest.update(chunk)
    if digest.hexdigest() != expected_sha:
        raise SystemExit("C-2 数据集失效：写入后校验和不一致")
    # 极端长行必须存在：抽查首个 1 GiB 边界之后是否存在 len>=long_line_bytes 的行
    # （体积不足 1 GiB 时不会插入长行——小体积仅供脚本自测）
    if total_bytes <= GIB:
        return
    with OUT_FILE.open("rb") as fh:
        fh.seek(GIB)
        window = fh.read(long_line_bytes + 1024)
    if b"L" * long_line_bytes not in window:
        raise SystemExit("C-2 数据集失效：未在 1 GiB 边界后检测到极端长行")


def main() -> None:
    parser = argparse.ArgumentParser(description="生成 C-2 超大单文件基准数据集（体积大，按需生成）")
    parser.add_argument("--size-mib", type=int, default=DEFAULT_SIZE // MIB,
                        help=f"主文件体积（MiB，默认 {DEFAULT_SIZE // MIB}，即 2 GiB+64 MiB）")
    parser.add_argument("--long-line-bytes", type=int, default=DEFAULT_LONG_LINE,
                        help=f"极端长行长度（字节，默认 {DEFAULT_LONG_LINE}；构造参数，非产品阈值）")
    args = parser.parse_args()

    total_bytes = args.size_mib * MIB
    if total_bytes <= 0 or args.long_line_bytes <= 0:
        raise SystemExit("参数必须为正数")

    if OUT_FILE.exists() and OUT_FILE.stat().st_size == total_bytes:
        print(f"C-2 已存在同体积产物，跳过生成：{OUT_FILE}（如需重建请先删除）")
        return

    sha = generate(total_bytes, args.long_line_bytes)
    verify(total_bytes, sha, args.long_line_bytes)

    log = OUT_DIR / "gen.log"
    log.write_text(
        f"size_bytes={total_bytes} long_line_bytes={args.long_line_bytes} "
        f"chunk={CHUNK} seed={SEED} sha256={sha}\n",
        encoding="utf-8",
    )
    print(f"C-2 已生成：{OUT_FILE}（{total_bytes} 字节；日志 {log}）")


if __name__ == "__main__":
    main()
