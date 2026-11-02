#!/usr/bin/env python3
"""生成 docs/testing/用例覆盖映射表-v0.2.csv：TC v0.4 §3 的阶段 0 实例逐条 → 执行载体。

数据来源（均为本会话实测，不凭记忆）：
- 实例矩阵与参数要点：docs/testing/测试用例库-TC-v0.4.md §3（25 行 × 槽位）、§1.2/§1.3（ID 与配方）、§3.1（296 条对账）
- 功能阶段归属：docs/baseline/功能测试方案与验收标准-T-v1.12.md §2.2 与自检表②
- 自动化锚点：cargo test 实测的 apply_flow 测试名、kernel/platform 每模块 #[test] 计数、
  crates/kernel/tests/assertions.tsv 语料状态、src-tauri 壳层测试名
- 语料人工项：fixtures/corpus/manual-only.txt
"""
import csv
from pathlib import Path

ROOT = Path("/home/chaoge/Documents/batchdesk")
OUT = ROOT / "docs/testing/用例覆盖映射表-v0.2.csv"

# 语料台账状态（crates/kernel/tests/assertions.tsv 实测）
CORPUS = {
    "K01": "asserted", "K02": "asserted", "K03": "asserted", "K04": "partial",
    "K05": "partial", "K06": "asserted", "K07": "partial", "K08": "partial",
    "T01": "asserted", "T02": "asserted", "T03": "asserted", "T04": "asserted",
    "T05": "asserted", "T06": "asserted", "T07": "asserted", "T08": "asserted",
    "R01": "asserted", "R02": "asserted", "R03": "asserted", "R04": "asserted",
    "R06": "asserted", "E03": "pending", "Z01": "asserted", "Z02": "asserted",
}
MANUAL = {"R05", "E01", "E02", "E04"}  # fixtures/corpus/manual-only.txt
# c18 横切 harness 覆盖的 plan 命令（apply_flow::c18_plan_dry_run_…）
HARNESS = {"C01", "C04", "C07", "C08", "C09", "C13", "C14", "F01", "F12"}

CI = "CI（cargo test --workspace --all-features）"
SHELL = "CI（cargo test --manifest-path src-tauri/Cargo.toml）"
WEBDRIVER = "WebDriver 待建（功能视图已挂载并由 App.navigation.test.tsx 断言可达；e2e-tauri.mjs 现仅覆盖 C09）"
PW = "CI（pnpm test:e2e，Playwright＋Chromium ＋ 浏览器 mock 夹具；真 WebView 仍走 e2e-tauri.mjs）"
MANUAL_CARRIER = "真人工（Windows 实机/受控环境）"

STATUS_BY_CORPUS = {"asserted": "✅", "partial": "◐", "pending": "⛔"}


def corpus_cell(ids):
    """把语料编号集合映射为（状态, 说明）。全 asserted→✅；含 partial→◐；含 pending/人工→⛔。"""
    states = []
    for cid in ids:
        if cid in MANUAL:
            states.append(("⛔", f"{cid} 属人工语料（manual-only.txt），CI 不执行"))
        else:
            st = CORPUS[cid]
            states.append((STATUS_BY_CORPUS[st], f"{cid} 台账＝{st}"))
    worst = "✅"
    for st, _ in states:
        if st == "⛔":
            worst = "⛔"
        elif st == "◐" and worst == "✅":
            worst = "◐"
    return worst, "；".join(note for _, note in states)


def rows_for(spec):
    fid, meta = spec["id"], spec
    out = []
    pre_recipe = ["P1", "P2", "P3", "P4", "P5", "P6"]
    for index, point in enumerate(meta["pre"], start=1):
        slot = f"PRE-{index:02d}"
        recipe = pre_recipe[min(index - 1, 5)] if meta.get("recipe_pre") is None else meta["recipe_pre"]
        if meta.get("p6"):
            recipe = "P6"
        status, carrier, anchor, gap = meta["pre_slots"].get(
            index,
            ("◐", CI, meta["plan_tests"], "零落盘与幂等未纳入 c18 harness（该功能不在 9 命令清单内）"),
        )
        out.append(row(fid, meta, slot, recipe, point, status, carrier, anchor, gap))
    exe_recipe = ["E1", "E2", "E3", "E4", "E5"]
    for index, point in enumerate(meta["exe"], start=1):
        slot = f"EXE-{index:02d}"
        recipe = "E○1" if meta.get("simple") and index == 1 else (
            "E○2" if meta.get("simple") and index == 2 else exe_recipe[min(index - 1, 4)]
        )
        recipe = recipe_for("EXE", fid, slot, recipe)
        status, carrier, anchor, gap = meta["exe_slots"].get(
            index, ("✅", CI, meta["exe1"], "")
        )
        out.append(row(fid, meta, slot, recipe, point, status, carrier, anchor, gap))
    rbk_recipe = meta.get("rbk_recipes") or ["R1", "R2", "R3"]
    for index, point in enumerate(meta["rbk"], start=1):
        slot = f"RBK-{index:02d}"
        recipe = "R○1" if meta.get("simple") and index == 1 else (
            "R○2" if meta.get("simple") and index == 2 else rbk_recipe[index - 1]
        )
        status, carrier, anchor, gap = meta["rbk_slots"].get(
            index, ("✅", CI, expand_same_as(meta), "")
        )
        out.append(row(fid, meta, slot, recipe, point, status, carrier, anchor, gap))
    return out


# 配方按 TC §3 行的**槽位内容**判定（TC 的列举顺序并不总是与 §2.3 的 E1…E5 对齐，
# 例如 F06/F07/F08/F10/F11/F12/F13/C02 把"取消"放在最后一槽）。此表只列与序号默认值
# （E1/E2/E3/E4/E5）不同的槽位；PRE/RBK 按 §2.2/§2.4 的默认顺序。
EXE_RECIPE = {
    "F03": {3: "E1"},
    "F05": {3: "E2", 4: "E2", 5: "E4"},
    "F06": {4: "E3", 5: "E4"},
    "F07": {4: "E3", 5: "E4"},
    "F08": {2: "E1", 3: "E1", 4: "E2", 5: "E4"},
    "F11": {4: "E3", 5: "E4"},
    "F12": {3: "E2", 4: "E2", 5: "E4"},
    "F13": {2: "E3", 3: "E1", 4: "E3", 5: "E4"},
    "C01": {3: "E2", 4: "E2", 5: "E5"},
    "C02": {3: "E3", 5: "E5"},
    "C03": {2: "E1", 3: "E2", 4: "E2", 5: "E5"},
    "C06": {2: "E2", 3: "E3", 4: "E3", 5: "E5"},
    "C07": {2: "E1", 3: "E2", 4: "E2", 5: "E2"},
    "C09": {3: "E2", 5: "E5"},
    "C10": {3: "E2", 4: "E3", 5: "E4"},
    "C15": {2: "E1", 3: "E4", 5: "E4"},
}

# 功能 → 其 apply 实际走的 generic_apply 执行臂（按 plan 构建器里的 PlanAction 实测：
# csv.rs:616 F05=Split、plan.rs:982 C01=EditContent、line_endings.rs:477／replace_content.rs:461
# =EditContent＋rewriter、plan.rs:758 C09=Delete、archive.rs C10=专用执行器）。
# 值为 None 的臂尚无条目级取消回滚断言，不得据此升级状态。
ARM = {
    "F01": "split", "F02": "split", "F03": "split", "F05": "split",
    "F04": "merge", "F06": "csv-merge", "F07": "csv-extract",
    "F08": "csv-rows", "F09": "csv-range", "F10": "listing", "F11": "create-dir",
    "F12": "rename", "F13": "rename",
    "C01": "edit-content", "C02": "content-rewriter", "C03": "content-rewriter",
    "C07": "copy",
    "C09": "delete",
    "C10": None,   # Archive/Extract 专用执行器；取消只在条目边界由壳层队列覆盖
    "C15": None,   # 队列本体；载体＝壳层测试
    "C18": None,   # 横切预览；载体＝壳层测试
}
# 每个臂的断言所在测试（写进锚点列，便于逐条复核）。
ARM_TEST = {
    "split": "generic_apply::tests::cancel_rolls_back_applied_entries_in_each_write_arm",
    "merge": "generic_apply::tests::cancel_rolls_back_the_batch_and_streaming_write_arms",
    "csv-merge": "generic_apply::tests::cancel_rolls_back_the_batch_and_streaming_write_arms",
    "csv-extract": "generic_apply::tests::cancel_rolls_back_applied_entries_in_each_write_arm",
    "csv-rows": "generic_apply::tests::cancel_rolls_back_the_batch_and_streaming_write_arms",
    "csv-range": "generic_apply::tests::cancel_rolls_back_the_batch_and_streaming_write_arms",
    "listing": "generic_apply::tests::cancel_rolls_back_the_batch_and_streaming_write_arms",
    "create-dir": "generic_apply::tests::cancel_rolls_back_applied_entries_in_each_write_arm",
    "rename": "generic_apply::tests::cancel_mid_batch_rolls_back_applied_entries",
    "edit-content": "generic_apply::tests::cancel_rolls_back_applied_entries_in_each_write_arm",
    "content-rewriter": "generic_apply::tests::cancel_rolls_back_the_batch_and_streaming_write_arms",
    "copy": "generic_apply::tests::cancel_rolls_back_applied_entries_in_each_write_arm",
    "delete": "generic_apply::tests::cancel_rolls_back_applied_entries_in_each_write_arm",
}


def recipe_for(kind, fid, slot, default_recipe):
    if kind == "EXE":
        return EXE_RECIPE.get(fid, {}).get(int(slot.split("-")[1]), default_recipe)
    return default_recipe


def add_arm_anchor(row, fid):
    """把该功能执行臂的条目级取消回滚断言写进锚点列；状态与缺口由逐槽数据决定。"""
    arm = ARM.get(fid)
    if arm is None:
        return
    test = ARM_TEST[arm]
    if test in row["自动化锚点"]:
        return
    row["自动化锚点"] = row["自动化锚点"] + f"＋{test}（{arm} 臂）"


def row(fid, meta, slot, recipe, point, status, carrier, anchor, gap):
    kind, number = slot.split("-")
    if status == "✅" and gap:
        # 「X 台账＝asserted」是信息性注记，挪进锚点列；真实缺口必须把状态降为 ◐，不许两者并存。
        segments = [seg for seg in gap.split("；") if seg]
        ledger = [seg for seg in segments if "台账＝asserted" in seg]
        rest = [seg for seg in segments if seg not in ledger]
        if not rest:
            anchor = anchor + "（语料台账：" + "／".join(ledger) + "）"
            gap = ""
        else:
            raise ValueError(f"TC-{fid}-{kind}-{number}: ✅ 行仍挂真实缺口 {rest}，请改为 ◐")
    built = {
        "功能编号": fid,
        "功能名": meta["name"],
        "适用阶段": meta["stage"],
        "优先级": meta["prio"],
        "实例ID": f"TC-{fid}-{kind}-{number}",
        "类型": kind,
        "序号": number,
        "配方": recipe,
        "参数要点（TC §3 行摘要）": point,
        "专属判定点出处": meta["src"],
        "状态": status,
        "载体": carrier,
        "自动化锚点": anchor,
        "缺口 / 待办": gap,
    }
    if kind == "EXE" and recipe == "E4":
        add_arm_anchor(built, fid)
    return built


