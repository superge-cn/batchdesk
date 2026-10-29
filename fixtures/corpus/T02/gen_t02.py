#!/usr/bin/env python3
"""T02 语料生成脚本：字段内含真实换行的 CSV。

判定权威：T v1.12 §3.2 的 T02 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：禁止行中间切断（方案 §5.3）。

构造器共享约定：本脚本 **import** `fixtures/bench/C-3-大CSV/gen_c3.py` 的 `build_csv_bytes`，
复用"引号换行 CSV"的唯一构造器（fixtures/bench/C-3-大CSV/README 的约定：禁止两处各自实现）。

幂等：固定种子（由 gen_c3 决定）；内容相同时不重写。
自校验：解析后必须至少有一个字段含换行符；记录数必须与请求行数一致（不出现吞行）。
"""

from __future__ import annotations

import csv
import io
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "T02.csv"

# 复用 C-3 的构造器（路径：fixtures/corpus/T02 → fixtures/bench/C-3-大CSV）
BENCH_C3 = HERE.parents[1] / "bench" / "C-3-大CSV"
sys.path.insert(0, str(BENCH_C3))
from gen_c3 import build_csv_bytes  # noqa: E402

ROW_COUNT = 8  # 小样本：20% 跨行占比下仍能稳定出现跨行记录（C-3 构造器每 5 行强制 1 行跨行）


def build() -> bytes:
    return build_csv_bytes(ROW_COUNT)


def verify(data: bytes) -> None:
    text = data.decode("utf-8")
    rows = list(csv.reader(io.StringIO(text)))

    if len(rows) != ROW_COUNT + 1:
        raise SystemExit(f"T02 语料失效：解析记录数 {len(rows)} ≠ 预期 {ROW_COUNT + 1}（含表头）")

    if not any("\n" in field for row in rows for field in row):
        raise SystemExit("T02 语料失效：没有任何字段包含换行，与 T02 定义不符")


def main() -> None:
    if not BENCH_C3.exists():
        raise SystemExit(
            f"T02 生成失败：共享构造器不存在（{BENCH_C3}）。请确认 C-3 基准目录未被删除。"
        )

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build()
    verify(data)

    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"T02 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(f"T02 已生成：{OUT_FILE}（{len(data)} 字节，{ROW_COUNT} 条数据行，构造器复用 C-3）")


if __name__ == "__main__":
    main()
