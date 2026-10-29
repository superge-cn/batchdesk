#!/usr/bin/env python3
"""K07 语料生成脚本：GBK 段与 UTF-8 段字节拼接的混合编码文本。

判定权威：T v1.12 §3.2 的 K07 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：标记疑似混合编码，不承诺修复（方案 §5.2）。

幂等：两段内容固定、边界固定；内容相同时不重写。
自校验：前半段可 GBK 解码、后半段可 UTF-8 解码；整体**不可** UTF-8 解码（否则失去区分力）。
"""

from __future__ import annotations

from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "K07.txt"

GBK_PART_TEXT = (
    "姓名,部门,金额\r\n"
    "张伟,行政部,1200.00\r\n"
    "李娜,财务部,3800.50\r\n"
)

UTF8_PART_TEXT = (
    "王强,技术部,760.00\r\n"
    "赵敏,人事部,2450.75\r\n"
)


def build() -> bytes:
    return GBK_PART_TEXT.encode("gbk") + UTF8_PART_TEXT.encode("utf-8")


def verify(data: bytes, gbk_part: bytes, utf8_part: bytes) -> None:
    # 1) 前半段按 GBK 解码成功
    if gbk_part.decode("gbk") != GBK_PART_TEXT:
        raise SystemExit("K07 语料失效：前半段 GBK 解码结果与预期不一致")

    # 2) 后半段按 UTF-8 解码成功
    if utf8_part.decode("utf-8") != UTF8_PART_TEXT:
        raise SystemExit("K07 语料失效：后半段 UTF-8 解码结果与预期不一致")

    # 3) 整体不可 UTF-8 解码——这是"混合"的核心区分力
    try:
        data.decode("utf-8")
    except UnicodeDecodeError:
        pass
    else:
        raise SystemExit(
            "K07 语料失效：整体可被 UTF-8 解码，检测器会误判为单一编码，语料失去区分力"
        )


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    gbk_part = GBK_PART_TEXT.encode("gbk")
    utf8_part = UTF8_PART_TEXT.encode("utf-8")
    data = gbk_part + utf8_part
    verify(data, gbk_part, utf8_part)

    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"K07 已是最新，跳过写入：{OUT_FILE}")
        return
    OUT_FILE.write_bytes(data)
    print(
        f"K07 已生成：{OUT_FILE}（{len(data)} 字节；GBK 段 {len(gbk_part)} + UTF-8 段 {len(utf8_part)}）"
    )


if __name__ == "__main__":
    main()
