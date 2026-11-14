import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import type { PlanDocument, ProgressEvent, TaskState } from "./lib/ipc/types";

// 条目内（字节级）进度（ADR-0100／方案 §7.5 双层进度）：
// N:1 的合并（F04）整条任务只有一条 entry，没有条目内进度的话界面长时间停在 0/1。
// 本用例钉住：字节进度要显示成人类可读大小，且条目边界事件要把它清掉（不能张冠李戴）。

type ProgressListener = (event: ProgressEvent) => void;
const progressListeners: ProgressListener[] = [];
const doneListeners: Array<(event: { taskId: string; kind: "apply" | "undo" }) => void> = [];
let settledState: TaskState = "completed";

const PASS_REPORT = { items: [], level: "pass", pass: 9, warn: 0, block: 0, taskBlocked: false };

const plan: PlanDocument = {
  planId: "01BYTEPROG",
  kind: "c09",
  entries: [{ entryId: "e1", source: "/tree/a.pdf", action: "delete", reason: "duplicate" }],
  stats: { total: 1, conflict: 0, warning: 0, blocked: 0 },
  warnings: [],
  params: {}
};

vi.mock("./lib/ipc", () => ({
  isTauri: () => true,
  planOperation: vi.fn(async () => plan),
  preflightPlan: vi.fn(async () => PASS_REPORT),
  applyPlan: vi.fn(async () => ({ taskId: "task-apply" })),
  undoPlan: vi.fn(async () => ({ taskId: "task-undo" })),
  cancelTask: vi.fn(async () => undefined),
  getUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  setUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  retryTask: vi.fn(async () => ({ taskId: "task-retry" })),
  pauseTask: vi.fn(async () => undefined),
  resumeTask: vi.fn(async () => undefined),
  cleanupRetention: vi.fn(async () => ({ removedEntries: 0, removedLogRecords: 0 })),
  recoverTask: vi.fn(async () => undefined),
  getTask: vi.fn(async (taskId: string) => ({
    summary: {
      taskId,
      state: settledState,
      phase: "finalizing",
      done: 1,
      total: 1,
      message: ""
    },
    entries: []
  })),
  getStorageInfo: vi.fn(async () => ({ quarantineBytes: 0, journalBytes: 0, logBytes: 0 })),
  getIpcMeta: vi.fn(async () => ({ version: "test", commands: [], events: [] })),
  listTasks: vi.fn(async () => []),
  renamePreview: vi.fn(async () => ({
    rows: [],
    counts: { toProcess: 0, skipped: 0, conflicts: 0, blocked: 0 },
    highlights: []
  })),
  undoDryRun: vi.fn(async () => ({ entries: [], conflictCount: 0, missingCount: 0 })),
  listenTaskProgress: (listener: ProgressListener) => {
    progressListeners.push(listener);
    return Promise.resolve(() => undefined);
  },
  listenTaskState: () => Promise.resolve(() => undefined),
  listenTaskDone: (callback: (event: { taskId: string; kind: "apply" | "undo" }) => void) => {
    doneListeners.push(callback);
    return Promise.resolve(() => undefined);
  },
  getConfig: vi.fn(async (scope: string) => ({ scope, values: {} })),
  setConfig: vi.fn(async (scope: string) => ({ scope, values: {} })),
  resetScope: vi.fn(async (scope: string) => ({ scope, values: {} })),
  getAiStatus: vi.fn(async () => ({
    available: false,
    configured: false,
    phase: 1,
    catalogVersion: "test",
    supportedKinds: [],
    consentRequired: true
  })),
  aiAsk: vi.fn(async () => ({ messageId: "ai-msg-test" })),
  aiCancel: vi.fn(async () => true),
  listenAiDelta: () => Promise.resolve(() => undefined),
  listenAiTool: () => Promise.resolve(() => undefined),
  listenAiBundle: () => Promise.resolve(() => undefined),
  listenAiDone: () => Promise.resolve(() => undefined),
  listenAiError: () => Promise.resolve(() => undefined),
  toIpcError: (value: unknown) => value
}));

import { planOperation } from "./lib/ipc";

async function startApply(user: ReturnType<typeof userEvent.setup>) {
  await user.type(screen.getByPlaceholderText(/资料归档|Documents/), "/mock/tree");
  await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
  await screen.findByText(/计划已生成/);
  await user.click(screen.getByLabelText(/我已核对预览/));
  await user.click(screen.getByRole("button", { name: /^移入隔离目录$/ }));
  await waitFor(() => expect(progressListeners.length).toBeGreaterThan(0));
}

function fireByteProgress() {
  act(() => {
    progressListeners.forEach((listener) =>
      listener({
        taskId: "task-apply",
        phase: "executing",
        done: 0,
        total: 0,
        entryDone: 1536,
        entryTotal: 4096
      })
    );
  });
}

function fireBoundary(done: number, total: number) {
  act(() => {
    progressListeners.forEach((listener) =>
      listener({ taskId: "task-apply", phase: "executing", done, total })
    );
  });
}

describe("条目内（字节级）进度（ADR-0100／方案 §7.5）", () => {
  beforeEach(() => {
    progressListeners.length = 0;
    doneListeners.length = 0;
    settledState = "completed";
    vi.clearAllMocks();
    vi.mocked(planOperation).mockResolvedValue(plan);
  });

  it("条目内事件显示人类可读的字节进度，且不改条目计数", async () => {
    const user = userEvent.setup();
    render(<App />);
    await startApply(user);

    fireByteProgress();

    expect(await screen.findByText(/1\.5 KiB\/4\.0 KiB/)).toBeVisible();
    // 条目内事件携带的 done/total 无意义（内核填 0），界面不得因此把条目计数改成 0/0
    expect(screen.getByText(/0\/1/)).toBeVisible();
  });

  it("对照：条目边界事件清掉字节进度，避免下一条目张冠李戴", async () => {
    const user = userEvent.setup();
    render(<App />);
    await startApply(user);

    fireByteProgress();
    expect(await screen.findByText(/1\.5 KiB\/4\.0 KiB/)).toBeVisible();

    fireBoundary(1, 1);

    await waitFor(() => expect(screen.queryByText(/KiB/)).toBeNull());
    expect(screen.getByText(/1\/1/)).toBeVisible();
  });
});
