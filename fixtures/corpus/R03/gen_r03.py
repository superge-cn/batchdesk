#!/usr/bin/env python3
"""R03 语料生成脚本：Windows 保留名作为目标名（CON/PRN/AUX/NUL/COM1/LPT1）。

判定权威：T v1.12 §3.2 的 R03 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：预检阶段拦截并提示（方案 §5.13）。

实现说明：
- 保留名在 Windows 上**无法**作为真实文件名创建，因此样本由"安全源文件 + 目标映射表"组成；
  映射表是测试期输入，不依赖宿主平台能否落盘保留名。
"""

from __future__ import annotations

from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
MAP_FILE = OUT_DIR / "rename_map.csv"

# Windows 保留设备名（大小写不敏感；含扩展名同样被拒绝）
RESERVED_TARGETS: tuple[str, ...] = (
    "CON.txt",
    "PRN.txt",
    "AUX.log",
    "NUL.txt",
    "COM1.txt",
    "LPT1.txt",
)

RESERVED_STEMS = {"CON", "PRN", "AUX", "NUL", "COM1", "LPT1"}


def source_names() -> list[str]:
    return [f"src{i:02d}.txt" for i in range(1, len(RESERVED_TARGETS) + 1)]


def build_map() -> str:
    lines = ["source,target,note"]
    for src, target in zip(source_names(), RESERVED_TARGETS):
        lines.append(f"{src},{target},Windows 保留设备名")
    return "\r\n".join(lines) + "\r\n"


def verify(text: str) -> None:
    rows = [ln for ln in text.split("\r\n") if ln][1:]
    if len(rows) != len(RESERVED_TARGETS):
        raise SystemExit(f"R03 语料失效：映射行数 {len(rows)} ≠ 预期 {len(RESERVED_TARGETS)}")

    for row in rows:
        src, target, _note = row.split(",")
        stem = target.split(".")[0].upper()
        if stem not in RESERVED_STEMS:
            raise SystemExit(f"R03 语料失效：目标 {target} 不是保留设备名")
        if not (OUT_DIR / src).exists():
            raise SystemExit(f"R03 语料失效：源文件缺失 {src}")

    covered = {row.split(",")[1].split(".")[0].upper() for row in rows}
    missing = RESERVED_STEMS - covered
    if missing:
        raise SystemExit(f"R03 语料失效：保留名覆盖不全，缺 {sorted(missing)}")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for src in source_names():
        target = OUT_DIR / src
        data = f"R03 源文件 {src}：目标名为 Windows 保留设备名\n".encode("utf-8")
        if not target.exists() or target.read_bytes() != data:
            target.write_bytes(data)

    text = build_map()
    verify(text)

    data = text.encode("utf-8")
    if MAP_FILE.exists() and MAP_FILE.read_bytes() == data:
        print(f"R03 已是最新，跳过写入：{MAP_FILE}")
        return
    MAP_FILE.write_bytes(data)
    print(f"R03 已生成：{OUT_DIR}（{len(RESERVED_TARGETS)} 个保留名映射）")


if __name__ == "__main__":
    main()
