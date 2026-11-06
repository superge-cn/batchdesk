import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FeaturePages } from "./FeaturePages";
import type { PlanDocument } from "./lib/ipc/types";

// E4（任务控制）与 RBK（撤销/重试）的前端行为断言：
// 取消与重试失败项必须在功能视图上可达，并且调用正确的壳层命令。
const stateListeners: Array<(event: { taskId: string; state: string }) => void> = [];

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
  // 列宽/排序把手会读界面状态；这些用例不测它，给一个空的稳定桩。
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
    summary: { state: "partial-failed", phase: "finalizing", done: 1, total: 2, message: "" },
    entries: []
  })),
  renamePreview: vi.fn(async () => ({ rows: [], counts: { toProcess: 0, skipped: 0, conflicts: 0, blocked: 0 }, highlights: [] })),
  undoDryRun: vi.fn(async () => ({ entries: [], conflictCount: 0, missingCount: 0 })),
  listenTaskProgress: (_callback: (...args: unknown[]) => void) =>
    Promise.resolve(() => undefined),
  listenTaskState: (callback: (event: { taskId: string; state: string }) => void) => {
    stateListeners.push(callback);
    return Promise.resolve(() => undefined);
  },
  listenTaskDone: (_callback: (...args: unknown[]) => void) => Promise.resolve(() => undefined),
  toIpcError: (value: unknown) => value
}));

import {
  applyPlan,
  cancelTask,
  planOperation,
  retryTask,
  undoDryRun,
  undoPlan
} from "./lib/ipc";

async function planThrough(user: ReturnType<typeof userEvent.setup>) {
  const dir = screen.getByLabelText(/改名目录/);
  await user.type(dir, "/dir");
  const replace = screen.getByLabelText(/替换为/);
  await user.type(replace, "x");
  const scope = screen.getByLabelText(/作用范围/);
  await user.type(scope, "full");
  await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
  await waitFor(() => expect(planOperation).toHaveBeenCalled());
}

