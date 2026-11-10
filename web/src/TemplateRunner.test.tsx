import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { TemplateRunner } from "./TemplateRunner";
import type {
  PlanDocument,
  RecipeApplySpec,
  RecipeDocument,
  TaskDetail
} from "./lib/ipc/types";

// 多步执行器（C37 Phase B，方案 §15.2.3）：按步 plan/apply、不预生成所有步骤；
// 一键模式在风险闸门通过后自动推进，命中即停下降级；某步失败停止后续；
// 无产物记跳过；已完成步骤可按各自 Journal 撤销。
//
// 任务终态由 getTask 直接给 completed（夹具），事件订阅路径不在这条链上验。

const chain: RecipeDocument = {
  recipeId: "r-chain",
  schemaVersion: 5,
  name: "清单后转码（两步）",
  steps: [
    { stepId: "step-1", kind: "listing", feature: "f10", paramsSnapshot: {}, source: "original" },
    {
      stepId: "step-2",
      kind: "edit-content",
      feature: "c01",
      paramsSnapshot: { target: "utf-8" },
      source: "previous"
    }
  ],
  createdAt: "2026-11-02T09:00:00Z"
};

function chainSpec(overrides: Partial<RecipeApplySpec> = {}): RecipeApplySpec {
  return {
    recipeId: "r-chain",
    applicable: false,
    reason: "multi-step recipe has 2 steps; apply step by step",
    required: [],
    outputs: [],
    oneClickAllowed: true,
    steps: [
      {
        stepId: "step-1",
        index: 0,
        feature: "f10",
        kind: "listing",
        source: "original",
        applicable: true,
        required: [{ key: "roots", many: true }],
        outputs: [{ key: "output", many: false }],
        highRisk: false
      },
      {
        stepId: "step-2",
        index: 1,
        feature: "c01",
        kind: "edit-content",
        source: "previous",
        applicable: true,
        required: [{ key: "files", many: true }],
        outputs: [],
        highRisk: false
      }
    ],
    ...overrides
  };
}

function plan(target: string, overrides: Partial<PlanDocument> = {}): PlanDocument {
  return {
    planId: `plan-${target}`,
    kind: "f10",
    params: {},
    entries: [
      {
        entryId: `entry-${target}`,
        action: "listing",
        source: "/data",
        target,
        reason: "fixture"
      }
    ],
    stats: { total: 1, conflict: 0, warning: 0, blocked: 0 },
    warnings: [],
    ...overrides
  };
}

const completedTask = {
  summary: { state: "completed" },
  entries: []
} as unknown as TaskDetail;

vi.mock("./lib/ipc", () => ({
  loadRecipe: vi.fn(),
  applyPlan: vi.fn(),
  undoPlan: vi.fn(async () => ({ taskId: "task-undo" })),
  getTask: vi.fn(async () => completedTask),
  listenTaskState: vi.fn(async () => () => undefined),
  toIpcError: (value: unknown) => value
}));

import { applyPlan, getTask, loadRecipe, undoPlan } from "./lib/ipc";

function renderRunner(
  spec: RecipeApplySpec = chainSpec(),
  initialMode: "one-click" | "stepwise" = "stepwise"
) {
  return render(
    <TemplateRunner
      item={chain}
      spec={spec}
      initialMode={initialMode}
      onModeChange={() => undefined}
      onClose={() => undefined}
    />
  );
}

async function planAndRunStep1(user: ReturnType<typeof userEvent.setup>) {
  await user.type(await screen.findByTestId("run-slot-roots"), "/data");
  await user.type(screen.getByTestId("run-slot-output"), "/out");
  await user.click(screen.getByTestId("run-plan"));
  await user.click(await screen.findByTestId("run-execute"));
}

