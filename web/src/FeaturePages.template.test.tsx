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

  it("采样口径进模板不进快照（v5，OQ-41）：表单当前值存进 sampling，快照保持计划形状", async () => {
    // c01 的路由必填 sampleWindow 但计划参数从不携带——保存时把表单当前值
    // 采集进 sampling 块；paramsSnapshot 仍是 plan.params（不含采样键，ADR-0072 不变）。
    nextPlan = {
      ...splitPlan,
      kind: "c01",
      params: { target: "utf-8", invalid: "replace-and-count", platformPrior: "other" },
      entries: [{ ...splitPlan.entries[0], action: "transcode", reason: "transcode" }]
    };
    const page = render(<FeaturePages view="c01" onBack={() => undefined} />);
    await userEvent.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await waitFor(() => expect(page.getByTestId("save-template")).toBeInTheDocument());

    await userEvent.click(page.getByTestId("save-template"));
    await userEvent.type(page.getByLabelText("模板名称"), "转码模板");
    // 表单里把采样窗口改成 65536：模板该存的就是这个值，而不是缺省
    const windowInput = page.getByLabelText(/采样窗口/);
    await userEvent.clear(windowInput);
    await userEvent.type(windowInput, "65536");
    await userEvent.click(page.getByTestId("save-template-submit"));

    await waitFor(() => expect(saveRecipe).toHaveBeenCalledTimes(1));
    const call = vi.mocked(saveRecipe).mock.calls[0][0];
    expect(call.sampling).toEqual({ sampleWindow: 65536 });
    // 快照＝计划自带那份，采样键不在里面
    expect(call.paramsSnapshot).toEqual(nextPlan.params);
    expect(call.paramsSnapshot).not.toHaveProperty("sampleWindow");
  });

  it("变换正文进模板不进快照（v6，OQ-43②）：规则按行拆数组、递归开关落布尔", async () => {
    // c08 的归档规则从不进 plan.params（计划只存 ruleCount）——保存时从表单
    // 采集进 content；paramsSnapshot 仍是计划形状。
    nextPlan = {
      ...splitPlan,
      kind: "c08",
      params: { ruleCount: 2, fileCount: 3, unmatchedKept: 0 },
      entries: [{ ...splitPlan.entries[0], action: "move", reason: "archived" }]
    };
    const page = render(<FeaturePages view="c08" onBack={() => undefined} />);
    await userEvent.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await waitFor(() => expect(page.getByTestId("save-template")).toBeInTheDocument());

    await userEvent.click(page.getByTestId("save-template"));
    await userEvent.type(page.getByLabelText("模板名称"), "归档模板");
    const rulesBox = page.getByLabelText(/规则链/);
    await userEvent.clear(rulesBox);
    await userEvent.paste("images|ext:jpg,png\nlogs|name:log");
    await userEvent.type(page.getByLabelText(/递归 yes/), "yes");
    await userEvent.click(page.getByTestId("save-template-submit"));

    await waitFor(() => expect(saveRecipe).toHaveBeenCalledTimes(1));
    const call = vi.mocked(saveRecipe).mock.calls[0][0];
    expect(call.content).toEqual({
      rules: ["images|ext:jpg,png", "logs|name:log"],
      recursive: true
    });
    expect(call.paramsSnapshot).toEqual(nextPlan.params);
    expect(call.paramsSnapshot).not.toHaveProperty("rules");
  });
});
