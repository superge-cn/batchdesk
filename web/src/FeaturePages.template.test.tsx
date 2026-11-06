import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FeaturePages } from "./FeaturePages";
import type { PlanDocument } from "./lib/ipc/types";

// 存为处理模板（方案 §15.2.1）：保存入口在预览处，存的是**这次计划的参数快照与动作**。
// 两条判据不能用"人维护的对照表"顶替：
// ① `kind` 取计划里实际出现的动作（功能码→动作是一对多，正是 OQ-36 的成因）；
// ② 参数快照直接用 `plan.params`，路径类参数由壳层/内核剔除（判据只留一处）。
const splitPlan: PlanDocument = {
  planId: "01PLAN-F05",
  kind: "f05",
  entries: [
    {
      entryId: "e1",
      source: "/data/a.csv",
      target: "/data/out/a-1.csv",
      action: "split",
      reason: "part 1"
    }
  ],
  stats: { total: 1, conflict: 0, warning: 0, blocked: 0 },
  warnings: [],
  params: { file: "/data/a.csv", outputDir: "/data/out", rows: 50000 }
};

const emptyPlan: PlanDocument = { ...splitPlan, entries: [], stats: { ...splitPlan.stats, total: 0 } };

const mixedPlan: PlanDocument = {
  ...splitPlan,
  entries: [
    { ...splitPlan.entries[0], entryId: "e1", action: "move" },
    { ...splitPlan.entries[0], entryId: "e2", action: "copy", target: "/data/out/b" }
  ],
  stats: { total: 2, conflict: 0, warning: 0, blocked: 0 }
};

let nextPlan: PlanDocument = splitPlan;

vi.mock("./lib/ipc", () => ({
  isTauri: () => true,
  // 列宽/排序把手会读界面状态；这些用例不测它，给一个空的稳定桩。
  getUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  setUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  planOperation: vi.fn(async () => nextPlan),
  applyPlan: vi.fn(async () => ({ taskId: "task-apply" })),
  undoPlan: vi.fn(async () => ({ taskId: "task-undo" })),
  cancelTask: vi.fn(async () => undefined),
  retryTask: vi.fn(async () => ({ taskId: "task-retry" })),
  getTask: vi.fn(async () => ({
    summary: { state: "completed", phase: "finalizing", done: 1, total: 1, message: "" },
    entries: []
  })),
  renamePreview: vi.fn(async () => ({
    rows: [],
    counts: { toProcess: 0, skipped: 0, conflicts: 0, blocked: 0 },
    highlights: []
  })),
  undoDryRun: vi.fn(async () => ({ entries: [], conflictCount: 0, missingCount: 0 })),
  saveRecipe: vi.fn(async (input: { name: string }) => ({
    recipeId: "r-new",
    schemaVersion: 3,
    name: input.name,
    steps: [],
    createdAt: "2026-11-05T00:00:00Z"
  })),
  listenTaskProgress: () => Promise.resolve(() => undefined),
  listenTaskState: () => Promise.resolve(() => undefined),
  listenTaskDone: () => Promise.resolve(() => undefined),
  toIpcError: (value: unknown) => value
}));

import { saveRecipe } from "./lib/ipc";

async function plan(page: ReturnType<typeof render>) {
  await userEvent.click(screen.getByRole("button", { name: /生成计划预览/ }));
  await waitFor(() => expect(page.getByTestId("save-template")).toBeInTheDocument());
}

describe("功能页的「存为处理模板」", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    nextPlan = splitPlan;
  });

  it("用计划里实际出现的动作存模板，参数快照取 plan.params", async () => {
    const page = render(<FeaturePages view="f05" onBack={() => undefined} />);
    await plan(page);

    await userEvent.click(page.getByTestId("save-template"));
    await userEvent.type(page.getByLabelText("模板名称"), "CSV 每 5 万行");
    await userEvent.type(page.getByLabelText(/适用扩展名/), ".CSV, txt");
    await userEvent.click(page.getByTestId("save-template-submit"));

    await waitFor(() => expect(saveRecipe).toHaveBeenCalledTimes(1));
    expect(saveRecipe).toHaveBeenCalledWith({
      name: "CSV 每 5 万行",
      description: undefined,
      // 扩展名归一：去掉前导点、转小写（"建议用于"要能按扩展名对上）
      match: { extensions: ["csv", "txt"] },
      // 动作来自计划条目，不来自"功能码→动作"对照表
      kind: "split",
      // OQ-36 裁决①：功能码在保存这一刻就写进模板，事后不必反推。
      // `split` 对应 f01/f02/f03/f05 四个功能——正是"反推不出来"的那一族，
      // 这里存的是 f05，说明绑定的来源是"当前页"而不是猜测。
      feature: "f05",
      // 快照原样交给壳层；路径剔除由内核 `strip_path_params` 与 `saveRecipe` 做（只留一处判据）
      paramsSnapshot: splitPlan.params
    });
    expect(await screen.findByText(/已存为模板「CSV 每 5 万行」/)).toBeVisible();
    // 存完收起表单，不会误以为还能再点一次
    expect(page.queryByTestId("template-form")).toBeNull();
  });

  it("计划含多种动作时拒绝保存，不挑一个把另一部分语义丢掉", async () => {
    nextPlan = mixedPlan;
    const page = render(<FeaturePages view="f05" onBack={() => undefined} />);
    await plan(page);

    await userEvent.click(page.getByTestId("save-template"));
    await userEvent.type(page.getByLabelText("模板名称"), "混排");
    await userEvent.click(page.getByTestId("save-template-submit"));

    expect(await screen.findByText(/含多种动作/)).toBeVisible();
    expect(saveRecipe).not.toHaveBeenCalled();
  });

  it("空计划不给存：没有条目就没有动作可存", async () => {
    nextPlan = emptyPlan;
    const page = render(<FeaturePages view="f05" onBack={() => undefined} />);
    await userEvent.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await waitFor(() => expect(page.getByTestId("save-template")).toBeDisabled());
  });
});
