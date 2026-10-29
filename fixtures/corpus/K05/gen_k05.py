#!/usr/bin/env python3
"""K05 语料生成脚本：纯 ASCII 短文本 CSV。

判定权威：T v1.12 §3.2 的 K05 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：默认 UTF-8 并提示（方案 §5.2 异常处理：长度不足采样窗口时给低置信提示）。

幂等：固定内容；内容相同时不重写。
自校验：全部字节 < 0x80；行数固定；必须"短"（长度远小于任何统计采样窗口，本脚本不引用具体窗口值）。
"""

from __future__ import annotations

from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "K05.csv"

HEADER = "id,name,qty"
ROWS: tuple[tuple[str, str, str], ...] = (
    ("1", "apple", "3"),
    ("2", "banana", "5"),
    ("3", "cherry", "7"),
)

# 说明：此处只约束"样本足够短"，不对应任何产品阈值；采样窗口数值以方案 §5.2 为准，脚本不复述。
SHORT_UPPER_BOUND = 128


def build() -> bytes:
    lines = [HEADER, *(",".join(row) for row in ROWS)]
    return ("\r\n".join(lines) + "\r\n").encode("ascii")


def verify(data: bytes) -> None:
    if not data.isascii():
        raise SystemExit("K05 语料失效：出现非 ASCII 字节，与纯 ASCII 定义不符")
    if len(data) >= SHORT_UPPER_BOUND:
        raise SystemExit(f"K05 语料失效：样本长度 {len(data)} 字节，已不再是短文件样本")
    if not data.startswith(HEADER.encode("ascii")):
        raise SystemExit("K05 语料失效：首行与预期表头不一致")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build()
    verify(data)

    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"K05 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(f"K05 已生成：{OUT_FILE}（{len(data)} 字节，纯 ASCII）")


if __name__ == "__main__":
    main()