def d(pre, exe, rbk, name, stage, prio, src, plan_tests, exe1, rbk1,
      pre_slots=None, exe_slots=None, rbk_slots=None, simple=False, p6=False, rbk_recipes=None):
    return {
        "id": None,
        "name": name, "stage": stage, "prio": prio, "src": src,
        "plan_tests": plan_tests, "exe1": exe1, "rbk1": rbk1,
        "pre": pre, "exe": exe, "rbk": rbk, "rbk_recipes": rbk_recipes,
        "pre_slots": pre_slots or {}, "exe_slots": exe_slots or {}, "rbk_slots": rbk_slots or {},
        "simple": simple, "p6": p6,
    }


def expand_same_as(meta) -> str:
    """rbk1 常写成"同上（…）"——省略式锚点无法机器复核。这里把"同上"就地展开成
    本功能的 EXE-01 锚点（即撤销断言实际所在的那条测试），保留括号里的说明。"""
    rbk1 = meta["rbk1"]
    if not rbk1.startswith("同上"):
        return rbk1
    note = rbk1[len("同上"):].strip()
    exe1 = meta["exe1"]
    return f"{exe1}（撤销断言同条；{note}）" if note else f"{exe1}（撤销断言同条）"


def default_pre_slots(harness, plan_tests, boundary, conflict_gap, expire_gap):
    # 三个槽位参数必须是 (锚点, 缺口) 二元组；写成 (状态, 载体, 锚点, 缺口) 会让锚点列落成"✅"，
    # 执行台账当场报"锚点不可机器定位"。这里提前失败，别把错位带进 CSV。
    for name, pair in (("boundary", boundary), ("conflict_gap", conflict_gap), ("expire_gap", expire_gap)):
        if not (isinstance(pair, tuple) and len(pair) == 2):
            raise ValueError(f"default_pre_slots 的 {name} 需要 (锚点, 缺口) 二元组，实际是 {pair!r}")
    slots = {
        1: ("✅" if harness else "◐", CI, plan_tests,
            "" if harness else "零落盘与重复预览幂等未纳入 c18 harness"),
        2: ("◐" if boundary[1] else "✅", CI, boundary[0], boundary[1]),
        3: ("◐", CI, conflict_gap[0], conflict_gap[1]),
        4: ("◐", CI, expire_gap[0], expire_gap[1]),
    }
    return slots


# ── 逐功能数据（要点取自 TC v0.3 §3 行）────────────────────────────────
FUNCS = []


def add(fid, **kw):
    """kw 里允许 rbk_recipes（TC §3 行的 RBK 槽位数超过 3 时显式标注配方）。"""
    spec = d(**kw)
    spec["id"] = fid
    if fid in HARNESS:
        spec["pre_slots"].setdefault(
            1, ("✅", CI, spec["plan_tests"], "重复预览幂等与源树零落盘由 c18 harness 覆盖")
        )
    FUNCS.append((fid, spec))


add("F01",
    name="TXT 按大小分割", stage="阶段 0", prio="P0★", src="方案 §5.1、§7.1；T §3.2 F01",
    plan_tests="kernel `split` 28 项（SplitAccumulator 行边界/超长行/无换行/空文件）＋platform `text` 19 项",
    exe1="apply_flow::f01_plan_apply_and_undo_write_line_aligned_parts",
    rbk1="同上（reverse=Quarantine，分片产物进隔离目录）",
    pre=["正常输入预览（多行 TXT＋T03 样本）", "目标字节取极小值/临界/典型三档（下限由最长单行决定）",
         "同名分片与批内冲突标注（C19）", "改目标值后旧预览失效"],
    exe=["正常切分：仅在行边界断开、分片可独立打开", "@T03 超长行独占一片并标注；@T06 无换行硬切告警",
         "同名分片按冲突策略执行（默认 sequence 不覆盖）", "执行中暂停/恢复/取消与失败重试",
         "C-2 大文件流式（三档策略不整读）"],
    rbk=["执行成功后撤销分片", "部分失败仅回滚成功项＋保留期窗口边界", "撤销时分片产物被占用/只读"],
    pre_slots=default_pre_slots(
        True, "kernel `split` 28 项；corpus.rs::t03_overlong_line_occupies_its_own_part_and_is_flagged、t05_empty_file_is_reported_without_parts、t06_no_newline_file_is_hard_cut_with_warning",
        ("kernel::split::tests::overlong_line_occupies_its_own_part（目标值小于单行仍独占一片）", ""),
        ("kernel `conflict` 9 项＋apply_flow::f12_reports_existing_conflicts_and_invalid_targets（共用命名冲突引擎）",
         "「未选冲突策略时执行入口阻断」是 UI 交互，待 WebDriver"),
        ("壳层 protected_snapshot_comparison_sees_canonical_changes（保护名单变更→旧预览失效 E1002）",
         "面板级「参数变更→旧预览失效」语义待 UI")),
    exe_slots={
        1: ("✅", CI, "apply_flow::f01_plan_apply_and_undo_write_line_aligned_parts", ""),
        2: (corpus_cell(["T03", "T06"])[0], CI,
            "corpus.rs::t03_overlong_line_occupies_its_own_part_and_is_flagged；t06_no_newline_file_is_hard_cut_with_warning",
            corpus_cell(["T03", "T06"])[1]),
        3: ("◐", CI, "kernel::conflict::tests::sequence_target_skips_occupied_names＋sequence_target_respects_case_insensitive_occupancy", "C19 四策略逐项执行未落（C19 属阶段 1）"),
        4: ("✅", SHELL, "task_state_events_cover_pause_resume_and_queued_cancel；retry_reruns_only_failed_entries_under_a_new_task_id；progress_bursts_are_coalesced_and_flushed_on_phase_change", ""),
        5: ("⛔", MANUAL_CARRIER, "platform `text::split_layout` 流式实现（64 KiB 分块）",
            "C-2 全量档已实测（`fixtures/bench/results.md` R-04…R-06：2 GiB 源峰值 RSS 3.9 MiB、内存恒定达标；吞吐因 SATA SSD 档位不匹配不作判定）；Windows 实机重测未跑"),
    },
    rbk_slots={
        1: ("✅", CI, "apply_flow::f01_plan_apply_and_undo_write_line_aligned_parts", ""),
        2: ("◐", CI, "apply_flow::cleanup_and_undo_respect_retention_window（C09 载体）",
            "本功能的部分失败注入与保留期窗口边界未单独测"),
        3: ("⛔", MANUAL_CARRIER, "kernel `journal` 5 项（reverse 语义）", "E01 占用语料属人工（manual-only.txt）"),
    })

add("F02",
    name="TXT 按行数分割", stage="阶段 0", prio="P0★", src="方案 §5.15、§7.1；T §3.2 F02",
    plan_tests="kernel `split`（SplitBasis::Lines 累加器）＋platform `text::split_layout_by_lines`",
    exe1="apply_flow::f02_plan_apply_and_undo_split_every_n_lines",
    rbk1="同上",
    pre=["正常 TXT 预览", "每 N 行取区间最小/最大/±1", "N 超总行数与同名分片标注", "改 N 后旧预览失效"],
    exe=["正常：每满 N 行一份、尾份保留", "@T06 无换行单片不误报硬切", "尾份与命名冲突策略",
         "执行中暂停/恢复/取消", "C-2 大文件（行数预扫描进度）"],
    rbk=["撤销新增分片", "部分失败回滚", "产物被占用"],
    pre_slots=default_pre_slots(
        False, "kernel `split` 28 项（Lines 基准）",
        ("kernel::split::tests::f02_n_equals_one_puts_each_line_in_its_own_part＋f02_single_part_when_n_exceeds_total_lines", ""),
        ("kernel `conflict` 9 项", "同名分片的 UI 阻断路径待 WebDriver"),
        ("壳层 protected_snapshot_comparison_ignores_order_and_duplicates＋FeaturePages.controls.test.tsx「改参数后旧计划立即失效」", "预览失效在通用面断言（改参数即旧计划失效并禁用执行/撤销），本功能页专属面未单独测")),
    exe_slots={
        2: (corpus_cell(["T06"])[0], CI, "crates/kernel/tests/corpus.rs::t06_no_newline_file_is_hard_cut_with_warning",
            corpus_cell(["T06"])[1]),
        4: ("✅", CI, "壳层 task_state_events_cover_pause_resume_and_queued_cancel＋generic_apply::tests::cancel_rolls_back_applied_entries_in_each_write_arm＋web Vitest：FeaturePages.controls.test.tsx（取消/重试入口调用形状）", ""),
        5: ("⛔", MANUAL_CARRIER, "platform `text::estimate_lines`/`split_layout_by_lines` 流式",
            "行数预扫描进度实测未跑；本轮 C-2 只测 F01（results.md R-04…R-06），F02 档未测"),
    },
    rbk_slots={
        2: ("◐", CI, "generic_apply::tests::a_failing_entry_marks_partial_and_undo_reverts_only_applied_entries（copy 臂：失败条目不落半成品、只有成功条目进 Journal、undo 只回滚已应用）", "本臂的部分失败注入未单独断言；保留期窗口边界仍以 C09 为载体"),
        3: ("⛔", MANUAL_CARRIER, "kernel `journal` 5 项", "E01 占用属人工语料"),
    })

