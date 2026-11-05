// i18n 行为探针：对应 web/src/i18n.test.tsx 的 7 条断言（设计 09 §3/§4/§7）。
// 取词/插值是纯函数；语言与主题的运行期行为走 Provider + jsdom。
import { act } from "react";
import { createRoot } from "react-dom/client";
import { I18nProvider, interpolate, translate, useI18n } from "../src/i18n";

const results: Array<[string, boolean, string]> = [];
function check(name: string, ok: boolean, detail = "") {
  results.push([name, ok, detail]);
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function settle(label: string, fn: () => unknown, timeoutMs = 5000): Promise<boolean> {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    if (fn()) return true;
    await sleep(20);
  }
  console.warn("settle 超时：" + label);
  return false;
}

function Probe() {
  const { lang, resolvedTheme, setLang, setTheme, t } = useI18n();
  return (
    <div>
      <span data-testid="lang">{lang}</span>
      <span data-testid="theme">{resolvedTheme}</span>
      <span data-testid="label">{t("nav.c09")}</span>
      <button type="button" onClick={() => setLang("en-US")}>
        to-en
      </button>
      <button type="button" onClick={() => setTheme("dark")}>
        to-dark
      </button>
    </div>
  );
}

/** 简易调用记录器（等价 vi.fn 的"被没有以这些参数调用"断言）。 */
function spyFn() {
  const fn = async (scope: string, patch: Record<string, unknown>) => {
    fn.calls.push([scope, patch]);
    return undefined;
  };
  (fn as unknown as { calls: Array<[string, Record<string, unknown>]> }).calls = [];
  return fn as typeof fn & { calls: Array<[string, Record<string, unknown>]> };
}

function testid(name: string): HTMLElement | null {
  return document.querySelector(`[data-testid="${name}"]`);
}

function button(label: string): HTMLElement | undefined {
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
  // ── 取词与插值（纯函数） ──
  check(
    "整句模板按参数插值",
    interpolate("{count} 条，{blocked} 条受阻", { count: 3, blocked: 1 }) === "3 条，1 条受阻"
  );
  check("缺参数保留占位符", interpolate("{count} 条", {}) === "{count} 条");
  check("无占位原样返回", interpolate("无占位") === "无占位");

  {
    const zh = translate("zh-CN", "nav.c09");
    const en = translate("en-US", "nav.c09");
    check("同一 key 两语言各自成文", zh !== en && zh !== "nav.c09" && en !== "nav.c09", `${zh}/${en}`);
  }

  {
    const samples: Array<[string, Record<string, string | number>]> = [
      ["c09.notice.planDone", { count: 2, blocked: 1 }],
      ["settings.saved", {}],
      ["tasks.summary", { total: 1, active: 0 }]
    ];
    let ok = true;
    let detail = "";
    for (const [key, params] of samples) {
      const zh = translate("zh-CN", key, params);
      const en = translate("en-US", key, params);
      if (zh.includes("{") || en.includes("{")) {
        ok = false;
        detail = key;
      }
    }
    check("占位符集合两语言一致（无残留 {）", ok, detail);
  }

  // ── 语言切换运行期行为 ──
  {
    document.documentElement.lang = "";
    const persist = spyFn();
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    await act(async () => {
      root.render(
        <I18nProvider initialLang="zh-CN" initialTheme="light" persist={persist}>
          <Probe />
        </I18nProvider>
      );
    });
    check("初始 lang=zh-CN", testid("lang")?.textContent === "zh-CN");
    check("初始文案是中文", testid("label")?.textContent === "重复文件查找");
    check("document.lang=zh-CN", document.documentElement.lang === "zh-CN");

    const toEn = button("to-en");
    if (toEn) await click(toEn);
    const switched = await settle("切到 en-US", () => testid("lang")?.textContent === "en-US");
    check("切语言即时生效", !!switched);
    check("英文文案即时生效", testid("label")?.textContent === "Duplicate files", testid("label")?.textContent);
    check("document.lang=en-US", document.documentElement.lang === "en-US");
    const langCall = persist.calls.find(([scope, patch]) => scope === "general" && patch.language === "en-US");
    check("持久化写 general.language", !!langCall, JSON.stringify(persist.calls));
    await act(async () => root.unmount());
    host.remove();
    document.documentElement.lang = "";
  }

  // ── 主题切换运行期行为 ──
  {
    const persist = spyFn();
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    await act(async () => {
      root.render(
        <I18nProvider initialLang="zh-CN" initialTheme="light" persist={persist}>
          <Probe />
        </I18nProvider>
      );
    });
    check("初始 dataset.theme=light", document.documentElement.dataset.theme === "light");
    const toDark = button("to-dark");
    if (toDark) await click(toDark);
    const dark = await settle("切到 dark", () => testid("theme")?.textContent === "dark");
    check("主题即时生效", !!dark);
    check("dataset.theme=dark", document.documentElement.dataset.theme === "dark");
    const themeCall = persist.calls.find(([scope, patch]) => scope === "general" && patch.theme === "dark");
    check("持久化写 general.theme", !!themeCall, JSON.stringify(persist.calls));
    await act(async () => root.unmount());
    host.remove();
    delete document.documentElement.dataset.theme;
  }

  // ── 无 Provider 默认中文 ──
  {
    function Bare() {
      const { lang, t } = useI18n();
      return (
        <span>
          {lang}:{t("nav.c09")}
        </span>
      );
    }
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    await act(async () => {
      root.render(<Bare />);
    });
    check("无 Provider 按默认中文取词", (host.textContent ?? "").includes("zh-CN:重复文件查找"), host.textContent ?? "");
    await act(async () => root.unmount());
    host.remove();
  }

  // ── 语言包完整性：渲染结果没有裸 key ──
  {
    let rawKey = "";
    for (const lang of ["zh-CN", "en-US"] as const) {
      const host = document.createElement("div");
      document.body.appendChild(host);
      const root = createRoot(host);
      await act(async () => {
        root.render(
          <I18nProvider initialLang={lang} initialTheme="light">
            <Probe />
          </I18nProvider>
        );
      });
      const text = document.body.textContent ?? "";
      const match = text.match(/(nav|c09|feature|settings)\.[a-zA-Z]+\./);
      if (match) rawKey = `${lang}:${match[0]}`;
      await act(async () => root.unmount());
      host.remove();
    }
    check("界面无裸露资源 key", rawKey === "", rawKey);
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
