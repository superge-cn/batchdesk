#!/usr/bin/env python3
"""K08 语料生成脚本：UTF-16 带 BOM 的多行 CSV（供分片后"每片均写 BOM"校验）。

判定权威：T v1.12 §3.2 的 K08 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：每片均写 BOM 规则校验（方案 §5.2、§5.3）。

幂等：固定内容、固定 CRLF；内容相同时不重写。
自校验：必须以 UTF-16 BOM 开头；按 UTF-16 解码后首行为表头；至少 3 行；无 U+FFFD。
"""

from __future__ import annotations

from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "K08.csv"

HEADER = "姓名,部门,金额,备注"

ROWS: tuple[tuple[str, str, str, str], ...] = (
    ("张伟", "行政部", "1200.00", "差旅报销"),
    ("李娜", "财务部", "3800.50", "设备采购"),
    ("王强", "技术部", "760.00", "耗材"),
    ("赵敏", "人事部", "2450.75", "培训费"),
    ("陈静", "市场部", "9800.00", "会务"),
)


def build() -> bytes:
    lines = [HEADER, *(",".join(row) for row in ROWS)]
    # utf-16（非 LE）：Python 会写入 BOM（FF FE 或 FE FF），正好是 K08 要求的带 BOM 形态
    return ("\r\n".join(lines) + "\r\n").encode("utf-16")


def verify(data: bytes) -> None:
    if not data.startswith((b"\xff\xfe", b"\xfe\xff")):
        raise SystemExit("K08 语料失效：缺少 UTF-16 BOM")

    text = data.decode("utf-16")
    if not text.startswith(HEADER):
        raise SystemExit("K08 语料失效：UTF-16 解码后首行与预期表头不一致")
    if "\ufffd" in text:
        raise SystemExit("K08 语料失效：解码结果含替换字符 U+FFFD")

    line_count = len([ln for ln in text.split("\r\n") if ln])
    if line_count < 3:
        raise SystemExit("K08 语料失效：有效行数不足 3 行，无法覆盖多片场景")

    if data.count(b"\x00") < 2:
        raise SystemExit("K08 语料失效：未检测到 UTF-16 的 NUL 字节特征")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build()
    verify(data)

    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"K08 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(f"K08 已生成：{OUT_FILE}（{len(data)} 字节，UTF-16 带 BOM，{len(ROWS)} 条数据行）")


if __name__ == "__main__":
    main()
