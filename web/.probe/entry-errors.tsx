// 错误文案分层（设计 06 §3／设计 09 §5）行为探针：对应 web/src/errors.test.tsx 的 7 条断言。
// 前四条是纯函数（不需要 DOM），后三条渲染 ErrorBanner。
import { act } from "react";
import { createRoot } from "react-dom/client";
import { ERROR_CODES, ERROR_TEXT_KEYS, describeError, isKnownErrorCode } from "../src/errors";
import { I18nProvider, translate } from "../src/i18n";
import { ErrorBanner } from "../src/ErrorBanner";
import enUS from "../src/locales/en-US.json";
import zhCN from "../src/locales/zh-CN.json";

const results: Array<[string, boolean, string]> = [];
function check(name: string, ok: boolean, detail = "") {
  results.push([name, ok, detail]);
}

const zh = (key: keyof typeof zhCN) => translate("zh-CN", key);
const en = (key: keyof typeof enUS) => translate("en-US", key);

async function withBanner(
  error: { code: string; message: string },
  assertions: (el: HTMLElement) => void
) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(
      <I18nProvider initialLang="zh-CN" initialTheme="light">
        <ErrorBanner error={{ ...error, retryable: false }} onClose={() => undefined} />
      </I18nProvider>
    );
  });
  assertions(host);
  await act(async () => root.unmount());
  host.remove();
}

async function main() {
  // ① 已知码位：本地化摘要与建议动作，后端英文原文不当主文案
  {
    const text = describeError((key) => zh(key), {
      code: "E2004",
      message: "no space left on device (os error 28)"
    });
    check("已知码位走本地化", text.localized === true);
    check("摘要是 zh 资源", text.title === zh("error.E2004.summary"), text.title);
    check("建议动作是 zh 资源", text.action === zh("error.E2004.action"));
    check("后端原文不出现在主行", !text.title.includes("os error"));
  }

  // ② 同一码位英文下给英文
  {
    const text = describeError((key) => en(key), { code: "E2009", message: "protected" });
    check("英文下给英文摘要", text.title === en("error.E2009.summary"), text.title);
    check("英文下给英文建议动作", text.action === en("error.E2009.action"));
  }

  // ③ 未收录码位退回后端原文并标记未本地化
  {
    const text = describeError((key) => zh(key), {
      code: "E8999",
      message: "model quota exhausted"
    });
    check("未知码位 localized=false", text.localized === false);
    check("未知码位退回 message", text.title === "model quota exhausted");
    check("未知码位无建议动作", text.action === undefined);
    check("E8999 不在码表", isKnownErrorCode("E8999") === false);
  }

  // ④ 码表全量收口：55 个码位、七个分段都非空
  {
    check("码位总数 55", ERROR_CODES.length === 55, String(ERROR_CODES.length));
    let allSegments = true;
    for (const segment of ["1", "2", "3", "4", "5", "6", "9"]) {
      if (ERROR_CODES.filter((code) => code.startsWith("E" + segment)).length === 0) allSegments = false;
    }
    check("七个分段都非空", allSegments);
    const mustKnown = ["E1006", "E2003", "E2007", "E2008", "E3005", "E3006", "E4001", "E5008", "E6004", "E6011"];
    check("此前未接入的码位都已收口", mustKnown.every((code) => isKnownErrorCode(code)));
  }

  // ⑤ 每个码位两语言都有 summary/action、非空、无占位符
  {
    let missing = "";
    let placeholder = "";
    for (const code of ERROR_CODES) {
      const keys = ERROR_TEXT_KEYS[code];
      for (const bundle of [zhCN, enUS] as Array<Record<string, string>>) {
        const summary = bundle[keys.summary];
        const action = bundle[keys.action];
        if (!summary) missing += code + "/zh?";
        if (!action) missing += code + "/act?";
        if (summary && summary.includes("{")) placeholder += code;
        if (action && action.includes("{")) placeholder += code + "a";
      }
    }
    check("全部码位两段齐全且非空", missing === "", missing.slice(0, 60));
    check("文案不带占位符", placeholder === "", placeholder.slice(0, 40));
  }

  // ⑥ 横幅渲染：码位 + 本地化摘要 + 建议动作 + 关闭按钮；后端原文不进界面
  await withBanner({ code: "E1002", message: "invalid params: unknown key" }, (host) => {
    const alert = host.querySelector('[role="alertdialog"], [role="alert"]');
    const text = host.textContent ?? "";
    check("横幅带 alert 语义", !!alert);
    check("横幅显示码位 E1002", text.includes("E1002"));
    check("横幅显示本地化摘要", text.includes(zh("error.E1002.summary")));
    check("横幅显示建议动作", text.includes(zh("error.E1002.action")));
    check("后端原文不进界面", !text.includes("unknown key"));
    const close = Array.from(host.querySelectorAll("button")).find((b) =>
      (b.textContent ?? "").includes(zh("common.close"))
    );
    check("关闭按钮存在", !!close);
  });

  // ⑦ 未收录码位的横幅：显示码位与后端原文，不出现 raw key
  await withBanner({ code: "E8999", message: "model quota exhausted" }, (host) => {
    const text = host.textContent ?? "";
    check("未知码位横幅仍显示码位", text.includes("E8999"));
    check("未知码位横幅显示后端原文", text.includes("model quota exhausted"));
    check("不出现 raw key", !text.includes("error.E8999"));
  });

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
