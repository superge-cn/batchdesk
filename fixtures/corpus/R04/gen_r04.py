#!/usr/bin/env python3
"""R04 语料生成脚本：目标名尾部带空格或点（Windows 会被静默裁掉）。

判定权威：T v1.12 §3.2 的 R04 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：预检拦截并提示（方案 §5.13）。

实现说明：
- 本脚本生成"安全源文件 + 目标映射表"，不在磁盘上真的创建尾部带空格/点的文件
  （Windows 无法表示该类名字，Linux 可创建但会掩盖被测平台的差异）。
"""

from __future__ import annotations

from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
MAP_FILE = OUT_DIR / "rename_map.csv"

# 目标名：词干尾部空格 / 词干尾部点 / 扩展名前的空格
MAP_ROWS: tuple[tuple[str, str, str], ...] = (
    ("s1.txt", "年度报表 .txt", "词干尾部带空格"),
    ("s2.txt", "年度报表.", "名字尾部带点"),
    ("s3.txt", " 年度报表.txt", "名字首部带空格（与尾部同属易被裁剪类）"),
)


def build_map() -> str:
    lines = ["source,target,note", *(f"{s},{t},{n}" for s, t, n in MAP_ROWS)]
    return "\r\n".join(lines) + "\r\n"


def verify(text: str) -> None:
    rows = [ln for ln in text.split("\r\n") if ln][1:]
    if len(rows) != len(MAP_ROWS):
        raise SystemExit(f"R04 语料失效：映射行数 {len(rows)} ≠ 预期 {len(MAP_ROWS)}")

    trailing_hits = 0
    leading_hits = 0
    for row in rows:
        src, target, _note = row.split(",", 2)
        stem = target.rsplit(".", 1)[0] if target.count(".") else target
        if target.endswith(".") or stem.endswith(" "):
            trailing_hits += 1
        if target.startswith(" "):
            leading_hits += 1
        if not (OUT_DIR / src).exists():
            raise SystemExit(f"R04 语料失效：源文件缺失 {src}")

    if trailing_hits < 2:
        raise SystemExit(f"R04 语料失效：尾部空格/点样本不足（实际 {trailing_hits}）")
    if leading_hits < 1:
        raise SystemExit("R04 语料失效：缺少首部空格样本")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for src, _target, _note in MAP_ROWS:
        target = OUT_DIR / src
        data = f"R04 源文件 {src}\n".encode("utf-8")
        if not target.exists() or target.read_bytes() != data:
            target.write_bytes(data)

    text = build_map()
    verify(text)

    data = text.encode("utf-8")
    if MAP_FILE.exists() and MAP_FILE.read_bytes() == data:
        print(f"R04 已是最新，跳过写入：{MAP_FILE}")
        return
    MAP_FILE.write_bytes(data)
    print(f"R04 已生成：{OUT_DIR}（{len(MAP_ROWS)} 条易裁剪名映射）")


if __name__ == "__main__":
    main()
