// App 级行为探针：对应 web/src/App.uiState.test.tsx 的 5 条断言。
// 执行器是 esbuild + jsdom（无 WASM），断言的是同一批用户可见行为。
import { act } from "react";
import { createRoot } from "react-dom/client";
import App from "../src/App";
import { state, resetState } from "./fake-ipc";

const results: Array<[string, boolean, string]> = [];
function check(name: string, ok: boolean, detail = "") {
  results.push([name, ok, detail]);
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** 轮询直到谓词返回真值（React 的 effect / promise 链需要若干微任务+宏任务）。 */
async function settle<T>(label: string, fn: () => T | undefined, timeoutMs = 5000): Promise<T | undefined> {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const value = fn();
    if (value) return value;
    await act(async () => {
      await sleep(20);
    });
  }
  console.warn("settle 超时：" + label);
  return undefined;
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

async function unmount(root: ReturnType<typeof createRoot>, host: HTMLElement) {
  await act(async () => root.unmount());
  host.remove();
}

function rootInput(): HTMLInputElement | null {
  return document.querySelector("input#root");
}

function navButton(label: string): HTMLButtonElement | undefined {
  return Array.from(document.querySelectorAll("button")).find((b) =>
    (b.textContent ?? "").includes(label)
  );
}

async function click(el: Element) {
  await act(async () => {
    el.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
  });
}

async function main() {
  // ① 启动预填上次目录 + 候选
  {
    resetState({ lastDirectory: "/data/last", recentDirectories: ["/data/last", "/data/older"] });
    const { root, host } = await mount();
    const input = await settle("预填目录", () => {
      const el = rootInput();
      return el && el.value === "/data/last" ? el : undefined;
    });
    check("启动预填上次目录", !!input, input ? "" : "input.value 未变成 /data/last");
    const options = Array.from(document.querySelectorAll("#c09-recent-dirs option")).map((o) =>
      o.getAttribute("value")
    );
    check(
      "最近目录挂成候选",
      options.length === 2 && options[0] === "/data/last" && options[1] === "/data/older",
      JSON.stringify(options)
    );
    await unmount(root, host);
  }

  // ② 生成预览后回写 UiState（去重置顶、兄弟字段不丢）
  {
    resetState({ lastSection: "c09", lastDirectory: "/data/last", recentDirectories: ["/data/last", "/data/older"] });
    const { root, host } = await mount();
    await settle("预填后再点预览", () => {
      const el = rootInput();
      return el && el.value === "/data/last" ? el : undefined;
    });
    const planButton = navButton("生成计划预览");
    if (planButton) await click(planButton);
    const wrote = await settle("回写 UiState", () => (state.setUiStateCalls.length > 0 ? true : undefined));
    check("生成预览后写回 UiState", !!wrote);
    const last = state.setUiStateCalls.at(-1) as
      | { navigation?: { lastDirectory?: string; recentDirectories?: string[]; lastSection?: string } }
      | undefined;
    check("lastDirectory 正确", last?.navigation?.lastDirectory === "/data/last", JSON.stringify(last?.navigation));
    check(
      "最近目录去重置顶",
      JSON.stringify(last?.navigation?.recentDirectories) === JSON.stringify(["/data/last", "/data/older"]),
      JSON.stringify(last?.navigation?.recentDirectories)
    );
    check("lastSection 未被擦掉", last?.navigation?.lastSection === "c09", String(last?.navigation?.lastSection));
    await unmount(root, host);
  }

  // ③ 无记录：不预填、不渲染候选
  {
    resetState();
    const { root, host } = await mount();
    await act(async () => {
      await sleep(100);
    });
    check("无记录时输入框为空", (rootInput()?.value ?? "") === "", JSON.stringify(rootInput()?.value));
    check("无记录时不渲染候选", document.querySelector("#c09-recent-dirs") === null);
    await unmount(root, host);
  }

  // ④ 启动回到上次的功能页
  {
    resetState({ lastSection: "feature", lastFeature: "f12" });
    const { root, host } = await mount();
    const active = await settle("回到 f12", () => {
      const btn = navButton("批量改名");
      return btn && btn.className.includes("active") ? btn : undefined;
    });
    check("启动回到上次功能页", !!active, active ? "" : "批量改名 未标记 active");
    await unmount(root, host);
  }

  // ⑤ 切页写回 lastSection
  {
    resetState();
    const { root, host } = await mount();
    const tasks = navButton("任务与撤销");
    if (tasks) await click(tasks);
    const wroteTasks = await settle("写回 tasks", () =>
      state.setUiStateCalls.length > 0 ? true : undefined
    );
    const afterTasks = state.setUiStateCalls.at(-1) as
      | { navigation?: { lastSection?: string } }
      | undefined;
    check("切到任务页写回 lastSection=tasks", !!wroteTasks && afterTasks?.navigation?.lastSection === "tasks", String(afterTasks?.navigation?.lastSection));

    const before = state.setUiStateCalls.length;
    const workspace = navButton("重复文件查找");
    if (workspace) await click(workspace);
    const wroteC09 = await settle("写回 c09", () =>
      state.setUiStateCalls.length > before ? true : undefined
    );
    const afterC09 = state.setUiStateCalls.at(-1) as
      | { navigation?: { lastSection?: string } }
      | undefined;
    check("切回工作台写回 lastSection=c09", !!wroteC09 && afterC09?.navigation?.lastSection === "c09", String(afterC09?.navigation?.lastSection));
    await unmount(root, host);
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
