// 取消回顾（方案 §7.7）行为探针：对应 web/src/App.cancel-review.test.tsx 的 5 条断言。
// 事件驱动：App 挂载后订阅 task://done，探针从 fake-ipc.doneListeners 拿到回调后推事件。
import { act } from "react";
import { createRoot } from "react-dom/client";
import App from "../src/App";
import { doneListeners, state, resetState } from "./fake-ipc";

const results: Array<[string, boolean, string]> = [];
function check(name: string, ok: boolean, detail = "") {
  results.push([name, ok, detail]);
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function settle(label: string, fn: () => unknown, timeoutMs = 5000): Promise<boolean> {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    if (fn()) return true;
    await act(async () => {
      await sleep(20);
    });
  }
  console.warn("settle 超时：" + label);
  return false;
}

const OK = { entryId: "e1", status: "ok" as const };

function dialog() {
  return document.querySelector('[role="alertdialog"]');
}

function button(label: string): HTMLButtonElement | undefined {
  return Array.from(document.querySelectorAll("button")).find((b) =>
    (b.textContent ?? "").includes(label)
  );
}

async function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(<App />);
  });
  return { root, host };
}

/** 等 done 订阅挂上后推一个终态事件（与 committed 测试的 finish() 同构）。 */
async function finish(
  entries: Array<{ entryId: string; status: string }>,
  kind: "apply" | "undo" = "apply"
) {
  await act(async () => {
    const subscribed = await settle("done 订阅", () => (doneListeners.length > 0 ? true : undefined));
    if (!subscribed) throw new Error("App 未订阅 task://done");
    doneListeners.forEach((listener) =>
      listener({ taskId: "task-1", kind, summary: { planId: "01CANCEL", taskId: "task-1", entries } })
    );
    // done 处理器里还要 getTask 一次才拿到终态。
    await sleep(0);
  });
}

async function main() {
  // ① 取消且有已完成项：先问数量，确认后走 undoPlan
  {
    resetState();
    const { root, host } = await mount();
    await finish([OK, { entryId: "e2", status: "overwritten" }, { entryId: "e3", status: "skipped" }]);
    const shown = await settle("对话框出现", () => (dialog() ? true : undefined));
    const text = dialog()?.textContent ?? "";
    check("取消后弹撤销询问", !!shown);
    check("询问里写明已完成数量", text.includes("已完成 2 项，是否一并撤销？"), text.slice(0, 80));
    check("询问里写明已落盘并记 Journal", text.includes("这 2 项已经写入磁盘并记入 Journal"));
    check("初始焦点在「保留已完成项」", document.activeElement === button("保留已完成项"));
    check("确认前未调用 undoPlan", state.undoCalls.length === 0, JSON.stringify(state.undoCalls));
    const yes = button("一并撤销");
    if (yes) await act(async () => yes.dispatchEvent(new window.MouseEvent("click", { bubbles: true })));
    const undoDone = await settle("undoPlan 被调用", () => (state.undoCalls.length > 0 ? true : undefined));
    check("确认后走 undoPlan 且带 planId", !!undoDone && state.undoCalls[0] === "01CANCEL", JSON.stringify(state.undoCalls));
    check("确认后对话框关闭", dialog() === null);
    await act(async () => root.unmount());
    host.remove();
  }

  // ② 选择保留：不调用撤销
  {
    resetState();
    const { root, host } = await mount();
    await finish([OK]);
    await settle("对话框出现", () => (dialog() ? true : undefined));
    const keep = button("保留已完成项");
    if (keep) await act(async () => keep.dispatchEvent(new window.MouseEvent("click", { bubbles: true })));
    await act(async () => {
      await sleep(30);
    });
    check("保留后对话框关闭", dialog() === null);
    check("保留不调用 undoPlan", state.undoCalls.length === 0, JSON.stringify(state.undoCalls));
    await act(async () => root.unmount());
    host.remove();
  }

  // ③ 对照：一条都没完成时不得打扰
  {
    resetState();
    const { root, host } = await mount();
    await finish([{ entryId: "e1", status: "skipped" }]);
    const quiet = await settle("等待可能的对话框", () => true);
    check("零完成不弹询问", dialog() === null);
    await act(async () => root.unmount());
    host.remove();
  }

  // ④ 对照：撤销任务被取消时不得问（只对 apply 成立）
  {
    resetState();
    const { root, host } = await mount();
    await finish([OK], "undo");
    await settle("等待", () => true);
    check("undo 任务被取消不弹询问", dialog() === null);
    await act(async () => root.unmount());
    host.remove();
  }

  // ⑤ 对照：正常完成不弹询问
  {
    resetState();
    state.settledState = "completed";
    const { root, host } = await mount();
    await finish([OK, { entryId: "e2", status: "ok" }]);
    await settle("等待", () => true);
    check("正常完成不弹询问", dialog() === null);
    await act(async () => root.unmount());
    host.remove();
  }

  let failed = 0;
  for (const [name, ok, detail] of results) {
    if (!ok) failed += 1;
    const suffix = !ok && detail ? "  (" + detail + ")" : "";
    console.log((ok ? "PASS  " : "FAIL  ") + name + suffix);
  }
  console.log("\n" + (results.length - failed) + "/" + results.length + " passed");
  process.exit(failed === 0 ? 0 : 1);
}

void main();
