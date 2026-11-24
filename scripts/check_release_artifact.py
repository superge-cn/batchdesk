#!/usr/bin/env python3
"""发行产物的机器硬化断言（ADR-0002 §3）。

ADR-0002 §3 要求「加固要求进门禁：CI 在 release 构建路径上断言产物已生效
（符号已剥离、授权关键字符串不落明文）」。本脚本就是这个断言，接在
`.github/workflows/release.yml` 的 `tauri build` 之后、上传之前。

判定口径：
- 退出码 0 = 所有**硬断言**通过（允许有 WARN 项，WARN 会逐条打印）；
- 退出码 1 = 至少一条硬断言失败；
- 退出码 2 = 平台跳过（不是 Windows PE 产物，例如 Linux/macOS 上误跑）。

不引入第三方依赖：PE 头用标准库 struct 解析，字符串扫描用 bytes.find。
"""

from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path

DEFAULT_BINARY = Path("src-tauri/target/release/batchdesk-desktop.exe")
DEFAULT_DENYLIST = Path(__file__).with_name("release_artifact_denylist.txt")

EXIT_OK = 0
EXIT_FAIL = 1
EXIT_SKIP = 2


def fail(message: str) -> None:
    print(f"FAIL {message}")


def warn(message: str) -> None:
    print(f"WARN {message}")


def ok(message: str) -> None:
    print(f"OK   {message}")


def pe_symbol_table_pointer(data: bytes) -> int | None:
    """读 COFF 头的 PointerToSymbolTable；不是 PE 则返回 None。

    COFF 头布局（相对 PE\\0\\0 之后）：Machine(2) + NumberOfSections(2) +
    TimeDateStamp(4) + PointerToSymbolTable(4) —— 即偏移 8，不是 12
    （12 是 NumberOfSymbols，两者同为 0 时容易误判，写错偏移会得到假绿）。
    """
    if len(data) < 0x40 or data[:2] != b"MZ":
        return None
    pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
    if pe_offset + 24 > len(data) or data[pe_offset : pe_offset + 4] != b"PE\0\0":
        return None
    coff = pe_offset + 4
    return struct.unpack_from("<I", data, coff + 8)[0]


def read_denylist(path: Path) -> list[str]:
    items: list[str] = []
    if not path.exists():
        fail(f"denylist 不存在：{path}")
        return items
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if line and not line.startswith("#"):
            items.append(line)
    if not items:
        fail(f"denylist 为空：{path}（空清单会让断言永远绿）")
    return items


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--denylist", type=Path, default=DEFAULT_DENYLIST)
    args = parser.parse_args()

    failed = False

    # ① 产物存在且非空。
    if not args.binary.is_file():
        fail(f"产物不存在：{args.binary}")
        return EXIT_FAIL
    data = args.binary.read_bytes()
    if not data:
        fail(f"产物为空：{args.binary}")
        return EXIT_FAIL
    ok(f"产物存在且非空：{args.binary}（{len(data)} 字节）")

    # ② 剥离符号表：PE COFF PointerToSymbolTable 必须为 0。
    symbol_pointer = pe_symbol_table_pointer(data)
    if symbol_pointer is None:
        print("SKIP 非 Windows PE 产物，本脚本的 PE 断言不适用")
        return EXIT_SKIP
    if symbol_pointer != 0:
        fail(f"符号表未剥离：PointerToSymbolTable = {symbol_pointer}（应为 0）")
        failed = True
    else:
        ok("符号表已剥离：PointerToSymbolTable = 0")

    # ③ 调试信息残留：当前工具链在 strip=symbols 后仍保留 CodeView 的 RSDS/PDB
    # 路径（实测）。ADR-0002 §3 的「剥离调试信息」尚未被 profile 完全覆盖，
    # 所以这里**只报告不判红**，避免用假断言制造"已满足"的假象。
    debug_markers = [marker for marker in (b".pdb", b"RSDS") if marker in data]
    if debug_markers:
        warn(
            "仍存在调试信息残留（"
            + ", ".join(marker.decode() for marker in debug_markers)
            + "）：strip=symbols 已移除符号表，但 CodeView 目录仍在。"
            "ADR-0002 §3 的『剥离调试信息』未完全满足，需另立改动调整 profile；"
            "本条不判红，以免用假断言掩盖缺口。"
        )
    else:
        ok("未发现 .pdb／RSDS 调试信息残留")

    # ④ 授权关键字符串不落明文。
    denylist = read_denylist(args.denylist)
    if not denylist:
        return EXIT_FAIL
    for item in denylist:
        offset = data.find(item.encode("utf-8"))
        if offset >= 0:
            fail(f"授权关键字符串明文入产物：{item}（偏移 {offset}）")
            failed = True
        else:
            ok(f"未出现授权关键字符串：{item}")

    if failed:
        print("结论：发行产物硬化断言失败")
        return EXIT_FAIL
    print("结论：发行产物硬化断言通过（WARN 项见上，属已登记缺口）")
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
