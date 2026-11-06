import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FeaturePages } from "./FeaturePages";
import type { PlanDocument } from "./lib/ipc/types";

// F11 的"集中报告"要在功能页逐项点名：计划级 warnings 若只折成"警告 N"，
// 用户看不到哪几行被排除、为什么（无效行/重复行/父路径为文件都在这一层）。
const PLAN_ID = "01F11PARENTFILE0000000000";

const warningPlan: PlanDocument = {
  planId: PLAN_ID,
  kind: "f11",
  params: {},
  entries: [
    {
      entryId: `${PLAN_ID}-0001`,
      source: "",
      target: "/root/a",
      action: "create-dir",
      reason: "create directory /root/a"
    },
    {
      entryId: `${PLAN_ID}-0002`,
      source: "",
      target: "/root/a/b",
      action: "create-dir",
      reason: "skipped: /root/a/b lies under the existing file /root/a"
    }
  ],
  stats: { total: 2, conflict: 1, warning: 1, blocked: 0 },
  warnings: [
    {
      kind: "parent-is-file",
      riskLevel: "caution",
      suggestion: "/root/a/b (parent /root/a is a file)"
    }
  ]
};

vi.mock("./lib/ipc", () => ({
  isTauri: () => true,
  // 列宽/排序把手会读界面状态；这些用例不测它，给一个空的稳定桩。
  getUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  setUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  planOperation: vi.fn(async () => warningPlan),
  applyPlan: vi.fn(async () => ({ taskId: "task-apply" })),
  undoPlan: vi.fn(async () => ({ taskId: "task-undo" })),
  cancelTask: vi.fn(async () => undefined),
  retryTask: vi.fn(async () => ({ taskId: "task-retry" })),
  pauseTask: vi.fn(async () => undefined),
  resumeTask: vi.fn(async () => undefined),
  getTask: vi.fn(async () => null),
  renamePreview: vi.fn(async () => ({
    rows: [],
    counts: { toProcess: 0, skipped: 0, conflicts: 0, blocked: 0 },
    highlights: []
  })),
  undoDryRun: vi.fn(async () => ({ entries: [], conflictCount: 0, missingCount: 0 })),
  listenTaskProgress: () => Promise.resolve(() => undefined),
  listenTaskState: () => Promise.resolve(() => undefined),
  listenTaskDone: () => Promise.resolve(() => undefined),
  toIpcError: (value: unknown) => value
}));

import { planOperation } from "./lib/ipc";

async function planThrough(user: ReturnType<typeof userEvent.setup>) {
  await user.type(screen.getByLabelText(/目标根目录/), "/root");
  // 精确到"每行一个相对路径"那条标签：卡片标题也叫"清单建档（F11）"。
  await user.type(screen.getByLabelText(/每行一个相对路径/), "a/b");
  await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
  await waitFor(() => expect(planOperation).toHaveBeenCalled());
}

describe("功能页的集中报告（warnings）逐项点名", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(planOperation).mockResolvedValue(warningPlan);
  });

  it("类别标题走设计 06 §2A 的界面文案，不露出内核 slug", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="f11" onBack={() => undefined} />);
    await planThrough(user);

    // 此前这里断的是 `parent-is-file` 本身——把线协议串当文案钉进了测试（09 §2）。
    const kind = await screen.findByText("父级位置是文件");
    const entry = kind.closest(".warning");
    expect(entry).not.toBeNull();
    expect(entry).toHaveClass("caution");
    // 建议行仍来自内核（是否继续原样显示属设计 06 §5 待决），本批不改它的显示
    expect(screen.getByText(/parent \/root\/a is a file/)).toBeVisible();
    expect(screen.queryByText("parent-is-file")).toBeNull();
  });

  it("界面没跟上的类别显示成缺陷，而不是伪装成一条正常文案", async () => {
    vi.mocked(planOperation).mockResolvedValue({
      ...warningPlan,
      warnings: [{ kind: "brand-new-category", riskLevel: "caution", suggestion: "not mapped yet" }]
    });
    const user = userEvent.setup();
    render(<FeaturePages view="f11" onBack={() => undefined} />);
    await planThrough(user);
    expect(await screen.findByText(/未登记的预警类别（brand-new-category）/)).toBeInTheDocument();
  });

  it("对照：计划没有 warnings 时不得凭空出现集中报告块", async () => {
    vi.mocked(planOperation).mockResolvedValue({
      ...warningPlan,
      warnings: [],
      stats: { total: 2, conflict: 0, warning: 0, blocked: 0 }
    });
    const user = userEvent.setup();
    render(<FeaturePages view="f11" onBack={() => undefined} />);
    await planThrough(user);
    await screen.findByText("/root/a/b");
    expect(document.querySelector(".warnings")).toBeNull();
  });
});
