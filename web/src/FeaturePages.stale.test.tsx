import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FeaturePages } from "./FeaturePages";
import { FEATURE_VIEWS } from "./navCatalog";
import { planOperation } from "./lib/ipc";

// T 配方 P4（改参数后旧预览失效）的**逐功能视图专属面**，23 个视图各一条断言。
//
// 为什么不能只靠通用面：`FeaturePages` 的失效判定是一行共享表达式
//（`JSON.stringify(form) !== plannedForm`），通用用例只证明这行代码会动；
// 它证明不了「每个视图声明的字段都真的绑进了同一个 form」——某个视图的
// FieldSpec 若写了渲染层写不回的字段名，用户改它就不会失效，而固定在
// f12 的通用用例看不见。本用例逐视图渲染、改该视图的第一个参数，把这条
// 接线变成 23 条可命名、可单独失败的断言。
//
// 判定权威：方案 §7.3（入口阻断）；T §3.2 各功能的 PRE-04 行。

const fakePlan = {
  planId: "01PLAN",
  kind: "f12",
  entries: [
    {
      entryId: "e1",
      source: "/dir/a.txt",
      target: "/dir/b.txt",
      reason: "rename",
      blocked: null
    }
  ],
  stats: { blocked: 0, warning: 0 },
  warnings: [],
  params: {}
};

vi.mock("./lib/ipc", () => ({
  isTauri: () => true,
  // 列宽/排序把手会读界面状态；本用例不测它，给一个稳定的空桩。
  getUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  setUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  planOperation: vi.fn(async () => fakePlan),
  applyPlan: vi.fn(async () => ({ taskId: "task-apply" })),
  undoPlan: vi.fn(async () => ({ taskId: "task-undo" })),
  cancelTask: vi.fn(async () => undefined),
  retryTask: vi.fn(async () => ({ taskId: "task-retry" })),
  pauseTask: vi.fn(async () => undefined),
  resumeTask: vi.fn(async () => undefined),
  getTask: vi.fn(async () => ({
    summary: { state: "queued", phase: "queued", done: 0, total: 1 },
    entries: []
  })),
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

describe("逐功能视图的参数变更失效（T 配方 P4 专属面）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("逐视图改参数后旧计划失效：执行/撤销/回滚预演全部关闭", async () => {
    const user = userEvent.setup();
    for (const view of FEATURE_VIEWS) {
      const { unmount } = render(<FeaturePages view={view} onBack={() => undefined} />);

      const grid = document.querySelector(".field-grid");
      expect(grid, `${view}：未渲染参数网格`).not.toBeNull();
      const firstField = grid?.querySelector("input, textarea");
      expect(firstField, `${view}：未渲染任何参数字段`).not.toBeNull();

      // 先生成一次计划：此刻执行入口必须可用（失效只在参数变更后出现）。
      await user.type(firstField as HTMLInputElement, "/mock/x");
      await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
      await screen.findByRole("heading", { name: "计划预览" });
      expect(screen.queryByTestId("plan-stale"), `${view}：刚生成计划就报失效`).toBeNull();
      expect(
        screen.getByRole("button", { name: /^执行$/ }),
        `${view}：刚生成计划执行入口不可用`
      ).toBeEnabled();

      // 改动同一参数 → 旧 planId 不得再被任何执行入口使用。
      await user.type(firstField as HTMLInputElement, "2");
      expect(screen.getByTestId("plan-stale"), `${view}：改参数后未出现失效提示`).toBeVisible();
      expect(
        screen.getByRole("button", { name: /^执行$/ }),
        `${view}：改参数后执行入口未关闭`
      ).toBeDisabled();
      expect(
        screen.getByRole("button", { name: /^撤销$/ }),
        `${view}：改参数后撤销入口未关闭`
      ).toBeDisabled();
      expect(
        screen.getByRole("button", { name: /回滚预演/ }),
        `${view}：改参数后回滚预演入口未关闭`
      ).toBeDisabled();

      unmount();
      vi.clearAllMocks();
    }
  });

  it("重新生成计划后失效提示消失、执行入口恢复", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="f01" onBack={() => undefined} />);
    const field = document.querySelector<HTMLInputElement>(".field-grid input")!;

    await user.type(field, "/mock/x");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("heading", { name: "计划预览" });

    await user.type(field, "2");
    expect(screen.getByTestId("plan-stale")).toBeVisible();

    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("heading", { name: "计划预览" });
    expect(screen.queryByTestId("plan-stale")).toBeNull();
    expect(screen.getByRole("button", { name: /^执行$/ })).toBeEnabled();
    expect(planOperation).toHaveBeenCalledTimes(2);
  });
});
