import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FeaturePages } from "./FeaturePages";
import { FEATURE_VIEWS } from "./navCatalog";
import type { FeatureView } from "./navCatalog";
import type { PlanDocument, PlanWarning } from "./lib/ipc/types";
import { planOperation } from "./lib/ipc";

// 计划级「集中报告」的**逐功能视图专属面**（T §3.2 各功能 PRE-03 的"UI 呈现/阻断"）。
//
// 为什么不能只靠通用面：`.warnings` 块由 `plan.warnings.map(...)` 统一渲染，
// 但**每个视图的计划形状与 blocked 语义不同**（f05 的多行表头是 blocked、
// f06 的无表头是 warning、c07 的保护命中同时进 blocked 与 warnings）。
// 只在一个视图上断言，证明不了其余视图的计划负载也能走到这个块。
// 本用例逐视图喂入该功能真实会产出的类别，把"类别 → 界面标题"这条链
// 变成每个视图各一条可单独失败的断言。
//
// 判定权威：方案 §5.x（各功能的标注/阻断）、设计 06 §2A（类别清单）；
// 类别 ↔ 标题映射本身由 `scripts/check_locales.py` 第 ⑪ 项单独门禁。

/** 各功能真实会产出的预警类别（取自 kernel 各 plan_* 的 warning kind）。 */
const VIEW_WARNING: Record<FeatureView, string> = {
  f01: "overlong-line-single-part",
  f02: "parts-exceed-lines",
  f03: "parts-exceed-lines",
  f04: "mixed-source-encodings",
  f05: "delimiter-not-detected",
  f06: "no-header-positional",
  f07: "duplicate-column-name",
  f08: "column-drift",
  f09: "range-exceeds-rows",
  f10: "empty-directory",
  f11: "parent-is-file",
  f12: "invalid-target-names",
  c01: "incomplete-hash",
  c02: "mixed-line-endings",
  c03: "csv-text-mode",
  c04: "invalid-target-names",
  c07: "protected-target-skipped",
  c08: "rule-hits",
  c10: "unsafe-entry-skipped",
  c11: "duplicate-rows",
  c12: "no-matching-rows",
  c13: "non-numeric-cells",
  c14: "schema-extra-columns"
};

/** 各功能会以「阻断条目」呈现的码（PRE-03 的"阻断"半边）。 */
const VIEW_BLOCKED: Partial<Record<FeatureView, string>> = {
  f02: "E2007",
  f04: "E2007",
  f05: "E3004",
  f07: "E3004",
  c02: "E3001",
  c03: "E3004",
  c07: "E1003",
  c10: "E2009"
};

function planWith(view: FeatureView, warning: PlanWarning, blockedCode?: string): PlanDocument {
  const entries = [
    {
      entryId: `${view}-e1`,
      source: "/mock/a",
      target: "/mock/b",
      action: "rename" as const,
      reason: "ok row",
      blocked: undefined as undefined | { code: string; reason: string; hint?: string }
    }
  ];
  if (blockedCode) {
    entries.push({
      entryId: `${view}-e2`,
      source: "/mock/bad",
      target: "/mock/bad2",
      action: "rename" as const,
      reason: "blocked row",
      blocked: { code: blockedCode, reason: "mock blocked reason" }
    });
  }
  return {
    planId: `01PLAN-${view}`,
    kind: view,
    entries,
    stats: {
      total: entries.length,
      conflict: 0,
      warning: 1,
      blocked: blockedCode ? 1 : 0
    },
    warnings: [warning],
    params: {}
  };
}

vi.mock("./lib/ipc", () => ({
  isTauri: () => true,
  getUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  setUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  planOperation: vi.fn(async () => ({ planId: "01PLAN", kind: "f12", entries: [], stats: {}, warnings: [], params: {} })),
  applyPlan: vi.fn(async () => ({ taskId: "task-apply" })),
  undoPlan: vi.fn(async () => ({ taskId: "task-undo" })),
  cancelTask: vi.fn(async () => undefined),
  retryTask: vi.fn(async () => ({ taskId: "task-retry" })),
  pauseTask: vi.fn(async () => undefined),
  resumeTask: vi.fn(async () => undefined),
  getTask: vi.fn(async () => null),
  renamePreview: vi.fn(async () => ({
    rows: [],
    counts: { toProcess: 0, skipped: 0, conflicts: 0, blocked: 0 },
    highlights: []
  })),
  undoDryRun: vi.fn(async () => ({ entries: [], conflictCount: 0, missingCount: 0 })),
  listenTaskProgress: vi.fn(async () => () => undefined),
  listenTaskState: vi.fn(async () => () => undefined),
  listenTaskDone: vi.fn(async () => () => undefined),
  toIpcError: (value: unknown) => value
}));

describe("逐功能视图的计划警告与阻断呈现（T §3.2 PRE-03 专属面）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("逐视图渲染集中报告：标题走本地化、不漏原始 key、建议随附", async () => {
    const user = userEvent.setup();
    for (const view of FEATURE_VIEWS) {
      vi.mocked(planOperation).mockResolvedValueOnce(
        planWith(view, {
          kind: VIEW_WARNING[view],
          riskLevel: "caution",
          suggestion: "mock suggestion"
        })
      );
      const { unmount } = render(<FeaturePages view={view} onBack={() => undefined} />);

      const grid = document.querySelector(".field-grid");
      expect(grid, `${view}：未渲染参数网格`).not.toBeNull();
      const firstField = grid?.querySelector("input, textarea");
      expect(firstField, `${view}：未渲染任何参数字段`).not.toBeNull();
      await user.type(firstField as HTMLInputElement, "/mock/x");
      await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
      await screen.findByRole("heading", { name: "计划预览" });

      const block = document.querySelector(".warnings");
      expect(block, `${view}：有计划警告却没有集中报告块`).not.toBeNull();
      const title = block!.querySelector("strong");
      expect(title?.textContent, `${view}：集中报告标题为空`).toBeTruthy();
      // 漏 key 会把 "warning.xxx.title" 直接摆上界面（09 §2 不允许）
      expect(title?.textContent ?? "", `${view}：标题漏出资源 key`).not.toContain("warning.");
      // suggestion 必须随附，否则用户只看到类别不知道是哪几行
      expect(block!.textContent, `${view}：集中报告未随附建议明细`).toContain("mock suggestion");

      unmount();
      vi.clearAllMocks();
    }
  });

  it("逐视图呈现阻断条目：码与落点同时可见", async () => {
    const user = userEvent.setup();
    const views = FEATURE_VIEWS.filter((view) => VIEW_BLOCKED[view]);
    expect(views.length).toBeGreaterThan(0);
    for (const view of views) {
      const code = VIEW_BLOCKED[view]!;
      vi.mocked(planOperation).mockResolvedValueOnce(
        planWith(
          view,
          { kind: VIEW_WARNING[view], riskLevel: "caution", suggestion: "mock suggestion" },
          code
        )
      );
      const { unmount } = render(<FeaturePages view={view} onBack={() => undefined} />);

      const grid = document.querySelector(".field-grid");
      const firstField = grid?.querySelector("input, textarea");
      await user.type(firstField as HTMLInputElement, "/mock/x");
      await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
      await screen.findByRole("heading", { name: "计划预览" });

      // 阻断条目的码要出现在计划表里（用户看得到"为什么这行不会执行"）
      expect(
        document.body.textContent ?? "",
        `${view}：阻断条目 ${code} 未呈现在界面上`
      ).toContain(code);

      unmount();
      vi.clearAllMocks();
    }
  });
});
