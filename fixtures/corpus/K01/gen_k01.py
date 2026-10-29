#!/usr/bin/env python3
"""K01 语料生成脚本：GBK 中文 CSV（无 BOM）。

判定权威：T v1.12 §3.2 的 K01 行（本脚本只负责产出可执行样本，不承载判定文本）。
预期行为：编码检测应判定为 GB18030 并正确转码（转码实现对应基线方案 §5.2）。

设计要点
- 幂等：重复运行产出逐字节一致（固定内容、固定 CRLF、不写 BOM）；内容相同时不重写文件。
- 自校验：生成物必须**无法作为 UTF-8 解码**，否则该语料对编码检测失去区分力，脚本报错退出。
- 合成数据：全部为虚构内容，不含任何真实客户数据（对齐 T v1.12 §3.3）。
"""

from __future__ import annotations

from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "K01.csv"

# 表头与数据行均为虚构内容；数值列用等宽优先的定长写法，便于对齐观察
HEADER = "姓名,部门,金额,备注"

ROWS: tuple[tuple[str, str, str, str], ...] = (
    ("张伟", "行政部", "1200.00", "差旅报销"),
    ("李娜", "财务部", "3800.50", "设备采购"),
    ("王强", "技术部", "760.00", "耗材"),
    ("赵敏", "人事部", "2450.75", "培训费"),
    ("陈静", "市场部", "9800.00", "会务"),
    ("刘洋", "行政部", "135.20", "快递费"),
    ("孙磊", "技术部", "5620.00", "软件授权"),
    ("周芳", "财务部", "410.00", "办公用品"),
)


def build() -> bytes:
    """构造语料字节串：GBK 编码、CRLF 行尾、无 BOM。"""
    lines = [HEADER, *(",".join(row) for row in ROWS)]
    text = "\r\n".join(lines) + "\r\n"
    return text.encode("gbk")


def verify(data: bytes) -> None:
    """自校验：语料必须具备区分力，且编码与行尾符合预期。"""
    # 1) 必须以 GBK 解码成功（否则语料本身有问题）
    decoded = data.decode("gbk")
    if not decoded.startswith(HEADER):
        raise SystemExit("K01 语料失效：GBK 解码后首行与预期表头不一致")

    # 2) 必须无法作为 UTF-8 解码（否则编码检测器可能判为 UTF-8，语料失去区分力）
    try:
        data.decode("utf-8")
    except UnicodeDecodeError:
        pass
    else:
        raise SystemExit(
            "K01 语料失效：内容可被 UTF-8 解码，检测器将无法区分 GBK 与 UTF-8。"
            "请调整样本（例如更换中文用字）后重跑。"
        )

    # 3) 不得携带 BOM
    if data.startswith(b"\xef\xbb\xbf") or data.startswith((b"\xff\xfe", b"\xfe\xff")):
        raise SystemExit("K01 语料失效：检测到 BOM，与本条语料定义（无 BOM）不符")

    # 4) 行尾必须全部为 CRLF：剥掉 CRLF 后不应残留任何 CR 或 LF；且末行以 CRLF 结束
    stripped = data.replace(b"\r\n", b"")
    if b"\r" in stripped or b"\n" in stripped:
        raise SystemExit("K01 语料失效：存在非 CRLF 的行尾")
    if not data.endswith(b"\r\n"):
        raise SystemExit("K01 语料失效：末行未以 CRLF 结束")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = build()
    verify(data)

    if OUT_FILE.exists() and OUT_FILE.read_bytes() == data:
        print(f"K01 已是最新，跳过写入：{OUT_FILE}")
        return

    OUT_FILE.write_bytes(data)
    print(f"K01 已生成：{OUT_FILE}（{len(data)} 字节，{len(ROWS)} 条数据行）")


if __name__ == "__main__":
    main()