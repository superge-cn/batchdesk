#!/usr/bin/env python3
"""T01 语料生成脚本：CSV 末尾引号不闭合（非法 CSV）。

判定权威：T v1.12 §3.2 的 T01 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：视为末条记录并告警（方案 §5.3；T v1.12 §3.2 T01 行）。

幂等：固定内容；内容相同时不重写。
自校验：引号计数必须为奇数（不闭合的必要条件）；csv 解析要么抛错要么吞掉后续数据。
"""

from __future__ import annotations

import csv
import io
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "T01.csv"

TEXT = (
    "姓名,部门,金额,备注\r\n"
    "张伟,行政部,1200.00,差旅报销\r\n"
    "李娜,财务部,3800.50,设备采购\r\n"
    "王强,技术部,\"760.00,耗材\r\n"
    "赵敏,人事部,2450.75,培训费\r\n"
)


def build() -> bytes:
    return TEXT.encode("utf-8")


def verify(data: bytes) -> None:
    text = data.decode("utf-8")

    if text.count('"') % 2 == 0:
        raise SystemExit("T01 语料失效：引号计数为偶数，不构成不闭合样本")

    # 不闭合会吞掉后续行：解析结果的记录数应显著少于原始行数
    try:
        rows = list(csv.reader(io.StringIO(text)))
    except csv.Error:
        return

    physical_lines = len([ln for ln in text.split("\r\n") if ln])
    if len(rows) >= physical_lines:
        raise SystemExit(
            f"T01 语料失效：解析得到 {len(rows)} 行，未出现吞行（物理行 {physical_lines}），"
            "不闭合特征不成立"
        )


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build()
    verify(data)

    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"T01 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(f"T01 已生成：{OUT_FILE}（{len(data)} 字节，引号不闭合）")


if __name__ == "__main__":
    main()
