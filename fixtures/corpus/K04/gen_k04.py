#!/usr/bin/env python3
"""K04 语料生成脚本：UTF-16LE 无 BOM 的中文 CSV。

判定权威：T v1.12 §3.2 的 K04 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：走统计判定并提示低置信（方案 §5.2）。

幂等：固定内容与编码；内容相同时不重写。
自校验：不得带 BOM；必须可按 UTF-16LE 解码且首行为表头；必须含 NUL 字节（UTF-16 特征）。
"""

from __future__ import annotations

from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "K04.csv"

HEADER = "姓名,部门,金额,备注"

ROWS: tuple[tuple[str, str, str, str], ...] = (
    ("张伟", "行政部", "1200.00", "差旅报销"),
    ("李娜", "财务部", "3800.50", "设备采购"),
    ("王强", "技术部", "760.00", "耗材"),
    ("赵敏", "人事部", "2450.75", "培训费"),
)


def build() -> bytes:
    lines = [HEADER, *(",".join(row) for row in ROWS)]
    # utf-16-le：不带 BOM（带 BOM 的形态由 K08 覆盖）
    return ("\r\n".join(lines) + "\r\n").encode("utf-16-le")


def verify(data: bytes) -> None:
    if data.startswith((b"\xff\xfe", b"\xfe\xff")):
        raise SystemExit("K04 语料失效：检测到 BOM，与 K04 定义（无 BOM）不符")

    text = data.decode("utf-16-le")
    if not text.startswith(HEADER):
        raise SystemExit("K04 语料失效：UTF-16LE 解码后首行与预期表头不一致")

    if b"\x00" not in data:
        raise SystemExit("K04 语料失效：无 NUL 字节，不具备 UTF-16 的字节特征")

    if len(data) % 2 != 0:
        raise SystemExit("K04 语料失效：字节长度为奇数，不可能是完整 UTF-16 码元序列")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build()
    verify(data)

    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"K04 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(f"K04 已生成：{OUT_FILE}（{len(data)} 字节，{len(ROWS)} 条数据行）")


if __name__ == "__main__":
    main()
