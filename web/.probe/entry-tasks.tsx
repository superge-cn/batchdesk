// 「任务与撤销」视图行为探针：对应 web/src/TaskHistory.test.tsx 的 9 条断言。
// 任务列表要兑现三件事——看得到状态、取消得了在跑的、撤得回已经执行的
// （撤销按 plan 走共享 Journal，不是页面自建；判据是 job 不是 kind）。
import { act } from "react";
import { createRoot } from "react-dom/client";
import { TaskHistory } from "../src/TaskHistory";
import { state, resetState } from "./fake-ipc";

const results: Array<[string, boolean, string]> = [];
function check(name: string, ok: boolean, detail = "") {
  results.push([name, ok, detail]);
}

import { settle } from "./settle";

type Live = Record<string, { state: string; phase: string; done: number; total: number }>;

function listed() {
  return [
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
}

async function renderView(live: Live = {}) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(<TaskHistory live={live} onBack={() => undefined} />);
  });
  return { root, host };
}

function bodyText(): string {
  return document.body.textContent ?? "";
}

function rowOf(id: string): HTMLElement | null {
  // 行内显示的是 shortId（…末 10 位），完整 taskId 只在 `title` 属性里（TaskHistory.tsx）。
  const span = document.querySelector(`[title="${id}"]`);
  return span ? (span.closest(".row") as HTMLElement | null) : null;
}

function rowButton(row: HTMLElement, label: string, id?: string): HTMLElement | undefined {
  // 操作按钮是图标＋aria-label（完整 id 在 aria-label 里），文本匹配不到。
  return Array.from(row.querySelectorAll("button")).find((b) => {
    const aria = b.getAttribute("aria-label") ?? "";
    // 有些按钮把完整 id 放进 aria-label，有些（如中断的「续跑」）只有可见文本。
    return (aria.includes(label) && (id ? aria.includes(id) : true)) || (b.textContent ?? "").includes(label);
  });
}

async function clickEl(el: Element) {
  await act(async () => {
    el.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
  });
}

