import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { getUiState, setUiState } from "./lib/ipc";

// 设计 03 §11.2/§11.3 的界面联动：启动预填上次目录、生成预览后记住这次目录，
// 并把最近目录做成输入框的候选。UiState 是**可丢的本机状态**，所以只验证"有记录就用、
// 没记录就不填"，不验证任何与业务判定相关的东西。

const PASS_REPORT = { items: [], level: "pass", pass: 9, warn: 0, block: 0, taskBlocked: false };

const plan = {
  planId: "01UISTATE",
  kind: "c09",
  entries: [{ entryId: "e1", source: "/data/last/a.pdf", action: "delete", reason: "duplicate" }],
  stats: { total: 1, conflict: 0, warning: 0, blocked: 0 },
  warnings: [],
  params: {}
};

vi.mock("./lib/ipc", () => ({
  isTauri: () => true,
  getUiState: vi.fn(),
  setUiState: vi.fn(),
  // I18nProvider 挂载时回读 general 组（语言/主题）；这里给空文档即可。
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
  toIpcError: (value: unknown) => value
}));

const lastNavigation = {
  lastSection: "c09",
  lastDirectory: "/data/last",
  recentDirectories: ["/data/last", "/data/older"]
};

beforeEach(() => {
  vi.mocked(getUiState).mockResolvedValue({
    uiStateVersion: 1,
    updatedAt: "2026-11-03T00:00:00Z",
    navigation: lastNavigation
  });
  vi.mocked(setUiState).mockImplementation(async (patch: Record<string, unknown>) => ({
    uiStateVersion: 1,
    updatedAt: "2026-11-03T00:00:01Z",
    navigation: (patch.navigation ?? undefined) as typeof lastNavigation
  }));
});

describe("上次目录与最近目录（设计 03 §11）", () => {
  it("启动时把上次目录填进输入框，并把最近目录挂成候选", async () => {
    render(<App />);

    const input = screen.getByLabelText(/扫描目录|要扫描的目录/);
    await waitFor(() => expect(input).toHaveValue("/data/last"));

    const options = Array.from(document.querySelectorAll("#c09-recent-dirs option")).map(
      (option) => option.getAttribute("value")
    );
    expect(options).toEqual(["/data/last", "/data/older"]);
  });

  it("生成预览后把这次目录写回 UiState，且保持去重置顶", async () => {
    const user = userEvent.setup();
    render(<App />);

    const input = screen.getByLabelText(/扫描目录|要扫描的目录/);
    await waitFor(() => expect(input).toHaveValue("/data/last"));

    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));

    await waitFor(() => expect(setUiState).toHaveBeenCalled());
    const patch = vi.mocked(setUiState).mock.calls[0][0] as {
      navigation: { lastDirectory: string; recentDirectories: string[]; lastSection?: string };
    };
    // 同一条目录要置顶而不是追加；兄弟字段（lastSection）不得被擦掉。
    expect(patch.navigation.lastDirectory).toBe("/data/last");
    expect(patch.navigation.recentDirectories).toEqual(["/data/last", "/data/older"]);
    expect(patch.navigation.lastSection).toBe("c09");
  });

  it("没有记录时不预填、也不写候选（空状态由用户自己选）", async () => {
    vi.mocked(getUiState).mockResolvedValue({ uiStateVersion: 1, updatedAt: "" });
    render(<App />);

    const input = screen.getByLabelText(/扫描目录|要扫描的目录/);
    expect(input).toHaveValue("");
    expect(document.querySelector("#c09-recent-dirs")).toBeNull();
  });
});

describe("上次页面（设计 03 §11.2 的 lastSection/lastFeature）", () => {
  it("启动时回到上次的功能页", async () => {
    vi.mocked(getUiState).mockResolvedValue({
      uiStateVersion: 1,
      updatedAt: "2026-11-03T00:00:00Z",
      navigation: { lastSection: "feature", lastFeature: "f12" }
    });

    render(<App />);

    const nav = await screen.findByRole("button", { name: /批量改名/ });
    await waitFor(() => expect(nav).toHaveClass("active"));
  });

  it("切换页面时写回 lastSection", async () => {
    vi.mocked(getUiState).mockResolvedValue({ uiStateVersion: 1, updatedAt: "" });
    const user = userEvent.setup();
    render(<App />);

    await user.click(screen.getByRole("button", { name: /任务与撤销/ }));
    let patch = vi.mocked(setUiState).mock.calls.at(-1)?.[0] as {
      navigation: { lastSection?: string };
    };
    expect(patch.navigation.lastSection).toBe("tasks");

    await user.click(screen.getByRole("button", { name: /重复文件查找/ }));
    patch = vi.mocked(setUiState).mock.calls.at(-1)?.[0] as {
      navigation: { lastSection?: string };
    };
    expect(patch.navigation.lastSection).toBe("c09");
  });
});