add("F03",
    name="TXT 按份数分割", stage="阶段 0", prio="P0★", src="方案 §5.15、§7.1；T §3.2 F03",
    plan_tests="kernel `split`（part_line_quotas 余数进前几片）＋platform `text::split_layout_by_parts`",
    exe1="apply_flow::f03_plan_apply_and_undo_split_by_part_count",
    rbk1="同上",
    pre=["正常 TXT 预览", "份数 1/临界/±1", "份数超行数的封顶标注", "改份数后旧预览失效"],
    exe=["正常均分：禁止行中断裂", "@T06 无换行", "余数分配与末片并入增长行", "执行中暂停/取消", "超大文件双遍读盘进度"],
    rbk=["撤销分片", "部分失败", "产物占用"],
    pre_slots=default_pre_slots(
        False, "kernel `split` 28 项（Quota 基准）",
        ("kernel::split::tests::part_line_quotas_distributes_remainder_to_first_parts＋quota_accumulator_closes_on_each_quota_and_keeps_tail", ""),
        ("kernel::split::tests::f03_plan_warns_when_parts_exceed_lines＋part_line_quotas_caps_parts_at_line_count", "UI 标注待 WebDriver"),
        ("壳层 protected_snapshot_comparison_*＋FeaturePages.controls.test.tsx「改参数后旧计划立即失效」", "预览失效在通用面断言（改参数即旧计划失效并禁用执行/撤销），本功能页专属面未单独测")),
    exe_slots={
        2: (corpus_cell(["T06"])[0], CI, "crates/kernel/tests/corpus.rs::t06_no_newline_file_is_hard_cut_with_warning",
            corpus_cell(["T06"])[1]),
        4: ("✅", CI, "壳层 task_state_events_cover_pause_resume_and_queued_cancel＋generic_apply::tests::cancel_rolls_back_applied_entries_in_each_write_arm＋web Vitest：FeaturePages.controls.test.tsx（取消/重试入口调用形状）", ""),
        5: ("⛔", MANUAL_CARRIER, "platform `text::split_layout_by_parts`（预扫描＋第二遍流式）",
            "双遍读盘进度实测未跑；本轮 C-2 只测 F01（results.md R-04…R-06），F03 档未测"),
    },
    rbk_slots={
        2: ("◐", CI, "generic_apply::tests::a_failing_entry_marks_partial_and_undo_reverts_only_applied_entries（copy 臂：失败条目不落半成品、只有成功条目进 Journal、undo 只回滚已应用）", "本臂的部分失败注入未单独断言；保留期窗口边界仍以 C09 为载体"),
        3: ("⛔", MANUAL_CARRIER, "kernel `journal` 5 项", "E01 占用属人工语料"),
    })

add("F04",
    name="TXT 合并", stage="阶段 0", prio="P0★", src="方案 §5.15、§5.2；T §3.2 F04",
    plan_tests="kernel `merge` 9 项＋`encoding` 12 项（BOM 策略）",
    exe1="apply_flow::f04_plan_apply_and_undo_merge_with_encoding_unification",
    rbk1="同上（新建产物 reverse=Quarantine／覆盖 WriteAtomic{dataRef}）",
    pre=["多 TXT 正常预览", "目标编码与 BOM 策略组合边界", "同名目标与编码未确认标注", "改编码后旧预览失效"],
    exe=["正常按序合并", "@K03 BOM 只写一次；@K08 UTF-16 转码", "非法编码组合/skip 策略", "执行中暂停/取消", "大文件流式合并"],
    rbk=["撤销合并产物", "部分失败", "产物占用"],
    pre_slots=default_pre_slots(
        False, "kernel `merge` 9 项",
        ("kernel `encoding` 12 项（BOM/置信门控）＋apply_flow::f04_detects_encodings_and_writes_one_bom", ""),
        ("kernel::merge::tests::ambiguous_encoding_blocks_the_merge_with_e3001＋existing_target_is_a_conflict_skipped_by_default", "UI 阻断待 WebDriver"),
        ("generic_apply::tests::merge_skips_when_a_source_size_changed_since_plan＋FeaturePages.controls.test.tsx「改参数后旧计划立即失效」", "预览失效在通用面断言（改参数即旧计划失效并禁用执行/撤销），本功能页专属面未单独测")),
    exe_slots={
        2: (corpus_cell(["K03", "K08"])[0], CI,
            "crates/kernel/tests/corpus.rs::k03_bom_wins_and_target_bom_follows_caller；k08_utf16_bom_sample_detects_and_transcodes_clean",
            corpus_cell(["K03", "K08"])[1] + "；F04 每片/产物 BOM 策略在 F05 侧仍 partial"),
        4: ("✅", SHELL, "壳层 task_state_events_cover_pause_resume_and_queued_cancel＋web Vitest：FeaturePages.controls.test.tsx（取消/重试入口调用形状）", ""),
        # ↑ merge／csv-range 臂的条目级取消回滚由 generic_apply 两测试钉住（配方列已标 E4）
        5: ("⛔", MANUAL_CARRIER, "generic_apply Merge 分支（整文件读入内存）",
            "实现缺口：大文件流式合并未做（内存 O(产物)）；进度仅到单条 entry"),
    },
    rbk_slots={
        2: ("◐", CI, "generic_apply::tests::a_failing_entry_marks_partial_and_undo_reverts_only_applied_entries（copy 臂：失败条目不落半成品、只有成功条目进 Journal、undo 只回滚已应用）", "本臂的部分失败注入未单独断言；保留期窗口边界仍以 C09 为载体"),
        3: ("⛔", MANUAL_CARRIER, "kernel `journal` 5 项", "E01 占用属人工语料"),
    })

add("F05",
    name="CSV 分割", stage="阶段 0", prio="P0★", src="方案 §5.3；T §3.2 F05",
    plan_tests="kernel `csv` 18 项（CsvRecordScanner/CsvSplitAccumulator/sniff_delimiter）＋platform `csv` 6 项",
    exe1="apply_flow::f05_plan_apply_and_undo_repeats_the_csv_header",
    rbk1="同上（copy_ranges_atomic 一次原子写，撤销进隔离目录）",
    pre=["正常 CSV 预览", "分片值/行数典型与临界", "@T02 引号内换行与 @K08 BOM 标注（多行表头不支持）", "改参数后旧预览失效"],
    exe=["正常（按记录边界切分、每片保留表头）", "@T01 引号不闭合按末条记录", "@T04 列数漂移计数上报",
         "@T02 引号内换行不切记录", "执行中暂停/取消"],
    rbk=["撤销分片产物", "部分失败", "产物占用"],
    pre_slots=default_pre_slots(
        False, "kernel `csv` 18 项",
        ("kernel::csv::tests::overlong_record_occupies_its_own_part＋platform `csv` 6 项", ""),
        ("kernel::csv::tests::plan_blocks_utf16_source_and_multi_line_header", "UI 标注与阻断待 WebDriver"),
        ("FeaturePages.controls.test.tsx「改参数后旧计划立即失效」", "预览失效在通用面断言（改参数即旧计划失效并禁用执行/撤销），本功能页专属面未单独测")),
    exe_slots={
        2: (corpus_cell(["T01"])[0], CI, "corpus.rs::t01_unclosed_quote_is_kept_as_last_record_with_warning", corpus_cell(["T01"])[1]),
        3: (corpus_cell(["T04"])[0], CI, "corpus.rs::t04_column_drift_is_counted_without_correction", corpus_cell(["T04"])[1]),
        4: (corpus_cell(["T02"])[0], CI, "corpus.rs::t02_quoted_newlines_do_not_split_records", corpus_cell(["T02"])[1]),
        5: ("✅", SHELL, "壳层 task_state_events_cover_pause_resume_and_queued_cancel＋web Vitest：FeaturePages.controls.test.tsx（取消/重试入口调用形状）", ""),
    },
    rbk_slots={
        2: ("◐", CI, "generic_apply::tests::a_failing_entry_marks_partial_and_undo_reverts_only_applied_entries（copy 臂：失败条目不落半成品、只有成功条目进 Journal、undo 只回滚已应用）", "本臂的部分失败注入未单独断言；保留期窗口边界仍以 C09 为载体"),
        3: ("⛔", MANUAL_CARRIER, "kernel `journal` 5 项", "E01 占用属人工语料"),
    })

add("F06",
    name="CSV 合并", stage="阶段 0", prio="P0★", src="方案 §5.4、11.6-16；T §3.2 F06",
    plan_tests="kernel `csv_merge` 14 项＋platform `csv_merge` 6 项",
    exe1="apply_flow::f06_plan_apply_and_undo_aligns_columns_and_adds_source_column",
    rbk1="同上",
    pre=["多 CSV 正常预览", "列对齐区间：strict 与非 strict", "@T07 无表头、@T08 重复/空白列名标注", "改来源列开关后旧预览失效"],
    exe=["正常：列名对齐缺失补空、额外列追加", "@T07 无表头按列序号对齐", "@T08 表头规范化与重复列名比对",
         "列不一致补空/skip 与 strict 阻断", "执行中暂停/取消"],
    rbk=["撤销合并产物", "部分失败", "产物占用"],
    pre_slots=default_pre_slots(
        False, "kernel `csv_merge` 14 项",
        ("kernel `csv_merge` strict 阻断 E3004 单测＋apply_flow::f06_strict_mode_blocks_a_schema_mismatch", ""),
        ("kernel::csv_merge::tests::plan_no_header_aligns_by_position＋plan_warns_when_headers_need_normalization_or_are_duplicated", "UI 呈现待 WebDriver"),
        ("generic_apply::tests::csv_merge_rejects_a_header_changed_since_plan＋FeaturePages.controls.test.tsx「改参数后旧计划立即失效」", "预览失效在通用面断言（改参数即旧计划失效并禁用执行/撤销），本功能页专属面未单独测")),
    exe_slots={
        2: (corpus_cell(["T07"])[0], CI, "corpus.rs::t07_headerless_sample_aligns_by_column_position＋apply_flow::f06_plan_surfaces_headerless_and_header_normalization_warnings",
            corpus_cell(["T07"])[1]),
        3: (corpus_cell(["T08"])[0], CI, "corpus.rs::t08_duplicate_and_padded_headers_normalize_without_collapsing（表头规范化与重复列名）", corpus_cell(["T08"])[1]),
        4: ("◐", CI, "kernel `generic_apply` 44 项（含冲突二次裁决）", "C19 四策略逐项执行未落（C19 属阶段 1）"),
        5: ("✅", SHELL, "壳层 task_state_events_cover_pause_resume_and_queued_cancel＋web Vitest：FeaturePages.controls.test.tsx（取消/重试入口调用形状）", ""),
        # ↑ csv-merge／csv-rows 臂的条目级取消回滚由 generic_apply 两测试钉住（配方列已标 E4）
    },
    rbk_slots={
        2: ("◐", CI, "generic_apply::tests::a_failing_entry_marks_partial_and_undo_reverts_only_applied_entries（copy 臂：失败条目不落半成品、只有成功条目进 Journal、undo 只回滚已应用）", "本臂的部分失败注入未单独断言；保留期窗口边界仍以 C09 为载体"),
        3: ("⛔", MANUAL_CARRIER, "kernel `journal` 5 项", "E01 占用属人工语料"),
    })

