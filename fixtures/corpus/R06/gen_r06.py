#!/usr/bin/env python3
"""R06 语料生成脚本：同一路径的 \\\\?\\ 前缀形态与普通形态。

判定权威：T v1.12 §3.2 的 R06 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：前缀归口文件系统适配层，不重复拼接（方案 §5.13；篇 04 §2 的 N1–N6）。

实现说明：
- 前缀处理必须由被测实现的归一化层完成（不变量 8：业务代码禁止自拼 \\\\?\\ 前缀），
  本脚本只产出"两种写法的路径对"，供等价性断言使用，不依赖 Windows 宿主。
"""

from __future__ import annotations

from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "paths.csv"

PLAIN = "C:\\资料归档\\2026\\年度报表.txt"
EXTENDED = "\\\\?\\" + PLAIN  # \\?\C:\资料归档\2026\年度报表.txt


def build() -> str:
    lines = [
        "case_id,style,path",
        f'R06-1,plain,"{PLAIN}"',
        f'R06-2,extended,"{EXTENDED}"',
    ]
    return "\r\n".join(lines) + "\r\n"


def verify(text: str) -> None:
    rows = [ln for ln in text.split("\r\n") if ln][1:]
    if len(rows) != 2:
        raise SystemExit(f"R06 语料失效：路径对行数 {len(rows)} ≠ 2")

    plain = PLAIN
    extended = EXTENDED

    if not extended.startswith("\\\\?\\"):
        raise SystemExit("R06 语料失效：扩展形态缺少 \\\\?\\ 前缀")
    if extended[4:] != plain:
        raise SystemExit("R06 语料失效：剥离前缀后与普通形态不一致")

    # 两种写法必须同时出现在样本中
    body = text.encode("utf-8").decode("utf-8")
    if PLAIN not in body or EXTENDED not in body:
        raise SystemExit("R06 语料失效：样本未同时包含两种写法")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    text = build()
    verify(text)

    data = text.encode("utf-8")
    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"R06 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(f"R06 已生成：{OUT_FILE}（普通形态 / 扩展形态各 1 条）")


if __name__ == "__main__":
    main()
