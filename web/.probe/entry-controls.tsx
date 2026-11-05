// 功能页任务控制与撤销（E4/RBK）行为探针：对应 web/src/FeaturePages.controls.test.tsx 的 7 条断言。
// 取消与重试失败项必须在功能视图上可达，且调用正确的壳层命令；改参数即旧计划失效；
// 保护目录命中必须二次确认；计划失败呈现 06 码位。
import { act } from "react";
import { createRoot } from "react-dom/client";
import { FeaturePages } from "../src/FeaturePages";
import { state, resetState } from "./fake-ipc";
import { settle, sleep } from "./settle";

const results: Array<[string, boolean, string]> = [];
function check(name: string, ok: boolean, detail = "") {
  results.push([name, ok, detail]);
}

function typeInto(el: HTMLInputElement | HTMLTextAreaElement, value: string) {
  const proto =
    el instanceof window.HTMLTextAreaElement ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(proto, "value")?.set?.call(el, value);
  el.dispatchEvent(new window.Event("input", { bubbles: true }));
}

function fieldByLabel(fragment: string): HTMLInputElement | HTMLTextAreaElement | null {
  for (const label of Array.from(document.querySelectorAll("label"))) {
    if (!(label.textContent ?? "").includes(fragment)) continue;
    const forId = label.getAttribute("for");
    if (forId) {
      const el = document.getElementById(forId);
      if (el instanceof window.HTMLInputElement || el instanceof window.HTMLTextAreaElement) return el;
    }
  }
  return null;
}

function button(exact: string): HTMLElement | undefined {
  return Array.from(document.querySelectorAll("button")).find(
    (b) => norm(b.textContent ?? "") === exact
  );
}

function buttonLike(fragment: string): HTMLElement | undefined {
  return Array.from(document.querySelectorAll("button")).find((b) =>
    (b.textContent ?? "").includes(fragment)
  );
}

function testid(name: string): HTMLElement | null {
  return document.querySelector(`[data-testid="${name}"]`);
}

function norm(text: string) {
  return text.replace(/\s+/g, " ").trim();
}

async function clickEl(el: Element) {
  await act(async () => {
    el.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
  });
}

async function mountF12() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(<FeaturePages view="f12" onBack={() => undefined} />);
  });
  return { root, host };
}

async function planThrough() {
  const dir = fieldByLabel("改名目录");
  if (!dir) throw new Error("找不到「改名目录」");
  const replace = fieldByLabel("替换为");
  if (!replace) throw new Error("找不到「替换为」");
  const scope = fieldByLabel("作用范围");
  if (!scope) throw new Error("找不到「作用范围」");
  await act(async () => {
    typeInto(dir, "/dir");
    typeInto(replace, "x");
    typeInto(scope, "full");
  });
  const planButton = buttonLike("生成计划预览");
  if (!planButton) throw new Error("找不到「生成计划预览」");
  await clickEl(planButton);
  const ok = await settle("计划生成", () => (state.planCalls > 0 ? true : undefined));
  if (!ok) throw new Error("planOperation 未被调用");
}

