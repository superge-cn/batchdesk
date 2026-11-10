import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FeaturePages } from "./FeaturePages";
import { planOperation, renamePreview, setUiState } from "./lib/ipc";
import type { RenamePreview } from "./lib/ipc/types";

// 23 个功能视图共用这一张预览表，所以把手只建一处、表 id 按功能码分键
// （设计 03 §11.2 的 `tables`：`feature.<功能码>`）。这里钉三件事：
// 把手在、写入的表 id 带功能码、排序不重新出计划也不作废计划（方案 §7.1/§7.3）。

const stateListeners: Array<(event: { taskId: string; state: string }) => void> = [];

const fakePlan = {
  planId: "01TABLE",
  kind: "f12",
  entries: [
    { entryId: "e1", source: "/dir/z.txt", target: "/dir/b.txt", reason: "rename" },
    { entryId: "e2", source: "/dir/a.txt", target: "/dir/c.txt", reason: "rename" }
  ],
  stats: { blocked: 0, warning: 0 },
  warnings: [],
  params: {}
};

vi.mock("./lib/ipc", () => ({
  isTauri: () => true,
  getUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  setUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  resetUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  getConfig: vi.fn(async () => ({ configVersion: 1, updatedAt: "", scopes: {} })),
  planOperation: vi.fn(async () => fakePlan),
  applyPlan: vi.fn(async () => ({ taskId: "task-apply" })),
  undoPlan: vi.fn(async () => ({ taskId: "task-undo" })),
  cancelTask: vi.fn(async () => undefined),
  retryTask: vi.fn(async () => ({ taskId: "task-retry" })),
  pauseTask: vi.fn(async () => undefined),
  resumeTask: vi.fn(async () => undefined),
  getTask: vi.fn(async () => ({
    summary: { state: "completed", phase: "finalizing", done: 1, total: 1, message: "" },
    entries: []
  })),
  renamePreview: vi.fn(async () => ({
    rows: [],
    counts: { toProcess: 0, skipped: 0, conflicts: 0, blocked: 0 },
    highlights: []
  })),
  undoDryRun: vi.fn(async () => ({ entries: [], conflictCount: 0, missingCount: 0 })),
  listenTaskProgress: () => Promise.resolve(() => undefined),
  listenTaskState: (callback: (event: { taskId: string; state: string }) => void) => {
    stateListeners.push(callback);
    return Promise.resolve(() => undefined);
  },
  listenTaskDone: () => Promise.resolve(() => undefined),
  toIpcError: (value: unknown) => value
}));

/** 行序断言用：每行取第一个 `.path`（功能页一行有两列都是 .path）。 */
function previewOrder(): (string | null)[] {
  return Array.from(document.querySelectorAll(".row:not(.row-head)")).map(
    (row) => row.querySelector(".path")?.textContent ?? null
  );
}

/** 防抖（§11.1）排空后，最近一次带 tables 的写入。 */
async function tablesPatch(): Promise<Record<string, Record<string, unknown>>> {
  const latest = () =>
    [...vi.mocked(setUiState).mock.calls]
      .reverse()
      .find(([patch]) => (patch as Record<string, unknown> | undefined)?.tables !== undefined);
  await waitFor(() => expect(latest()).toBeDefined());
  await new Promise((resolve) => setTimeout(resolve, 400));
  return (latest()![0] as { tables: Record<string, Record<string, unknown>> }).tables;
}

beforeEach(() => {
  vi.mocked(planOperation).mockClear();
  vi.mocked(setUiState).mockClear();
  document.documentElement.style.setProperty("--table-column-min", "88px");
  document.documentElement.style.setProperty("--table-column-max", "640px");
  document.documentElement.style.setProperty("--resize-step", "16px");
});

