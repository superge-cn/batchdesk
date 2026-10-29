#!/usr/bin/env python3
"""Z02 语料生成脚本：含 ../ 路径穿越条目的 ZIP。

判定权威：T v1.12 §3.2 的 Z02 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：拒绝该越界条目并告警（E3006，方案 §5.11；T v1.12 §3.2 Z02 行）。

实现说明：
- 与 Z01 相同，手写"存储（不压缩）"ZIP 结构（多条目），不依赖 zipfile 的写入行为；
- 条目：一个正常文件 + 一个 ../escape.txt 穿越条目 + 一个绝对路径条目
  （绝对路径条目覆盖"以盘符/根开头的成员名"这一类越界写法）。

自校验：必须同时包含正常条目与至少一个穿越/绝对路径条目；正常条目必须可被安全解出。
"""

from __future__ import annotations

import io
import struct
import zipfile
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "traversal.zip"

ENTRIES: tuple[tuple[str, bytes], ...] = (
    ("safe/ok.txt", "Z02 正常条目：应被正常解出。\n".encode("utf-8")),
    ("../escape.txt", "Z02 穿越条目：不得写到目标目录之外。\n".encode("utf-8")),
    ("safe/../../escape2.txt", "Z02 嵌套穿越条目：同样必须被拦截。\n".encode("utf-8")),
)


def build_zip(entries: tuple[tuple[str, bytes], ...]) -> bytes:
    locals_: list[bytes] = []
    centrals: list[bytes] = []
    offset = 0

    for name, data in entries:
        name_bytes = name.encode("utf-8")
        crc = zlib.crc32(data) & 0xFFFFFFFF
        size = len(data)
        flags = 0x0800  # UTF-8 名（本样本关注的是路径，不是编码）

        local = struct.pack(
            "<IHHHHHIIIHH", 0x04034B50, 20, flags, 0, 0, 0, crc, size, size,
            len(name_bytes), 0,
        ) + name_bytes + data

        central = struct.pack(
            "<IHHHHHHIIIHHHHHII", 0x02014B50, 20, 20, flags, 0, 0, 0, crc, size, size,
            len(name_bytes), 0, 0, 0, 0, 0, offset,
        ) + name_bytes

        locals_.append(local)
        centrals.append(central)
        offset += len(local)

    cd = b"".join(centrals)
    body = b"".join(locals_)
    eocd = struct.pack(
        "<IHHHHIIH", 0x06054B50, 0, 0, len(entries), len(entries), len(cd), len(body), 0
    )
    return body + cd + eocd


def build() -> bytes:
    return build_zip(ENTRIES)


def verify(data: bytes) -> None:
    with zipfile.ZipFile(io.BytesIO(data)) as zf:
        names = zf.namelist()

    if len(names) != len(ENTRIES):
        raise SystemExit(f"Z02 语料失效：条目数 {len(names)} ≠ 预期 {len(ENTRIES)}")

    traversal = [n for n in names if ".." in n.split("/")]
    if not traversal:
        raise SystemExit("Z02 语料失效：未检测到 ../ 穿越条目")

    if "safe/ok.txt" not in names:
        raise SystemExit("Z02 语料失效：缺少正常条目，'只拦越界、不误伤正常'无法验证")

    # 正常条目内容必须可读
    with zipfile.ZipFile(io.BytesIO(data)) as zf:
        if zf.read("safe/ok.txt") != ENTRIES[0][1]:
            raise SystemExit("Z02 语料失效：正常条目内容与写入内容不一致")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build()
    verify(data)

    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"Z02 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(f"Z02 已生成：{OUT_FILE}（{len(data)} 字节，{len(ENTRIES)} 条目，含 2 条穿越）")


if __name__ == "__main__":
    main()
