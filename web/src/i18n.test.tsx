import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { I18nProvider, interpolate, translate, useI18n, type Lang } from "./i18n";

// 设计 09 §4：语言切换即时生效、写 general.language、`lang` 属性跟随；
// §3：主题落 dataset 供 CSS 令牌切换。这里只测这三条，不测样式。

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

afterEach(() => {
  document.documentElement.lang = "";
  delete document.documentElement.dataset.theme;
});

describe("i18n 取词与插值", () => {
  it("整句模板按参数插值，缺参数时保留占位符而不是渲染 undefined", () => {
    expect(interpolate("{count} 条，{blocked} 条受阻", { count: 3, blocked: 1 })).toBe(
      "3 条，1 条受阻"
    );
    expect(interpolate("{count} 条", {})).toBe("{count} 条");
    expect(interpolate("无占位")).toBe("无占位");
  });

  it("同一 key 在两个语言包里给出各自语言的文案", () => {
    const zh = translate("zh-CN", "nav.c09");
    const en = translate("en-US", "nav.c09");
    expect(zh).not.toBe(en);
    expect(zh).not.toBe("nav.c09");
    expect(en).not.toBe("nav.c09");
  });

  it("中英文的占位符集合一致（I18N-01 的静态前提，运行期再兜一次）", () => {
    const samples: Array<[string, Record<string, string | number>]> = [
      ["c09.notice.planDone", { count: 2, blocked: 1 }],
      ["settings.saved", {}],
      ["tasks.summary", { total: 1, active: 0 }]
    ];
    for (const [key, params] of samples) {
      const zh = translate("zh-CN", key, params);
      const en = translate("en-US", key, params);
      expect(zh).not.toContain("{");
      expect(en).not.toContain("{");
    }
  });
});

describe("语言与主题的运行期行为", () => {
  it("切换语言即时改变文案与 document.lang，并写 general.language", async () => {
    const persist = vi.fn(async () => undefined);
    render(
      <I18nProvider initialLang="zh-CN" initialTheme="light" persist={persist}>
        <Probe />
      </I18nProvider>
    );
    expect(screen.getByTestId("lang")).toHaveTextContent("zh-CN");
    expect(screen.getByTestId("label")).toHaveTextContent("重复文件查找");
    expect(document.documentElement.lang).toBe("zh-CN");

    await userEvent.click(screen.getByRole("button", { name: "to-en" }));

    await waitFor(() => expect(screen.getByTestId("lang")).toHaveTextContent("en-US"));
    expect(screen.getByTestId("label")).toHaveTextContent("Duplicate files");
    expect(document.documentElement.lang).toBe("en-US");
    expect(persist).toHaveBeenCalledWith("general", { language: "en-US" });
  });

  it("切换主题即时落 dataset，并写 general.theme", async () => {
    const persist = vi.fn(async () => undefined);
    render(
      <I18nProvider initialLang="zh-CN" initialTheme="light" persist={persist}>
        <Probe />
      </I18nProvider>
    );
    expect(document.documentElement.dataset.theme).toBe("light");

    await userEvent.click(screen.getByRole("button", { name: "to-dark" }));

    await waitFor(() => expect(screen.getByTestId("theme")).toHaveTextContent("dark"));
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(persist).toHaveBeenCalledWith("general", { theme: "dark" });
  });

  it("没有 Provider 时按默认中文取词，组件单挂也不炸", () => {
    function Bare() {
      const { lang, t } = useI18n();
      return (
        <span>
          {lang}:{t("nav.c09")}
        </span>
      );
    }
    render(<Bare />);
    expect(screen.getByText(/zh-CN:重复文件查找/)).toBeInTheDocument();
  });
});

describe("语言包完整性", () => {
  it("渲染出的界面里没有裸露的资源 key（I18N-07：不显示 raw key）", () => {
    const langs: Lang[] = ["zh-CN", "en-US"];
    for (const lang of langs) {
      const { unmount } = render(
        <I18nProvider initialLang={lang} initialTheme="light">
          <Probe />
        </I18nProvider>
      );
      const text = document.body.textContent ?? "";
      expect(text).not.toMatch(/(nav|c09|feature|settings)\.[a-zA-Z]+\./);
      unmount();
    }
  });
});
