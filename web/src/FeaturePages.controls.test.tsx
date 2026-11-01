import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FeaturePages } from "./FeaturePages";

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
