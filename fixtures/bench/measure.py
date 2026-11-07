#!/usr/bin/env python3
"""C-1／C-2 基准测量夹具（本地/专用机执行，CI 不跑）。

判定权威：方案 §9.1（指标与阈值）、T v1.12 附录 C（数据集构造）；字段口径见
docs/testing/性能基准执行手册.md §3–§4。本脚本只测量与记录，不自立任何阈值。

- 预热 1 次 + 正式 3 次，全部原始值入 JSONL，中位数由 3 次正式值计算（手册 §3）。
- 本机无 /usr/bin/time，墙钟与峰值 RSS 由本脚本计时与 /proc/<pid>/status 采样得到。
- C-2 带 RSS 保险丝：采样到 VmRSS 超过 --rss-limit-mib 立即 kill 并停测，不产出结果行。

前置：cargo build --release；数据集先生成（fixtures/bench/C-1…/gen_c1.py、C-2…/gen_c2.py）。
用法：
    python3 fixtures/bench/measure.py c1
    python3 fixtures/bench/measure.py c2 --rss-limit-mib 1024              # F01 按字节
    python3 fixtures/bench/measure.py c2 --split lines --lines 3000000     # F02 按行数
    python3 fixtures/bench/measure.py c2 --split parts --parts 9           # F03 按份数
    python3 fixtures/bench/measure.py c2rw --case c01                      # C-2 档就地改写：转码
    python3 fixtures/bench/measure.py c2rw --case c02                      # 换行符统一
    python3 fixtures/bench/measure.py c2rw --case c03-plain                # 查找替换（字面）
    python3 fixtures/bench/measure.py c2rw --case c03-regex                # 查找替换（可界定正则）
    python3 fixtures/bench/measure.py c2rw --case c03-cell                 # 查找替换（CSV 单元格）
    python3 fixtures/bench/measure.py c2rw --case c03-plain --size-mib 512 # 只取 C-2 集的前 512 MiB
    python3 fixtures/bench/measure.py c2rw --case c01 --gb-multibyte --size-mib 64  # 现生成的 GB18030 多字节变体（方向性读数，见下）

c2rw 一律在 C-2 集的**工作副本**上就地改写（改写会覆盖源，故绝不碰 `out/big_file.txt` 本体），
产物按方案 §9.1 的"改写"语义逐字节校验：校验不过＝样本作废。校验在计时区外做。
"""

from __future__ import annotations

import argparse
import codecs
import hashlib
import json
import os
import random
import re
import shutil
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
BIN = REPO / "target" / "release" / ("batchdesk.exe" if os.name == "nt" else "batchdesk")
C1_ROOT = REPO / "fixtures" / "bench" / "C-1-目录树10万" / "out"
C2_SRC = REPO / "fixtures" / "bench" / "C-2-超大单文件" / "out" / "big_file.txt"

BLOCK = 8 * 1024 * 1024

# c2rw 的五个臂：每条都是「构造参数＋期望校验」，判定口径来自方案 §9.1，本表不自立阈值。
#   args —— 计划命令的参数（采样窗口/字节数由 --sample-* 传入，属构造参数）
#   name —— 工作副本的文件名：扩展名决定 kernel 的内容结构分派（.csv 走单元格臂）
#   check —— 期望校验方式：identity/digest＝与期望流逐字节一致；cells＝CSV 单元格臂不做逐字节期望
#   find/replace —— 期望变换；两类替换都是定长且不重叠左到右，与内核的执行纪律同口径
C2RW_CASES = {
    "c01": {
        "name": "big_file.txt",
        "args": lambda src, a: ["c01", "--file", str(src), "--source", "gb18030", "--target", "utf-8",
                                "--sample-window", str(a.sample_window), "--sample-blocks",
                                str(a.sample_blocks), "--json"],
        "check": "identity",
    },
    "c02": {
        "name": "big_file.txt",
        "args": lambda src, a: ["c02", "--file", str(src), "--to", "crlf", "--structure", "text",
                                "--sample-bytes", str(a.sample_bytes), "--json"],
        "check": "newline",
        "kind": "literal",
        "find": b"\n",
        "replace": b"\r\n",
        "unit": 1,
    },
    "c03-plain": {
        "name": "big_file.txt",
        "args": lambda src, a: ["c03", "--file", str(src), "--mode", "plain", "--csv-mode", "text",
                                "--find", "normal-row", "--replace", "normal_row",
                                "--sample-bytes", str(a.sample_bytes), "--json"],
        "check": "digest",
        "kind": "literal",
        "find": b"normal-row",
        "replace": b"normal_row",
        "unit": 10,
    },
    "c03-regex": {
        "name": "big_file.txt",
        "args": lambda src, a: ["c03", "--file", str(src), "--mode", "regex", "--csv-mode", "text",
                                "--find", "[0-9]{9}", "--replace", "987654321",
                                "--sample-bytes", str(a.sample_bytes), "--json"],
        "check": "digest",
        "kind": "regex",
        "find": rb"[0-9]{9}",
        "replace": b"987654321",
        "unit": 9,
    },
    "c03-cell": {
        "name": "big_file.csv",
        "args": lambda src, a: ["c03", "--file", str(src), "--mode", "plain", "--csv-mode", "cell",
                                "--delimiter", ",", "--find", "normal-row", "--replace", "normal_row",
                                "--sample-bytes", str(a.sample_bytes), "--json"],
        "check": "cells",
        "kind": "literal",
        "find": b"normal-row",
        "replace": b"normal_row",
        "unit": 10,
    },
}



