import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FeaturePages } from "./FeaturePages";

// 方案 6.2 的回归护栏：功能页的计划预览必须走窗口化。
// 3000 条计划的旧实现会把 3000 个 .row 一次性塞进 DOM（页面卡死），
// 这条断言在那种实现下必红。
const ENTRY_COUNT = 3000;

const bigPlan = {
  planId: "01BIG",
  kind: "f12",
  entries: Array.from({ length: ENTRY_COUNT }, (_, index) => ({
    entryId: `e${index}`,
    source: `/dir/file-${index}.txt`,
    target: `/dir/file-${index}-renamed.txt`,
    reason: "rename"
  })),
  stats: { total: ENTRY_COUNT, blocked: 0, warning: 0, conflict: 0 },
  warnings: [],
  params: {}
};

vi.mock("./lib/ipc", () => ({
  isTauri: () => true,
  planOperation: vi.fn(async () => bigPlan),
  applyPlan: vi.fn(async () => ({ taskId: "task-apply" })),
  undoPlan: vi.fn(async () => ({ taskId: "task-undo" })),
  cancelTask: vi.fn(async () => undefined),
  retryTask: vi.fn(async () => ({ taskId: "task-retry" })),
  pauseTask: vi.fn(async () => undefined),
  resumeTask: vi.fn(async () => undefined),
  getTask: vi.fn(async () => ({
    summary: { state: "completed", phase: "finalizing", done: 0, total: 0, message: "" },
    entries: []
  })),
  renamePreview: vi.fn(async () => ({
    rows: [],
    counts: { toProcess: 0, skipped: 0, conflicts: 0, blocked: 0 },
    highlights: []
  })),
  undoDryRun: vi.fn(async () => ({ entries: [], conflictCount: 0, missingCount: 0 })),
  listenTaskProgress: (_callback: (...args: unknown[]) => void) => Promise.resolve(() => undefined),
  listenTaskState: (_callback: (...args: unknown[]) => void) => Promise.resolve(() => undefined),
  listenTaskDone: (_callback: (...args: unknown[]) => void) => Promise.resolve(() => undefined),
  toIpcError: (value: unknown) => value
}));

import { planOperation } from "./lib/ipc";

describe("功能页计划预览的窗口化（方案 6.2）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("3000 条计划只渲染窗口内的行，DOM 行数不随条目数线性增长", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="f12" onBack={() => undefined} />);
    await user.type(screen.getByLabelText(/改名目录/), "/dir");
    await user.type(screen.getByLabelText(/替换为/), "x");
    await user.type(screen.getByLabelText(/作用范围/), "full");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await waitFor(() => expect(planOperation).toHaveBeenCalled());

    // 计数如实显示 3000 条（计数文案被拆成多个文本节点，按整卡文本断言）
    await waitFor(() => {
      const section = screen.getByRole("heading", { name: "计划预览" }).closest("section");
      expect(section).toHaveTextContent("3000 条");
    });

    const root = screen.getByRole("list", { name: "计划条目" });
    const rendered = root.querySelectorAll('[role="listitem"]').length;
    expect(rendered).toBeGreaterThan(0);
    expect(rendered).toBeLessThan(40);
    expect(root).toHaveAttribute("aria-rowcount", String(ENTRY_COUNT));

    // 尾部条目不在 DOM 里（旧实现会全部渲染）
    expect(screen.queryByText(/file-2900\.txt/)).toBeNull();
    expect(screen.getByText(/file-0\.txt/)).toBeInTheDocument();
  });
});
