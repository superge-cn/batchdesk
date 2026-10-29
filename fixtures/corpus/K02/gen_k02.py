#!/usr/bin/env python3
"""K02 语料生成脚本：UTF-8 无 BOM 中文 CSV。

判定权威：T v1.12 §3.2 的 K02 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：判 UTF-8，不误判 GBK（方案 §5.2 检测流水线）。

幂等：固定内容、固定 CRLF、不写 BOM；内容相同时不重写。
自校验：必须是合法 UTF-8、必须含非 ASCII（否则与 K05 的 ASCII 样本失去区分力）、不得带 BOM。
"""

from __future__ import annotations

from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "K02.csv"

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
    return ("\r\n".join(lines) + "\r\n").encode("utf-8")


def verify(data: bytes) -> None:
    # 1) 必须是合法 UTF-8（解码失败即说明样本坏了）
    text = data.decode("utf-8")
    if not text.startswith(HEADER):
        raise SystemExit("K02 语料失效：UTF-8 解码后首行与预期表头不一致")

    # 2) 必须含非 ASCII，否则无法与 K05（纯 ASCII）形成对照
    if data.isascii():
        raise SystemExit("K02 语料失效：内容为纯 ASCII，与 K05 样本失去区分力")

    # 3) 不得带 BOM（BOM 属 K03 的特征）
    if data.startswith(b"\xef\xbb\xbf"):
        raise SystemExit("K02 语料失效：检测到 UTF-8 BOM，与 K02 定义（无 BOM）不符")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build()
    verify(data)

    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"K02 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(f"K02 已生成：{OUT_FILE}（{len(data)} 字节，{len(ROWS)} 条数据行）")


if __name__ == "__main__":
    main()
