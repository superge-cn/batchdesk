import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { TaskHistory } from "./TaskHistory";
import type { TaskSummary } from "./lib/ipc/types";

// 「任务与撤销」视图：任务列表要能兑现三件事——看得到状态、取消得了在跑的、
// 撤得回已经执行的（撤销按 plan 走共享 Journal，不是页面自建）。
const listed: TaskSummary[] = [
  {
    taskId: "01TASK-APPLY-0001",
    kind: "c09",
    job: "apply",
    state: "partial-failed",
    phase: "finalizing",
    done: 2,
    total: 3,
    planId: "01PLAN-A",
    startedAt: "2026-11-03T10:00:00Z",
    finishedAt: "2026-11-03T10:01:00Z"
  },
  {
    taskId: "01TASK-APPLY-0002",
    kind: "c03",
    job: "apply",
    state: "running",
    phase: "executing",
    done: 1,
    total: 10,
    planId: "01PLAN-B",
    startedAt: "2026-11-03T09:00:00Z"
  },
  {
    taskId: "01TASK-UNDO-0003",
    kind: "c09",
    job: "undo",
    state: "completed",
    phase: "finalizing",
    done: 3,
    total: 3,
    planId: "01PLAN-C"
  }
];

vi.mock("./lib/ipc", () => ({
  isTauri: () => true,
  // 列宽把手会读界面状态；这些用例不测它，给一个空的稳定桩。
  getUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  setUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  listTasks: vi.fn(async () => ({ items: listed, total: listed.length })),
  cancelTask: vi.fn(async () => undefined),
  retryTask: vi.fn(async () => ({ taskId: "01TASK-RETRY" })),
  undoPlan: vi.fn(async () => ({ taskId: "01TASK-UNDO-NEW" })),
  recoverTask: vi.fn(async () => ({ taskId: "01TASK-RECOVER" })),
  toIpcError: (value: unknown) => value
}));

import { cancelTask, listTasks, recoverTask, retryTask, setUiState, undoPlan } from "./lib/ipc";

function renderView(live: Parameters<typeof TaskHistory>[0]["live"] = {}) {
  return render(<TaskHistory live={live} onBack={() => undefined} />);
}

