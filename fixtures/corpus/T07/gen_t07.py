#!/usr/bin/env python3
"""T07 语料生成脚本：无表头 CSV（首行即数据）。

判定权威：T v1.12 §3.2 的 T07 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：无表头时按列序号对齐并提示（方案 §5.4；T v1.12 §3.2 T07 行）。

幂等：固定内容；内容相同时不重写。
自校验：首行必须可被当作数据解析（全部列均为数值或纯值），且与后续行同构；
        同时不含任何常见表头词（姓名/名称/部门/金额/日期 等）。
"""

from __future__ import annotations

from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "T07.csv"

# 首行即数据（无表头）
ROWS: tuple[tuple[str, str, str], ...] = (
    ("1001", "张伟", "1200.00"),
    ("1002", "李娜", "3800.50"),
    ("1003", "王强", "760.00"),
    ("1004", "赵敏", "2450.75"),
)

HEADER_HINTS = ("姓名", "名称", "部门", "金额", "日期", "备注", "编号", "id", "name")


def build() -> bytes:
    return ("\r\n".join(",".join(row) for row in ROWS) + "\r\n").encode("utf-8")


def verify(data: bytes) -> None:
    lines = [ln for ln in data.decode("utf-8").split("\r\n") if ln]
    if len(lines) != len(ROWS):
        raise SystemExit(f"T07 语料失效：行数 {len(lines)} ≠ 预期 {len(ROWS)}")

    first_cells = [c.strip().lower() for c in lines[0].split(",")]
    if any(cell in {hint.lower() for hint in HEADER_HINTS} for cell in first_cells):
        raise SystemExit(f"T07 语料失效：首行命中表头词（{first_cells}），不再是'无表头'样本")

    widths = {len(ln.split(",")) for ln in lines}
    if len(widths) != 1:
        raise SystemExit("T07 语料失效：各行列数不一致，会与 T04（列数漂移）场景重叠")

    if not first_cells[0].isdigit():
        raise SystemExit("T07 语料失效：首列不是数字，'首行是数据'的证据不足")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build()
    verify(data)

    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"T07 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(f"T07 已生成：{OUT_FILE}（{len(data)} 字节，无表头，{len(ROWS)} 行数据）")


if __name__ == "__main__":
    main()
