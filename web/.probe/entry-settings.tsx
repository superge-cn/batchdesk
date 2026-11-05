// 不依赖 Vitest/Vite 的行为探针：渲染真实组件，断言"重置界面状态"区块。
// 与本仓 web/src/SettingsPage.test.tsx 的两条断言同义，只是换了执行器
// （当前宿主无法分配 WASM，Vitest/Vite 起不来）。
import { act } from "react";
import { createRoot } from "react-dom/client";
import { I18nProvider } from "../src/i18n";
import { SettingsPage } from "../src/SettingsPage";
import { state } from "./fake-ipc";

const results: Array<[string, boolean, string]> = [];
function check(name: string, ok: boolean, detail = "") {
  results.push([name, ok, detail]);
}

function bodyText(): string {
  return document.body.textContent ?? "";
}

async function renderWith(onReset?: () => Promise<void>) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(
      <I18nProvider initialLang="zh-CN" initialTheme="light">
        <SettingsPage onBack={() => undefined} onResetUiState={onReset} />
      </I18nProvider>
    );
  });
  return { root, host };
}

function findButton(host: HTMLElement, label: string): HTMLButtonElement | undefined {
  return Array.from(host.querySelectorAll("button")).find((b) =>
    (b.textContent ?? "").includes(label)
  );
}

async function main() {
  // ① 有回调：区块渲染、点击调用一次、给出提示、不碰 ConfigDocument 命令
  {
    let called = 0;
    const { root, host } = await renderWith(async () => {
      called += 1;
    });
    check("区块标题渲染", bodyText().includes("本机界面状态"));
    const button = findButton(host, "重置界面状态");
    check("按钮存在且可用", !!button && !button.disabled);
    if (button) {
      await act(async () => {
        button.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
      });
    }
    check("回调恰好调用一次", called === 1, "called=" + called);
    check("出现完成提示", bodyText().includes("最近位置已清空"));
    check("未触发 setConfig", state.calls.setConfig === 0, "setConfig=" + state.calls.setConfig);
    check("未触发 resetScope", state.calls.resetScope === 0, "resetScope=" + state.calls.resetScope);
    await act(async () => root.unmount());
    host.remove();
  }

  // ② 不传回调：按钮保持 disabled（独立渲染不假装能重置）
  {
    const { root, host } = await renderWith(undefined);
    const button = findButton(host, "重置界面状态");
    check("无回调时按钮 disabled", !!button && button.disabled);
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