describe("任务与撤销视图", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("列出任务并把状态、进度如实呈现（不是只显示 id）", async () => {
    renderView();
    await waitFor(() => expect(listTasks).toHaveBeenCalled());
    expect(await screen.findByText(/共 3 条任务/)).toBeInTheDocument();
    expect(screen.getByText(/部分失败/)).toBeInTheDocument();
    expect(screen.getByText(/已完成/)).toBeInTheDocument();
    expect(screen.getByText(/2\/3/)).toBeInTheDocument();
  });

  it("功能码在列表里给名字，码位仍然保留（09 §2／06 §3 的支持句柄口径）", async () => {
    renderView();
    // 此前这里印的是 `c03`／`c09`——对使用者是内部编码；名字来自侧栏那份表（navCatalog 唯一来源）。
    expect(await screen.findByText(/编码转换（C01）|查找替换（C03）/)).toBeInTheDocument();
    const row = (await screen.findByText(/01TASK-APPLY-0002/)).closest(".row") as HTMLElement;
    expect(row.textContent).not.toMatch(/\bc03\b/);
  });

  it("执行中的任务可取消，调用 cancelTask", async () => {
    const user = userEvent.setup();
    renderView();
    const cancel = await screen.findByRole("button", { name: /取消任务 01TASK-APPLY-0002/ });
    await user.click(cancel);
    await waitFor(() => expect(cancelTask).toHaveBeenCalledWith("01TASK-APPLY-0002"));
    expect(await screen.findByText(/取消已提交/)).toBeInTheDocument();
  });

  it("失败任务可重试，调用 retryTask（只重跑失败条目）", async () => {
    const user = userEvent.setup();
    renderView();
    const retry = await screen.findByRole("button", { name: /重试失败项 01TASK-APPLY-0001/ });
    await user.click(retry);
    await waitFor(() => expect(retryTask).toHaveBeenCalledWith("01TASK-APPLY-0001"));
  });

  it("撤销按 planId 走 undoPlan，且 undo 任务自己不提供重试", async () => {
    const user = userEvent.setup();
    renderView();
    const undo = await screen.findByRole("button", { name: /撤销计划 01PLAN-A/ });
    await user.click(undo);
    await waitFor(() => expect(undoPlan).toHaveBeenCalledWith("01PLAN-A"));
    // undo 任务的重试无意义：壳层只接受 apply 任务重试
    expect(screen.queryByRole("button", { name: /重试失败项 01TASK-UNDO-0003/ })).toBeNull();
  });

  it("中断任务给的是续跑/回滚/放弃，而不是取消", async () => {
    vi.mocked(listTasks).mockResolvedValueOnce({
      items: [
        {
          taskId: "01TASK-INTERRUPTED",
          kind: "c09",
          job: "apply",
          state: "interrupted",
          phase: "executing",
          done: 1,
          total: 4,
          planId: "01PLAN-I"
        }
      ],
      total: 1
    });
    const user = userEvent.setup();
    renderView();
    expect(await screen.findByText(/检测|已中断|中断/)).toBeInTheDocument();
    const row = screen.getByText(/01TASK-INTERRUPTED/).closest(".row") as HTMLElement;
    expect(within(row).queryByRole("button", { name: /取消任务/ })).toBeNull();
    await user.click(within(row).getByRole("button", { name: /续跑/ }));
    await waitFor(() => expect(recoverTask).toHaveBeenCalledWith("01TASK-INTERRUPTED", "resume"));
  });

  it("回滚任务不给二次撤销，回滚失败也不给重试（壳层会拒，判据是 job 不是 kind）", async () => {
    // 真实可达形态：撤销任务的部分条目回滚失败 → 状态 partial-failed，kind 仍是"被回滚
    // 计划的功能码"（壳层从不发 kind:"undo"）。旧实现按 kind !== "undo" 判别，于是给出
    // 「重试失败项」，而 retryTask 对撤销任务必然 E1002；「撤销」同理造成二次回滚。
    vi.mocked(listTasks).mockResolvedValueOnce({
      items: [
        {
          taskId: "01TASK-UNDO-FAILED",
          kind: "f12",
          job: "undo",
          state: "partial-failed",
          phase: "finalizing",
          done: 1,
          total: 2,
          planId: "01PLAN-D"
        }
      ],
      total: 1
    });
    renderView();
    await waitFor(() => expect(listTasks).toHaveBeenCalled());
    const row = (await screen.findByText(/01TASK-UNDO-FAILED/)).closest(".row") as HTMLElement;
    expect(within(row).queryByRole("button", { name: /重试失败项/ })).toBeNull();
    expect(within(row).queryByRole("button", { name: /撤销计划/ })).toBeNull();
    // 回滚为终态：只留取消/恢复类入口，不给再次回滚的通道
    expect(within(row).getByText(/部分失败/)).toBeInTheDocument();
  });

  it("实时事件态覆盖静态快照：跑完的任务立刻显示已完成、取消按钮消失", async () => {
    renderView({
      "01TASK-APPLY-0002": {
        state: "completed",
        phase: "finalizing",
        done: 10,
        total: 10
      }
    });
    await waitFor(() => expect(listTasks).toHaveBeenCalled());
    const row = (await screen.findByText(/01TASK-APPLY-0002/)).closest(".row") as HTMLElement;
    expect(within(row).getByText(/已完成/)).toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: /取消任务/ })).toBeNull();
  });

  it("加载失败时只显示本地化文案，不渲染后端 message／detail 原文", async () => {
    // 06 §3：文案不得暴露路径、文件名或堆栈；detail 仅进诊断日志。
    vi.mocked(listTasks).mockRejectedValueOnce({
      code: "E2002",
      message: "permission denied while reading /home/operator/private/notes.txt",
      detail: "os error 13 at src/tasks.rs:412",
      retryable: false
    });
    renderView();
    const banner = await screen.findByRole("alert");
    expect(within(banner).getByText(/没有访问权限/)).toBeInTheDocument();
    // 码位本身保留为支持句柄
    expect(within(banner).getByText("E2002")).toBeInTheDocument();
    expect(banner.textContent).not.toContain("/home/operator");
    expect(banner.textContent).not.toContain("permission denied");
    expect(banner.textContent).not.toContain("os error 13");
  });
});

describe("任务表的列宽把手（设计 03 §11.2 的 tables）", () => {
  it("四个列都有分隔把手，键盘调宽写进 table.tasks", async () => {
    renderView();
    const handles = await screen.findAllByRole("separator");
    expect(handles.length).toBe(4);

    handles[0].focus();
    await userEvent.keyboard("{ArrowRight}");

    await waitFor(() =>
      expect(
        vi.mocked(setUiState).mock.calls.some(
          ([patch]) =>
            (patch as { tables?: Record<string, { columnWidths?: Record<string, number> }> })
              .tables?.["table.tasks"]?.columnWidths?.task !== undefined
        )
      ).toBe(true)
    );
  });

  it("列头排序按 TaskSummary 字段写 table.tasks.sort（actions 列不参与）", async () => {
    renderView();
    await screen.findAllByRole("separator");
    // task／progress／state 三列可排序；actions 是操作列，没有可比较的值（渲染成 span）。
    const sortButtons = screen.getAllByRole("button", { name: /排序/ });
    expect(sortButtons).toHaveLength(3);

    await userEvent.click(sortButtons[0]);
    await waitFor(() =>
      expect(
        vi.mocked(setUiState).mock.calls.some(
          ([patch]) =>
            (patch as { tables?: Record<string, { sort?: { column: string; direction: string } }> })
              .tables?.["table.tasks"]?.sort?.column === "task"
        )
      ).toBe(true)
    );
  });

  it("再点同一列翻成降序：排序是视图态，不改派发序", async () => {
    renderView();
    const [first] = await screen.findAllByRole("button", { name: /排序/ });
    await userEvent.click(first);
    await userEvent.click(first);
    await waitFor(() =>
      expect(
        vi.mocked(setUiState).mock.calls.some(
          ([patch]) =>
            (patch as { tables?: Record<string, { sort?: { column: string; direction: string } }> })
              .tables?.["table.tasks"]?.sort?.direction === "desc"
        )
      ).toBe(true)
    );
  });
});