def _win_rss_kib(pid: int) -> int | None:
    """Windows 下的 WorkingSet 采样（KiB）；进程已退出时返回 None（ADR-0082）。

    WorkingSetSize 与 Linux VmRSS 同为「驻留物理内存」口径，但不完全等价（含可换页内核-
    用户共享页的差异）；结果行的 RSS 列在 Windows 上按此口径记录，不做跨平台直接比较。
    """
    import ctypes

    class ProcessMemoryCounters(ctypes.Structure):
        _fields_ = [
            ("cb", ctypes.c_uint32),
            ("PageFaultCount", ctypes.c_uint32),
            ("PeakWorkingSetSize", ctypes.c_size_t),
            ("WorkingSetSize", ctypes.c_size_t),
            ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
            ("QuotaPagedPoolUsage", ctypes.c_size_t),
            ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
            ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
            ("PagefileUsage", ctypes.c_size_t),
            ("PeakPagefileUsage", ctypes.c_size_t),
        ]

    PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
    kernel32 = ctypes.windll.kernel32
    handle = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
    if not handle:
        return None
    try:
        counters = ProcessMemoryCounters()
        counters.cb = ctypes.sizeof(counters)
        getter = getattr(kernel32, "K32GetProcessMemoryInfo", None)
        if getter is None:
            getter = ctypes.windll.psapi.GetProcessMemoryInfo
        if not getter(handle, ctypes.byref(counters), counters.cb):
            return None
        return counters.WorkingSetSize // 1024
    finally:
        kernel32.CloseHandle(handle)


def sample_rss(pid: int, holder: dict, stop: threading.Event, limit_kib: int) -> None:
    peak = 0
    samples: list[int] = []
    if os.name == "nt":
        def read_kib() -> int | None:
            return _win_rss_kib(pid)
    else:
        path = f"/proc/{pid}/status"

        def read_kib() -> int | None:
            try:
                with open(path, encoding="ascii") as fh:
                    return next((int(line.split()[1]) for line in fh if line.startswith("VmRSS:")), 0)
            except (FileNotFoundError, ProcessLookupError):
                return None
    while not stop.is_set():
        kib = read_kib()
        if kib is None:
            break
        peak = max(peak, kib)
        samples.append(kib)
        if limit_kib and kib > limit_kib:
            holder["breached"] = True
            # 越限那一刻的读数是结论的一部分：记「越限时已观测到的峰值」，不记 kill 之后的值。
            holder["peak_breached_kib"] = max(peak, kib)
            try:
                os.kill(pid, 9)
            except ProcessLookupError:
                pass
            return
        time.sleep(0.05)
    holder["peak_kib"] = peak
    holder["samples"] = samples


def run(cmd: list[str], out_path: Path, rss_limit_mib: int, allow_breach: bool = False) -> dict:
    """Run one child, capture its stdout to out_path, time it and sample its RSS."""
    with out_path.open("wb") as sink:
        proc = subprocess.Popen(cmd, stdout=sink, stderr=subprocess.STDOUT)
        holder: dict = {}
        stop = threading.Event()
        timer = threading.Thread(target=sample_rss, args=(proc.pid, holder, stop, rss_limit_mib * 1024))
        started = time.monotonic()
        timer.start()
        code = proc.wait()
        wall = time.monotonic() - started
        stop.set()
        timer.join()
    if holder.get("breached"):
        # 保险丝触发＝本轮不产出结果行（手册 §3 失败协议）。c2rw 把「哪一段越限」当作结论记录，
        # 所以允许调用方接住它；默认仍然直接停测，保持 c1/c2 的既有行为不变。
        breached = {
            "exit": code,
            "wall_s": round(wall, 3),
            "peak_rss_kib": holder.get("peak_breached_kib", 0),
            "median_rss_kib": 0,
            "rss_samples": 0,
            "breached": True,
            "rss_limit_mib": rss_limit_mib,
        }
        if allow_breach:
            return breached
        raise SystemExit(
            f"RSS 保险丝触发（>{rss_limit_mib} MiB）：已终止 {cmd[1]}，本轮不产出结果行（手册 §3 失败协议）"
        )
    samples = holder.get("samples", [])
    if code != 0:
        raise SystemExit(f"命令退出码 {code}：{out_path}（失败样本不得混入中位数）")
    return {
        "exit": code,
        "wall_s": round(wall, 3),
        "peak_rss_kib": holder.get("peak_kib", 0),
        "median_rss_kib": round(statistics.median(samples)) if samples else 0,
        "rss_samples": len(samples),
    }



def workdir(tag: str) -> Path:
    # 缺省 /tmp 保持 Linux 既有行为；Windows 上 /tmp 解析为「当前盘根」，可能落进小容量系统盘，
    # 允许用 BATCHDESK_BENCH_DIR 显式指到大盘（ADR-0082）。
    root = Path(os.environ.get("BATCHDESK_BENCH_DIR", "/tmp"))
    d = root / "batchdesk-bench" / tag
    shutil.rmtree(d, ignore_errors=True)
    d.mkdir(parents=True)
    return d