describe("功能页预览表的把手（设计 03 §11.2 的 tables）", () => {
  function openPreview(view: "f12" | "c03") {
    const user = userEvent.setup();
    const { unmount } = render(<FeaturePages view={view} onBack={() => undefined} />);
    return { user, unmount };
  }

  async function withPreview(view: "f12" | "c03") {
    const opened = openPreview(view);
    await opened.user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("button", { name: /按源排序/ });
    return opened;
  }

  it("表头两列都有排序按钮与列宽把手", async () => {
    await withPreview("f12");

    expect(screen.getByRole("button", { name: /按源排序/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /按目标 \/ 说明排序/ })).toBeInTheDocument();
    expect(screen.getByRole("separator", { name: /调整源列宽/ })).toBeInTheDocument();
    expect(screen.getByRole("separator", { name: /调整目标 \/ 说明列宽/ })).toBeInTheDocument();
  });

  it("排序写进带功能码的表 id，且行序真的变了", async () => {
    const { user } = await withPreview("f12");
    expect(previewOrder()).toEqual(["/dir/z.txt", "/dir/a.txt"]);

    await user.click(screen.getByRole("button", { name: /按源排序/ }));

    await waitFor(() => expect(previewOrder()).toEqual(["/dir/a.txt", "/dir/z.txt"]));
    expect(await tablesPatch()).toEqual({
      "table.feature.f12": { sort: { column: "source", direction: "asc" } }
    });
  });

  it("每个功能页各留一份记录：c03 不会沿用 f12 的列宽/排序", async () => {
    const first = await withPreview("f12");
    await first.user.click(screen.getByRole("button", { name: /按源排序/ }));
    expect(Object.keys(await tablesPatch())).toEqual(["table.feature.f12"]);
    first.unmount();

    const second = await withPreview("c03");
    await second.user.click(screen.getByRole("button", { name: /按源排序/ }));

    expect(Object.keys(await tablesPatch())).toEqual(["table.feature.c03"]);
  });

  it("排序不重新出计划、也不改变派发的计划（方案 §7.1/§7.3）", async () => {
    const { user } = await withPreview("f12");
    expect(planOperation).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: /按目标 \/ 说明排序/ }));

    await tablesPatch();
    expect(planOperation).toHaveBeenCalledTimes(1);
    expect(screen.queryByText(/参数已改动/)).toBeNull();
  });
});

// C05 预览表（F12 页内嵌的冲突检测表）与功能计划预览是**两张表**：表 id 分开存，
// 否则拖 C05 的列会擦掉 F12 计划表的列宽（设计 03 §11.2 的 `tables` 分键）。
describe("C05 预览表的把手（F12 页内嵌）", () => {
  const c05Preview: RenamePreview = {
    planId: "01TABLE",
    kind: "f12",
    counts: { toProcess: 2, skipped: 0, conflicts: 1, blocked: 0 },
    rows: [
      { entryId: "c1", oldPath: "/dir/z.txt", newPath: "/dir/b.txt", status: "normal", twoPhase: false },
      { entryId: "c2", oldPath: "/dir/a.txt", newPath: "/dir/c.txt", status: "conflict", twoPhase: true }
    ],
    highlights: []
  };

  async function withC05Preview() {
    vi.mocked(renamePreview).mockResolvedValueOnce(c05Preview);
    const user = userEvent.setup();
    render(<FeaturePages view="f12" onBack={() => undefined} />);
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("button", { name: /按状态排序/ });
    return user;
  }

  /** 只看 C05 那张表的行（F12 页上还有计划预览表，不能全局取）。 */
  function c05Paths(): string[] {
    const card = document.querySelector('[aria-labelledby="c05-preview"]');
    if (!card) return [];
    return Array.from(card.querySelectorAll(".row:not(.row-head) .path")).map(
      (cell) => cell.textContent ?? ""
    );
  }

  it("C05 表有自己的排序按钮与列宽把手", async () => {
    await withC05Preview();
    expect(screen.getByRole("button", { name: /按状态排序/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /按旧名 → 新名排序/ })).toBeInTheDocument();
    expect(screen.getByRole("separator", { name: /调整状态列宽/ })).toBeInTheDocument();
    expect(
      screen.getByRole("separator", { name: /调整旧名 → 新名列宽/ })
    ).toBeInTheDocument();
  });

  it("拖 C05 列宽写到 table.feature.c05，不碰 F12 计划表", async () => {
    const user = await withC05Preview();
    const handle = screen.getByRole("separator", { name: /调整状态列宽/ });
    handle.focus();
    await user.keyboard("{ArrowRight}");
    await waitFor(() =>
      expect(
        vi.mocked(setUiState).mock.calls.some(
          ([patch]) =>
            (patch as { tables?: Record<string, { columnWidths?: Record<string, number> }> })
              .tables?.["table.feature.c05"]?.columnWidths?.status !== undefined
        )
      ).toBe(true)
    );
  });

  it("C05 排序写 table.feature.c05.sort，行序按 oldPath 变", async () => {
    const user = await withC05Preview();
    expect(c05Paths()[0]).toContain("/dir/z.txt");

    await user.click(screen.getByRole("button", { name: /按旧名 → 新名排序/ }));

    await waitFor(() => expect(c05Paths()[0]).toContain("/dir/a.txt"));
    expect(await tablesPatch()).toEqual({
      "table.feature.c05": { sort: { column: "rename", direction: "asc" } }
    });
  });
});
