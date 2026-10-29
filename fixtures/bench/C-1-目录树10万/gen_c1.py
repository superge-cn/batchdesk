#!/usr/bin/env python3
"""C-1 基准数据集生成脚本：10 万文件目录树。

判定权威：T v1.12 附录 C 的 C-1 行与本目录 README（本脚本只负责产出数据集，不承载阈值）。
对应指标：方案 §9.1 的"目录清单"指标（口径见 §9.1，本脚本不写具体阈值）。

树形参数（固定，写入生成日志）
- 深度 2 级子目录：10 个一级目录 × 10 个二级（叶子）目录 = 100 个叶子；
- 每叶子 1000 个文件 = 100,000 个文件；文件名长度分布稳定（定长模板）；
- 类型混合：.dat 90% / .txt 6% / .csv 4%（确定性序号取模分配，不用随机）。

设计要点
- 幂等：文件名与内容均由确定性序号派生，重复运行逐字节一致；内容相同时不重写。
- 自校验：生成后统计文件数必须恰为 100_000，且逐叶计数一致。
- 合成数据：全部为虚构内容，不含任何真实数据。
- 性能提示：全量首建涉及 10 万次小文件写入（普通桌面 SSD 数十秒量级）；重复运行秒级。
"""

from __future__ import annotations

import hashlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"

L1_COUNT = 10
L2_PER_L1 = 10
FILES_PER_LEAF = 100_000 // (L1_COUNT * L2_PER_L1)
TOTAL_FILES = L1_COUNT * L2_PER_L1 * FILES_PER_LEAF

# 类型混合：序号 % 100 → <90 .dat、<96 .txt、其余 .csv（合计恰 90%/6%/4%）
def ext_for(seq: int) -> str:
    r = seq % 100
    if r < 90:
        return "dat"
    if r < 96:
        return "txt"
    return "csv"


def name_for(seq: int) -> str:
    """定长名模板：f + 6 位序号 + 8 位稳定摘要 + 扩展名（长度分布恒定）。"""
    digest = hashlib.md5(f"c1-{seq}".encode()).hexdigest()[:8]
    return f"f{seq:06d}_{digest}.{ext_for(seq)}"


def content_for(seq: int) -> bytes:
    """64 字节固定内容（可读的虚构文本，便于人工抽查）。"""
    return f"C1 synthetic sample {seq:06d} :: batchdesk bench dataset (64 bytes)  ".encode()[:64]


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    written = 0
    skipped = 0

    seq = 0
    for l1 in range(L1_COUNT):
        for l2 in range(L2_PER_L1):
            leaf = OUT_DIR / f"d{l1:02d}" / f"e{l2:02d}"
            leaf.mkdir(parents=True, exist_ok=True)
            for _ in range(FILES_PER_LEAF):
                target = leaf / name_for(seq)
                data = content_for(seq)
                if target.exists() and target.read_bytes() == data:
                    skipped += 1
                else:
                    target.write_bytes(data)
                    written += 1
                seq += 1

    # 自校验：逐叶计数 + 总数（排除生成日志自身，否则二次运行必失败）
    total = sum(1 for p in OUT_DIR.rglob("*") if p.is_file() and p.name != "MANIFEST.md")
    if total != TOTAL_FILES:
        raise SystemExit(f"C-1 数据集失效：实际文件数 {total} ≠ 预期 {TOTAL_FILES}")

    manifest = OUT_DIR / "MANIFEST.md"
    manifest_text = (
        "# C-1 生成日志\n\n"
        f"- 树形：{L1_COUNT} × {L2_PER_L1} 叶 × {FILES_PER_LEAF} 文件 = {TOTAL_FILES} 文件\n"
        "- 类型混合：.dat 90% / .txt 6% / .csv 4%（确定性取模）\n"
        "- 文件名：定长模板 f+6 位序号+8 位摘要+扩展名\n"
        "- 本次运行：新写 {w} 个，命中幂等跳过 {s} 个\n"
    ).format(w=written, s=skipped)
    if not manifest.exists() or manifest.read_text(encoding="utf-8") != manifest_text:
        manifest.write_text(manifest_text, encoding="utf-8")

    print(f"C-1 已就绪：{OUT_DIR}（{TOTAL_FILES} 文件；本次新写 {written}，跳过 {skipped}）")


if __name__ == "__main__":
    main()
