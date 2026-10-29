#!/usr/bin/env python3
"""R01 语料生成脚本：深层目录 + 长路径（覆盖方案 §5.13 的三档分级）。

判定权威：T v1.12 §3.2 的 R01 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：按方案 §5.13 的长路径分级处理（安全档正常；警告档允许并提示；超长档检查
         LongPathsEnabled，未启用则跳过并给出可操作引导）。

实现说明（跨平台安全）：
- 本脚本只生成 `paths.csv`（三档边界路径字符串）与一个浅层真实目录；
  **不在生成期创建超出安全档的真实路径**——Windows 未启用长路径支持时创建会直接失败，
  会把"生成"与"被测行为"混为一谈。真实路径的创建属测试期动作（由被测实现完成）。
- 分档边界值来自方案 §5.13，脚本不另立阈值。
"""

from __future__ import annotations

import csv
import io
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "paths.csv"

# 虚构根目录（仅用于构造路径字符串，不要求真实存在）
FAKE_ROOT = "C:\\资料归档"

# 分层边界样本（取自方案 §5.13 的分级边界；此处只列三个代表点）
SAMPLES: tuple[int, ...] = (200, 259, 260)

SEGMENT = "年度归档资料层"  # 7 个字符，构造嵌套目录


def make_path(total_len: int) -> str:
    """构造长度严格等于 total_len 的绝对路径（目录段重复 + 补齐文件名）。"""
    path = FAKE_ROOT + "\\"
    while len(path) + len(SEGMENT) + 1 + 12 <= total_len:
        path += SEGMENT + "\\"

    name_len = total_len - len(path)
    if name_len < 2:
        raise ValueError(f"路径构造失败：total_len={total_len} 过短")
    # 文件名：'记' + 补齐字符，长度恰为 name_len
    return path + "记" + "补" * (name_len - 1)


def tier_of(length: int) -> str:
    # 构造边界需要具体长度；权威仍是方案 §5.13，基线升版时同步本函数。
    if length <= 200:
        return "正常（安全档）"
    if length <= 259:
        return "警告但允许（警告档）"
    return "超长（超长档，须检查 LongPathsEnabled）"


def build() -> str:
    buf = io.StringIO()
    writer = csv.writer(buf, lineterminator="\r\n")
    writer.writerow(["case_id", "path", "char_len", "tier", "note"])
    for idx, total in enumerate(SAMPLES, start=1):
        path = make_path(total)
        note = "边界样本" if idx == 1 else "分级代表点"
        writer.writerow([f"R01-{idx}", path, len(path), tier_of(len(path)), note])
    return buf.getvalue()


def verify(text: str) -> None:
    rows = list(csv.reader(io.StringIO(text)))
    body = rows[1:]
    if len(body) != len(SAMPLES):
        raise SystemExit(f"R01 语料失效：样本行数 {len(body)} ≠ 预期 {len(SAMPLES)}")

    lengths = []
    for row in body:
        declared = int(row[2])
        actual = len(row[1])
        if declared != actual:
            raise SystemExit(f"R01 语料失效：声明长度 {declared} ≠ 实际长度 {actual}")
        lengths.append(actual)

    if lengths != list(SAMPLES):
        raise SystemExit(f"R01 语料失效：实际长度 {lengths} ≠ 设计长度 {list(SAMPLES)}")

    # 三档必须全部出现（否则语料无法覆盖 §5.13 的分级分支）
    tiers = {tier_of(n) for n in lengths}
    if len(tiers) != 3:
        raise SystemExit(f"R01 语料失效：只覆盖了 {len(tiers)} 个分级，未覆盖三档")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    # 浅层真实目录：仅用于让样本目录具备可遍历结构（不含超长路径）
    (OUT_DIR / "shallow" / "sub").mkdir(parents=True, exist_ok=True)

    text = build()
    verify(text)

    data = text.encode("utf-8")
    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"R01 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(f"R01 已生成：{OUT_FILE}（三档边界样本：{', '.join(map(str, SAMPLES))} 字符）")


if __name__ == "__main__":
    main()
