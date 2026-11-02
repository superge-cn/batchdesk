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
    python3 fixtures/bench/measure.py c2 --rss-limit-mib 1024
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
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
            try:
                os.kill(pid, 9)
            except ProcessLookupError:
                pass
            return
        time.sleep(0.05)
    holder["peak_kib"] = peak
    holder["samples"] = samples


def run(cmd: list[str], out_path: Path, rss_limit_mib: int) -> dict:
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


def measure_c2(tag: str, target_bytes: int, rss_limit_mib: int) -> dict:
    d = workdir(tag)
    (d / "parts").mkdir()
    plan_path = d / "plan.json"
    src_bytes = C2_SRC.stat().st_size
    plan = run([str(BIN), "f01", "--file", str(C2_SRC), "--target-bytes", str(target_bytes),
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
    return {"tag": tag, "src_bytes": src_bytes, "n_parts": len(parts), "part_bytes": part_bytes,
            "part_sizes": [p.stat().st_size for p in parts], "parts_sha256": digest.hexdigest(),
            "plan": plan, "apply": apply_,
            "total_wall_s": round(plan["wall_s"] + apply_["wall_s"], 3),
            "mbps_apply": round(part_bytes / 1048576 / apply_["wall_s"], 1),
            "mbps_total": round(src_bytes / 1048576 / (plan["wall_s"] + apply_["wall_s"]), 1)}


def summarise(rows: list[dict]) -> dict:
    def col(name: str) -> list:
        return [r[name] for r in rows]

    med = lambda vals: round(statistics.median(vals), 3)  # noqa: E731
    return {
        "n_runs": len(rows),
        "total_wall": col("total_wall_s"),
        "median_total_wall": med(col("total_wall_s")),
        "plan_wall": [r["plan"]["wall_s"] for r in rows],
        "median_plan_wall": med([r["plan"]["wall_s"] for r in rows]),
        "apply_wall": [r["apply"]["wall_s"] for r in rows],
        "median_apply_wall": med([r["apply"]["wall_s"] for r in rows]),
        "peak_rss_kib": [max(r["plan"]["peak_rss_kib"], r["apply"]["peak_rss_kib"]) for r in rows],
        "median_peak_rss_mib": med([max(r["plan"]["peak_rss_kib"], r["apply"]["peak_rss_kib"]) / 1024 for r in rows]),
    }


def main() -> None:
    p = argparse.ArgumentParser(description="C-1／C-2 基准测量（预热 1＋正式 3）")
    p.add_argument("scenario", choices=["c1", "c2"])
    p.add_argument("--target-bytes", type=int, default=256 * 1024 * 1024,
                   help="C-2 分片目标字节（构造参数，非产品阈值；默认 256 MiB）")
    p.add_argument("--rss-limit-mib", type=int, default=1024,
                   help="RSS 保险丝上限（MiB，0=关闭）：超过即终止子进程并停测")
    a = p.parse_args()
    for path in (BIN, C1_ROOT if a.scenario == "c1" else C2_SRC):
        if not path.exists():
            raise SystemExit(f"前置缺失：{path}（先 cargo build --release 并生成数据集）")
    measure = (lambda t: measure_c1(t, a.rss_limit_mib)) if a.scenario == "c1" else \
        (lambda t: measure_c2(t, a.target_bytes, a.rss_limit_mib))
    rows = [measure("warm")] + [measure(f"run{i}") for i in (1, 2, 3)]
    for r in rows:
        print(json.dumps(r, ensure_ascii=False), flush=True)
    print(json.dumps({"scenario": a.scenario, **summarise(rows[1:])}, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