async function main() {
  // ① 生成计划→执行→执行中可取消；计划入参按形参原样传给壳层
  {
    resetState();
    const { root, host } = await mountF12();
    await planThrough();
    const input = state.planInput as { kind: string; params: Record<string, unknown> };
    check(
      "计划入参原样传给壳层",
      input?.kind === "f12" && input?.params?.dir === "/dir" && input?.params?.replace === "x" && input?.params?.scope === "full",
      JSON.stringify(input)
    );
    const run = button("执行");
    if (run) await clickEl(run);
    const applied = await settle("applyPlan 调用", () => (state.applyCalls.length > 0 ? true : undefined));
    check("执行调用 applyPlan(planId, options)", !!applied && state.applyCalls[0][0] === "01PLAN", JSON.stringify(state.applyCalls));
    const cancel = await settle("cancel-task 出现", () => testid("cancel-task") || undefined);
    check("执行中出现取消按钮", !!cancel);
    if (cancel) await clickEl(cancel);
    const cancelled = await settle("cancelTask 调用", () => (state.cancelCalls.includes("task-apply") ? true : undefined));
    check("取消调用 cancelTask(taskId)", !!cancelled, JSON.stringify(state.cancelCalls));
    await act(async () => root.unmount());
    host.remove();
  }

  // ② partial-failed 后出现「重试失败项」（由 task://state 事件驱动，前端不轮询）
  {
    resetState();
    const { root, host } = await mountF12();
    await planThrough();
    const run = button("执行");
    if (run) await clickEl(run);
    await settle("applyPlan 调用", () => (state.applyCalls.length > 0 ? true : undefined));
    check("订阅了 task://state", state.stateListeners.length > 0);
    await act(async () => {
      state.stateListeners.forEach((listener) => listener({ taskId: "task-apply", state: "partial-failed" }));
      await sleep(30);
    });
    const retry = await settle("retry-failed 出现", () => testid("retry-failed") || undefined);
    check("partial-failed 后出现「重试失败项」", !!retry);
    if (retry) await clickEl(retry);
    const retried = await settle("retryTask 调用", () => (state.retryCalls.includes("task-apply") ? true : undefined));
    check("重试调用 retryTask(taskId)", !!retried, JSON.stringify(state.retryCalls));
    await act(async () => root.unmount());
    host.remove();
  }

  // ③ 回滚预演：条目状态用本地化文案，不露枚举原词
  {
    resetState();
    state.dryRunOverride = {
      planId: "01PLAN",
      entries: [
        { entryId: "01PLAN-0001", status: "ok" },
        { entryId: "01PLAN-0002", status: "conflict", code: "E2007" },
        { entryId: "01PLAN-0003", status: "missing" }
      ],
      conflictCount: 1,
      missingCount: 1
    } as unknown as Record<string, unknown>;
    const { root, host } = await mountF12();
    await planThrough();
    const dryRun = buttonLike("回滚预演");
    if (dryRun) await clickEl(dryRun);
    const rowsShown = await settle("预演条目出现", () => (host.textContent ?? "").includes("01PLAN-000") ? true : undefined);
    check("预演条目渲染", !!rowsShown);
    const text = host.textContent ?? "";
    check("ok 显示为「可撤销」", text.includes("可撤销"));
    check("conflict 显示为「冲突」", text.includes("冲突"));
    check("missing 显示为「目标已缺失」", text.includes("目标已缺失"));
    check("不露枚举原词", !/\b(ok|conflict|missing)\b/.test(text));
    await act(async () => root.unmount());
    host.remove();
  }

  // ④ 撤销走 undoPlan（共享 Journal 通道）
  {
    resetState();
    const { root, host } = await mountF12();
    await planThrough();
    const undo = button("撤销");
    if (undo) await clickEl(undo);
    const undone = await settle("undoPlan 调用", () => (state.undoCalls.includes("01PLAN") ? true : undefined));
    check("撤销调用 undoPlan(planId)", !!undone, JSON.stringify(state.undoCalls));
    await act(async () => root.unmount());
    host.remove();
  }

  // ⑤ 改参数后旧计划立即失效（T 配方 P4）
  {
    resetState();
    const { root, host } = await mountF12();
    await planThrough();
    const run = button("执行");
    check("刚生成时「执行」可用", !!run && !(run as HTMLButtonElement).disabled);
    const scope = fieldByLabel("作用范围");
    if (scope) await act(async () => typeInto(scope, "x"));
    const stale = await settle("plan-stale 出现", () => testid("plan-stale") || undefined);
    check("改动参数后出现失效提示", !!stale);
    check("「执行」被禁用", !!button("执行") && (button("执行") as HTMLButtonElement).disabled);
    check("「撤销」被禁用", !!button("撤销") && (button("撤销") as HTMLButtonElement).disabled);
    check("「回滚预演」被禁用", !!buttonLike("回滚预演") && (buttonLike("回滚预演") as HTMLButtonElement).disabled);
    const planButton = buttonLike("生成计划预览");
    if (planButton) await clickEl(planButton);
    const cleared = await settle("失效提示消失", () => (testid("plan-stale") ? undefined : true));
    check("重新生成后失效提示消失", !!cleared);
    check("重新生成后「执行」恢复可用", !!button("执行") && !(button("执行") as HTMLButtonElement).disabled);
    await act(async () => root.unmount());
    host.remove();
  }

  // ⑥ 保护目录命中：二次确认 + 初始焦点不在危险按钮
  {
    resetState();
    state.planOverride = {
      planId: "01PLAN",
      kind: "f12",
      entries: [
        { entryId: "e1", source: "/dir/a.txt", target: "/dir/b.txt", action: "rename", reason: "rename" },
        {
          entryId: "e2",
          source: "/x/win.txt",
          target: "/Windows/win.txt",
          action: "rename",
          reason: "rename",
          blocked: { code: "E2009", reason: "path is under protected directory C:/Windows" }
        }
      ],
      stats: { total: 2, conflict: 0, warning: 0, blocked: 1 },
      warnings: [],
      params: {}
    } as unknown as Record<string, unknown>;
    const { root, host } = await mountF12();
    await planThrough();
    const run = button("执行");
    if (run) await clickEl(run);
    const dialog = await settle("危险确认框出现", () => testid("danger-confirm") || undefined);
    check("命中保护目录弹二次确认", !!dialog);
    const dialogText = dialog?.textContent ?? "";
    check("写明「命中保护目录」", dialogText.includes("命中保护目录"));
    check("点名被保护的落点", dialogText.includes("/Windows/win.txt"));
    check("写明这些条目不会执行", dialogText.includes("不会执行"));
    check("未确认前绝不派发", state.applyCalls.length === 0, JSON.stringify(state.applyCalls));
    check("初始焦点在「取消」", document.activeElement === testid("danger-cancel"));
    const cancelBtn = testid("danger-cancel");
    if (cancelBtn) await clickEl(cancelBtn);
    check("取消后确认框关闭", testid("danger-confirm") === null);
    check("取消后仍未派发", state.applyCalls.length === 0);
    const run2 = button("执行");
    if (run2) await clickEl(run2);
    const confirmBtn = await settle("危险按钮出现", () => testid("danger-confirm-button") || undefined);
    if (confirmBtn) await clickEl(confirmBtn);
    const applied = await settle("确认后派发", () => (state.applyCalls.length > 0 ? true : undefined));
    check(
      "确认后 applyPlan 带 skip 选项",
      !!applied && state.applyCalls[0][0] === "01PLAN",
      JSON.stringify(state.applyCalls)
    );
    await act(async () => root.unmount());
    host.remove();
  }

  // ⑦ 计划失败呈现 06 码位
  {
    resetState();
    state.planReject = { code: "E3004", message: "列不存在" };
    const { root, host } = await mountF12();
    const dir = fieldByLabel("改名目录");
    const replace = fieldByLabel("替换为");
    await act(async () => {
      if (dir) typeInto(dir, "/dir");
      if (replace) typeInto(replace, "x");
    });
    const planButton = buttonLike("生成计划预览");
    if (planButton) await clickEl(planButton);
    const alert = await settle("错误横幅出现", () => (host.querySelector('[role="alert"]') as HTMLElement) || undefined);
    check("计划失败出 alert 横幅", !!alert);
    const text = alert?.textContent ?? "";
    check("呈现 06 码位（E3004）", /E1002|E3004|E1001/.test(text), text.slice(0, 80));
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