describe("多步模板执行器（Phase B，§15.2.3）", () => {
  beforeEach(() => {
    // mockReset 连 once 队列一起清：否则上一条用例没消费完的 mockResolvedValueOnce
    // 会漏进下一条，计划目标对不上（历史教训：夹具状态串味）。
    vi.clearAllMocks();
    vi.mocked(loadRecipe).mockReset();
    vi.mocked(applyPlan).mockReset();
    vi.mocked(undoPlan).mockReset();
    vi.mocked(getTask).mockReset();
    vi.mocked(getTask).mockResolvedValue(completedTask);
    vi.mocked(applyPlan).mockResolvedValue({ taskId: "task-1" });
    vi.mocked(undoPlan).mockResolvedValue({ taskId: "task-undo" });
  });

  it("逐步骤：每步先预览再执行，后续步骤输入取自上一步产物", async () => {
    vi.mocked(loadRecipe)
      .mockResolvedValueOnce(plan("/out/sample-b.txt"))
      .mockResolvedValueOnce(plan("/out/transcoded.txt"));
    vi.mocked(applyPlan).mockResolvedValue({ taskId: "task-1" });
    renderRunner();
    const user = userEvent.setup();
    await planAndRunStep1(user);

    await waitFor(() =>
      expect(loadRecipe).toHaveBeenCalledWith(
        "r-chain",
        { roots: ["/data"], output: "/out" },
        0
      )
    );
    await waitFor(() => expect(screen.getByTestId("run-step-status-0")).toHaveTextContent("已完成"));

    // 第 2 步：必填 files 由第 1 步计划的 target 预填（source: previous）
    const files = screen.getByTestId("run-slot-files");
    expect((files as HTMLTextAreaElement).value).toBe("/out/sample-b.txt");
    await user.click(screen.getByTestId("run-plan"));
    await waitFor(() =>
      expect(loadRecipe).toHaveBeenLastCalledWith(
        "r-chain",
        { files: ["/out/sample-b.txt"] },
        1
      )
    );
    await user.click(await screen.findByTestId("run-execute"));
    expect(await screen.findByTestId("run-notice")).toHaveTextContent("多步执行完成");
    expect(screen.getByTestId("run-step-status-1")).toHaveTextContent("已完成");
  });

  it("一键：首步确认后，后续步骤计划通过风险闸门即自动执行", async () => {
    vi.mocked(loadRecipe)
      .mockResolvedValueOnce(plan("/out/sample-b.txt"))
      .mockResolvedValueOnce(plan("/out/transcoded.txt"));
    vi.mocked(applyPlan).mockResolvedValue({ taskId: "task-1" });
    renderRunner(chainSpec(), "one-click");
    const user = userEvent.setup();
    await planAndRunStep1(user);

    // 自动推进：第 2 步被自动 plan + apply，不需要再点
    await waitFor(() => expect(applyPlan).toHaveBeenCalledTimes(2));
    await waitFor(() =>
      expect(loadRecipe).toHaveBeenLastCalledWith(
        "r-chain",
        { files: ["/out/sample-b.txt"] },
        1
      )
    );
    expect(await screen.findByTestId("run-notice")).toHaveTextContent("多步执行完成");
  });

  it("风险闸门：后续步骤计划有警告时停下降级，不自动 apply", async () => {
    vi.mocked(loadRecipe)
      .mockResolvedValueOnce(plan("/out/sample-b.txt"))
      .mockResolvedValueOnce(
        plan("/out/transcoded.txt", {
          stats: { total: 1, conflict: 0, warning: 1, blocked: 0 },
          warnings: [{ kind: "encoding-ambiguous", riskLevel: "caution" }]
        })
      );
    vi.mocked(applyPlan).mockResolvedValue({ taskId: "task-1" });
    renderRunner(chainSpec(), "one-click");
    const user = userEvent.setup();
    await planAndRunStep1(user);

    // 闸门命中：第 2 步停在「待确认」，applyPlan 只被调用过一次
    await waitFor(() => expect(screen.getByTestId("run-step-status-1")).toHaveTextContent("待确认"));
    expect(await screen.findByTestId("run-notice")).toHaveTextContent("风险闸门");
    expect(applyPlan).toHaveBeenCalledTimes(1);
    // 降级后仍可人工确认执行
    await user.click(screen.getByTestId("run-execute"));
    await waitFor(() => expect(applyPlan).toHaveBeenCalledTimes(2));
  });

  it("含高风险步骤：一键不可用（即便本机偏好存的是一键）", async () => {
    renderRunner(chainSpec({ oneClickAllowed: false }), "one-click");
    expect(screen.getByTestId("run-mode-one-click")).toBeDisabled();
    expect(screen.getByTestId("run-mode-stepwise")).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByTestId("run-one-click-disabled")).toHaveTextContent("高风险");
  });

  it("某步失败停止后续：applyPlan 抛错后不再 plan 第 2 步", async () => {
    vi.mocked(loadRecipe).mockResolvedValueOnce(plan("/out/sample-b.txt"));
    vi.mocked(applyPlan).mockRejectedValue({
      code: "E2003",
      message: "mock: file was locked",
      retryable: true
    });
    renderRunner();
    const user = userEvent.setup();
    await planAndRunStep1(user);

    expect(await screen.findByTestId("run-notice")).toHaveTextContent("已停止后续");
    expect(screen.getByTestId("run-step-status-0")).toHaveTextContent("失败");
    expect(loadRecipe).toHaveBeenCalledTimes(1);
  });

  it("某步无产物记「跳过」并继续，不当作成功", async () => {
    vi.mocked(loadRecipe)
      .mockResolvedValueOnce(plan("/out/none.txt", { entries: [], stats: { total: 0, conflict: 0, warning: 0, blocked: 0 } }))
      .mockResolvedValueOnce(plan("/out/transcoded.txt"));
    vi.mocked(applyPlan).mockResolvedValue({ taskId: "task-1" });
    renderRunner();
    const user = userEvent.setup();

    await user.type(await screen.findByTestId("run-slot-roots"), "/data");
    await user.type(screen.getByTestId("run-slot-output"), "/out");
    await user.click(screen.getByTestId("run-plan"));

    await waitFor(() => expect(screen.getByTestId("run-step-status-0")).toHaveTextContent("跳过"));
    expect(screen.getByTestId("run-notice")).toHaveTextContent("跳过");
    expect(applyPlan).not.toHaveBeenCalled();
    // 直接进入第 2 步
    expect(await screen.findByTestId("run-slot-files")).toBeVisible();
  });

  it("已完成步骤可按各自 Journal 撤销", async () => {
    vi.mocked(loadRecipe).mockResolvedValueOnce(plan("/out/sample-b.txt"));
    vi.mocked(applyPlan).mockResolvedValue({ taskId: "task-1" });
    renderRunner();
    const user = userEvent.setup();
    await planAndRunStep1(user);

    await user.click(await screen.findByTestId("run-undo-0"));
    await waitFor(() =>
      expect(undoPlan).toHaveBeenCalledWith("plan-/out/sample-b.txt")
    );
    expect(await screen.findByTestId("run-notice")).toHaveTextContent("task-undo");
  });
});
