#!/usr/bin/env python3
"""criterion 基线对比（CI bench job「与基线对比」步调用）。

读取当前 criterion 产物（`<dir>/<bench>/new/estimates.json` 的 `median.point_estimate`，
单位纳秒），与基线逐项比对：

- 基线目录（`--baseline DIR`，与当前产物同布局）：CI 里来自**上一次主分支成功运行**的
  `bench-results` artifact（见 ci.yml 的「取上一次主分支 bench 产物作基线」步）；
- 基线种子（`--baseline FILE.json` 或缺省）：仓库内 `fixtures/bench/criterion-baseline.json`，
  供本地与「尚无更早成功运行」时兜底。种子是快照、会过期，正式口径以 artifact 链为准。

阈值口径：漂移阈值只从 `--threshold-pct` 或环境变量 `BENCH_DRIFT_PCT` 取（CI 注入仓库
变量），**脚本不内置默认值**；两者都未给时进入「仅报告」模式——退出码 0，漂移表照常
打印，步骤名中的「超阈值即失败」暂不生效，直到阈值经产品所有者裁决落进仓库变量。

失败判据（与阈值是否设置无关）：
- 当前产物缺某基准的 `new/estimates.json` 或其中无 `median.point_estimate`（结构损坏）；
- 基线里有而当前产物里没有的基准（基准静默消失）。
新基准（基线里没有）只报告、不判失败。

只使用标准库。

用法：
    python scripts/compare_criterion.py --current target/criterion [--baseline DIR|FILE] \
        [--threshold-pct N]
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

SEED_BASELINE = Path(__file__).resolve().parent.parent / "fixtures" / "bench" / "criterion-baseline.json"


def load_medians_from_dir(root: Path) -> dict[str, float]:
    """从 criterion 产物目录读各基准的 median 点估计（纳秒）。"""
    medians: dict[str, float] = {}
    for estimates in sorted(root.glob("*/new/estimates.json")):
        bench = estimates.parent.parent.name
        try:
            payload = json.loads(estimates.read_text(encoding="utf-8"))
            medians[bench] = float(payload["median"]["point_estimate"])
        except (json.JSONDecodeError, KeyError, TypeError, ValueError) as exc:
            raise SystemExit(f"结构损坏：{estimates} 无法读出 median.point_estimate：{exc}") from exc
    if not medians:
        raise SystemExit(f"结构损坏：{root} 下没有任何 */new/estimates.json")
    return medians


def load_medians_from_seed(path: Path) -> dict[str, float]:
    """从仓库种子 JSON 读各基准的 median 点估计（纳秒）。"""
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
        medians = {str(k): float(v) for k, v in payload["medians"].items()}
    except (json.JSONDecodeError, KeyError, TypeError, ValueError) as exc:
        raise SystemExit(f"结构损坏：种子基线 {path} 无法读出 medians：{exc}") from exc
    if not medians:
        raise SystemExit(f"结构损坏：种子基线 {path} 的 medians 为空")
    return medians


def main() -> int:
    parser = argparse.ArgumentParser(description="criterion 基线对比")
    parser.add_argument("--current", required=True, type=Path, help="当前 criterion 产物目录")
    parser.add_argument(
        "--baseline",
        type=Path,
        default=None,
        help="基线：criterion 产物目录或种子 JSON；缺省用 fixtures/bench/criterion-baseline.json",
    )
    parser.add_argument("--threshold-pct", type=float, default=None, help="漂移阈值（百分比），优先于环境变量")
    args = parser.parse_args()

    threshold = args.threshold_pct
    if threshold is None:
        env = os.environ.get("BENCH_DRIFT_PCT", "").strip()
        if env:
            threshold = float(env)

    baseline_path = args.baseline if args.baseline is not None else SEED_BASELINE
    if baseline_path.is_dir():
        baseline = load_medians_from_dir(baseline_path)
        baseline_source = f"目录 {baseline_path}"
    elif baseline_path.is_file():
        baseline = load_medians_from_seed(baseline_path)
        baseline_source = f"种子 {baseline_path}"
    else:
        raise SystemExit(f"基线不存在：{baseline_path}")

    current = load_medians_from_dir(args.current)

    missing = sorted(set(baseline) - set(current))
    if missing:
        for bench in missing:
            print(f"FAIL {bench}: 基线里有而当前产物没有（基准静默消失）")
        return 1

    print(f"基线来源：{baseline_source}")
    if threshold is None:
        print("阈值：未设（BENCH_DRIFT_PCT 为空）——仅报告模式，漂移不判失败")
    else:
        print(f"阈值：±{threshold:g}%（来自 {'--threshold-pct' if args.threshold_pct is not None else 'BENCH_DRIFT_PCT'}）")
    print()

    failed = False
    width = max(len(b) for b in current)
    for bench in sorted(current):
        cur = current[bench]
        if bench not in baseline:
            print(f"NEW  {bench:<{width}} 当前 {cur:>14.1f} ns  基线无此基准（新增，不判失败）")
            continue
        base = baseline[bench]
        drift = (cur - base) / base * 100.0
        verdict = "OK"
        if threshold is not None and abs(drift) > threshold:
            verdict = "FAIL"
            failed = True
        print(f"{verdict:<4} {bench:<{width}} 基线 {base:>14.1f} ns  当前 {cur:>14.1f} ns  漂移 {drift:+7.2f}%")

    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