describe("功能视图的任务控制与撤销", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    stateListeners.length = 0;
  });

  it("生成计划→执行→执行中可取消（调用 cancelTask）", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="f12" onBack={() => undefined} />);
    await planThrough(user);
    expect(vi.mocked(planOperation).mock.calls[0][0]).toMatchObject({
      kind: "f12",
      params: { dir: "/dir", replace: "x", scope: "full" }
    });

    await user.click(screen.getByRole("button", { name: /^执行$/ }));
    await waitFor(() => expect(applyPlan).toHaveBeenCalledWith("01PLAN", expect.anything()));
    const cancel = screen.getByTestId("cancel-task");
    await user.click(cancel);
    await waitFor(() => expect(cancelTask).toHaveBeenCalledWith("task-apply"));
  });

  it("partial-failed 后出现「重试失败项」并调用 retryTask", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="f12" onBack={() => undefined} />);
    await planThrough(user);
    await user.click(screen.getByRole("button", { name: /^执行$/ }));
    await waitFor(() => expect(applyPlan).toHaveBeenCalled());
    // 由 task://state 事件驱动终态（前端不轮询）
    expect(stateListeners.length).toBeGreaterThan(0);
    stateListeners[0]({ taskId: "task-apply", state: "partial-failed" });
    const retry = await screen.findByTestId("retry-failed");
    await user.click(retry);
    await waitFor(() => expect(retryTask).toHaveBeenCalledWith("task-apply"));
  });

  it("回滚预演的条目状态用本地化文案，不露出枚举原词（09 §2）", async () => {
    // C06 预演报告里每行是 UndoDryRunStatus（ok/conflict/missing）——线协议字符串，
    // 不是用户文案。三档各给一条，断界面显示的是资源包里的措辞。
    vi.mocked(undoDryRun).mockResolvedValueOnce({
      planId: "01PLAN",
      entries: [
        { entryId: "01PLAN-0001", status: "ok" },
        { entryId: "01PLAN-0002", status: "conflict", code: "E2007" },
        { entryId: "01PLAN-0003", status: "missing" }
      ],
      conflictCount: 1,
      missingCount: 1
    });
    const user = userEvent.setup();
    render(<FeaturePages view="f12" onBack={() => undefined} />);
    await planThrough(user);
    await user.click(screen.getByRole("button", { name: /回滚预演/ }));

    const rows = await screen.findAllByText(/01PLAN-000/);
    const texts = rows.map((row) => row.textContent ?? "");
    expect(texts.join(" ")).toContain("可撤销");
    expect(texts.join(" ")).toContain("冲突");
    expect(texts.join(" ")).toContain("目标已缺失");
    expect(texts.join(" ")).not.toMatch(/\b(ok|conflict|missing)\b/);
  });

  it("撤销调用 undoPlan（走共享 Journal 通道，不自建撤销）", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="f12" onBack={() => undefined} />);
    await planThrough(user);
    await user.click(screen.getByRole("button", { name: /^撤销$/ }));
    await waitFor(() => expect(undoPlan).toHaveBeenCalledWith("01PLAN"));
  });

  it("改参数后旧计划立即失效：提示出现且执行/撤销被禁用（T 配方 P4）", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="f12" onBack={() => undefined} />);
    await planThrough(user);
    // 刚生成时可用
    expect(screen.getByRole("button", { name: /^执行$/ })).toBeEnabled();
    // 改动任一参数 → 旧 planId 不得再被执行入口使用
    await user.type(screen.getByLabelText(/作用范围/), "x");
    expect(screen.getByTestId("plan-stale")).toBeVisible();
    expect(screen.getByRole("button", { name: /^执行$/ })).toBeDisabled();
    expect(screen.getByRole("button", { name: /^撤销$/ })).toBeDisabled();
    expect(screen.getByRole("button", { name: /回滚预演/ })).toBeDisabled();
    // 重新生成后恢复
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await waitFor(() => expect(screen.queryByTestId("plan-stale")).toBeNull());
    expect(screen.getByRole("button", { name: /^执行$/ })).toBeEnabled();
  });

  it("保护目录命中：执行前必须二次确认，且初始焦点不在危险按钮（方案 §3.3/§7.11、U §3.5）", async () => {
    const user = userEvent.setup();
    const protectedPlan: PlanDocument = {
      ...fakePlan,
      entries: [
        { entryId: "e1", source: "/dir/a.txt", target: "/dir/b.txt", action: "rename", reason: "rename" },
        {
          entryId: "e2",
          source: "/x/win.txt",
          target: "/Windows/win.txt",
          action: "rename",
          reason: "rename",
          blocked: { code: "E2009", reason: "path is under protected directory C:/Windows" }
        }
      ],
      stats: { total: 2, conflict: 0, warning: 0, blocked: 1 }
    };
    vi.mocked(planOperation).mockResolvedValueOnce(protectedPlan);
    render(<FeaturePages view="f12" onBack={() => undefined} />);
    await planThrough(user);

    await user.click(screen.getByRole("button", { name: /^执行$/ }));
    const dialog = await screen.findByTestId("danger-confirm");
    expect(dialog).toHaveTextContent("命中保护目录");
    // 清单里点名被保护的落点，且说明这些条目不会执行
    expect(dialog).toHaveTextContent("/Windows/win.txt");
    expect(dialog).toHaveTextContent("不会执行");
    // 未确认前绝不派发任务
    expect(applyPlan).not.toHaveBeenCalled();
    // 危险按钮不带默认焦点：焦点在「取消」上（回车即确认的误操作路径被消除）
    expect(document.activeElement).toBe(screen.getByTestId("danger-cancel"));

    await user.click(screen.getByTestId("danger-cancel"));
    expect(screen.queryByTestId("danger-confirm")).toBeNull();
    expect(applyPlan).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: /^执行$/ }));
    await user.click(await screen.findByTestId("danger-confirm-button"));
    await waitFor(() =>
      expect(applyPlan).toHaveBeenCalledWith("01PLAN", { onConflict: "skip", overwriteConfirmed: false })
    );
  });

  it("计划失败时呈现 06 码位而非自由文本", async () => {
    vi.mocked(planOperation).mockRejectedValueOnce({ code: "E3004", message: "列不存在" });
    const user = userEvent.setup();
    render(<FeaturePages view="f12" onBack={() => undefined} />);
    await user.type(screen.getByLabelText(/改名目录/), "/dir");
    await user.type(screen.getByLabelText(/替换为/), "x");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    const banner = await screen.findByRole("alert");
    expect(within(banner).getByText(/E1002|E3004|E1001/)).toBeInTheDocument();
  });
});
