#!/usr/bin/env python3
"""R02 语料生成脚本：仅大小写不同的目标名（A.txt vs a.txt）。

判定权威：T v1.12 §3.2 的 R02 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：感知运行卷的大小写规则，不假设一个平台结论（方案 §5.13；篇 04 §2 的 CaseSense）。

实现说明：
- 本脚本**不**在同一目录创建两个仅大小写不同的真实文件（macOS/Windows 默认卷无法共存）；
  样本由"两个源文件 + 改名映射表"组成，冲突判定属被测实现的运行期行为，依运行卷而定。
"""

from __future__ import annotations

from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
MAP_FILE = OUT_DIR / "rename_map.csv"

SOURCES: dict[str, str] = {
    "s1.txt": "R02 源文件一：目标名 A.txt\n",
    "s2.txt": "R02 源文件二：目标名 a.txt\n",
}

MAP_ROWS: tuple[tuple[str, str, str], ...] = (
    ("s1.txt", "A.txt", "与 a.txt 仅大小写不同"),
    ("s2.txt", "a.txt", "与 A.txt 仅大小写不同"),
)


def build_map() -> str:
    lines = ["source,target,note", *(f"{s},{t},{n}" for s, t, n in MAP_ROWS)]
    return "\r\n".join(lines) + "\r\n"


def verify(text: str) -> None:
    rows = [ln for ln in text.split("\r\n") if ln][1:]
    targets = [row.split(",")[1] for row in rows]

    if len(targets) != 2:
        raise SystemExit(f"R02 语料失效：目标名应为 2 个，实际 {len(targets)}")

    a, b = targets
    if a == b:
        raise SystemExit("R02 语料失效：两个目标名完全相同，与'仅大小写不同'定义不符")
    if a.lower() != b.lower():
        raise SystemExit("R02 语料失效：两个目标名去掉大小写差异后仍不相同")

    for name in SOURCES:
        if not (OUT_DIR / name).exists():
            raise SystemExit(f"R02 语料失效：源文件缺失 {name}")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for name, content in SOURCES.items():
        target = OUT_DIR / name
        data = content.encode("utf-8")
        if not target.exists() or target.read_bytes() != data:
            target.write_bytes(data)

    text = build_map()
    verify(text)

    data = text.encode("utf-8")
    if MAP_FILE.exists() and MAP_FILE.read_bytes() == data:
        print(f"R02 已是最新，跳过写入：{MAP_FILE}")
        return
    MAP_FILE.write_bytes(data)
    print(f"R02 已生成：{OUT_DIR}（2 个源文件 + rename_map.csv）")


if __name__ == "__main__":
    main()
