#!/usr/bin/env python3
"""Z01 语料生成脚本：非 UTF-8（GBK）文件名且不带 UTF-8 标志位的 ZIP。

判定权威：T v1.12 §3.2 的 Z01 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：回退 GBK 并提示（方案 §5.11）。

实现说明：
- Python 的 zipfile **总是**为非 ASCII 名写 UTF-8 标志位（bit 11），无法用它造出本样本；
  因此本脚本按 ZIP 结构手写"存储（不压缩）"单文件包：文件名用 GBK 字节，通用标志位为 0。
- 结构：本地文件头 + 中央目录 + 结束记录；CRC32 与长度均按真实值填写。

自校验：本地文件头与中央目录的通用标志位必须不含 0x0800；文件名必须可按 GBK 解码，
         且必须**不可**按 UTF-8 解码（否则与 K03/K02 的样本语义重叠）。
"""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "gbk_name.zip"

GBK_NAME = "中文资料.txt"
CONTENT = "Z01 样本内容：文件名使用 GBK 编码，未设置 UTF-8 标志位。\n"
UTF8_FLAG = 0x0800


def build_zip(name_bytes: bytes, data: bytes) -> bytes:
    crc = zlib.crc32(data) & 0xFFFFFFFF
    size = len(data)
    name_len = len(name_bytes)
    flags = 0  # 不设 bit 11（UTF-8 标志）

    # 本地文件头（30 字节 + 文件名 + 数据）
    local = struct.pack(
        "<IHHHHHIIIHH",
        0x04034B50,
        20,      # 解压所需版本
        flags,
        0,       # 存储，不压缩
        0,       # 修改时间
        0,       # 修改日期
        crc,
        size,
        size,
        name_len,
        0,       # 扩展字段长度
    ) + name_bytes + data

    # 中央目录（46 字节 + 文件名）
    central = struct.pack(
        "<IHHHHHHIIIHHHHHII",
        0x02014B50,
        20,      # 创建版本
        20,      # 解压所需版本
        flags,
        0,
        0,
        0,
        crc,
        size,
        size,
        name_len,
        0,       # 扩展字段
        0,       # 注释
        0,       # 起始磁盘
        0,       # 内部属性
        0,       # 外部属性
        0,       # 本地头偏移
    ) + name_bytes

    # 中央目录结束记录
    eocd = struct.pack(
        "<IHHHHIIH",
        0x06054B50,
        0,
        0,
        1,
        1,
        len(central),
        len(local),
        0,
    )
    return local + central + eocd


def build() -> bytes:
    return build_zip(GBK_NAME.encode("gbk"), CONTENT.encode("utf-8"))


def verify(data: bytes) -> None:
    if data[:4] != b"PK\x03\x04":
        raise SystemExit("Z01 语料失效：缺少本地文件头签名")

    local_flags = struct.unpack("<H", data[6:8])[0]
    if local_flags & UTF8_FLAG:
        raise SystemExit("Z01 语料失效：本地文件头设置了 UTF-8 标志位（bit 11）")

    name_len = struct.unpack("<H", data[26:28])[0]
    raw_name = data[30:30 + name_len]

    if raw_name.decode("gbk") != GBK_NAME:
        raise SystemExit("Z01 语料失效：文件名按 GBK 解码后与预期不一致")

    try:
        raw_name.decode("utf-8")
    except UnicodeDecodeError:
        pass
    else:
        raise SystemExit("Z01 语料失效：文件名可按 UTF-8 解码，'回退 GBK'的区分力不成立")

    # 结构可被标准库打开（只校验容器有效性，名称解码差异正是被测点）
    import io
    import zipfile

    with zipfile.ZipFile(io.BytesIO(data)) as zf:
        if len(zf.namelist()) != 1:
            raise SystemExit("Z01 语料失效：包内条目数不为 1")
        if zf.read(zf.namelist()[0]) != CONTENT.encode("utf-8"):
            raise SystemExit("Z01 语料失效：解出的内容与写入内容不一致")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build()
    verify(data)

    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"Z01 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(f"Z01 已生成：{OUT_FILE}（{len(data)} 字节；GBK 文件名，标志位无 bit 11）")


if __name__ == "__main__":
    main()
