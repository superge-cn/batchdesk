#!/usr/bin/env python3
"""E03 语料生成脚本：只读文件。

判定权威：T v1.12 §3.2 的 E03 行（本脚本只负责产出样本，不承载判定文本）。
预期行为：预检标记并跳过（E2002 无权限，篇 01 §7 码表）；任务不整体失败（方案 §5.13；T v1.12 §3.2 E03 行）。

实现说明：
- 用 os.chmod 去掉全部写位；POSIX 与 Windows（映射为只读属性）均适用；
- 重复运行需先恢复可写才能改写内容，故本脚本在写入前临时加写位、写后立即收回。

自校验：文件必须存在且"写位全清"（st_mode 的 0o222 为 0）。
"""

from __future__ import annotations

import os
import stat
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT_DIR = HERE / "out"
OUT_FILE = OUT_DIR / "readonly.txt"

CONTENT = "E03 只读样本：内容固定，用于触发写入权限错误。\n"


def restore_writable(path: Path) -> None:
    os.chmod(path, stat.S_IRUSR | stat.S_IWUSR | stat.S_IRGRP | stat.S_IROTH)


def verify(path: Path) -> None:
    if not path.exists():
        raise SystemExit(f"E03 语料失效：样本不存在 {path}")

    mode = path.stat().st_mode
    if mode & 0o222:
        raise SystemExit(f"E03 语料失效：文件仍可写（mode={oct(mode & 0o777)}），只读特征不成立")

    if path.read_text(encoding="utf-8") != CONTENT:
        raise SystemExit("E03 语料失效：内容与预期不一致")


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    data = CONTENT.encode("utf-8")

    if OUT_FILE.exists():
        restore_writable(OUT_FILE)
        if OUT_FILE.read_bytes() != data:
            OUT_FILE.write_bytes(data)
        else:
            print(f"E03 内容已最新，仅重设只读属性：{OUT_FILE}")
    else:
        OUT_FILE.write_bytes(data)

    os.chmod(OUT_FILE, stat.S_IRUSR | stat.S_IRGRP | stat.S_IROTH)
    verify(OUT_FILE)
    print(f"E03 已就绪：{OUT_FILE}（只读，{len(data)} 字节）")


if __name__ == "__main__":
    main()