add("F07",
    name="CSV 抽取指定列", stage="阶段 0", prio="P0★", src="方案 §5.15；T §3.2 F07",
    plan_tests="kernel `csv_extract` 12 项（ColumnPick 名称/1-based 序号）",
    exe1="apply_flow::f07_plan_apply_and_undo_extracts_columns_in_order",
    rbk1="同上",
    pre=["按列名/列序正常预览", "列序号 1/最大/越界临界", "@T08 重复列名取首列并告警、列不存在 E3004", "改选择后旧预览失效"],
    exe=["正常导出：按选择顺序、保留原行序", "@T08 重复列名按出现序号", "列不存在/越界报错不写产物",
         "重复输出目标 E1002 与 skip", "执行中暂停/取消"],
    rbk=["撤销导出产物", "部分失败", "产物占用"],
    pre_slots=default_pre_slots(
        False, "kernel `csv_extract` 12 项",
        ("kernel::csv_extract::tests::out_of_range_index_blocks_with_e3004", ""),
        ("kernel `csv_extract` 重复表头取首列告警单测＋apply_flow::f07_missing_column_blocks_with_e3004", "UI 呈现待 WebDriver"),
        ("kernel::generic_apply::tests::csv_extract_rejects_a_header_changed_since_plan＋FeaturePages.controls.test.tsx「改参数后旧计划立即失效」", "预览失效在通用面断言（改参数即旧计划失效并禁用执行/撤销），本功能页专属面未单独测")),
    exe_slots={
        2: (corpus_cell(["T08"])[0], CI, "corpus.rs::t08_duplicate_and_padded_headers_normalize_without_collapsing", corpus_cell(["T08"])[1]),
        4: ("◐", CI, "kernel `conflict` 9 项（existing-target→skip）", "C19 四策略逐项执行未落（C19 属阶段 1）"),
        5: ("✅", CI, "壳层 task_state_events_cover_pause_resume_and_queued_cancel＋generic_apply::tests::cancel_rolls_back_applied_entries_in_each_write_arm＋web Vitest：FeaturePages.controls.test.tsx（取消/重试入口调用形状）", ""),
    },
    rbk_slots={
        2: ("◐", CI, "generic_apply::tests::a_failing_entry_marks_partial_and_undo_reverts_only_applied_entries（copy 臂：失败条目不落半成品、只有成功条目进 Journal、undo 只回滚已应用）", "本臂的部分失败注入未单独断言；保留期窗口边界仍以 C09 为载体"),
        3: ("⛔", MANUAL_CARRIER, "kernel `journal` 5 项", "E01 占用属人工语料"),
    })

add("F08",
    name="CSV 抽取数量行", stage="阶段 0", prio="P0★", src="方案 §5.15；T §3.2 F08",
    plan_tests="kernel `csv_rows` 10 项（前/后 N 窗口、N=0、环形缓冲）",
    exe1="apply_flow::f08_plan_apply_and_undo_takes_the_first_rows",
    rbk1="同上",
    pre=["正常预览", "N=0/1/临界/±1/超总行数", "列数漂移与目标重名标注", "改 N 后旧预览失效"],
    exe=["正常取前 N", "后 N 取尾", "N=0 仅写表头", "@T04 列数漂移原样输出", "执行中暂停/取消"],
    rbk=["撤销产物", "部分失败", "产物占用"],
    pre_slots=default_pre_slots(
        False, "kernel `csv_rows` 10 项",
        ("apply_flow::f08_zero_rows_writes_only_the_header＋kernel count-exceeds-rows 告警单测", ""),
        ("kernel::csv_rows::tests::duplicate_targets_and_mixed_header_modes_are_rejected＋occupied_target_is_marked_as_skip_conflict", "UI 呈现待 WebDriver"),
        ("kernel::generic_apply::tests::csv_rows_rejects_a_header_changed_since_plan＋FeaturePages.controls.test.tsx「改参数后旧计划立即失效」", "预览失效在通用面断言（改参数即旧计划失效并禁用执行/撤销），本功能页专属面未单独测")),
    exe_slots={
        2: ("✅", CI, "apply_flow::f08_last_window_keeps_the_tail", ""),
        3: ("✅", CI, "apply_flow::f08_zero_rows_writes_only_the_header", ""),
        4: (corpus_cell(["T04"])[0], CI, "corpus.rs::t04_column_drift_is_counted_without_correction", corpus_cell(["T04"])[1]),
        5: ("✅", SHELL, "壳层 task_state_events_cover_pause_resume_and_queued_cancel＋web Vitest：FeaturePages.controls.test.tsx（取消/重试入口调用形状）", ""),
        # ↑ csv-merge／csv-rows 臂的条目级取消回滚由 generic_apply 两测试钉住（配方列已标 E4）
    },
    rbk_slots={
        2: ("◐", CI, "generic_apply::tests::a_failing_entry_marks_partial_and_undo_reverts_only_applied_entries（copy 臂：失败条目不落半成品、只有成功条目进 Journal、undo 只回滚已应用）", "本臂的部分失败注入未单独断言；保留期窗口边界仍以 C09 为载体"),
        3: ("⛔", MANUAL_CARRIER, "kernel `journal` 5 项", "E01 占用属人工语料"),
    })

add("F09",
    name="CSV 抽取位置行", stage="阶段 0", prio="P0★", src="方案 §5.15；T §3.2 F09",
    plan_tests="kernel `csv_range` 9 项＋platform `csv_range` 8 项（扫到 Y 即停）",
    exe1="apply_flow::f09_plan_apply_and_undo_extracts_row_range_preserving_crlf",
    rbk1="同上",
    pre=["正常区间预览", "X=Y/Y<X/Y 超总行数/跳前段", "越界与多行表头标注", "改区间后旧预览失效"],
    exe=["正常取 X–Y 且保留源行尾（CRLF）", "@T02 引号内换行按一条记录", "非法区间 E1002/E3004 不写产物",
         "执行中暂停/取消", "跳过前段不整读（read_ranges）"],
    rbk=["撤销产物", "部分失败", "产物占用"],
    pre_slots=default_pre_slots(
        False, "kernel `csv_range` 9 项",
        ("apply_flow::f09_rejects_an_invalid_range＋kernel range-exceeds-rows 告警单测", ""),
        ("kernel::csv_range::tests::encoding_and_multiline_header_block_the_entry", "UI 呈现待 WebDriver"),
        ("FeaturePages.controls.test.tsx「改参数后旧计划立即失效」", "预览失效在通用面断言（改参数即旧计划失效并禁用执行/撤销），本功能页专属面未单独测")),
    exe_slots={
        2: (corpus_cell(["T02"])[0], CI, "corpus.rs::t02_quoted_newlines_do_not_split_records", corpus_cell(["T02"])[1]),
        3: ("✅", CI, "apply_flow::f09_range_beyond_rows_warns_and_clamps_the_output", ""),
        4: ("✅", SHELL, "壳层 task_state_events_cover_pause_resume_and_queued_cancel＋web Vitest：FeaturePages.controls.test.tsx（取消/重试入口调用形状）", ""),
        # ↑ merge／csv-range 臂的条目级取消回滚由 generic_apply 两测试钉住（配方列已标 E4）
        5: ("◐", CI, "FsAdapter::read_ranges 单测＋kernel range apply 断言不调用 read_bytes",
            "选中区间仍读入内存后转码；本轮 C-2 只测 F01，F09 区间档未测（results.md R-04…R-06）"),
    },
    rbk_slots={
        2: ("◐", CI, "generic_apply::tests::a_failing_entry_marks_partial_and_undo_reverts_only_applied_entries（copy 臂：失败条目不落半成品、只有成功条目进 Journal、undo 只回滚已应用）", "本臂的部分失败注入未单独断言；保留期窗口边界仍以 C09 为载体"),
        3: ("⛔", MANUAL_CARRIER, "kernel `journal` 5 项", "E01 占用属人工语料"),
    })

add("F10",
    name="生成目录清单", stage="阶段 0", prio="P0★", src="方案 §5.15、§4.8；T §3.2 F10",
    plan_tests="kernel `listing` 6 项＋platform `listing` 4 项",
    exe1="apply_flow::f10_plan_apply_and_undo_writes_directory_listing",
    rbk1="壳层 listing_plan_streams_csv_through_the_desktop_queue_and_undoes（BOM＋隔离目录）",
    pre=["C-1 目录树预览（摘要计数）", "清单字段组合与层级边界", "空目录/目标在根内/占位文件标注", "改根目录后旧预览失效"],
    exe=["正常导出 UTF-8 BOM CSV", "@E02 占位文件跳过并单列（人工）", "无权限目录跳过计数",
         "执行中取消", "C-1 十万级流式遍历实测"],
    rbk=["删除清单产物", "部分失败", "产物占用"],
    pre_slots=default_pre_slots(
        False, "kernel `listing` 6 项",
        ("platform `listing` 4 项（层级从 1 起、目录大小 0、RFC3339）", ""),
        ("apply_flow::f10_empty_directory_and_target_inside_root_are_reported", "占位文件单列报告属人工（E02）"),
        ("FeaturePages.controls.test.tsx「改参数后旧计划立即失效」", "预览失效在通用面断言（改参数即旧计划失效并禁用执行/撤销），本功能页专属面未单独测")),
    exe_slots={
        2: ("⛔", MANUAL_CARRIER, "kernel `listing` 占位跳过计数已实现", "E02 属人工语料（manual-only.txt）→ Windows 实机日"),
        3: ("◐", CI, "platform::listing::tests::inspect_counts_files_dirs_and_depth＋kernel::listing::tests::placeholder_and_skipped_entries_warn", "真无权限环境断言未跑"),
        4: ("✅", SHELL, "task_state_events_cover_pause_resume_and_queued_cancel", ""),
        5: ("⛔", MANUAL_CARRIER, "FsDirectoryLister 流式写出（write_atomic_with）", "C-1 十万级 Linux 预跑已测（results.md R-01…R-03：合计中位 1.588 秒，§9.1「≤30 秒」达标）；Windows 实机重测未跑"),
    },
    rbk_slots={
        1: ("✅", SHELL, "listing_plan_streams_csv_through_the_desktop_queue_and_undoes", ""),
        2: ("◐", CI, "apply_flow::f10_plan_apply_and_undo_writes_directory_listing", "部分失败注入未测"),
        3: ("⛔", MANUAL_CARRIER, "kernel `journal` 5 项", "E01 占用属人工语料"),
    })

