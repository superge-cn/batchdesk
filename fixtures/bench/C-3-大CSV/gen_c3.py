#!/usr/bin/env python3
"""C-3 基准数据集生成脚本：大 CSV（含引号换行）。

判定权威：T v1.12 附录 C 的 C-3 行与本目录 README（本脚本只负责产出数据集，不承载阈值）。
对应指标：方案 §5.3 的分片正确性（禁止行中间切断、每片保留表头）+ §9.1 的吞吐。

共享构造逻辑约定（README「注意不要重复实现」）
- 本模块的 `build_field` / `build_rows` / `build_csv_bytes` 是"引号换行 CSV"的唯一构造器；
- corpus/T02 语料脚本落地时 **import 本模块** 复用同一构造器（小样本、固定内容），
  本脚本只负责把同一构造器放大到基准量级，禁止两处各自实现。

参数（固定，写入生成日志——README 要求）
- 列数：6；引号占比：10%；跨行记录占比：20%；行数：100_000（--rows 可调，仅用于放大）；
- 随机源：random.Random(2026) 固定种子 → 重复运行逐字节一致（幂等）。

设计要点
- 自校验：用 csv.reader 读回解析，行数一致、跨行记录确有存在、无解析异常；
- 合成数据：全部为虚构内容，不含任何真实数据。
"""

from __future__ import annotations

import argparse
import csv
import io
import random
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "big.csv"

SEED = 2026
HEADER = ("单号", "姓名", "部门", "金额", "日期", "备注")

FIELD_TEMPLATES: tuple[str, ...] = (
    "正常字段",
    '带,逗号的字段',
    '带"引号"的字段',
    "第一行\n第二行",
    '混合"引号"与\n换行',
    "长字段" + "填充" * 20,
)


def build_field(rng: random.Random, row_idx: int, col_idx: int) -> str:
    """按目标占比混合字段形态：10% 模板含引号/换行，20% 行至少含一个跨行字段。"""
    is_quoted_or_multiline = rng.random() < 0.10
    force_multiline = col_idx == 0 and row_idx % 5 == 0  # 每 5 行强制 1 行跨行 → 20%
    if is_quoted_or_multiline or force_multiline:
        # 只取含引号或换行的模板（索引 2..5）
        return FIELD_TEMPLATES[rng.randint(2, 5)]
    return FIELD_TEMPLATES[rng.randint(0, 1)]


def build_rows(row_count: int) -> list[tuple[str, ...]]:
    rng = random.Random(SEED)
    return [tuple(build_field(rng, i, c) for c in range(6)) for i in range(row_count)]


def build_csv_bytes(row_count: int) -> bytes:
    buf = io.StringIO()
    writer = csv.writer(buf, lineterminator="\r\n")  # RFC 4180：CRLF、必要字段加引号
    writer.writerow(HEADER)
    for row in build_rows(row_count):
        writer.writerow(row)
    return buf.getvalue().encode("utf-8")


def verify(data: bytes, row_count: int) -> None:
    text = data.decode("utf-8")
    rows = list(csv.reader(io.StringIO(text)))
    if len(rows) != row_count + 1:  # 含表头
        raise SystemExit(f"C-3 数据集失效：解析行数 {len(rows)} ≠ 预期 {row_count + 1}")
    # 跨行记录必须真实存在（字段内含 \n）
    if "\n" not in text.split("\r\n", 1)[1][:5000]:
        raise SystemExit("C-3 数据集失效：未检测到跨行记录，语料失去区分力")


def main() -> None:
    parser = argparse.ArgumentParser(description="生成 C-3 大 CSV 基准数据集")
    parser.add_argument("--rows", type=int, default=100_000, help="数据行数（默认 100000）")
    args = parser.parse_args()

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build_csv_bytes(args.rows)
    verify(data, args.rows)

    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"C-3 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)

    log = OUT_DIR / "gen.log"
    entry = (
        f"rows={args.rows} cols=6 quoted_or_multiline=10% cross_line_rows=20% seed={SEED}\n"
    )
    log.write_text(entry, encoding="utf-8")
    print(f"C-3 已生成：{OUT_FILE}（{args.rows} 行，{len(data)} 字节；日志 {log}）")


if __name__ == "__main__":
    main()
