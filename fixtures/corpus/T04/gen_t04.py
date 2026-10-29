#!/usr/bin/env python3
"""T04 语料生成脚本：列数漂移的 CSV（表头 3 列，数据行在 2/4 列间跳动）。

判定权威：T v1.12 §3.2 的 T04 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：列数不一致计数上报，不静默修正/补列（方案 §5.3；T v1.12 §3.2 T04 行）。

幂等：固定内容；内容相同时不重写。
自校验：解析后必须同时存在少于表头列数与多于表头列数的数据行。
"""

from __future__ import annotations

from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "T04.csv"

HEADER = ("姓名", "部门", "金额")
ROWS: tuple[tuple[str, ...], ...] = (
    ("张伟", "行政部"),                                  # 2 列（少）
    ("李娜", "财务部", "3800.50"),                        # 3 列（正常）
    ("王强", "技术部", "760.00", "耗材"),                  # 4 列（多）
    ("赵敏",),                                           # 1 列（严重偏少）
    ("陈静", "市场部", "9800.00"),                        # 3 列（正常）
)


def build() -> bytes:
    lines = [",".join(HEADER), *(",".join(row) for row in ROWS)]
    return ("\r\n".join(lines) + "\r\n").encode("utf-8")


def verify(data: bytes) -> None:
    lines = [ln for ln in data.decode("utf-8").split("\r\n") if ln]
    expected = len(HEADER)

    widths = [len(ln.split(",")) for ln in lines[1:]]
    if not any(w < expected for w in widths):
        raise SystemExit("T04 语料失效：不存在少于表头列数的数据行")
    if not any(w > expected for w in widths):
        raise SystemExit("T04 语料失效：不存在多于表头列数的数据行")

    if len(lines[0].split(",")) != expected:
        raise SystemExit("T04 语料失效：表头列数与预期不一致")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build()
    verify(data)

    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"T04 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(f"T04 已生成：{OUT_FILE}（{len(data)} 字节；列数在 1–4 间漂移，表头 3 列）")


if __name__ == "__main__":
    main()
