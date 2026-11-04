import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import type { ApplyReport, TaskState } from "./lib/ipc/types";

// 设计 06 §3：文案不得暴露路径、文件名或堆栈，`detail` 仅进诊断日志。
// 错误横幅走了统一的 ErrorBanner，但执行结果表里每条失败项仍直接打印后端 `IpcError`
// 的技术串——这一处是最容易漏的：它不是"一条横幅"，而是一张表。

type DoneEvent = { taskId: string; kind: "apply" | "undo"; summary: ApplyReport };
const doneListeners: Array<(event: DoneEvent) => void> = [];
let settledState: TaskState = "partial-failed";

const PASS_REPORT = { items: [], level: "pass", pass: 9, warn: 0, block: 0, taskBlocked: false };

const plan = {
  planId: "01RESULT",
  kind: "c09",
  entries: [{ entryId: "e1", source: "/tree/a.pdf", action: "delete", reason: "duplicate" }],
  stats: { total: 2, conflict: 0, warning: 0, blocked: 0 },
  warnings: [],
  params: {}
};

const RAW_MESSAGE = "permission denied while reading /home/operator/private/notes.txt";
const RAW_DETAIL = "os error 13 at src/tasks.rs:412";

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
  getTask: vi.fn(async () => ({
    summary: { state: settledState, phase: "finalizing", done: 1, total: 2, message: "" },
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
  listenTaskProgress: () => Promise.resolve(() => undefined),
  listenTaskState: () => Promise.resolve(() => undefined),
  listenTaskDone: (callback: (event: DoneEvent) => void) => {
    doneListeners.push(callback);
    return Promise.resolve(() => undefined);
  },
  getConfig: vi.fn(async (scope: string) => ({ scope, values: {} })),
  setConfig: vi.fn(async (scope: string) => ({ scope, values: {} })),
  resetScope: vi.fn(async (scope: string) => ({ scope, values: {} })),
  toIpcError: (value: unknown) => value
}));

const FAILURE_EVENT: DoneEvent = {
  taskId: "task-apply",
  kind: "apply",
  summary: {
    planId: "01RESULT",
    taskId: "task-apply",
    entries: [
      { entryId: "e1", status: "ok" },
      {
        entryId: "e2",
        status: "failed",
        error: { code: "E2002", message: RAW_MESSAGE, detail: RAW_DETAIL, retryable: false }
      }
    ]
  }
};

describe("执行结果表的失败条目文案（06 §2／§3）", () => {
  beforeEach(() => {
    doneListeners.length = 0;
    settledState = "partial-failed";
  });

  it("失败条目显示码位＋本地化摘要，不显示后端 message／detail 原文", async () => {
    const user = userEvent.setup();
    render(<App />);
    await user.type(screen.getByPlaceholderText(/资料归档|Documents/), "/mock/tree");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByText(/计划已生成/);
    await user.click(screen.getByLabelText(/我已核对预览/));
    await user.click(screen.getByRole("button", { name: /^移入隔离目录$/ }));
    await waitFor(() => expect(doneListeners.length).toBeGreaterThan(0));

    await act(async () => {
      doneListeners.forEach((listener) => listener(FAILURE_EVENT));
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    const cell = await screen.findByText(/E2002/);
    expect(cell).toHaveTextContent("没有访问权限");
    expect(cell.textContent).not.toContain(RAW_MESSAGE);
    expect(cell.textContent).not.toContain(RAW_DETAIL);
    // 悬停提示同样不得带出技术原文
    expect(cell.getAttribute("title")).not.toContain("/home/operator");
  });

  it("条目状态用本地化文案，不露出内核枚举原词（09 §2）", async () => {
    const user = userEvent.setup();
    render(<App />);
    await user.type(screen.getByPlaceholderText(/资料归档|Documents/), "/mock/tree");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByText(/计划已生成/);
    await user.click(screen.getByLabelText(/我已核对预览/));
    await user.click(screen.getByRole("button", { name: /^移入隔离目录$/ }));
    await waitFor(() => expect(doneListeners.length).toBeGreaterThan(0));

    await act(async () => {
      doneListeners.forEach((listener) => listener(FAILURE_EVENT));
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    // 一条成功、一条失败：徽章文案来自资源包，而不是 ApplyStatus 的线协议字符串
    expect(await screen.findByText("成功")).toBeInTheDocument();
    expect(screen.getByText("失败")).toBeInTheDocument();
    expect(screen.queryByText("ok")).toBeNull();
    expect(screen.queryByText("failed")).toBeNull();
  });
});