add("F11",
    name="按清单建文件夹", stage="阶段 0", prio="P0★", src="方案 §5.15、§5.8；T §3.2 F11",
    plan_tests="kernel `create_dirs` 11 项",
    exe1="apply_flow::f11_plan_apply_and_undo_creates_multi_level_directories",
    rbk1="同上（Journal CreateDir/RemoveDir，撤销只删空目录）",
    pre=["正常清单预览", "多级路径与无效行边界", "绝对路径/../非法名/保护名单集中报告", "目标已存在与改清单后失效"],
    exe=["正常建目录（父先子后、多级路径）", "@E04 预检阻断（人工）", "无效行集中报告", "已存在目标 skip", "执行中取消"],
    rbk=["删除新建目录", "部分失败（非空目录不递归删）", "目标被占用"],
    pre_slots={**default_pre_slots(
        False, "kernel `create_dirs` 11 项",
        ("kernel::create_dirs::tests::duplicate_rows_and_invalid_rows_are_reported", ""),
        ("apply_flow::f11_accepts_the_f10_listing_format_and_reports_invalid_rows", ""),
        ("apply_flow::f11_existing_target_is_skipped＋FeaturePages.controls.test.tsx「改参数后旧计划立即失效」", "预览失效在通用面断言（改参数即旧计划失效并禁用执行/撤销），本功能页专属面未单独测")),
        # 第 3 槽含"父路径为文件"面：祖先位置被现存文件占据的分支在计划期即 skip＋集中报告。
        3: ("✅", CI,
            "apply_flow::f11_accepts_the_f10_listing_format_and_reports_invalid_rows"
            "＋apply_flow::f11_skips_branches_whose_parent_position_is_an_existing_file"
            "＋kernel `create_dirs` 的 parent-is-file 三条单测（含无该事实时不报的对照断言）", "")},
    exe_slots={
        2: ("⛔", MANUAL_CARRIER, "kernel `preflight` 9 项（磁盘/权限判定）", "E04 属人工语料（manual-only.txt）"),
        4: ("✅", CI, "apply_flow::f11_existing_target_is_skipped", ""),
        5: ("✅", SHELL, "task_state_events_cover_pause_resume_and_queued_cancel", ""),
    },
    rbk_slots={
        2: ("✅", CI, "apply_flow::f11_undo_keeps_a_directory_that_became_non_empty_and_reports_the_failure＋apply_flow::f11_plan_apply_and_undo_creates_multi_level_directories", ""),
        3: ("⛔", MANUAL_CARRIER, "platform `fs` remove_dir_if_empty 单测", "E01 占用属人工语料"),
    })

add("F12",
    name="批量重命名", stage="阶段 0", prio="P0★", src="方案 §5.13；T §3.2 F12",
    plan_tests="kernel `rename` 10 项（冲突扫描/拓扑序/两阶段/长路径三级）＋`conflict` 9 项",
    exe1="apply_flow::f12_prefix_plan_apply_and_undo_renames_files_and_directories",
    rbk1="同上（Journal 逆改名）",
    pre=["正常改名预览（前后缀/字面替换/scope）", "scope full/stem/ext 边界（含 @R06 前缀类样本）", "@R03 保留名、@R04 尾部空格/点、重名与大小写冲突标注", "改规则后旧预览失效"],
    exe=["正常改名：无环拓扑序、有环两阶段", "@R02 大小写折叠恢复（实机）", "@R01 长路径分级 200/259/260",
         "@E03 只读目标", "执行中暂停/取消"],
    rbk=["映射反查恢复", "部分失败", "目标被占用"],
    pre_slots=default_pre_slots(
        True, "kernel `rename` 10 项＋corpus.rs R03/R04/R06 断言",
        ("kernel::rename::tests::scope_stem_and_ext_are_applied_separately", ""),
        ("corpus.rs::r03_reserved_names_are_blocked_in_preview、r04_trailing_space_and_dot_targets_are_blocked_in_preview、apply_flow::f12_reports_existing_conflicts_and_invalid_targets",
         "「未选策略执行入口阻断」待 UI；序号/模板规则属 F13/阶段 1"),
        ("stale-plan.spec.ts「F12 功能页：改前缀后执行/撤销/回滚预演全部关闭」＋FeaturePages.controls.test.tsx「改参数后旧计划立即失效」", "预览失效在通用面断言（改参数即旧计划失效并禁用执行/撤销），本功能页专属面未单独测")),
    exe_slots={
        2: ("◐", CI, "corpus.rs::r02_case_only_targets_conflict_only_on_case_insensitive_hosts（台账 asserted，计划层折叠判定已断言）",
            "文件系统层大小写不敏感行为需 Windows/macOS 实机核对（C06-EXE-02 同源）→ Windows 实机日"),
        3: ("◐", CI, "kernel `rename` 长路径三级（200/259/260×开关）单测＋corpus.rs::r01_long_path_samples_follow_path_length_tiers",
            "UNC canonicalize 行为矩阵待实机（Windows 实机日）"),
        4: ("⛔", MANUAL_CARRIER, "kernel `generic_apply` E2007 占用语义", "E03 只读语料台账＝pending，且占用/只读属实机"),
        5: ("✅", CI, "壳层 task_state_events_cover_pause_resume_and_queued_cancel＋web Vitest：FeaturePages.controls.test.tsx（取消/重试入口调用形状）", ""),
    },
    rbk_slots={
        1: ("✅", CI, "apply_flow::f12_prefix_plan_apply_and_undo_renames_files_and_directories", ""),
        2: ("✅", CI, "apply_flow::c06_undo_dry_run_previews_rollback_and_occupied_targets_are_skipped（同族）", ""),
        3: ("◐", CI, "apply_flow::c06_undo_dry_run_previews_rollback_and_occupied_targets_are_skipped（占用落点判 E2007）", "只读属性场景属实机"),
    })

add("F13",
    name="正则重命名", stage="阶段 0", prio="P0★", src="方案 §5.5、§7.8；T §3.2 F13",
    plan_tests="kernel `regex_rename` 11 项（捕获组/命名组/缺失引用 E1002/病态回溯样本）",
    exe1="apply_flow::f13_regex_capture_plan_apply_and_undo",
    rbk1="同上",
    pre=["捕获组改名预览", "非法正则与空 pattern E1002 边界", "缺失捕获引用/保留名标注", "改模板后旧预览失效"],
    exe=["正常捕获组改名", "非法正则报错不写", "无匹配不产条目", "两阶段与冲突复用 F12", "执行中取消"],
    rbk=["撤销改名映射", "部分失败", "目标占用"],
    pre_slots=default_pre_slots(
        False, "kernel `regex_rename` 11 项",
        ("apply_flow::f13_rejects_invalid_regex_and_missing_capture_references", ""),
        ("kernel::regex_rename::tests::reserved_target_names_are_blocked＋missing_capture_reference_is_rejected", "UI 呈现待 WebDriver"),
        ("FeaturePages.controls.test.tsx「改参数后旧计划立即失效」", "预览失效在通用面断言（改参数即旧计划失效并禁用执行/撤销），本功能页专属面未单独测")),
    exe_slots={
        2: ("✅", CI, "apply_flow::f13_rejects_invalid_regex_and_missing_capture_references", ""),
        3: ("✅", CI, "apply_flow::f13_no_match_creates_no_entries_and_writes_nothing", ""),
        4: ("✅", CI, "kernel `rename` 10 项（两阶段编排共用）", ""),
        5: ("✅", CI, "壳层 task_state_events_cover_pause_resume_and_queued_cancel＋web Vitest：FeaturePages.controls.test.tsx（取消/重试入口调用形状）", ""),
    },
    rbk_slots={
        2: ("◐", CI, "apply_flow::c06_undo_dry_run_previews_rollback_and_occupied_targets_are_skipped（同族载体）", "本功能部分失败注入未测"),
        3: ("⛔", MANUAL_CARRIER, "kernel `journal` 5 项", "E01/E03 属人工与实机场景"),
    })

add("C01",
    name="文件编码转换", stage="阶段 0", prio="P0★", src="方案 §5.2；T §3.2 K01–K08",
    plan_tests="kernel `plan_c01`＋`encoding` 12 项（四级流水线、零阈值置信门控）",
    exe1="apply_flow::c01_plan_apply_and_undo_round_trip",
    rbk1="同上（edit-content 备份→dataRef 还原）",
    pre=["K01–K08 抽样预览", "目标编码与 BOM 组合边界", "K04/K05 低置信与 K06 伪装标注", "改目标编码后旧预览失效"],
    exe=["正常转码（含 @K03 BOM 策略；替换字符计数为 0）", "@K01 GB18030 判定与转码", "@K06 魔数排除",
         "@K07 混合编码不承诺修复", "执行中取消／>10 MB 流式"],
    rbk=["撤销转码还原", "部分失败", "产物占用"],
    pre_slots=default_pre_slots(
        True, "kernel `encoding` 12 项＋plan_c01 7 项",
        ("kernel::encoding::tests::bom_detection_orders_utf32_before_utf16", ""),
        ("corpus.rs::k04_and_k06_are_never_silently_converted、k05_ascii_sample_defaults_to_utf8_with_hint、k07_mixed_encoding_is_flagged_not_repaired", "低置信候选预览（§5.2）UI 未接"),
        ("FeaturePages.controls.test.tsx「改参数后旧计划立即失效」", "预览失效在通用面断言（改参数即旧计划失效并禁用执行/撤销），本功能页专属面未单独测")),
    exe_slots={
        2: (corpus_cell(["K01"])[0], CI, "corpus.rs::k01_gbk_sample_detects_as_gb18030_and_transcodes_clean", corpus_cell(["K01"])[1]),
        3: (corpus_cell(["K06"])[0], CI, "corpus.rs::k06_binary_magic_is_rejected_by_c02_plan", corpus_cell(["K06"])[1]),
        4: (corpus_cell(["K07"])[0], CI, "corpus.rs::k07_mixed_encoding_is_flagged_not_repaired", corpus_cell(["K07"])[1]),
        5: ("⛔", MANUAL_CARRIER, "generic_apply edit-content 分支（整文件读入）＋generic_apply::tests::cancel_rolls_back_applied_entries_in_each_write_arm（edit-content 臂，覆盖本行「执行中取消」部分）"
            "＋apply_flow::c01_reports_invalid_sequence_replacement_counts_per_entry"
            "＋generic_apply::tests::edit_content_carries_detail_per_entry_and_keeps_it_absent_when_not_produced",
            "实现缺口：>10 MB 流式转码未做（整文件读入内存）；**替换计数上报已做**＝条目级 detail（`apply_flow::c01_reports_invalid_sequence_replacement_counts_per_entry` ＋kernel `generic_apply` 的 detail 用例）；C-2 级实测本轮只覆盖 F01"),
    },
    rbk_slots={
        2: ("◐", CI, "apply_flow::apply_skips_entries_changed_between_plan_and_apply（E2010）", "本功能部分失败注入未测"),
        3: ("⛔", MANUAL_CARRIER, "kernel `journal` 5 项", "E01 占用属人工语料"),
    })

