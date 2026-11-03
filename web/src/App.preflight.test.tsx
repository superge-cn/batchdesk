import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";

// OQ-31 裁决后的 S01 E○2 语义（TC v0.4 §3 S01 行）：预检是同步快照，不存在"中途取消"；
// 可判定的是——预检耗时期间界面仍可操作，且预检结果落地前（含落地瞬间参数已变）不派发执行。
const PASS_REPORT = { items: [], level: "pass", pass: 9, warn: 0, block: 0, taskBlocked: false };

const plan = {
  planId: "01PREFLIGHT",
  kind: "c09",
  entries: [{ entryId: "e1", source: "/tree/a.pdf", action: "delete", reason: "duplicate" }],
  stats: { total: 1, conflict: 0, warning: 0, blocked: 0 },
  warnings: [],
  params: {}
};

let resolvers: Array<(value: unknown) => void> = [];
let calls = 0;

/** 放行最近一次预检，让 onPlan/onApply 继续往下走。 */
function release(report: unknown = PASS_REPORT) {
  const resolve = resolvers.shift();
  if (!resolve) {
    throw new Error("没有待放行的预检调用");
  }
  resolve(report);
}

/** 等下一次预检发起（生成计划也会预检一次，故逐次计数）。 */
async function expectPreflight() {
  calls += 1;
  await waitFor(() => expect(preflightPlan).toHaveBeenCalledTimes(calls));
}

vi.mock("./lib/ipc", () => ({
  isTauri: () => true,
  planOperation: vi.fn(async () => plan),
  preflightPlan: vi.fn(
    () =>
      new Promise((resolve) => {
        resolvers.push(resolve);
      })
  ),
  applyPlan: vi.fn(async () => ({ taskId: "task-apply" })),
  undoPlan: vi.fn(async () => ({ taskId: "task-undo" })),
  cancelTask: vi.fn(async () => undefined),
  retryTask: vi.fn(async () => ({ taskId: "task-retry" })),
  pauseTask: vi.fn(async () => undefined),
  resumeTask: vi.fn(async () => undefined),
  cleanupRetention: vi.fn(async () => ({ removedEntries: 0, removedLogRecords: 0 })),
  recoverTask: vi.fn(async () => undefined),
  getTask: vi.fn(async () => null),
  getStorageInfo: vi.fn(async () => ({ quarantineBytes: 0, journalBytes: 0, logBytes: 0 })),
  getIpcMeta: vi.fn(async () => ({ version: "test", commands: [], events: [] })),
  listTasks: vi.fn(async () => []),
  renamePreview: vi.fn(async () => ({
    rows: [],
    counts: { toProcess: 0, skipped: 0, conflicts: 0, blocked: 0 },
    highlights: []
  })),
  undoDryRun: vi.fn(async () => ({ entries: [], conflictCount: 0, missingCount: 0 })),
  listenTaskProgress: () => Promise.resolve(() => undefined),
  listenTaskState: () => Promise.resolve(() => undefined),
  listenTaskDone: () => Promise.resolve(() => undefined),
  getConfig: vi.fn(async (scope: string) => ({
    scope,
    values: {}
  })),
  setConfig: vi.fn(async (scope: string) => ({ scope, values: {} })),
  resetScope: vi.fn(async (scope: string) => ({ scope, values: {} })),
  toIpcError: (value: unknown) => value
}));

import { applyPlan, preflightPlan } from "./lib/ipc";

async function generatePlan(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
  await expectPreflight();
  release();
  await screen.findByText(/计划已生成/);
}

async function startApply(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByLabelText(/我已核对预览/));
  await user.click(screen.getByRole("button", { name: /^移入隔离目录$/ }));
  await expectPreflight();
}

describe("S01 预检的时序语义（TC v0.4 的 E○2 改写）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resolvers = [];
    calls = 0;
  });

  it("预检落地前不派发；期间界面仍可操作；落地瞬间参数已变则拒绝派发", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.type(screen.getByPlaceholderText(/资料归档|Documents/), "/mock/tree");
    await generatePlan(user);
    await startApply(user);
    expect(applyPlan).not.toHaveBeenCalled();

    // 预检耗时期间界面没有锁死：参数输入框仍可编辑
    const root = screen.getByPlaceholderText(/资料归档|Documents/);
    expect(root).toBeEnabled();
    await user.type(root, "-copy");
    expect(root).toHaveValue("/mock/tree-copy");

    // 预检结果落地时参数已改 → 仍不得拿旧计划派发（§7.3 的异步竞态窗口）
    release();
    await screen.findByText(/参数已改动，旧计划预览失效/);
    expect(applyPlan).not.toHaveBeenCalled();

    // 重新预览后才走通"预检通过→执行"
    await generatePlan(user);
    await startApply(user);
    release();
    await waitFor(() =>
      expect(applyPlan).toHaveBeenCalledWith("01PREFLIGHT", {
        onConflict: "skip",
        overwriteConfirmed: false
      })
    );
  });

  it("预检阻断时不派发执行（E2004 前移为执行前阻断，零副作用）", async () => {
    const user = userEvent.setup();
    render(<App />);
    await user.type(screen.getByPlaceholderText(/资料归档|Documents/), "/mock/tree");
    await generatePlan(user);
    await startApply(user);

    release({
      items: [{ kind: "disk-space", level: "block", code: "E2004", message: "余量不足" }],
      level: "block",
      pass: 8,
      warn: 0,
      block: 1,
      taskBlocked: true
    });
    await screen.findByText(/执行已阻断，任务零副作用/);
    expect(applyPlan).not.toHaveBeenCalled();
  });
});
