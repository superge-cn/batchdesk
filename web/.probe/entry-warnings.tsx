// 功能页「集中报告」（warnings）行为探针：对应 web/src/FeaturePages.warnings.test.tsx 的 3 条断言。
// 场景是 F11 清单建档：父路径为文件时，计划级 warnings 必须在功能页**逐项点名**——
// 类别走界面文案、没跟上的类别显示成缺陷、没有 warnings 时不得凭空出现报告块。
import { act } from "react";
import { createRoot } from "react-dom/client";
import { FeaturePages } from "../src/FeaturePages";
import { PLAN, resetState, state } from "./fake-ipc";

const results: Array<[string, boolean, string]> = [];
function check(name: string, ok: boolean, detail = "") {
  results.push([name, ok, detail]);
}

import { settle } from "./settle";

/** React 受控输入：走原生 value setter + input 事件，等价于 RTL 的 user.type。 */
function typeInto(el: HTMLInputElement | HTMLTextAreaElement, value: string) {
  const proto = el instanceof window.HTMLTextAreaElement ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
  setter?.call(el, value);
  el.dispatchEvent(new window.Event("input", { bubbles: true }));
}

/** 按 label 文案找关联控件（SettingsPage/FeaturePages 都是 label[for] 风格）。 */
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

async function renderF11AndPlan() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(<FeaturePages view="f11" onBack={() => undefined} />);
  });
  const rootInput = await settle("目标根目录输入框", () => fieldByLabel("目标根目录") || undefined);
  if (!rootInput) throw new Error("找不到「目标根目录」输入框");
  const listInput = await settle("清单输入框", () => fieldByLabel("每行一个相对路径") || undefined);
  if (!listInput) throw new Error("找不到「每行一个相对路径」输入框");
  await act(async () => {
    typeInto(rootInput, "/root");
    typeInto(listInput, "a/b");
  });
  const planButton = Array.from(document.querySelectorAll("button")).find((b) =>
    (b.textContent ?? "").includes("生成计划预览")
  );
  if (!planButton) throw new Error("找不到「生成计划预览」");
  await act(async () => {
    planButton.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
  });
  const planned = await settle("计划生成", () => (state.planCalls > 0 ? true : undefined));
  if (!planned) throw new Error("planOperation 未被调用");
  return { root, host };
}

async function main() {
  const warningPlan = {
    ...PLAN,
    planId: "01F11PARENTFILE0000000000",
    kind: "f11",
    params: {},
    entries: [
      {
        entryId: "01F11PARENTFILE0000000000-0001",
        source: "",
        target: "/root/a",
        action: "create-dir",
        reason: "create directory /root/a"
      },
      {
        entryId: "01F11PARENTFILE0000000000-0002",
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

  // ① 类别标题走界面文案，不露出内核 slug
  {
    resetState();
    state.planOverride = warningPlan as unknown as Record<string, unknown>;
    const { root, host } = await renderF11AndPlan();
    const named = await settle("类别显示为界面文案", () =>
      (host.textContent ?? "").includes("父级位置是文件") ? true : undefined
    );
    check("类别显示「父级位置是文件」", !!named);
    const warningEl = Array.from(host.querySelectorAll(".warning")).find((el) =>
      (el.textContent ?? "").includes("父级位置是文件")
    );
    check("预警块带 caution 风险级", !!warningEl && warningEl.classList.contains("caution"));
    check("建议行保留内核原文", (host.textContent ?? "").includes("parent /root/a is a file"));
    check("不露出内核 slug parent-is-file", !(host.textContent ?? "").includes("parent-is-file"));
    await act(async () => root.unmount());
    host.remove();
  }

  // ② 界面没跟上的类别显示成缺陷，而不是伪装成正常文案
  {
    resetState();
    state.planOverride = {
      ...warningPlan,
      warnings: [{ kind: "brand-new-category", riskLevel: "caution", suggestion: "not mapped yet" }]
    } as unknown as Record<string, unknown>;
    const { root, host } = await renderF11AndPlan();
    const shown = await settle("未登记类别显示成缺陷", () =>
      (host.textContent ?? "").includes("未登记的预警类别（brand-new-category）") ? true : undefined
    );
    check("未登记类别显式暴露", !!shown);
    await act(async () => root.unmount());
    host.remove();
  }

  // ③ 对照：没有 warnings 时不得凭空出现集中报告块
  {
    resetState();
    state.planOverride = {
      ...warningPlan,
      warnings: [],
      stats: { total: 2, conflict: 0, warning: 0, blocked: 0 }
    } as unknown as Record<string, unknown>;
    const { root, host } = await renderF11AndPlan();
    const entryShown = await settle("计划条目渲染", () =>
      (host.textContent ?? "").includes("/root/a/b") ? true : undefined
    );
    check("计划条目正常渲染", !!entryShown);
    check("不凭空出现集中报告块", host.querySelector(".warnings") === null);
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
