import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FeaturePages } from "./FeaturePages";
import type { ApplyReport, PlanDocument } from "./lib/ipc/types";

// 方案 §5.11「压缩完成后自动生成文件清单」＋ 设计 06 §3（`ApplyEntryReport.detail`
// 是执行计数类技术行、不含绝对路径、**界面在显示**）：C10 的逐文件清单必须真的
// 出现在功能页的终态横幅里，而不是只躺在 JSON 报告里。

const PLAN_ID = "01C10MANIFEST00000000000";

const c10Plan: PlanDocument = {
  planId: PLAN_ID,
  kind: "c10",
  params: {},
  entries: [
    {
      entryId: `${PLAN_ID}-0001`,
      source: "/in/bundle.zip",
      target: "/out",
      action: "extract",
      reason: "extract 2 files"
    }
  ],
  stats: { total: 1, conflict: 0, warning: 0, blocked: 0 },
  warnings: []
};

const MANIFEST = "2 个文件，共 11 字节\na.txt 6\nb.txt 5";

const doneReport: ApplyReport = {
  planId: PLAN_ID,
  taskId: "task-apply",
  entries: [{ entryId: `${PLAN_ID}-0001`, status: "ok", detail: MANIFEST }]
};

type DoneEvent = { taskId: string; kind: "apply" | "undo"; summary: ApplyReport };
let doneHandler: ((event: DoneEvent) => void) | null = null;

vi.mock("./lib/ipc", () => ({
  isTauri: () => true,
  getUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  setUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  planOperation: vi.fn(async () => c10Plan),
  applyPlan: vi.fn(async () => ({ taskId: "task-apply" })),
  undoPlan: vi.fn(async () => ({ taskId: "task-undo" })),
  cancelTask: vi.fn(async () => undefined),
  retryTask: vi.fn(async () => ({ taskId: "task-retry" })),
  pauseTask: vi.fn(async () => undefined),
  resumeTask: vi.fn(async () => undefined),
  getTask: vi.fn(async () => ({
    summary: {
      taskId: "task-apply",
      kind: "c10",
      job: "apply",
      state: "completed",
      phase: "done",
      done: 1,
      total: 1,
      planId: PLAN_ID
    },
    entries: doneReport.entries
  })),
  renamePreview: vi.fn(async () => ({
    rows: [],
    counts: { toProcess: 0, skipped: 0, conflicts: 0, blocked: 0 },
    highlights: []
  })),
  undoDryRun: vi.fn(async () => ({ entries: [], conflictCount: 0, missingCount: 0 })),
  listenTaskProgress: () => Promise.resolve(() => undefined),
  listenTaskState: () => Promise.resolve(() => undefined),
  listenTaskDone: (handler: (event: DoneEvent) => void) => {
    doneHandler = handler;
    return Promise.resolve(() => undefined);
  },
  toIpcError: (value: unknown) => value
}));

import { planOperation } from "./lib/ipc";

describe("功能页的逐文件清单（C10）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    doneHandler = null;
    vi.mocked(planOperation).mockResolvedValue(c10Plan);
  });

  it("压缩/解压成功后，条目明细（逐文件清单）出现在终态横幅", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="c10" onBack={() => undefined} />);

    await user.type(screen.getByLabelText(/压缩包路径/), "/in/bundle.zip");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await waitFor(() => expect(planOperation).toHaveBeenCalled());
    await user.click(screen.getByRole("button", { name: "执行" }));
    await waitFor(() => expect(doneHandler).not.toBeNull());

    // 壳层 done 事件带着报告到达——清单据此上界面。
    act(() => {
      doneHandler?.({ taskId: "task-apply", kind: "apply", summary: doneReport });
    });

    const manifest = await screen.findByText(/2 个文件，共 11 字节/);
    expect(manifest).toBeVisible();
    expect(manifest.className).toContain("manifest");
    expect(screen.getByText(/a\.txt 6/)).toBeVisible();
    expect(screen.getByText(/b\.txt 5/)).toBeVisible();
  });

  it("对照：没有明细的条目不得凭空造出清单块", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="c10" onBack={() => undefined} />);

    await user.type(screen.getByLabelText(/压缩包路径/), "/in/bundle.zip");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await waitFor(() => expect(planOperation).toHaveBeenCalled());
    await user.click(screen.getByRole("button", { name: "执行" }));
    await waitFor(() => expect(doneHandler).not.toBeNull());

    act(() => {
      doneHandler?.({
        taskId: "task-apply",
        kind: "apply",
        summary: {
          planId: PLAN_ID,
          taskId: "task-apply",
          entries: [{ entryId: `${PLAN_ID}-0001`, status: "ok" }]
        }
      });
    });

    await waitFor(() => expect(screen.getByText(/执行完成/)).toBeInTheDocument());
    expect(screen.queryByText(/个文件，共/)).toBeNull();
  });
});
