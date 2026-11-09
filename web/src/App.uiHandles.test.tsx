import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { getUiState, planOperation, setUiState } from "./lib/ipc";

// 设计 03 §11.2 的 layout/tables 把手（侧栏拖宽、列宽、列头排序）与 §11.3 的
// clamp／重置口径。纯函数层的语义在 lib/uiState.test.ts，这里只验"界面上真能改、
// 改了就落盘、且没有顺手把别的分节擦掉"。
//
// jsdom 没有布局：单元格实测宽度为 0，把手从 `:root` 的 token 起步（见 TableHead
// 的 widthOfCell），所以这些用例先把 token 显式设成已知值，让数值可断言。

const PASS_REPORT = { items: [], level: "pass", pass: 9, warn: 0, block: 0, taskBlocked: false };

const plan = {
  planId: "01HANDLES",
  kind: "c09",
  entries: [
    { entryId: "e1", source: "/data/z.pdf", action: "delete", reason: "duplicate" },
    { entryId: "e2", source: "/data/a.pdf", action: "delete", reason: "older" },
    {
      entryId: "e3",
      source: "/data/m.pdf",
      action: "delete",
      reason: "duplicate",
      blocked: { code: "E2009", message: "protected" }
    }
  ],
  stats: { total: 3, conflict: 0, warning: 0, blocked: 1 },
  warnings: [],
  params: {}
};