def read_blocks(path: Path, limit: int | None):
    """按块读出；limit 用于「取 C-2 集前 N 字节」这类子档，不改数据集本体。"""
    done = 0
    with path.open("rb") as fh:
        while True:
            size = BLOCK if limit is None else min(BLOCK, limit - done)
            if size <= 0:
                return
            block = fh.read(size)
            if not block:
                return
            done += len(block)
            yield block


def digest_of(path: Path, limit: int | None = None) -> tuple[str, int]:
    digest = hashlib.sha256()
    total = 0
    for block in read_blocks(path, limit):
        digest.update(block)
        total += len(block)
    return digest.hexdigest(), total


# ── C01 的 GB18030 多字节变体（方向性读数用，不是 T v1.12 附录 C 的 C-2 构造）────────────
# 附录 C 的 C-2 只规定了「行长度含正常与极端两类」，没有编码维度；既有数据集是纯 ASCII，
# `c01 --source gb18030` 在它上面只走 1:1 子集，所以 §9.1 的大文件吞吐从未在真实多字节输入上量过。
# 下面三个常量都是构造参数（非产品阈值）：行长与 gen_c2.py 的「常规行约 60 字节」同量级。
GB_LINE_BYTES = 60
GB_EVERY_FOUR_BYTE_LINE = 50
GB_SEED = 2026  # 与 gen_c2.py 同种子，重复运行逐字节一致
GB_BLOCK_LINES = 200_000  # 每批 20 万行（≈12 MiB），把生成期间的内存钉住
GB_CACHE_DIR = Path("/tmp/batchdesk-bench")  # 与 measure.py 的工作副本同处，不入库


