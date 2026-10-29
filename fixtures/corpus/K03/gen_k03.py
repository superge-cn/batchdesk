#!/usr/bin/env python3
"""K03 语料生成脚本：UTF-8 带 BOM 中文 CSV。

判定权威：T v1.12 §3.2 的 K03 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：BOM 优先命中，导出策略符合方案 §5.2。

幂等：固定内容、固定 CRLF、固定 BOM；内容相同时不重写。
自校验：必须以 UTF-8 BOM 开头；剥 BOM 后必须为合法 UTF-8 且首行为表头。
"""

from __future__ import annotations

from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "K03.csv"

UTF8_BOM = b"\xef\xbb\xbf"
HEADER = "姓名,部门,金额,备注"

ROWS: tuple[tuple[str, str, str, str], ...] = (
    ("张伟", "行政部", "1200.00", "差旅报销"),
    ("李娜", "财务部", "3800.50", "设备采购"),
    ("王强", "技术部", "760.00", "耗材"),
    ("赵敏", "人事部", "2450.75", "培训费"),
    ("陈静", "市场部", "9800.00", "会务"),
    ("刘洋", "行政部", "135.20", "快递费"),
    ("孙磊", "技术部", "5620.00", "软件授权"),
    ("周芳", "财务部", "410.00", "办公用品"),
)


def build() -> bytes:
    lines = [HEADER, *(",".join(row) for row in ROWS)]
    return UTF8_BOM + ("\r\n".join(lines) + "\r\n").encode("utf-8")


def verify(data: bytes) -> None:
    if not data.startswith(UTF8_BOM):
        raise SystemExit("K03 语料失效：缺少 UTF-8 BOM，与 K03 定义不符")

    text = data[len(UTF8_BOM):].decode("utf-8")
    if not text.startswith(HEADER):
        raise SystemExit("K03 语料失效：剥 BOM 后首行与预期表头不一致")
    if data[len(UTF8_BOM):].isascii():
        raise SystemExit("K03 语料失效：剥 BOM 后为纯 ASCII，与 K02/K05 的对照关系不成立")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build()
    verify(data)

    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"K03 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(f"K03 已生成：{OUT_FILE}（{len(data)} 字节，含 3 字节 BOM）")


if __name__ == "__main__":
    main()