add("C02",
    name="换行符转换", stage="阶段 0", prio="P0★", src="方案 §5.15；T §3.2 C02",
    plan_tests="kernel `line_endings` 14 项＋`binary` 2 项＋platform `line_endings` 3 项",
    exe1="apply_flow::c02_text_line_endings_plan_apply_and_undo",
    rbk1="同上（rewrite_atomic_with 流式＋备份还原）",
    pre=["CRLF/LF/CR 混合预览与计数", "目标换行与结构模式边界", "@K03 BOM、二进制魔数排除标注", "改目标后旧预览失效"],
    exe=["正常统一换行（CRLF/LF/CR 混合输入）", "@T02 CSV 引号内换行不误改", "二进制/UTF-16/编码未确认 blocked E3001", "执行中取消", "大文件流式改写实测"],
    rbk=["撤销还原原字节", "部分失败", "产物占用"],
    pre_slots=default_pre_slots(
        False, "kernel `line_endings` 14 项",
        ("platform::line_endings::tests::counts_mixed_text_newlines＋text_mixed_newlines_are_counted_and_converted", ""),
        ("apply_flow::c02_blocks_binary_files_and_skips_already_target_files＋corpus K06 asserted", "UI 呈现待 WebDriver"),
        ("FeaturePages.controls.test.tsx「改参数后旧计划立即失效」", "预览失效在通用面断言（改参数即旧计划失效并禁用执行/撤销），本功能页专属面未单独测")),
    exe_slots={
        2: (corpus_cell(["T02"])[0], CI, "apply_flow::c02_csv_keeps_quoted_newlines_when_converting_records",
            corpus_cell(["T02"])[1]),
        3: ("✅", CI, "kernel `binary` 2 项＋plan_c02 UTF-16/未确认 E3001 单测", ""),
        4: ("◐", SHELL, "task_state_events_cover_pause_resume_and_queued_cancel", "条目边界取消已测（content-rewriter 臂）；单条内部 mid-stream 取消未做（流式重写中途不中断）"),
        5: ("⛔", MANUAL_CARRIER, "FsAdapter::rewrite_atomic_with 流式（设计 04 v1.8）", "C-2 级实测未跑（OQ-16）"),
    },
    rbk_slots={
        2: ("◐", CI, "generic_apply::tests::a_failing_entry_marks_partial_and_undo_reverts_only_applied_entries（copy 臂：失败条目不落半成品、只有成功条目进 Journal、undo 只回滚已应用）", "本臂的部分失败注入未单独断言；保留期窗口边界仍以 C09 为载体"),
        3: ("⛔", MANUAL_CARRIER, "kernel `journal` 5 项", "E01 占用属人工语料"),
    })

add("C03",
    name="批量查找替换", stage="阶段 0", prio="P0★", src="方案 §5.14；T §3.2 C03",
    plan_tests="kernel `replace_content` 11 项＋platform `replace_content` 3 项",
    exe1="apply_flow::c03_plain_replace_plan_apply_and_undo",
    rbk1="同上",
    pre=["普通与正则替换预览（匹配数/上下文）", "正则捕获组与空替换边界", "@K06 二进制、未闭合引号 E3004 标注", "改规则后旧预览失效"],
    exe=["正常替换（非重叠）", "正则捕获组替换", "@K06 不进入文本管道", "@T01 CSV 引号内命中的单元格保护", "执行中取消/行区间"],
    rbk=["撤销还原", "部分失败", "产物占用"],
    pre_slots=default_pre_slots(
        False, "kernel `replace_content` 11 项",
        ("kernel::replace_content::tests::empty_replacement_is_a_danger_warning＋plan_skips_no_matches_and_creates_entries_for_matches", ""),
        ("apply_flow::c03_blocks_binary_and_unclosed_csv", "UI 呈现待 WebDriver"),
        ("FeaturePages.controls.test.tsx「改参数后旧计划立即失效」", "预览失效在通用面断言（改参数即旧计划失效并禁用执行/撤销），本功能页专属面未单独测")),
    exe_slots={
        2: ("✅", CI, "apply_flow::c03_regex_capture_replace_plan_apply_and_undo", ""),
        3: (corpus_cell(["K06"])[0], CI, "corpus.rs::K06（C02 接入的魔数管道）", corpus_cell(["K06"])[1]),
        4: ("◐", CI, "apply_flow::c03_csv_cell_mode_preserves_quoted_newlines（T01 台账＝asserted）",
            "CSV 单元格模式会把记录终止符规范化为 LF，源 CRLF 未保留"),
        5: ("⛔", MANUAL_CARRIER, "ReplaceRewriter 整段变换＋generic_apply::tests::cancel_rolls_back_the_batch_and_streaming_write_arms（content-rewriter 臂，覆盖本行「执行中取消」部分）", "实现缺口：流式重叠窗口与行区间未做"),
    },
    rbk_slots={
        2: ("◐", CI, "apply_flow::apply_skips_entries_changed_between_plan_and_apply", "本功能部分失败注入未测"),
        3: ("⛔", MANUAL_CARRIER, "kernel `journal` 5 项", "E01 占用属人工语料"),
    })

add("C07",
    name="清单驱动移动", stage="阶段 0", prio="P0★", src="方案 §5.8；T §3.2 C07",
    plan_tests="kernel `c07` 5 项＋`generic_apply` 跨盘 2 项",
    exe1="apply_flow::c07_move_plan_apply_and_undo_via_manifest",
    rbk1="同上（Journal 记 rename，撤销＝反向移动）",
    pre=["清单驱动预览", "跨盘复制与目标存在边界", "缺失源/自指向/保护名单 E1003/E2001 标注", "@R01 长路径与改清单后失效"],
    exe=["正常移动与复制", "跨盘校验后删源（校验失败绝不删源）", "@R01 长路径分级",
         "@E01 目标占用（人工）", "@R05 断线（人工）"],
    rbk=["撤销含删源恢复", "部分失败", "撤销时占用"],
    pre_slots=default_pre_slots(
        True, "kernel `c07` 5 项",
        ("kernel::c07::tests::missing_sources_skip_or_error_by_policy", ""),
        ("kernel::c07::tests::escaping_or_absolute_segments_are_rejected_with_e1003＋occupied_targets_conflict_and_self_targets_are_blocked", "UI 呈现待 WebDriver"),
        ("kernel `preflight` path_length_level 共用单测＋corpus.rs::r01_long_path_samples_follow_path_length_tiers",
         "UNC 矩阵待实机")),
    exe_slots={
        2: ("✅", CI, "kernel `generic_apply` 跨盘 2 项（复制→verify_move_integrity→删源；故障注入保源清副本）", ""),
        3: (corpus_cell(["R01"])[0], CI, "corpus.rs::r01_long_path_samples_follow_path_length_tiers", corpus_cell(["R01"])[1]),
        4: ("⛔", MANUAL_CARRIER, "kernel `generic_apply` E2007 语义", "E01 属人工语料（manual-only.txt）"),
        5: ("⛔", MANUAL_CARRIER, "无", "R05 断线属人工语料且阶段末执行（fixtures/corpus/manual-results.md）"),
    },
    rbk_slots={
        2: ("◐", CI, "apply_flow::c07_missing_sources_skip_or_error_and_copy_keeps_source", "跨盘移动在 Windows 实机的核对待实机日"),
        3: ("⛔", MANUAL_CARRIER, "kernel `journal` 5 项", "E01 占用属人工语料"),
    })

add("C09",
    name="重复文件查找", stage="阶段 0", prio="P0★", src="方案 §5.10、11.6-20；T §3.2 C09、T05",
    plan_tests="kernel `duplicate` 9 项＋`plan` 21 项（plan_c09/KeepPolicy）",
    exe1="apply_flow::apply_and_undo_restore_files",
    rbk1="同上＋壳层 apply_and_undo_run_through_the_queue_with_events",
    pre=["C-6 重复集预览（大小分组＋两级哈希先预览）", "快速/全量哈希模式与 OQ-28 采样窗口边界",
         "@T05 空文件与保护名单标注", "删除策略（keep=earliest 默认）与改参数后失效"],
    exe=["正常三段式哈希", "@T05 空文件单独分组", "@E02 占位跳过（人工）", "执行中取消（E2E 已测）", "大集（C-1/基准）"],
    rbk=["隔离目录恢复", "部分撤销", "撤销时目标占用"],
    pre_slots=default_pre_slots(
        True, "kernel `duplicate` 9 项＋C-6 assert_c09.py（本地手动）",
        ("kernel `plan` 21 项（快速模式仅出候选、清理前须补全量哈希）", "采样窗口值已确认（2026-11-02＝头尾各 64 KiB 缺省、显式值优先），待方案 §5.10 收编后把注源从 PENDING-DECISIONS 改为基线章节号"),
        ("corpus.rs::t05_empty_file_is_reported_without_parts；kernel S02 保护命中", "E02 占位属人工语料"),
        ("kernel::plan::tests::earliest_keeps_oldest_mtime（OQ-29 待基线收编）＋stale-plan.spec.ts「C09 工作台：改保留策略后执行入口关闭」", "预览失效在通用面断言（改参数即旧计划失效并禁用执行/撤销），本功能页专属面未单独测")),
    exe_slots={
        2: (corpus_cell(["T05"])[0], CI, "corpus.rs::t05_empty_file_is_reported_without_parts", corpus_cell(["T05"])[1]),
        3: ("⛔", MANUAL_CARRIER, "platform `scan` 10 项（占位探测）", "E02 OneDrive 占位属人工 → Windows 实机日"),
        4: ("✅", SHELL, "task_state_events_cover_pause_resume_and_queued_cancel＋scripts/e2e-tauri.mjs（执行中取消回滚）", ""),
        5: ("⛔", MANUAL_CARRIER, "fixtures/bench C-6 的 assert_c09.py（本地手动脚本，不在 CI）", "C-6 断言未进 CI；C-1 大集归入正式基准（OQ-16）→ 受控环境"),
    },
    rbk_slots={
        1: ("✅", CI, "apply_flow::apply_and_undo_restore_files＋壳层队列测试", ""),
        2: ("✅", CI, "apply_flow::cleanup_and_undo_respect_retention_window（保留期窗口内/外）", ""),
        3: ("◐", CI, "壳层 recover_rollback_restores_applied_entries_and_cancels_the_original_task", "占用场景属实机"),
    })

add("C10",
    name="批量压缩/解压", stage="阶段 0", prio="P0★", src="方案 §5.11；T §3.2 Z01、Z02",
    plan_tests="kernel `archive` 9 项＋platform `archive` 3 项",
    exe1="apply_flow::c10_compress_extract_roundtrip_and_self_inclusion",
    rbk1="壳层 zip_compress_plan_executes_through_the_desktop_queue_and_undoes",
    pre=["压缩/解压预览（条目摘要）", "@Z01 GBK 名与限额边界（E3007）", "@Z02 路径穿越条目标注", "改限额后旧预览失效"],
    exe=["正常压缩与解压", "@Z01 名正确显示（bit11/GB18030 回退）", "@Z02 只拦越界并解出安全条目",
         "密码包/密码错误", "取消与逐文件报告"],
    rbk=["撤销清理产物与空目录", "部分失败", "产物占用"],
    pre_slots=default_pre_slots(
        False, "kernel `archive` 9 项",
        ("kernel::archive::tests::plan_blocks_when_limits_are_exceeded", ""),
        ("corpus.rs 的 Z02 断言（apply_flow::c10_z02_rejects_traversal_entries_but_extracts_the_safe_one）", "UI 呈现待 WebDriver"),
        ("FeaturePages.controls.test.tsx「改参数后旧计划立即失效」", "预览失效在通用面断言（改参数即旧计划失效并禁用执行/撤销），本功能页专属面未单独测")),
    exe_slots={
        2: ("◐", CI, "apply_flow::c10_z01_gbk_name_extracts_and_undo_removes_the_product（Z01 台账＝asserted）",
            "GBK 名「写入」侧未实现（压缩时不写 bit11/GB18030 标志）"),
        3: (corpus_cell(["Z02"])[0], CI, "apply_flow::c10_z02_rejects_traversal_entries_but_extracts_the_safe_one",
            corpus_cell(["Z02"])[1]),
        4: ("⛔", MANUAL_CARRIER, "无（zip 加密条目仅上报 encrypted 字段）",
            "实现缺口：密码包/密码错误未做（ADR-0029 登记）"),
        5: ("◐", SHELL, "task_state_events_cover_pause_resume_and_queued_cancel",
            "archive 臂（apply_archive_plan 专用执行器）的条目级取消回滚未断言；逐文件报告导出未做（ADR-0044）"),
    },
    rbk_slots={
        1: ("✅", SHELL, "zip_compress_plan_executes_through_the_desktop_queue_and_undoes", ""),
        2: ("✅", CI, "apply_flow::c10_z01_gbk_name_extracts_and_undo_removes_the_product＋tasks::tests::zip_compress_plan_executes_through_the_desktop_queue_and_undoes", ""),
        3: ("⛔", MANUAL_CARRIER, "kernel `journal` 5 项", "E01 占用属人工语料"),
    })