def gb_target_bytes(size_mib: int) -> int:
    if size_mib <= 0:
        raise SystemExit(
            "--gb-multibyte 必须配 --size-mib：这份数据集是现生成的、不入库，没有整档可言"
        )
    return max(1, size_mib * 1048576 // GB_LINE_BYTES) * GB_LINE_BYTES


def build_gb_multibyte(dest: Path, total_bytes: int, seed: int = GB_SEED) -> int:
    """现生成一份真多字节的 GB18030 数据集：60 字节一行，主体是 GB18030 双字节汉字，
    每 GB_EVERY_FOUR_BYTE_LINE 行把两对汉字换成一条四字节序列（CJK 扩展 B），字节数不变。

    行长由构造保证（每批整块自校 len(block)＝批行数×60），所以切片与分块都不会切在字符中间——
    跨块解码由被测实现自己在 64 KiB 缓冲边界上处理，数据侧不额外制造错位场景。
    这份数据不含非法序列，`detail` 的替换计数必须为 0（夹具另做反向断言）：本臂要量的是多字节
    解码与 UTF-8 展开的代价，非法序列路径的正确性另有 CI 锚点（encoding 的 ReplaceAndCount）。
    """
    rng = random.Random(seed)
    han = [chr(code) for code in range(0x4E00, 0x4E00 + 1024)]  # CJK 基本区：GB18030 双字节
    rare = [chr(code) for code in range(0x20000, 0x20000 + 256)]  # CJK 扩展 B：GB18030 四字节
    digits = "0123456789"
    lines_total = total_bytes // GB_LINE_BYTES
    four_byte_lines = 0
    digest = hashlib.sha256()
    with dest.open("wb") as sink:
        index = 0
        while index < lines_total:
            count = min(GB_BLOCK_LINES, lines_total - index)
            idxs = range(index, index + count)
            four = {i for i in idxs if i % GB_EVERY_FOUR_BYTE_LINE == 0}
            four_byte_lines += len(four)
            han_pool = rng.choices(han, k=24 * (count - len(four)) + 20 * len(four))
            rare_pool = rng.choices(rare, k=2 * len(four))
            digit_pool = rng.choices(digits, k=9 * count)
            hp = rp = dp = 0
            text = []
            for i in idxs:
                head = "".join(han_pool[hp:hp + 12])
                hp += 12
                num = "".join(digit_pool[dp:dp + 9])
                dp += 9
                if i in four:
                    units = han_pool[hp:hp + 8] + rare_pool[rp:rp + 2]
                    hp += 8
                    rp += 2
                    rng.shuffle(units)
                    tail = "".join(units)
                else:
                    tail = "".join(han_pool[hp:hp + 12])
                    hp += 12
                text.append(f"{head},{num},{tail}\n")
            block = "".join(text).encode("gb18030")
            if len(block) != count * GB_LINE_BYTES:
                raise SystemExit(f"生成自校失败：批 {count} 行编码后 {len(block)} 字节 "
                                 f"≠ {count}×{GB_LINE_BYTES}")
            digest.update(block)
            sink.write(block)
            index += count
    return {"bytes": dest.stat().st_size, "lines": lines_total,
            "four_byte_lines": four_byte_lines, "sha256": digest.hexdigest()}


def gb_dataset(total_bytes: int) -> tuple[Path, dict]:
    """多字节变体的数据集：按体积现生成一次、逐轮复用，并缓存期望产物（同一份数据四轮才可比）。

    与 C-2 本体同一条纪律——不入库、每轮只做工作副本；sidecar 里记的是生成结果与独立算出的
    期望 UTF-8 摘要，因此换体积会另起一份文件，不会把两档数据混成同一行台账。
    """
    GB_CACHE_DIR.mkdir(parents=True, exist_ok=True)
    dest = GB_CACHE_DIR / f"gb18030-multibyte-{total_bytes}B-seed{GB_SEED}.txt"
    side = dest.with_suffix(".json")
    if not (dest.exists() and side.exists() and dest.stat().st_size == total_bytes):
        stats = build_gb_multibyte(dest, total_bytes)
        want_digest, want_bytes = expected_utf8_from_gb_stream(dest)
        side.write_text(json.dumps({**stats, "want_sha256": want_digest,
                                    "want_bytes": want_bytes}), encoding="utf-8")
    meta = json.loads(side.read_text(encoding="utf-8"))
    if meta["bytes"] != dest.stat().st_size:
        raise SystemExit(f"多字节数据集缓存与 sidecar 不一致（{meta['bytes']} ≠ "
                         f"{dest.stat().st_size}），删除 {dest} 重跑")
    return dest, meta


def expected_utf8_from_gb_stream(path: Path) -> tuple[str, int]:
    """流式算「GB18030 源 → UTF-8 产物」的期望摘要（sha256 与字节数）。

    整档 2 GiB 不能整读：`read_bytes`＋整段 decode/encode 的峰值 ≈ 源×2.4，
    机器吃不消；增量解码器自带跨块字节 carry，逐块解码＋编码的产物与整段一次
    变换逐字节一致，内存恒定。期望仍由 Python 编解码器独立算出（被测实现是
    encoding_rs，另一套实现），「逐字节一致」这条断言能证伪。
    """
    decoder = codecs.getincrementaldecoder("gb18030")()
    digest = hashlib.sha256()
    total = 0
    with path.open("rb") as fh:
        while True:
            block = fh.read(BLOCK)
            if not block:
                break
            chunk = decoder.decode(block).encode("utf-8")
            digest.update(chunk)
            total += len(chunk)
    chunk = decoder.decode(b"", True).encode("utf-8")
    digest.update(chunk)
    total += len(chunk)
    return digest.hexdigest(), total


def expected_utf8_from_gb(data: bytes) -> tuple[str, int]:
    """期望产物由 Python 的 gb18030 编解码器独立算出（被测实现是 encoding_rs，另一套实现），
    两侧同循 GB18030 标准，所以「逐字节一致」这条断言能证伪，不是拿实现的输出自证。"""
    want = data.decode("gb18030").encode("utf-8")
    return hashlib.sha256(want).hexdigest(), len(want)


def has_border(needle: bytes) -> bool:
    """needle 是否有 border（某个真前缀＝某个真后缀）。有 border 时，一次命中内部还可能
    再起一次命中（"aaa" 里的 "aa"），整块替换与固定回退就会重复计数；无 border 时
    「一次命中的内部不会另起一次命中」成立，才能走 C 层整块快速路径。"""
    return any(needle[:k] == needle[len(needle) - k:] for k in range(1, len(needle)))


def count_needle(path: Path, needle: bytes, limit: int | None = None) -> int:
    """非重叠左到右计数（与 C03 plain 的替换纪律同口径）。

    单字节与无 border 的针走整块 `bytes.count`＋固定回退 len-1 字节：这种针不会在一次
    命中的内部再起一次命中，所以尾巴里不会重复计数。其余（有 border 的字面串）必须逐命中
    推进起点，代价 O(命中数)——只在子档用得动。
    """
    length = len(needle)
    fast = length == 1 or not has_border(needle)
    found = 0
    pending = b""
    for block in read_blocks(path, limit):
        buf = pending + block
        if fast:
            found += buf.count(needle)
            pending = buf[len(buf) - length + 1:] if length > 1 else b""
            continue
        index = 0
        while True:
            at = buf.find(needle, index)
            if at < 0:
                break
            found += 1
            index = at + length
        pending = buf[max(index, len(buf) - length + 1):]
    if fast and pending:
        found += pending.count(needle)
    elif pending:
        index = 0
        while True:
            at = pending.find(needle, index)
            if at < 0:
                break
            found += 1
            index = at + length
    return found


def _sub_step(buf: bytes, finder, repl: bytes, unit: int) -> tuple[bytes, bytes]:
    """流式非重叠左到右替换的一步：只提交「起点 ≤ len(buf)-unit」的匹配（这种匹配一定完整落在
    已读进的字节里），其余留到下一块，尾巴从**最后一个已提交的匹配末尾**接着扫——不能按固定
    长度回退，否则自叠加模式（如 "aa"）会把已被消费的字节再算一次。"""
    limit = len(buf) - unit
    if limit < 0:
        return b"", buf
    out = bytearray()
    index = 0
    cursor = 0
    while True:
        at = finder(buf, index)
        if at < 0 or at > limit:
            break
        out += buf[cursor:at]
        out += repl
        cursor = at + unit
        index = cursor
    emit_end = max(cursor, limit + 1)
    out += buf[cursor:emit_end]
    return bytes(out), buf[emit_end:]


def expected_digest(path: Path, spec: dict, limit: int | None = None) -> tuple[str, int]:
    """期望产物的 sha256 与字节数（校验用，在计时区外跑，不落盘）。

    两条路径，产物必须逐字节相同（randomized 对账把住）：
    - 整块快速路径只用于 **unit=1**（LF→CRLF）：单字节匹配既不跨块也不自叠加，逐块
      `bytes.replace` 与整段一次替换逐字相等；
    - 其余全部走 `_sub_step` 的逐步提交（有 border 的字面串、无 border 的长字面串、定长正则）。
      长字面串**不能**走"整块替换＋固定回退"：那种做法会在回退点切断一次已提交的命中
      （`abcd` 恰好压在边界上时，替换串的前缀被发出、剩余部分被当成输入带进下一块），
      随机对账当场把它打回，所以这里宁可留 O(命中数) 的循环——2 GiB 每轮校验约 4 分钟。
    """
    digest = hashlib.sha256()
    total = 0
    find = spec["find"]
    repl = spec["replace"]
    unit = spec["unit"]
    fast = spec["kind"] == "literal" and unit == 1
    if fast:
        pending = b""
        for block in read_blocks(path, limit):
            buf = pending + block
            if unit == 1:
                out, pending = buf.replace(find, repl), b""
            elif len(buf) < unit:
                pending = buf
                continue
            else:
                decided = len(buf) - unit + 1
                full = buf.replace(find, repl)
                if len(full) != len(buf):
                    raise SystemExit("等长替换前提被破坏，截取前缀的算法不再成立")
                out, pending = full[:decided], buf[decided:]
            digest.update(out)
            total += len(out)
        if pending:
            digest.update(pending)
            total += len(pending)
        return digest.hexdigest(), total
    finder = _finder(spec)
    pending = b""
    for block in read_blocks(path, limit):
        out, pending = _sub_step(pending + block, finder, repl, unit)
        digest.update(out)
        total += len(out)
    out, pending = _sub_step(pending, finder, repl, unit)
    out += pending
    digest.update(out)
    total += len(out)
    return digest.hexdigest(), total


def _finder(spec: dict):
    """逐步提交路径用的「下一个命中起点」：字面串走 `find`，正则走 `search`。"""
    if spec["kind"] == "literal":
        needle = spec["find"]

        def finder(buf: bytes, index: int) -> int:
            return buf.find(needle, index)

        return finder
    compiled = re.compile(spec["find"])

    def finder(buf: bytes, index: int) -> int:
        match = compiled.search(buf, index)
        return -1 if match is None else match.start()

    return finder



def measure_c1(tag: str, rss_limit_mib: int) -> dict:
    d = workdir(tag)
    csv_path = d / "list.csv"
    plan_path = d / "plan.json"
    plan = run([str(BIN), "f10", "--root", str(C1_ROOT), "--output", str(csv_path), "--json"],
               plan_path, rss_limit_mib)
    apply_ = run([str(BIN), "apply", "--plan", str(plan_path), "--quarantine-dir", str(d / "q"),
                  "--journal", str(d / "j.json"), "--json"], d / "apply.out", rss_limit_mib)
    rows = sum(1 for _ in csv_path.open(encoding="utf-8-sig")) - 1
    digest = hashlib.sha256(csv_path.read_bytes()).hexdigest()
    return {"tag": tag, "rows": rows, "csv_bytes": csv_path.stat().st_size, "csv_sha256": digest,
            "plan": plan, "apply": apply_,
            "total_wall_s": round(plan["wall_s"] + apply_["wall_s"], 3)}


def measure_c2(tag: str, split: str, target_bytes: int, target_lines: int, n_parts: int,
               rss_limit_mib: int) -> dict:
    d = workdir(f"c2-{split}-{tag}")
    (d / "parts").mkdir()
    plan_path = d / "plan.json"
    src_bytes = C2_SRC.stat().st_size
    # 三种分片策略的构造参数互不相同：F01 按字节、F02 按行数（需先预扫描行数）、
    # F03 按份数（行数预扫描后均分）。参数只影响数据集构造，不是产品阈值。
    if split == "bytes":
        plan_args = ["f01", "--target-bytes", str(target_bytes)]
    elif split == "lines":
        plan_args = ["f02", "--lines", str(target_lines)]
    else:
        plan_args = ["f03", "--parts", str(n_parts)]
    plan = run([str(BIN), *plan_args, "--file", str(C2_SRC),
                "--output-dir", str(d / "parts"), "--json"], plan_path, rss_limit_mib)
    apply_ = run([str(BIN), "apply", "--plan", str(plan_path), "--quarantine-dir", str(d / "q"),
                  "--journal", str(d / "j.json"), "--json"], d / "apply.out", rss_limit_mib)
    parts = sorted((d / "parts").iterdir(), key=lambda p: p.name)
    part_bytes = sum(p.stat().st_size for p in parts)
    if part_bytes != src_bytes:
        raise SystemExit(f"分片总字节 {part_bytes} ≠ 源 {src_bytes}，样本作废")
    # 产物哈希在计时区外计算（读一遍 2 GiB，不影响 wall_s 口径）
    digest = hashlib.sha256()
    for part in parts:
        with part.open("rb") as fh:
            for block in iter(lambda: fh.read(8 * 1024 * 1024), b""):
                digest.update(block)
    return {"tag": tag, "split": split, "src_bytes": src_bytes, "n_parts": len(parts),
            "part_bytes": part_bytes,
            "part_sizes": [p.stat().st_size for p in parts], "parts_sha256": digest.hexdigest(),
            "plan": plan, "apply": apply_,
            "total_wall_s": round(plan["wall_s"] + apply_["wall_s"], 3),
            "mbps_apply": round(part_bytes / 1048576 / apply_["wall_s"], 1),
            "mbps_total": round(src_bytes / 1048576 / (plan["wall_s"] + apply_["wall_s"]), 1)}


def count_byte(path: Path, char: bytes, limit: int | None = None) -> int:
    return sum(block.count(char) for block in read_blocks(path, limit))


def count_non_ascii(path: Path, limit: int | None = None) -> int:
    return sum(len(block.translate(None, bytes(range(128)))) for block in read_blocks(path, limit))


def c2rw_limit(size_mib: int) -> int | None:
    """None＝整档（数据集多大测多大）；否则取前 size_mib MiB，用作装得下的子档。"""
    if size_mib <= 0:
        return None
    requested = size_mib * 1024 * 1024
    full = C2_SRC.stat().st_size
    return requested if requested < full else None


def measure_c2rw(case: str, tag: str, a) -> dict:
    spec = C2RW_CASES[case]
    d = workdir(f"c2rw-{case}-{tag}")
    limit = c2rw_limit(a.size_mib)
    src = d / spec["name"]
    gb = getattr(a, "gb_multibyte", False)
    GB_META: dict | None = None
    check = "gb18030" if gb else spec["check"]
    if gb and case != "c01":
        raise SystemExit("--gb-multibyte 只对 --case c01 成立（其余臂的期望变换按 ASCII 定义）")
    if gb:
        # 多字节变体：数据集现生成一次后逐轮复用（不入库、不碰 C-2 本体），没有 limit 切片可言。
        limit = None
        gb_src, GB_META = gb_dataset(gb_target_bytes(a.size_mib))
        want_digest, want_bytes = GB_META["want_sha256"], GB_META["want_bytes"]
        with src.open("wb") as sink:
            for block in read_blocks(gb_src, None):
                sink.write(block)
        src_bytes = src.stat().st_size
    else:
        # 就地改写会覆盖源，所以永远在数据集之外做工作副本；子档取的是同一生成器产物的前缀切片。
        with src.open("wb") as sink:
            for block in read_blocks(C2_SRC, limit):
                sink.write(block)
        src_bytes = src.stat().st_size
        want_digest = want_bytes = None

    # 前置断言：期望校验的口径前提必须为真，否则数值毫无意义（样本作废，不出结果行）。
    if check == "identity" and count_non_ascii(C2_SRC, limit):
        raise SystemExit(f"{case} 的逐字节相同期望只对纯 ASCII 源成立，本轮作废")
    if check == "newline" and count_byte(C2_SRC, b"\r", limit):
        raise SystemExit(f"{case} 的 LF→CRLF 期望要求源不含裸 CR，本轮作废")
    if check == "gb18030":
        if not count_non_ascii(src):
            raise SystemExit("多字节臂的源不含非 ASCII 字节＝退回 1:1 子集，本轮作废")
        if want_bytes <= src_bytes:
            raise SystemExit(f"期望产物 {want_bytes} 未大于源 {src_bytes}＝没有真的发生多字节扩展，本轮作废")
    hits = None if gb else (
        count_needle(C2_SRC, spec["find"], limit) if spec.get("kind") == "literal" else None)
    if hits is not None and not hits:
        raise SystemExit(f"{case} 在子档内零命中：计划会整条 skip，测不出吞吐，本轮作废")

    plan_path = d / "plan.json"
    plan = run([str(BIN), *spec["args"](src, a)], plan_path, a.rss_limit_mib, allow_breach=True)
    if plan.get("breached"):
        # 越限本身是结论（哪一段、多大体积、RSS 到没到上限），如实带出而不是当成脚本崩溃。
        if not a.keep_workdir:
            shutil.rmtree(d, ignore_errors=True)
        return {"case": case, "tag": tag, "src_bytes": src_bytes, "limit_bytes": limit,
                "breached": "plan", "plan": plan, "total_wall_s": plan["wall_s"]}
    entry = only_edit_content_entry(plan_path)
    if a.plan_only:
        # 只测计划段：C03 的采集是否整文件读入，用这一段就能定量回答，不必陪着跑几十分钟的改写。
        row = {"case": case, "tag": tag, "src_bytes": src_bytes, "limit_bytes": limit,
               "plan_only": True, "reason": entry["reason"], "plan": plan,
               "total_wall_s": plan["wall_s"]}
        if not a.keep_workdir:
            shutil.rmtree(d, ignore_errors=True)
        return row

    apply_ = run([str(BIN), "apply", "--plan", str(plan_path), "--quarantine-dir", str(d / "q"),
                  "--journal", str(d / "j.json"), "--json"], d / "apply.out", a.rss_limit_mib)
    report = only_applied_report(d / "apply.out")

    # 校验在计时区外做：期望产物由本脚本按同一纪律自行算出，不复用被测实现的输出。
    out_digest, out_bytes = digest_of(src)
    if check == "identity":
        want_digest, want_bytes = digest_of(C2_SRC, limit)
    elif check in ("cells", "gb18030"):
        pass  # cells 无逐字节期望；gb18030 的期望在计时区外已按 Python 编解码器算好
    else:
        want_digest, want_bytes = expected_digest(C2_SRC, spec, limit)
    if check == "gb18030":
        if out_digest != want_digest:
            raise SystemExit(f"{case} 多字节臂产物与期望逐字节不一致：out {out_digest[:16]}… ≠ "
                             f"want {want_digest[:16]}…，样本作废")
        if count_byte(src, b"\xef\xbf\xbd"):
            raise SystemExit(f"{case} 多字节臂产物含 U+FFFD，但生成的源不含非法序列——"
                             "说明解码口径不符，样本作废")
        verify = {"mode": "gb18030", "src_bytes": src_bytes, "out_bytes": out_bytes,
                  "want_bytes": want_bytes, "out_sha256": out_digest, "digest_match": True,
                  "expansion": round(out_bytes / src_bytes, 3),
                  "gb_lines": GB_META["lines"], "gb_four_byte_lines": GB_META["four_byte_lines"],
                  "gb_src_sha256": GB_META["sha256"]}
    elif check == "cells":
        remaining = count_needle(src, spec["find"])
        replaced = count_needle(src, spec["replace"])
        records_out = count_byte(src, b"\n")
        records_src = count_byte(C2_SRC, b"\n", limit)
        # 单元格臂按记录重新序列化，末条无终止符的记录会被补上 LF（＋1 条）——这是已登记的
        # 规范化行为（映射表 TC-C03-EXE-04 的缺口文字），允许并记录，不当成产物不符。
        records_delta = records_out - records_src
        if remaining or replaced != hits or records_delta not in (0, 1):
            raise SystemExit(f"{case} 单元格臂产物不符：残留 {remaining}、替换 {replaced}/{hits}、"
                             f"记录数 {records_out} vs 源 {records_src}，样本作废")
        verify = {"mode": "cells", "src_bytes": src_bytes, "out_bytes": out_bytes,
                  "hits": hits, "out_hits": remaining, "out_replaced": replaced,
                  "records_src": records_src, "records_out": records_out,
                  "records_delta": records_delta}
    else:
        if out_digest != want_digest:
            raise SystemExit(f"{case} 产物与期望逐字节不一致：out {out_digest[:16]}… ≠ "
                             f"want {want_digest[:16]}…，样本作废")
        verify = {"mode": check, "src_bytes": src_bytes, "out_bytes": out_bytes,
                  "want_bytes": want_bytes, "out_sha256": out_digest, "digest_match": True}

    row = {"case": case, "tag": tag, "limit_bytes": limit, "reason": entry["reason"],
           "detail": report.get("detail"), "plan": plan, "apply": apply_,
           "total_wall_s": round(plan["wall_s"] + apply_["wall_s"], 3), **verify}
    row["mbps_plan"] = round(src_bytes / 1048576 / plan["wall_s"], 1)
    row["mbps_apply"] = round(src_bytes / 1048576 / apply_["wall_s"], 1)
    row["mbps_total"] = round(src_bytes / 1048576 / row["total_wall_s"], 1)
    if check == "gb18030":
        # 多字节臂的产物比源大（UTF-8 展开），所以另给一个「产物字节/秒」读数——
        # 只引源口径会把「实际写出量更大」这件事藏掉，两个数一起才读得出代价在哪一侧。
        row["mbps_out_apply"] = round(out_bytes / 1048576 / apply_["wall_s"], 1)
        row["gb_multibyte"] = True
    if not a.keep_workdir:
        shutil.rmtree(d, ignore_errors=True)
    return row


def only_edit_content_entry(plan_path: Path) -> dict:
    """计划必须只有 1 条 edit-content 条目，且既不被阻断也不被跳过——否则本轮什么都没改写。"""
    entries = json.loads(plan_path.read_text(encoding="utf-8"))["entries"]
    if len(entries) != 1:
        raise SystemExit(f"计划条目数 {len(entries)} ≠ 1，样本作废")
    entry = entries[0]
    if entry.get("blocked"):
        raise SystemExit(f"计划条目被阻断：{entry['blocked']}，样本作废")
    if entry.get("resolution"):
        raise SystemExit(f"计划条目被跳过（{entry['resolution']}）：本轮无实际改写，样本作废")
    if entry.get("action") != "edit-content":
        raise SystemExit(f"计划条目动作是 {entry.get('action')}，不是 edit-content，样本作废")
    return entry


def only_applied_report(apply_path: Path) -> dict:
    """执行必须真的写过这一条：`state=completed` 且条目状态是 ok/overwritten。
    skipped/failed 会让「吞吐」变成空转的读数，样本作废（手册 §3 失败协议）。"""
    payload = json.loads(apply_path.read_text(encoding="utf-8"))
    if payload["state"] != "completed":
        raise SystemExit(f"执行终态 {payload['state']} ≠ completed，样本作废")
    entries = payload["report"]["entries"]
    statuses = [item["status"] for item in entries]
    if statuses != ["ok"] and statuses != ["overwritten"]:
        raise SystemExit(f"执行结果 {statuses} 不是写成功，样本作废")
    return entries[0]


def summarise(rows: list[dict]) -> dict:
    def col(name: str) -> list:
        return [r[name] for r in rows]

    def peak(row: dict) -> int:
        stages = [row["plan"]["peak_rss_kib"]]
        if "apply" in row:
            stages.append(row["apply"]["peak_rss_kib"])
        return max(stages)

    med = lambda vals: round(statistics.median(vals), 3)  # noqa: E731
    peaks = [peak(r) for r in rows]
    summary = {
        "n_runs": len(rows),
        "total_wall": col("total_wall_s"),
        "median_total_wall": med(col("total_wall_s")),
        "plan_wall": [r["plan"]["wall_s"] for r in rows],
        "median_plan_wall": med([r["plan"]["wall_s"] for r in rows]),
        "peak_rss_kib": peaks,
        "median_peak_rss_mib": med([peak / 1024 for peak in peaks]),
    }
    if all("apply" in r for r in rows):
        summary["apply_wall"] = [r["apply"]["wall_s"] for r in rows]
        summary["median_apply_wall"] = med([r["apply"]["wall_s"] for r in rows])
    return summary


def main() -> None:
    p = argparse.ArgumentParser(description="C-1／C-2 基准测量（预热 1＋正式 3）")
    p.add_argument("scenario", choices=["c1", "c2", "c2rw"])
    p.add_argument("--split", choices=["bytes", "lines", "parts"], default="bytes",
                   help="C-2 的分片策略：bytes=F01、lines=F02（行数预扫描）、parts=F03（双遍读盘）")
    p.add_argument("--case", choices=sorted(C2RW_CASES),
                   help="c2rw 的执行臂：c01 转码／c02 换行／c03-plain／c03-regex／c03-cell")
    p.add_argument("--size-mib", type=int, default=0,
                   help="c2rw 取 C-2 集前 N MiB 作子档（0＝整档；装不下全量时用它量化体积-内存关系）")
    p.add_argument("--sample-bytes", type=int, default=1048576,
                   help="c02/c03 的采样字节数（构造参数，非产品阈值；默认 1 MiB）")
    p.add_argument("--sample-window", type=int, default=65536,
                   help="c01 的采样窗口字节（构造参数；默认 64 KiB）")
    p.add_argument("--sample-blocks", type=int, default=16,
                   help="c01 的采样块数（构造参数；默认 16，即头部＋16 个等距窗口）")
    p.add_argument("--target-bytes", type=int, default=256 * 1024 * 1024,
                   help="C-2 分片目标字节（构造参数，非产品阈值；默认 256 MiB）")
    p.add_argument("--lines", type=int, default=3_000_000,
                   help="C-2 每片行数（构造参数；默认 300 万行 ≈ 9 片）")
    p.add_argument("--parts", type=int, default=9,
                   help="C-2 份数（构造参数；默认 9 份）")
    p.add_argument("--rss-limit-mib", type=int, default=1024,
                   help="RSS 保险丝上限（MiB，0=关闭）：超过即终止子进程并停测")
    p.add_argument("--keep-workdir", action="store_true",
                   help="c2rw 测后不清工作目录（默认逐轮删除，避免 GiB 级副本堆积）")
    p.add_argument("--plan-only", action="store_true",
                   help="c2rw 只跑到计划段（用来量化计划段的体积-内存关系，不做改写与校验）")
    p.add_argument("--gb-multibyte", action="store_true",
                   help="c2rw --case c01：现生成 GB18030 多字节数据集（60 字节一行、每 50 行含一条"
                        "四字节序列）代替 C-2 的 ASCII 切片。它不属于 T 附录 C 的 C-2 构造，"
                        "所以数值只作方向性参照、不作 §9.1 达标判定；必须配 --size-mib")
    a = p.parse_args()
    if a.scenario == "c2rw" and not a.case:
        p.error("c2rw 需要 --case")
    for path in (BIN, C1_ROOT if a.scenario == "c1" else C2_SRC):
        if not path.exists() and not (a.gb_multibyte and path == C2_SRC):
            raise SystemExit(f"前置缺失：{path}（先 cargo build --release 并生成数据集）")
    if a.scenario == "c1":
        measure = lambda t: measure_c1(t, a.rss_limit_mib)  # noqa: E731
    elif a.scenario == "c2":
        measure = lambda t: measure_c2(t, a.split, a.target_bytes, a.lines, a.parts,  # noqa: E731
                                       a.rss_limit_mib)
    else:
        measure = lambda t: measure_c2rw(a.case, t, a)  # noqa: E731
    rows = [measure("warm")]
    if rows[0].get("breached"):
        # 预热就越限＝这一档在本机跑不动（不是抖动），不再重复三次；把越限行交出台账。
        print(json.dumps(rows[0], ensure_ascii=False), flush=True)
        raise SystemExit(3)
    rows += [measure(f"run{i}") for i in (1, 2, 3)]
    for r in rows:
        print(json.dumps(r, ensure_ascii=False), flush=True)
    summary = {"scenario": a.scenario, "split": a.split, **summarise(rows[1:])}
    if a.scenario == "c2rw":
        summary["case"] = a.case
        summary["size_mib"] = a.size_mib
        summary["plan_only"] = a.plan_only
        summary["gb_multibyte"] = a.gb_multibyte
        if not a.plan_only:
            summary["median_mbps_apply"] = round(
                statistics.median([r["mbps_apply"] for r in rows[1:]]), 1)
            if a.gb_multibyte:
                summary["median_mbps_out_apply"] = round(
                    statistics.median([r["mbps_out_apply"] for r in rows[1:]]), 1)
    print(json.dumps(summary, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
