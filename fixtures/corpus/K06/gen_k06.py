#!/usr/bin/env python3
"""K06 语料生成脚本：二进制文件（PNG）改扩展名为 .txt。

判定权威：T v1.12 §3.2 的 K06 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：魔数探测排除，不进入文本管道（方案 §5.14）。

幂等：内嵌固定 1×1 PNG 字节（base64），内容相同时不重写。
自校验：文件签名必须是 PNG；必须含 NUL 字节；必须无法作为 UTF-8 解码。
"""

from __future__ import annotations

import base64
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "photo.txt"  # 扩展名故意改为 .txt

PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"

# 固定的 1×1 透明 PNG（67 字节）；内嵌以避免任何外部图片依赖
PNG_BASE64 = (
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
)


def build() -> bytes:
    return base64.b64decode(PNG_BASE64)


def verify(data: bytes) -> None:
    if not data.startswith(PNG_SIGNATURE):
        raise SystemExit("K06 语料失效：缺少 PNG 签名，魔数探测将无法识别")

    try:
        data.decode("utf-8")
    except UnicodeDecodeError:
        pass
    else:
        raise SystemExit("K06 语料失效：内容可被 UTF-8 解码，魔数探测的区分力不成立")

    if b"\x00" not in data:
        raise SystemExit("K06 语料失效：未检测到 NUL 字节，二进制特征不足")

    if OUT_FILE.suffix != ".txt":
        raise SystemExit("K06 语料失效：输出扩展名不是 .txt")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build()
    verify(data)

    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"K06 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(f"K06 已生成：{OUT_FILE}（{len(data)} 字节，PNG 内容 / .txt 扩展名）")


if __name__ == "__main__":
    main()
