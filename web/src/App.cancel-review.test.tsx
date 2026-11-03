import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import type { ApplyReport, TaskState } from "./lib/ipc/types";

// 方案 §7.7：取消＝停止派发＋安全收尾＋询问是否回滚已完成项。
// 壳层以 keepApplied 执行，取消后由界面问「已完成 N 项，是否一并撤销？」；
// 答复撤销走与普通撤销相同的 undoPlan/Journal 通道。
type DoneEvent = { taskId: string; kind: "apply" | "undo"; summary: ApplyReport };
const doneListeners: Array<(event: DoneEvent) => void> = [];
let settledState: TaskState = "cancelled";

vi.mock("./lib/ipc", () => ({
  isTauri: () => true,
  planOperation: vi.fn(async () => null),
  preflightPlan: vi.fn(async () => ({ items: [], level: "pass", pass: 0, warn: 0, block: 0, taskBlocked: false })),
  applyPlan: vi.fn(async () => ({ taskId: "task-apply" })),
  undoPlan: vi.fn(async () => ({ taskId: "task-undo" })),
  cancelTask: vi.fn(async () => undefined),
  retryTask: vi.fn(async () => ({ taskId: "task-retry" })),
  pauseTask: vi.fn(async () => undefined),
  resumeTask: vi.fn(async () => undefined),
  cleanupRetention: vi.fn(async () => ({ removedEntries: 0, removedLogRecords: 0 })),
  recoverTask: vi.fn(async () => undefined),
  getTask: vi.fn(async () => ({
    summary: { state: settledState, phase: "finalizing", done: 2, total: 3, message: "" },
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
  getConfig: vi.fn(async (scope: string) => ({
    scope,
    values: {}
  })),
  setConfig: vi.fn(async (scope: string) => ({ scope, values: {} })),
  resetScope: vi.fn(async (scope: string) => ({ scope, values: {} })),
  toIpcError: (value: unknown) => value
}));

import { undoPlan } from "./lib/ipc";

function finish(entries: ApplyReport["entries"], kind: "apply" | "undo" = "apply") {
  const event = {
    taskId: "task-1",
    kind,
    summary: { planId: "01CANCEL", taskId: "task-1", entries }
  };
  return act(async () => {
    // 订阅在 effect 里异步注册；先等它挂上，再推事件。
    await waitFor(() => expect(doneListeners.length).toBeGreaterThan(0));
    doneListeners.forEach((listener) => listener(event));
    // done 处理器里还要 getTask 一次才拿到终态；等它落地，负向断言才不会因"还没跑"而假绿。
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

const OK = { entryId: "e1", status: "ok" as const };

describe("取消后的撤销询问（方案 §7.7）", () => {
  beforeEach(() => {
    doneListeners.length = 0;
    settledState = "cancelled";
    vi.mocked(undoPlan).mockClear();
  });

  it("取消且有已完成项：先问数量，确认后走 undoPlan 撤销", async () => {
    render(<App />);
    await finish([OK, { entryId: "e2", status: "overwritten" }, { entryId: "e3", status: "skipped" }]);

    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("已完成 2 项，是否一并撤销？");
    expect(dialog).toHaveTextContent("这 2 项已经写入磁盘并记入 Journal");
    // 危险动作不带默认焦点：初始焦点在「保留已完成项」。
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "保留已完成项" }));
    expect(undoPlan).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "一并撤销" }));
    await waitFor(() => expect(undoPlan).toHaveBeenCalledWith("01CANCEL"));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("选择保留：不调用撤销，条目留在 Journal 里", async () => {
    render(<App />);
    await finish([OK]);

    await screen.findByRole("alertdialog");
    await userEvent.click(screen.getByRole("button", { name: "保留已完成项" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(undoPlan).not.toHaveBeenCalled();
  });

  it("对照：取消但一条都没完成时不得打扰用户", async () => {
    render(<App />);
    await finish([{ entryId: "e1", status: "skipped" }]);
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("对照：撤销任务被取消时不得问「是否撤销」（只对 apply 成立）", async () => {
    render(<App />);
    await finish([OK], "undo");
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("对照：正常完成的任务不得弹撤销询问", async () => {
    settledState = "completed";
    render(<App />);
    await finish([OK, { entryId: "e2", status: "ok" }]);
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });
});