add("C15",
    name="任务队列与进度", stage="阶段 0", prio="P0★", src="方案 §5.12、§7.5–7.7",
    plan_tests="kernel `generic_apply` 44 项（含并发 6 项：限额 20 万条目 E3007/并发越界 E1002）",
    exe1="壳层 generic_plan_kinds_execute_through_the_desktop_queue",
    rbk1="壳层 apply_and_undo_run_through_the_queue_with_events",
    pre=["多任务入队预览", "并发上限 1–16 与 20 万条目边界", "暂停/取消对预览的影响与队列重排标注", "改并发后旧预览失效"],
    exe=["排队执行", "双层进度（条目级＋总量级）", "暂停/恢复", "取消", "并发可控与介质自适应"],
    rbk=["取消后已完成项按各功能 Journal 回滚", "部分失败重试", "撤销时资源占用"],
    pre_slots=default_pre_slots(
        False, "kernel `apply` 15 项（validate_apply_limits）",
        ("kernel `generic_apply` 并发 6 项（越界 E1002、超限 E3007）", ""),
        ("tasks::tests::task_state_events_cover_pause_resume_and_queued_cancel＋kernel::generic_apply::tests::pooled_same_source_edit_content_is_serialized", "队列 50 待执行的引导排队 UI 未做"),
        ("无（并发变更即时生效于下一批，面板重算待 UI）", "运行中调参「下一批生效」UI 未做")),
    exe_slots={
        1: ("✅", SHELL, "generic_plan_kinds_execute_through_the_desktop_queue", ""),
        2: ("✅", SHELL, "progress_bursts_are_coalesced_and_flushed_on_phase_change（合帧取 §9.1 区间下限）", ""),
        3: ("✅", SHELL, "task_state_events_cover_pause_resume_and_queued_cancel", ""),
        4: ("✅", SHELL, "task_state_events_cover_pause_resume_and_queued_cancel＋scripts/e2e-tauri.mjs", ""),
        5: ("◐", CI, "kernel `generic_apply` 并发 6 项（缺省 1＝串行）", "介质自适应默认值未做；致命错误自动暂停未做"),
    },
    rbk_slots={
        1: ("✅", SHELL, "recover_rollback_restores_applied_entries_and_cancels_the_original_task", ""),
        2: ("◐", SHELL, "retry_reruns_only_failed_entries_under_a_new_task_id＋retry_after_orchestration_failure_requeues_all_entries",
            "E2E 未覆盖重试场景（需可复现失败注入）"),
        3: ("◐", SHELL, "restart_marks_unfinished_tasks_interrupted_and_resume_skips_applied_entries", "占用场景属实机"),
    })

add("C18",
    name="执行前预览/干跑", stage="阶段 0", prio="P0★", src="方案 §7.1、§7.4",
    plan_tests="kernel `preview` 4 项＋apply_flow::c18 harness（9 个 plan 命令×2 遍）",
    exe1="apply_flow::c18_plan_dry_run_is_idempotent_and_side_effect_free_across_features",
    rbk1="apply_flow::c05_preview_reports_counts_labels_and_zero_side_effects（零副作用断言）",
    pre=["干跑零落盘（源树快照前后一致）", "影响范围统计（四类计数）", "差异逐项", "重复预览幂等",
         "参数变更失效", "空/巨输入提示而非异常"],
    exe=["预览→执行一致", "执行中取消"],
    rbk=["干跑零副作用", "失效即阻断"],
    simple=True,
    pre_slots={
        1: ("◐", CI, "apply_flow::c18_plan_dry_run_is_idempotent_and_side_effect_free_across_features",
            "harness 覆盖 c01/c04/c07/c08/c09/c13/c14/f01/f12；其余命令待纳入（守门规则已写入 ADR-0039）"),
        2: ("✅", CI, "kernel `preview` 4 项（§7.4 四类固定计数＋高危汇总）", ""),
        3: ("✅", CI, "kernel::preview::tests::two_phase_entries_collapse_into_one_mapping_row"
            "＋apply_flow::c05_preview_reports_counts_labels_and_zero_side_effects（新旧名对照行逐项）", ""),
        4: ("✅", CI, "apply_flow::c18_plan_dry_run_is_idempotent_and_side_effect_free_across_features（derived_ulid 确定性派生）", ""),
        5: ("✅", PW, "e2e/stale-plan.spec.ts 两条（C09 工作台改保留策略、F12 功能页改前缀）＋Vitest「改参数后旧计划立即失效」＋kernel `plan` E1002 参数校验单测；判定在入口层（方案 §7.3／ADR-0047，计划为不可变产物）", ""),
        6: ("◐", CI, "corpus.rs::t05_empty_file_is_reported_without_parts＋c18 harness 空输入不异常",
            "巨输入（C-1／C-2 级）实测未跑（OQ-16）；harness 现覆盖 9 命令，其余命令待纳入（ADR-0039）"),
    },
    exe_slots={
        1: ("◐", CI, "apply_flow::c18_plan_dry_run_is_idempotent_and_side_effect_free_across_features＋apply_flow::c01_plan_apply_and_undo_round_trip", "「预览与执行结果一致」逐功能比对未做"),
        2: ("✅", SHELL, "task_state_events_cover_pause_resume_and_queued_cancel", ""),
    },
    rbk_slots={
        1: ("✅", CI, "apply_flow::c05_preview_reports_counts_labels_and_zero_side_effects", ""),
        2: ("◐", CI, "platform::oplog::tests::newer_schema_version_is_rejected_read_only＋apply_flow::c18_plan_dry_run_is_idempotent_and_side_effect_free_across_features", "UI 阻断入口待 WebDriver"),
    })

add("C05",
    name="预览与冲突检测", stage="阶段 0", prio="P0★", src="方案 §5.13、§7.4；T §3.2 C05",
    plan_tests="kernel `preview` 4 项（§5.5 对照行标注、§7.4 四类计数）",
    exe1="apply_flow::c05_preview_reports_counts_labels_and_zero_side_effects",
    rbk1="apply_flow::c05_preview_reports_counts_labels_and_zero_side_effects",
    pre=["正常输入触发预览、逐项差异可见、零落盘、可退出", "参数边界（--find 空、scope 非法 E1002）",
         "危险项标注＋未选策略阻断＋不覆盖", "预览失效：改参数后旧预览失效、执行被阻断",
         "冲突矩阵抽样（existing/within-batch/case-only）", "预览刷新"],
    exe=["正常标注链跑通（预览→确认→执行）", "阻断路径零副作用"],
    rbk=["映射可查（Journal/oplog 反查）", "失效后零副作用"],
    simple=True,
    pre_slots={
        1: ("✅", CI, "apply_flow::c05_preview_reports_counts_labels_and_zero_side_effects", ""),
        2: ("◐", CI, "kernel::name_replace::tests::empty_find_is_rejected_in_both_modes＋invalid_regex_and_missing_capture_are_rejected", "缺 CLI 层边界 ±1 断言（已登记小缺口）"),
        3: ("◐", PW, "kernel::preview::tests::conflict_blocked_and_long_path_rows_are_labelled；e2e/rename-preview.spec.ts「无计划时执行入口被禁用」",
            "UI 恒传 onConflict=skip、不提供策略选择入口（C19 属阶段 1），故「未选策略阻断」只能测到无计划禁用态"),
        4: ("✅", PW, "e2e/rename-preview.spec.ts「改参数后重新预览：旧面板被新面板替换」＋e2e/stale-plan.spec.ts「F12 功能页：改前缀后执行/撤销/回滚预演全部关闭」＋Vitest「改参数后旧计划立即失效：提示出现且执行/撤销被禁用」",
            ""),
        5: ("◐", CI, "kernel::conflict::tests::detects_conflicts_with_existing_files＋kernel::rename::tests::existing_target_conflict_is_detected_on_case_insensitive_hosts", "preview 层未逐类断言（可补两条参数化单测）"),
        6: ("✅", PW, "e2e/rename-preview.spec.ts「改参数后重新预览」＋「回滚预演面板呈现零冲突判定」", ""),
    },
    exe_slots={
        1: ("◐", CI, "kernel `preview` 4 项＋CLI c05", "「确认→执行」衔接待 UI/e2e"),
        2: ("✅", CI, "apply_flow::c05_preview_reports_counts_labels_and_zero_side_effects", ""),
    },
    rbk_slots={
        1: ("✅", SHELL, "apply_and_undo_run_through_the_queue_with_events（oplog 与 Journal 反查）", ""),
        2: ("✅", CI, "apply_flow::c05_preview_reports_counts_labels_and_zero_side_effects（预览前后文件系统一致）", ""),
    })

