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

c2rw 一律在 C-2 集的**工作副本**上就地改写（改写会覆盖源，故绝不碰 `out/big_file.txt` 本体），
产物按方案 §9.1 的"改写"语义逐字节校验：校验不过＝样本作废。校验在计时区外做。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
BIN = REPO / "target" / "release" / "batchdesk"
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



def sample_rss(pid: int, holder: dict, stop: threading.Event, limit_kib: int) -> None:
    peak = 0
    samples: list[int] = []
    path = f"/proc/{pid}/status"
    while not stop.is_set():
        try:
            with open(path, encoding="ascii") as fh:
                kib = next((int(line.split()[1]) for line in fh if line.startswith("VmRSS:")), 0)
        except (FileNotFoundError, ProcessLookupError):
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
    d = Path("/tmp") / "batchdesk-bench" / tag
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


def count_needle(path: Path, needle: bytes, limit: int | None = None) -> int:
    """非重叠左到右计数（与 C03 plain 的替换纪律同口径）。

    单字节（如 LF）没有跨块与自叠加问题，逐块 `count` 即可。更长的针要逐匹配推进起点：
    尾巴必须从**上一次命中的末尾**接着扫，否则自叠加模式会把已消费的字节再数一遍
    （"aaa" 里的 "aa" 只算一次）；代价是 O(命中数) 的 Python 循环，所以它只用在子档的前置
    断言与单元格臂校验上，整档只有 unit=1 那条跑得动。
    """
    length = len(needle)
    if length == 1:
        return sum(block.count(needle) for block in read_blocks(path, limit))
    found = 0
    tail = b""
    for block in read_blocks(path, limit):
        buf = tail + block
        index = 0
        while True:
            at = buf.find(needle, index)
            if at < 0:
                break
            found += 1
            index = at + length
        tail = buf[max(index, len(buf) - length + 1):]
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

    unit=1（LF→CRLF）时逐块整段替换即可：单字节匹配没有跨块问题，`bytes.replace` 的左到右
    非重叠语义与内核一致。unit>1 时用 `_sub_step` 的逐步提交算法——它是 O(命中数) 的 Python
    循环，所以 2 GiB 档只有 unit=1 这条跑得动（C03 两臂因此只到子档，见 R-14／R-15 口径）。
    """
    digest = hashlib.sha256()
    total = 0
    find = spec["find"]
    repl = spec["replace"]
    unit = spec["unit"]
    if spec["kind"] == "literal" and unit == 1:
        for block in read_blocks(path, limit):
            out = block.replace(find, repl)
            digest.update(out)
            total += len(out)
        return digest.hexdigest(), total
    if spec["kind"] == "literal":
        def finder(buf: bytes, index: int) -> int:
            return buf.find(find, index)
    else:
        compiled = re.compile(find)

        def finder(buf: bytes, index: int) -> int:
            match = compiled.search(buf, index)
            return -1 if match is None else match.start()
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
    # 就地改写会覆盖源，所以永远在数据集之外做工作副本；子档取的是同一生成器产物的前缀切片。
    with src.open("wb") as sink:
        for block in read_blocks(C2_SRC, limit):
            sink.write(block)
    src_bytes = src.stat().st_size

    # 前置断言：期望校验的口径前提必须为真，否则数值毫无意义（样本作废，不出结果行）。
    if spec["check"] == "identity" and count_non_ascii(C2_SRC, limit):
        raise SystemExit(f"{case} 的逐字节相同期望只对纯 ASCII 源成立，本轮作废")
    if spec["check"] == "newline" and count_byte(C2_SRC, b"\r", limit):
        raise SystemExit(f"{case} 的 LF→CRLF 期望要求源不含裸 CR，本轮作废")
    hits = count_needle(C2_SRC, spec["find"], limit) if spec.get("kind") == "literal" else None
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
    if spec["check"] == "identity":
        want_digest, want_bytes = digest_of(C2_SRC, limit)
    elif spec["check"] == "cells":
        want_digest = want_bytes = None
    else:
        want_digest, want_bytes = expected_digest(C2_SRC, spec, limit)
    if spec["check"] == "cells":
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
        verify = {"mode": spec["check"], "src_bytes": src_bytes, "out_bytes": out_bytes,
                  "want_bytes": want_bytes, "out_sha256": out_digest, "digest_match": True}

    row = {"case": case, "tag": tag, "limit_bytes": limit, "reason": entry["reason"],
           "detail": report.get("detail"), "plan": plan, "apply": apply_,
           "total_wall_s": round(plan["wall_s"] + apply_["wall_s"], 3), **verify}
    row["mbps_plan"] = round(src_bytes / 1048576 / plan["wall_s"], 1)
    row["mbps_apply"] = round(src_bytes / 1048576 / apply_["wall_s"], 1)
    row["mbps_total"] = round(src_bytes / 1048576 / row["total_wall_s"], 1)
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
    a = p.parse_args()
    if a.scenario == "c2rw" and not a.case:
        p.error("c2rw 需要 --case")
    for path in (BIN, C1_ROOT if a.scenario == "c1" else C2_SRC):
        if not path.exists():
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
        if not a.plan_only:
            summary["median_mbps_apply"] = round(
                statistics.median([r["mbps_apply"] for r in rows[1:]]), 1)
    print(json.dumps(summary, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
