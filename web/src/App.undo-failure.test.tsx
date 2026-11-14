import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import type { ApplyReport, PlanDocument, TaskState } from "./lib/ipc/types";

// R3 判定点③「提示满足三层结构」（方案 §4.6：人话摘要 → 可展开详情 → 一键复制诊断信息；
// 设计 06 §3 收窄为界面只渲染 summary＋action，技术 detail 仅进诊断日志）。
// 撤销失败此前只有一个计数横幅（「M 项失败」），用户看不到哪几条失败、为什么、该怎么办
// ——本用例把「逐条给出 码位 · 人话摘要 ＋ 建议动作、且不露路径原文」钉住。

type DoneEvent = { taskId: string; kind: "apply" | "undo"; summary: ApplyReport };
const doneListeners: Array<(event: DoneEvent) => void> = [];
let undoState: TaskState = "completed";

const PASS_REPORT = { items: [], level: "pass", pass: 9, warn: 0, block: 0, taskBlocked: false };

const plan: PlanDocument = {
  planId: "01UNDOFAIL",
  kind: "c09",
  entries: [{ entryId: "e1", source: "/tree/a.pdf", action: "delete", reason: "duplicate" }],
  stats: { total: 1, conflict: 0, warning: 0, blocked: 0 },
  warnings: [],
  params: {}
};

const APPLY_REPORT: ApplyReport = {
  planId: "01UNDOFAIL",
  taskId: "task-apply",
  entries: [{ entryId: "e1", status: "ok" }]
};

const RAW_DETAIL = "os error 32 at D:/secret/private/notes.txt:412";

const UNDO_REPORT: ApplyReport = {
  planId: "01UNDOFAIL",
  taskId: "task-undo",
  entries: [
    { entryId: "e1", status: "failed", error: { code: "E2008", message: "rename failed", detail: RAW_DETAIL, retryable: false } }
  ]
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
      state: taskId === "task-undo" ? undoState : "completed",
      phase: "finalizing",
      done: 1,
      total: 1,
      message: ""
    },
    entries: taskId === "task-undo" ? UNDO_REPORT.entries : []
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

async function applyThrough(user: ReturnType<typeof userEvent.setup>) {
  await user.type(screen.getByPlaceholderText(/资料归档|Documents/), "/mock/tree");
  await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
  await screen.findByText(/计划已生成/);
  await user.click(screen.getByLabelText(/我已核对预览/));
  await user.click(screen.getByRole("button", { name: /^移入隔离目录$/ }));
  await waitFor(() => expect(doneListeners.length).toBeGreaterThan(0));
  act(() => {
    doneListeners.forEach((listener) =>
      listener({ taskId: "task-apply", kind: "apply", summary: APPLY_REPORT })
    );
  });
  // apply 完成（有 report）后撤销入口才可用——等它 enabled，而不是猜横幅文案。
  const undoButton = screen.getByRole("button", { name: /撤销本次计划/ });
  await waitFor(() => expect(undoButton).toBeEnabled());
}

describe("撤销失败清单的三层结构（R3③／方案 §4.6／设计 06 §3）", () => {
  beforeEach(() => {
    doneListeners.length = 0;
    undoState = "completed";
    vi.clearAllMocks();
    vi.mocked(planOperation).mockResolvedValue(plan);
  });

  it("撤销有失败时逐条给出码位＋人话摘要＋建议动作，且不露路径原文", async () => {
    const user = userEvent.setup();
    render(<App />);
    await applyThrough(user);

    await user.click(screen.getByRole("button", { name: /撤销本次计划/ }));
    await waitFor(() => expect(doneListeners.length).toBeGreaterThan(0));

    act(() => {
      doneListeners.forEach((listener) =>
        listener({ taskId: "task-undo", kind: "undo", summary: UNDO_REPORT })
      );
    });

    // 第一层：码位＋人话摘要（本地化，不是后端 message 原文）
    expect(await screen.findByText(/E2008/)).toBeVisible();
    expect(screen.getByText(/改名没有成功，文件还在原来的位置。/)).toBeVisible();
    // 第三层：建议动作
    expect(
      screen.getByText(/确认目标目录可写、目标名未被占用后重新执行。/)
    ).toBeVisible();
    // 技术 detail 只进诊断日志（设计 06 §3）：路径与行号不得出现在界面上
    expect(screen.queryByText(/private\/notes\.txt/)).toBeNull();
    expect(screen.queryByText(/D:\/secret/)).toBeNull();
    expect(screen.queryByText(/os error 32/)).toBeNull();
  });

  it("对照：撤销全部成功时不出现失败清单", async () => {
    const user = userEvent.setup();
    render(<App />);
    await applyThrough(user);

    await user.click(screen.getByRole("button", { name: /撤销本次计划/ }));
    await waitFor(() => expect(doneListeners.length).toBeGreaterThan(0));

    act(() => {
      doneListeners.forEach((listener) =>
        listener({
          taskId: "task-undo",
          kind: "undo",
          summary: {
            planId: "01UNDOFAIL",
            taskId: "task-undo",
            entries: [{ entryId: "e1", status: "ok" }]
          }
        })
      );
    });

    await screen.findByText(/Journal 已恢复/);
    expect(screen.queryByText(/失败原因/)).toBeNull();
    expect(screen.queryByText(/建议动作/)).toBeNull();
  });
});
