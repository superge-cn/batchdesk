#!/usr/bin/env python3
"""C-6 基准数据集生成脚本：重复文件集（三类）。

判定权威：T v1.12 附录 C 的 C-6 行与本目录 README（本脚本只负责产出数据集，不承载阈值）。
对应指标：方案 §5.10 的三段式哈希（大小分组 → 快速哈希 → 全量哈希）；
        C09 的正确性，特别是第③类硬链接**不得**被判为重复（§5.10 边界与异常）。

三类固定构造（数量与体积为构造参数，非产品阈值）
- ① 同大小不同内容：20 对，每份 1 MiB（大小相同、内容全异）→ 必须走到全量哈希才分开；
- ② 同内容不同名：同一份 2 MiB 内容复制 10 份（不同文件名）→ 必须被识别为重复；
- ③ 硬链接：1 份 1 MiB 源文件 + 3 个硬链接入口（同 inode）→ **不得**判为重复；
- 随机源：random.Random(2026) 固定种子 → 逐字节可复现（幂等）。

设计要点
- 自校验：① 逐对大小相等且哈希不同；② 全部哈希相等；③ 各入口 inode 相同（文件系统不支持硬链接时明确跳过并记录）；
- 合成数据：全部为虚构内容；产物落 `out/`，不入库。
"""

from __future__ import annotations

import argparse
import hashlib
import os
import random
import shutil
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"

SEED = 2026
MIB = 1024 * 1024

PAIRS = 20
PAIR_SIZE = 1 * MIB
SHARED_SIZE = 2 * MIB
SHARED_COPIES = 10
LINK_SIZE = 1 * MIB
LINK_ALIASES = 3


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(MIB), b""):
            digest.update(chunk)
    return digest.hexdigest()


def wipe_out_dir() -> None:
    """只清理本脚本自己的 out/ 目录（幂等重建的前提）。"""
    if OUT_DIR.exists():
        shutil.rmtree(OUT_DIR)
    OUT_DIR.mkdir(parents=True)


def build_same_size_different_content(rng: random.Random) -> None:
    d = OUT_DIR / "same_size"
    d.mkdir()
    for i in range(PAIRS):
        for variant in ("a", "b"):
            (d / f"pair{i:02d}_{variant}.bin").write_bytes(rng.randbytes(PAIR_SIZE))


def build_same_content_different_name(rng: random.Random) -> None:
    d = OUT_DIR / "same_content"
    d.mkdir()
    payload = rng.randbytes(SHARED_SIZE)
    for i in range(SHARED_COPIES):
        (d / f"copy_{i:02d}.dat").write_bytes(payload)


def build_hardlinks(rng: random.Random) -> list[str]:
    d = OUT_DIR / "hardlinks"
    d.mkdir()
    src = d / "source.bin"
    src.write_bytes(rng.randbytes(LINK_SIZE))
    made: list[str] = []
    for i in range(LINK_ALIASES):
        alias = d / f"alias_{i:02d}.bin"
        try:
            os.link(src, alias)
        except OSError as exc:  # exFAT/FAT 等无硬链接语义的文件系统
            return [f"hardlink_unsupported: {exc}"]
        made.append(alias.name)
    return made


def verify(link_aliases: list[str]) -> None:
    # ① 同大小不同内容
    d1 = OUT_DIR / "same_size"
    for i in range(PAIRS):
        a, b = d1 / f"pair{i:02d}_a.bin", d1 / f"pair{i:02d}_b.bin"
        if a.stat().st_size != b.stat().st_size:
            raise SystemExit(f"C-6 数据集失效：pair{i:02d} 两份大小不等，失去'同大小'区分力")
        if sha256_of(a) == sha256_of(b):
            raise SystemExit(f"C-6 数据集失效：pair{i:02d} 两份内容相同，与'不同内容'矛盾")
    # ② 同内容不同名
    d2 = OUT_DIR / "same_content"
    hashes = {sha256_of(p) for p in sorted(d2.iterdir())}
    if len(hashes) != 1:
        raise SystemExit("C-6 数据集失效：same_content 下的内容哈希不唯一")
    # ③ 硬链接
    if link_aliases and not link_aliases[0].startswith("hardlink_unsupported"):
        d3 = OUT_DIR / "hardlinks"
        inodes = {os.stat(d3 / name).st_ino for name in ["source.bin", *link_aliases]}
        if len(inodes) != 1:
            raise SystemExit("C-6 数据集失效：硬链接入口未共享同一 inode")


def main() -> None:
    global PAIRS
    parser = argparse.ArgumentParser(description="生成 C-6 重复文件集基准数据集")
    parser.add_argument("--pairs", type=int, default=PAIRS, help=f"同大小对数量（默认 {PAIRS}）")
    args = parser.parse_args()

    PAIRS = args.pairs

    wipe_out_dir()
    rng = random.Random(SEED)
    build_same_size_different_content(rng)
    build_same_content_different_name(rng)
    link_aliases = build_hardlinks(rng)
    verify(link_aliases)

    log = OUT_DIR / "gen.log"
    log.write_text(
        f"same_size_pairs={PAIRS} pair_size={PAIR_SIZE} shared_size={SHARED_SIZE} "
        f"shared_copies={SHARED_COPIES} link_size={LINK_SIZE} link_aliases={LINK_ALIASES} "
        f"seed={SEED} hardlinks={'ok' if link_aliases and not link_aliases[0].startswith('hardlink') else (link_aliases[0] if link_aliases else 'none')}\n",
        encoding="utf-8",
    )
    print(f"C-6 已生成：{OUT_DIR}（日志 {log}）")


if __name__ == "__main__":
    main()
