#!/usr/bin/env python3
"""T08 语料生成脚本：重复列名 + 列名首尾带空格的 CSV。

判定权威：T v1.12 §3.2 的 T08 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：列名规范化并显式告警（方案 §5.4）。

幂等：固定内容；内容相同时不重写。
自校验：表头必须同时满足"存在重复列名"与"存在首尾空格列名"两个特征。
"""

from __future__ import annotations

from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "T08.csv"

# 表头：姓名重复；" 部门 " 与 "金额 " 带空格
HEADER_RAW = ("姓名", "姓名", " 部门 ", "金额 ")

ROWS: tuple[tuple[str, str, str, str], ...] = (
    ("张伟", "张伟（别名）", "行政部", "1200.00"),
    ("李娜", "李娜（别名）", "财务部", "3800.50"),
    ("王强", "王强（别名）", "技术部", "760.00"),
)


def build() -> bytes:
    lines = [",".join(HEADER_RAW), *(",".join(row) for row in ROWS)]
    return ("\r\n".join(lines) + "\r\n").encode("utf-8")


def verify(data: bytes) -> None:
    lines = [ln for ln in data.decode("utf-8").split("\r\n") if ln]
    header = lines[0].split(",")

    if len(header) != len(set(header)):
        # 有重复即通过该特征；这里用反向断言避免误判
        pass
    else:
        raise SystemExit("T08 语料失效：表头无重复列名")

    stripped_violations = [cell for cell in header if cell != cell.strip()]
    if not stripped_violations:
        raise SystemExit("T08 语料失效：表头无首尾空格列名")

    # 规范化后是否仍重复（若规范化即可去重，则不属于'重复列名'场景）
    normalized = [cell.strip() for cell in header]
    if len(normalized) == len(set(normalized)):
        raise SystemExit("T08 语料失效：去除首尾空格后列名不再重复，两个特征互相抵消")

    body_widths = {len(ln.split(",")) for ln in lines[1:]}
    if body_widths != {len(header)}:
        raise SystemExit("T08 语料失效：数据行列数与表头不一致")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build()
    verify(data)

    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"T08 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(f"T08 已生成：{OUT_FILE}（{len(data)} 字节；重复列名 1 处、带空格列名 2 处）")


if __name__ == "__main__":
    main()
