import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FeaturePages } from "./FeaturePages";
import type { PlanDocument } from "./lib/ipc/types";

// C10 解压密码（ADR-0074）：密码是**执行期秘密**，界面这一侧的责任只有两条——
// 输入框不把明文摊在屏幕上、密码只随计划请求进壳层（计划文档里没有它）。
const PLAN_ID = "01C10PASSWORD0000000000000";

const plan: PlanDocument = {
  planId: PLAN_ID,
  kind: "c10",
  params: { maxTotalBytes: 10000, maxRatio: "100", passwordProvided: true },
  entries: [
    {
      entryId: `${PLAN_ID}-0001`,
      source: "/data/secret.zip",
      target: "/out",
      action: "extract",
      reason: "extract 2 files, 0 dirs"
    }
  ],
  stats: { total: 1, conflict: 0, warning: 1, blocked: 0 },
  warnings: [
    {
      kind: "encrypted-archive",
      riskLevel: "caution",
      suggestion: "encrypted entries are decrypted at execution"
    }
  ]
};

vi.mock("./lib/ipc", () => ({
  isTauri: () => true,
  // 列宽/排序把手会读界面状态；这些用例不测它，给一个空的稳定桩。
  getUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  setUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  planOperation: vi.fn(async () => plan),
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

describe("C10 解压密码（ADR-0074）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(planOperation).mockResolvedValue(plan);
  });

  it("密码字段是 type=password，且只在填写后才进计划请求", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="c10" onBack={() => undefined} />);

    const field = screen.getByLabelText(/解压密码/);
    expect(field.getAttribute("type")).toBe("password");
    expect(field.getAttribute("autocomplete")).toBe("off");

    await user.type(screen.getByLabelText(/压缩包路径/), "/data/secret.zip");
    await user.type(screen.getByLabelText(/解压输出目录/), "/out");
    await user.type(screen.getByLabelText(/解压总量上限/), "10000");
    await user.type(screen.getByLabelText(/解压比例上限/), "100");
    await user.type(field, "secret123");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));

    await waitFor(() => expect(planOperation).toHaveBeenCalled());
    const call = vi.mocked(planOperation).mock.calls[0][0] as {
      kind: string;
      params: Record<string, unknown>;
    };
    expect(call.kind).toBe("c10");
    expect(call.params.password).toBe("secret123");
  });

  it("不填密码时请求里没有这个字段（后端据此不置 passwordProvided）", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="c10" onBack={() => undefined} />);

    await user.type(screen.getByLabelText(/压缩包路径/), "/data/plain.zip");
    await user.type(screen.getByLabelText(/解压输出目录/), "/out");
    await user.type(screen.getByLabelText(/解压总量上限/), "10000");
    await user.type(screen.getByLabelText(/解压比例上限/), "100");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));

    await waitFor(() => expect(planOperation).toHaveBeenCalled());
    const call = vi.mocked(planOperation).mock.calls[0][0] as {
      params: Record<string, unknown>;
    };
    expect("password" in call.params).toBe(false);
  });
});