add("C06",
    name="重命名撤销/回滚", stage="阶段 0", prio="P0★", src="方案 §7.8、11.6-22、§5.5",
    plan_tests="kernel `apply` 15 项（undo_dry_run 4 类 reverse 全覆盖）",
    exe1="apply_flow::c06_undo_is_terminal_and_the_plan_can_be_reapplied",
    rbk1="同上",
    pre=["映射可查", "映射缺失提示"],
    exe=["正常撤销", "@R02 大小写折叠恢复（实机）", "部分撤销（仅回滚成功项）", "窗口外撤销被拒", "大批量撤销"],
    rbk=["撤销后再执行", "回滚为终态、不可二次回滚", "撤销失败状态不变", "跨批次互不干扰", "日志一致（Journal/操作日志）"],
    rbk_recipes=["R1", "R2", "R3", "R2", "R3"],
    pre_slots={
        1: ("✅", SHELL, "apply_and_undo_run_through_the_queue_with_events", ""),
        2: ("✅", CI, "apply_flow::c06_unknown_plan_id_reports_missing_mapping（E1001）", ""),
    },
    exe_slots={
        1: ("✅", CI, "apply_flow::f12_prefix_plan_apply_and_undo_renames_files_and_directories", ""),
        2: ("⛔", MANUAL_CARRIER, "corpus.rs::R02 计划层折叠判定 asserted", "文件系统大小写不敏感行为需 Windows/macOS 实机 → Windows 实机日"),
        3: ("✅", CI, "apply_flow::c06_undo_dry_run_previews_rollback_and_occupied_targets_are_skipped", ""),
        4: ("✅", CI, "apply_flow::cleanup_and_undo_respect_retention_window", ""),
        5: ("◐", CI, "kernel `journal` 5 项＋undo 幂等单测", "千级条目规模归 C-1/C-2 基准（OQ-16）"),
    },
    rbk_slots={
        1: ("✅", CI, "apply_flow::c06_undo_is_terminal_and_the_plan_can_be_reapplied", ""),
        2: ("✅", CI, "apply_flow::c06_undo_is_terminal_and_the_plan_can_be_reapplied（回滚为终态，二次撤销 reverted=0）", ""),
        3: ("✅", CI, "apply_flow::c06_undo_dry_run_previews_rollback_and_occupied_targets_are_skipped（占用条目保持原状）", ""),
        4: ("✅", CI, "apply_flow::c06_cross_batch_undos_are_independent", ""),
        5: ("◐", SHELL, "apply_and_undo_run_through_the_queue_with_events（oplog 落库断言）",
            "reverted 状态回写 oplog 的显式断言待补（已登记小缺口）"),
    })

add("S01",
    name="执行前预检", stage="阶段 0", prio="P0", src="方案 §3.3、§4.9、§9.1、11.6-12",
    plan_tests="kernel `preflight` 9 项＋platform `preflight` 7 项",
    exe1="tasks::tests::preflight_passing_plan_then_applies_through_the_queue_and_quarantines_files",
    rbk1="tasks::tests::blocked_preflight_leaves_the_disk_untouched_and_the_plan_stored",
    pre=["预检项齐全、逐项有结论", "磁盘余量按 §9.1 判定式取临界与 ±1", "命中阻断（E2004 零副作用）",
         "环境变化重算", "UNC 不可达降级不阻塞", "报告可读"],
    exe=["预检通过→执行", "预检期间界面可继续操作且结果落地前不派发"],
    rbk=["不通过零副作用", "无「跳过预检」入口"],
    simple=True, p6=True,
    pre_slots={
        1: ("◐", CI, "kernel `preflight` 9 项", "报告呈现待 UI/WebDriver"),
        2: ("✅", CI, "kernel::preflight::tests::disk_pass_at_boundary_and_block_one_byte_below＋temp_peak_is_part_of_the_requirement＋disk_margin_uses_floor_and_ratio", ""),
        3: ("✅", SHELL, "tasks::tests::preflight_gate_blocks_dispatch_with_e2004_when_disk_margin_is_exceeded"
            "＋Vitest「预检阻断时不派发执行（E2004 前移为执行前阻断，零副作用）」（`App.preflight.test.tsx`）", ""),
        4: ("✅", SHELL, "tasks::tests::preflight_gate_is_recomputed_every_call_so_a_changed_environment_flips_the_verdict"
            "（同一计划连跑三次：余量变化翻转结论；把源文件设为只读后下一次出现 E2002 事实项）", ""),
        5: ("◐", SHELL, "tasks::tests::preflight_gate_warns_on_unreachable_unc_without_blocking_the_task"
            "＋platform::preflight::tests::unc_share_root_picks_the_share_not_the_local_root"
            "＋kernel::preflight::tests::unc_timeout_and_unreachable_warn_but_do_not_block",
            "真共享的可达／慢速／断线三态归 Windows 实机日（本轮用不存在的 host 验证降级不阻塞）"),
        6: ("◐", PW, "e2e/preflight-report.spec.ts（报告含三类计数与逐项结论；另断言无跳过预检入口）",
            "文案观感（人话/分层）仍属人工走查（U v1.3）"),
    },
    exe_slots={
        1: ("✅", SHELL, "tasks::tests::preflight_passing_plan_then_applies_through_the_queue_and_quarantines_files", ""),
        2: ("◐", CI, "kernel::preflight 9 项是纯函数判定（不变量 1 保证不写盘）"
            "＋Vitest「预检落地前不派发；期间界面仍可操作；落地瞬间参数已变则拒绝派发」（`App.preflight.test.tsx`）",
            "壳层事实采集（probe_root／stat／security_settings 读取）尚无独立的零写入断言；预检耗时下的观感属人工走查"),
    },
    rbk_slots={
        1: ("✅", SHELL, "tasks::tests::blocked_preflight_leaves_the_disk_untouched_and_the_plan_stored"
            "（源文件不动、隔离目录空、Journal 不创建）", ""),
        2: ("✅", PW, "e2e/preflight-report.spec.ts「界面上不存在跳过预检的入口」（负向断言）", ""),
        # ↑ 负向 UI 断言：真实浏览器里不存在跳过预检的按钮，默认必检
    })

add("S02",
    name="保护目录执行层复核", stage="阶段 0", prio="P0", src="方案 §3.3、§9.2、11.6-9；T v1.12 R-T15",
    plan_tests="kernel `preflight` 保护命中单测＋`generic_apply` 44 项",
    exe1="kernel::generic_apply::tests::protected_target_is_skipped_before_any_change",
    rbk1="同上（阻断零副作用）",
    pre=["默认空名单不误拦", "自定义名单命中（literal）", "折叠/子路径（canonical 命中）",
         "名单变更重算（旧预览失效 E1002）", "UNC/链接路径名单", "二次确认文案"],
    exe=["命中写入尝试被阻断并给提示", "默认名单态写入不误拦"],
    rbk=["阻断零副作用", "名单变更后旧判定失效"],
    simple=True, p6=True,
    pre_slots={
        1: ("✅", CI, "apply_flow::empty_protected_list_blocks_nothing_while_a_hit_blocks_and_writes_nothing"
            "（f01／f04／f10／f12／c01 五类计划：空名单 blocked=0；保护落点后 blocked≥1 且带 E2009）", ""),
        2: ("✅", CI, "kernel::preflight::tests::protected_literal_and_canonical_hits_block_the_entry", ""),
        3: ("✅", CI, "generic_apply::tests::protected_target_is_skipped_before_any_change＋generic_apply::tests::protected_source_is_rechecked_during_execution", ""),
        4: ("◐", SHELL, "protected_snapshot_comparison_ignores_order_and_duplicates、protected_snapshot_comparison_sees_canonical_changes",
            "端到端重算待 e2e（壳层单测已断言比较逻辑）"),
        5: ("◐", CI, "platform::preflight::tests::non_network_paths_are_not_probed＋unc_share_root_picks_the_share_not_the_local_root", "名单×UNC 组合断言未写；真环境归实机日"),
        6: ("◐", PW, "e2e/protection-confirm.spec.ts（对话框出现＋点名被保护落点＋声明这些条目不会执行＋初始焦点落「取消」且危险按钮不带默认焦点）＋Vitest「保护目录命中：执行前必须二次确认」（组件 `web/src/DangerConfirm.tsx`；方案 §3.3/§7.11＋U §3.5）",
            "文案观感（是否人话、分层是否清晰）仍属人工走查（U §7）"),
    },
    exe_slots={
        1: ("✅", CI, "kernel::generic_apply::tests::protected_target_is_skipped_before_any_change＋kernel::generic_apply::tests::protected_source_is_rechecked_during_execution"
            "＋e2e/protection-confirm.spec.ts（危险二次确认对话框）＋Vitest 保护命中用例",
            ""),
        2: ("✅", CI, "apply_flow::empty_protected_list_blocks_nothing_while_a_hit_blocks_and_writes_nothing"
            "（同一用例的 apply 段：空名单真转码落盘；命中保护 → skipped／E2009 且源文件零变更）", ""),
    },
    rbk_slots={
        1: ("✅", CI, "kernel::generic_apply::tests::protected_target_is_skipped_before_any_change", ""),
        2: ("✅", SHELL, "protected_snapshot_comparison_sees_canonical_changes（E1002）", ""),
    })

# ── C17a（4 条 XCT 型）───────────────────────────────────────────────
C17A = [
    ("TC-C17a-XCT-01", "P/XCT", "最小日志落库（§10.2 字段集全落列）", "✅", SHELL,
     "platform::oplog 5 项＋apply_and_undo_run_through_the_queue_with_events 内落库断言", ""),
    ("TC-C17a-XCT-02", "P/XCT", "供 C06 反查（与 Journal 一致）", "◐", SHELL,
     "platform::oplog::tests::schema_migrates_once_and_round_trips_records＋platform::oplog::tests::full_paths_follow_the_log_full_paths_switch", "与 Journal 的逐字段一致性断言待补（已登记小缺口）"),
    ("TC-C17a-XCT-03", "P/XCT", "脱敏开关生效且不影响回滚", "✅", SHELL,
     "platform::oplog::tests::full_paths_follow_the_log_full_paths_switch", ""),
    ("TC-C17a-XCT-04", "P/XCT", "日志不含文件内容、与完整日志分轨", "◐", CI,
     "operation_log schema 无内容列（结构保证）＋design 10 v0.2 分轨", "CLI 侧操作日志未做（OQ-30）"),
]


def main():
    header = ["功能编号", "功能名", "适用阶段", "优先级", "实例ID", "类型", "序号", "配方",
              "参数要点（TC §3 行摘要）", "专属判定点出处", "状态", "载体", "自动化锚点", "缺口 / 待办"]
    rows = []
    for fid, spec in FUNCS:
        rows.extend(rows_for(spec))
    for instance, recipe, point, status, carrier, anchor, gap in C17A:
        rows.append({
            "功能编号": "C17a", "功能名": "最小操作/撤销日志", "适用阶段": "阶段 0", "优先级": "P0",
            "实例ID": instance, "类型": "XCT", "序号": instance[-2:], "配方": "XCT",
            "参数要点（TC §3 行摘要）": point, "专属判定点出处": "方案 §10.2、11.6-13、§7.8；T §2.2 注 3",
            "状态": status, "载体": carrier, "自动化锚点": anchor, "缺口 / 待办": gap,
        })
    with OUT.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=header, lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)
    print(f"rows={len(rows)}  ->  {OUT}")
    stats = {}
    for r in rows:
        stats[r["状态"]] = stats.get(r["状态"], 0) + 1
    print("状态分布：", stats)
    per_func = {}
    for r in rows:
        per_func[r["功能编号"]] = per_func.get(r["功能编号"], 0) + 1
    print("每功能条数：", per_func)


main()