vi.mock("./lib/ipc", () => ({
  isTauri: () => true,
  getUiState: vi.fn(),
  setUiState: vi.fn(),
  resetUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  getConfig: vi.fn(async () => ({ configVersion: 1, updatedAt: "", scopes: {} })),
  setConfig: vi.fn(async () => ({ scope: "general", values: {} })),
  resetScope: vi.fn(async () => ({ scope: "general", values: {} })),
  planOperation: vi.fn(async () => plan),
  preflightPlan: vi.fn(async () => PASS_REPORT),
  applyPlan: vi.fn(async () => ({ taskId: "task-apply" })),
  undoPlan: vi.fn(async () => ({ taskId: "task-undo" })),
  cancelTask: vi.fn(async () => undefined),
  retryTask: vi.fn(async () => ({ taskId: "task-retry" })),
  pauseTask: vi.fn(async () => undefined),
  resumeTask: vi.fn(async () => undefined),
  listTasks: vi.fn(async () => ({ items: [], total: 0 })),
  getStorageInfo: vi.fn(async () => ({ quarantineDir: "/q", journalFile: "/j" })),
  getIpcMeta: vi.fn(async () => ({ contractVersion: 1, kernelVersion: "0.1.0" })),
  listenTaskProgress: vi.fn(async () => () => undefined),
  listenTaskState: vi.fn(async () => () => undefined),
  listenTaskDone: vi.fn(async () => () => undefined),
  // F15：App 挂载即渲染 AiPanel（关闭态也会读状态并订阅事件），这里桩掉 AI 通道。
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

/** 行序断言用：只取数据行的第一列（表头里没有 .path）。 */
function previewOrder(): (string | null)[] {
  return Array.from(document.querySelectorAll(".row:not(.row-head) .path")).map(
    (cell) => cell.textContent
  );
}

/** 侧栏宽度的落点：`.glass-shell` 上的内联变量（没有内联值＝用 CSS 默认）。 */
function shellSidebarWidth(): string | null {
  const shell = document.querySelector(".glass-shell") as HTMLElement | null;
  return shell ? shell.style.getPropertyValue("--layout-sidebar-width") || null : null;
}

/** 列宽的落点：表容器上的 `--col-N`——虚拟滚动里的行靠它跟着变宽，只写状态不算生效。 */
function previewColumnVar(index: number): string | null {
  const head = screen.getByRole("button", { name: /按待清理文件排序/ });
  const table = head.closest(".table") as HTMLElement | null;
  return table ? table.style.getPropertyValue(`--col-${index}`) || null : null;
}

/**
 * 最近一次真正带 `tables` 的写入。
 *
 * 两个坑：① 生成预览时也会写 navigation 分节，所以不能取"最后一次调用"；
 * ② 落盘是防抖的（设计 03 §11.1，300ms），连着点两次排序时旧的 flush 可能还在路上，
 * 所以先等防抖排空再取——断言的是"最终持久化成了什么"。
 */
async function tablesPatch(): Promise<Record<string, Record<string, unknown>>> {
  const latest = () =>
    [...vi.mocked(setUiState).mock.calls]
      .reverse()
      .find(([patch]) => (patch as Record<string, unknown> | undefined)?.tables !== undefined);
  await waitFor(() => expect(latest()).toBeDefined());
  await new Promise((resolve) => setTimeout(resolve, 400));
  const call = latest();
  return (call![0] as { tables: Record<string, Record<string, unknown>> }).tables;
}

beforeEach(() => {
  const root = document.documentElement.style;
  root.setProperty("--layout-sidebar-width", "232px");
  root.setProperty("--layout-sidebar-min", "176px");
  root.setProperty("--layout-sidebar-max", "384px");
  root.setProperty("--resize-step", "16px");
  root.setProperty("--table-column-min", "88px");
  root.setProperty("--table-column-max", "640px");
  vi.mocked(getUiState).mockResolvedValue({ uiStateVersion: 1, updatedAt: "" });
  vi.mocked(setUiState).mockImplementation(async (patch: Record<string, unknown>) => ({
    uiStateVersion: 1,
    updatedAt: "2026-11-06T00:00:00Z",
    ...(patch as object)
  }));
  vi.mocked(planOperation).mockClear();
});

describe("侧栏拖宽（设计 03 §11.2 的 layout.sidebarWidth）", () => {
  it("键盘调宽后写 layout 分节，且不动 navigation 分节", async () => {
    const user = userEvent.setup();
    render(<App />);

    const handle = await screen.findByRole("separator", { name: /调整侧栏宽度/ });
    handle.focus();
    await user.keyboard("{ArrowRight}");

    await waitFor(() => expect(setUiState).toHaveBeenCalled());
    const patch = vi.mocked(setUiState).mock.calls.at(-1)?.[0] as Record<string, unknown>;
    // 232px 起步＋16px 步进（都来自 :root 的 token）。
    expect(patch.layout).toEqual({ sidebarWidth: 248 });
    expect("navigation" in patch).toBe(false);
  });

  it("超出上界就贴边，不把越界的宽度写进本机状态（§11.3 clamp）", async () => {
    document.documentElement.style.setProperty("--layout-sidebar-max", "240px");
    const user = userEvent.setup();
    render(<App />);

    const handle = await screen.findByRole("separator", { name: /调整侧栏宽度/ });
    handle.focus();
    await user.keyboard("{ArrowRight}");

    await waitFor(() => expect(setUiState).toHaveBeenCalled());
    const patch = vi.mocked(setUiState).mock.calls.at(-1)?.[0] as { layout: { sidebarWidth: number } };
    expect(patch.layout.sidebarWidth).toBe(240);
  });

  it("启动时按已记录的宽度恢复，越界值同样 clamp 后再应用", async () => {
    vi.mocked(getUiState).mockResolvedValue({
      uiStateVersion: 1,
      updatedAt: "2026-11-06T00:00:00Z",
      layout: { sidebarWidth: 9999 }
    });
    render(<App />);

    // 9999 越上界 → 贴到 --layout-sidebar-max（384px），不把越界值摆上去。
    await waitFor(() => expect(shellSidebarWidth()).toBe("384px"));
  });

  it("重置界面状态回到默认宽度，旧的侧栏值不再带着", async () => {
    vi.mocked(getUiState).mockResolvedValue({
      uiStateVersion: 1,
      updatedAt: "2026-11-06T00:00:00Z",
      layout: { sidebarWidth: 300 }
    });
    const user = userEvent.setup();
    render(<App />);
    await waitFor(() => expect(shellSidebarWidth()).toBe("300px"));

    await user.click(screen.getByRole("button", { name: /设置/ }));
    await user.click(await screen.findByRole("button", { name: /重置界面状态/ }));

    // §11.3：重置只影响 UiState——宽度回到 CSS 默认（内联变量撤掉，不是写回一个数）。
    await waitFor(() => expect(shellSidebarWidth()).toBeNull());
  });
});

describe("列头排序与列宽（设计 03 §11.2 的 tables）", () => {
  async function withPreview() {
    const user = userEvent.setup();
    render(<App />);
    await user.type(screen.getByLabelText(/扫描目录|要扫描的目录/), "/data");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("button", { name: /按待清理文件排序/ });
    return user;
  }

  it("默认显示序就是计划序（没有任何排序记录时不动行）", async () => {
    await withPreview();
    expect(previewOrder()).toEqual(["/data/z.pdf", "/data/a.pdf", "/data/m.pdf"]);
  });

  it("启动带回来的列宽与排序直接生效（设计 03 §11.3 的恢复）", async () => {
    vi.mocked(getUiState).mockResolvedValue({
      uiStateVersion: 1,
      updatedAt: "2026-11-06T00:00:00Z",
      tables: {
        "table.c09.plan": {
          columnWidths: { reason: 222 },
          sort: { column: "source", direction: "desc" }
        }
      }
    });
    await withPreview();

    await waitFor(() => expect(previewColumnVar(2)).toBe("222px"));
    // 恢复的排序直接作用在行序上：source 降序＝z → m → a。
    expect(previewOrder()).toEqual(["/data/z.pdf", "/data/m.pdf", "/data/a.pdf"]);
  });

  it("点列名升序、再点降序，两次都把 tables 写回本机状态", async () => {
    const user = await withPreview();

    await user.click(screen.getByRole("button", { name: /按待清理文件排序/ }));
    await waitFor(() => expect(previewOrder()).toEqual(["/data/a.pdf", "/data/m.pdf", "/data/z.pdf"]));
    expect(await tablesPatch()).toEqual({
      "table.c09.plan": { sort: { column: "source", direction: "asc" } }
    });

    await user.click(screen.getByRole("button", { name: /按待清理文件排序/ }));
    await waitFor(() => expect(previewOrder()).toEqual(["/data/z.pdf", "/data/m.pdf", "/data/a.pdf"]));
    expect(await tablesPatch()).toEqual({
      "table.c09.plan": { sort: { column: "source", direction: "desc" } }
    });
  });

  it("排序不重新出计划、也不让旧计划失效（方案 §7.1/§7.3：排序是视图态不是参数）", async () => {
    const user = await withPreview();
    expect(planOperation).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: /按保留依据 \/ 原因排序/ }));

    await waitFor(() => expect(setUiState).toHaveBeenCalled());
    expect(planOperation).toHaveBeenCalledTimes(1);
    expect(screen.queryByTestId("plan-stale")).toBeNull();
  });

  it("键盘调列宽把 columnWidths 写进 tables，列名排序值不受影响", async () => {
    const user = await withPreview();

    const handle = screen.getByRole("separator", { name: /调整待清理文件列宽/ });
    handle.focus();
    await user.keyboard("{ArrowRight}");

    // jsdom 量不到单元格 → 从 --table-column-min（88px）起步，一步 16px。
    const tables = await tablesPatch();
    expect(tables["table.c09.plan"]?.columnWidths).toEqual({ source: 104 });
    expect(tables["table.c09.plan"]?.sort).toBeUndefined();
    // 写进状态还不算数：宽度要真的落到表容器的 --col-1 上，否则虚拟滚动的行不跟着宽。
    await waitFor(() => expect(previewColumnVar(1)).toBe("104px"));
  });

  it("每张表各留一份记录：改了预览表不会带上执行结果表", async () => {
    const user = await withPreview();

    await user.click(screen.getByRole("button", { name: /按待清理文件排序/ }));

    expect(Object.keys(await tablesPatch())).toEqual(["table.c09.plan"]);
  });
});