async function main() {
  // ① 列出任务并把状态、进度如实呈现
  {
    resetState();
    state.listOverride = { items: listed(), total: 3 };
    const { root, host } = await renderView();
    const listedShown = await settle("列表加载", () => (bodyText().includes("共 3 条任务") ? true : undefined));
    check("列出任务并显示总数", !!listedShown && bodyText().includes("共 3 条任务"));
    check("部分失败状态如实呈现", bodyText().includes("部分失败"));
    check("已完成状态如实呈现", bodyText().includes("已完成"));
    check("进度 2/3 呈现", bodyText().includes("2/3"));
    await act(async () => root.unmount());
    host.remove();
  }

  // ② 功能码给名字，码位仍保留；行内不露内部编码
  {
    resetState();
    state.listOverride = { items: listed(), total: 3 };
    const { root, host } = await renderView();
    const named = await settle("功能码名字", () =>
      /编码转换（C01）|查找替换（C03）/.test(bodyText()) ? true : undefined
    );
    check("功能码显示名字（编码转换/查找替换）", !!named);
    const row = await settle("定位 0002 行", () => rowOf("01TASK-APPLY-0002") || undefined);
    check("行内不露内部编码 c03", !!row && !/\bc03\b/.test(row.textContent ?? ""));
    await act(async () => root.unmount());
    host.remove();
  }

  // ③ 执行中的任务可取消
  {
    resetState();
    state.listOverride = { items: listed(), total: 3 };
    const { root, host } = await renderView();
    const cancel = await settle("取消按钮", () => {
      const row = rowOf("01TASK-APPLY-0002");
      return (row && rowButton(row, "取消任务", "01TASK-APPLY-0002")) || undefined;
    });
    check("在跑任务有「取消任务」按钮", !!cancel);
    if (cancel) await clickEl(cancel);
    const cancelled = await settle("cancelTask 调用", () => (state.cancelCalls.includes("01TASK-APPLY-0002") ? true : undefined));
    check("调用 cancelTask(taskId)", !!cancelled, JSON.stringify(state.cancelCalls));
    const noted = await settle("取消已提交提示", () => (bodyText().includes("取消已提交") ? true : undefined));
    check("显示「取消已提交」", !!noted);
    await act(async () => root.unmount());
    host.remove();
  }

  // ④ 失败任务可重试
  {
    resetState();
    state.listOverride = { items: listed(), total: 3 };
    const { root, host } = await renderView();
    const retry = await settle("重试按钮", () => {
      const row = rowOf("01TASK-APPLY-0001");
      return (row && rowButton(row, "重试失败项", "01TASK-APPLY-0001")) || undefined;
    });
    check("失败任务有「重试失败项」按钮", !!retry);
    if (retry) await clickEl(retry);
    const retried = await settle("retryTask 调用", () => (state.retryCalls.includes("01TASK-APPLY-0001") ? true : undefined));
    check("调用 retryTask(taskId)", !!retried, JSON.stringify(state.retryCalls));
    await act(async () => root.unmount());
    host.remove();
  }

  // ⑤ 撤销按 planId 走 undoPlan；undo 任务不提供重试
  {
    resetState();
    state.listOverride = { items: listed(), total: 3 };
    const { root, host } = await renderView();
    const undo = await settle("撤销计划按钮", () => {
      const row = rowOf("01TASK-APPLY-0001");
      return (row && rowButton(row, "撤销计划", "01PLAN-A")) || undefined;
    });
    check("apply 任务有「撤销计划 01PLAN-A」按钮", !!undo);
    if (undo) await clickEl(undo);
    const undone = await settle("undoPlan 调用", () => (state.undoCalls.includes("01PLAN-A") ? true : undefined));
    check("调用 undoPlan(planId)", !!undone, JSON.stringify(state.undoCalls));
    const undoRow = rowOf("01TASK-UNDO-0003");
    check("undo 任务不提供「重试失败项」", !undoRow || !rowButton(undoRow, "重试失败项"));
    await act(async () => root.unmount());
    host.remove();
  }

  // ⑥ 中断任务：续跑/回滚/放弃，而不是取消
  {
    resetState();
    state.listOverride = {
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
    };
    const { root, host } = await renderView();
    const noted = await settle("中断标识", () => (/检测|已中断|中断/.test(bodyText()) ? true : undefined));
    check("中断状态有标识", !!noted);
    const row = await settle("定位中断行", () => rowOf("01TASK-INTERRUPTED") || undefined);
    check("中断任务不给「取消任务」", !!row && !rowButton(row, "取消任务"));
    const resume = row ? rowButton(row, "续跑", "01TASK-INTERRUPTED") : undefined;
    check("中断任务给「续跑」", !!resume);
    if (resume) await clickEl(resume);
    const recovered = await settle("recoverTask 调用", () =>
      state.recoverCalls.some(([id, action]) => id === "01TASK-INTERRUPTED" && action === "resume") ? true : undefined
    );
    check("调用 recoverTask(taskId, resume)", !!recovered, JSON.stringify(state.recoverCalls));
    await act(async () => root.unmount());
    host.remove();
  }

  // ⑦ 回滚任务不给二次撤销/重试（判据是 job 不是 kind）
  {
    resetState();
    state.listOverride = {
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
    };
    const { root, host } = await renderView();
    const row = await settle("定位回滚失败行", () => rowOf("01TASK-UNDO-FAILED") || undefined);
    check("回滚失败任务显示「部分失败」", !!row && (row.textContent ?? "").includes("部分失败"));
    check("回滚任务不给「重试失败项」", !!row && !rowButton(row, "重试失败项"));
    check("回滚任务不给「撤销计划」（防二次回滚）", !!row && !rowButton(row, "撤销计划"));
    await act(async () => root.unmount());
    host.remove();
  }

  // ⑧ 实时事件态覆盖静态快照
  {
    resetState();
    state.listOverride = { items: listed(), total: 3 };
    const live: Live = {
      "01TASK-APPLY-0002": { state: "completed", phase: "finalizing", done: 10, total: 10 }
    };
    const { root, host } = await renderView(live);
    const row = await settle("定位 0002 行", () => rowOf("01TASK-APPLY-0002") || undefined);
    check("实时态覆盖为「已完成」", !!row && (row.textContent ?? "").includes("已完成"));
    check("完成后取消按钮消失", !!row && !rowButton(row, "取消任务"));
    await act(async () => root.unmount());
    host.remove();
  }

  // ⑨ 加载失败：本地化文案 + 码位句柄，不渲染后端原文
  {
    resetState();
    state.listReject = {
      code: "E2002",
      message: "permission denied while reading /home/operator/private/notes.txt",
      detail: "os error 13 at src/tasks.rs:412",
      retryable: false
    };
    const { root, host } = await renderView();
    const alert = await settle("错误横幅", () => (host.querySelector('[role="alert"]') as HTMLElement) || undefined);
    check("加载失败出 alert 横幅", !!alert);
    const text = alert?.textContent ?? "";
    if (!text.includes("没有访问权限")) {
      require("node:fs").writeSync(1, "    DEBUG banner=" + text.slice(0, 200) + "\n");
    }
    check("显示本地化文案「没有访问权限」", text.includes("没有访问权限"));
    check("保留码位 E2002", text.includes("E2002"));
    check("不渲染路径", !text.includes("/home/operator"));
    check("不渲染 message 原文", !text.includes("permission denied"));
    check("不渲染 detail", !text.includes("os error 13"));
    await act(async () => root.unmount());
    host.remove();
  }

  let failed = 0;
  for (const [name, ok, detail] of results) {
    if (!ok) failed += 1;
    const suffix = !ok && detail ? "  (" + detail + ")" : "";
    require("node:fs").writeSync(1, (ok ? "PASS  " : "FAIL  ") + name + suffix + "\n");
  }
  require("node:fs").writeSync(1, "\n" + (results.length - failed) + "/" + results.length + " passed\n");
  process.exit(failed === 0 ? 0 : 1);
}

void main();
