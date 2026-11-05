import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { ErrorBanner } from "./ErrorBanner";
import {
  ERROR_CODES,
  ERROR_TEXT_KEYS,
  describeError,
  isKnownErrorCode,
} from "./errors";
import { I18nProvider, translate } from "./i18n";
import enUS from "./locales/en-US.json";
import zhCN from "./locales/zh-CN.json";

// 设计 06 §3 的文案 key 约定：summary 一句人话、action 建议动作，都进界面；
// detail 只进诊断日志。这里钉住四件事：本地化、英文、未知码位兜底、资源完整性。

const zh = (key: keyof typeof zhCN) => translate("zh-CN", key);
const en = (key: keyof typeof enUS) => translate("en-US", key);

describe("错误码文案（设计 06 §3／设计 09 §5）", () => {
  it("已知码位给出本地化摘要与建议动作，不再把后端英文原文当主文案", () => {
    const text = describeError((key) => zh(key), {
      code: "E2004",
      message: "no space left on device (os error 28)",
    });
    expect(text.localized).toBe(true);
    expect(text.title).toBe(zh("error.E2004.summary"));
    expect(text.action).toBe(zh("error.E2004.action"));
    // 技术原文不得出现在界面文案里（06 §3：detail 仅进诊断日志）。
    expect(text.title).not.toContain("os error");
  });

  it("同一码位在英文下给英文文案", () => {
    const text = describeError((key) => en(key), {
      code: "E2009",
      message: "protected",
    });
    expect(text.title).toBe(en("error.E2009.summary"));
    expect(text.action).toBe(en("error.E2009.action"));
  });

  it("未收录的码位退回后端原文而不是空白，并标记未本地化", () => {
    // E8999 不在设计 06 §2 的码表里（06 只有 E1xxx/E2xxx/E3xxx/E4xxx/E5xxx/E6xxx/E9001），
    // 用它代表"真正的未知码"；曾经这里用 E6011，本批把 06 全表接入后它就成已知码了。
    const text = describeError((key) => zh(key), {
      code: "E8999",
      message: "model quota exhausted",
    });
    expect(text.localized).toBe(false);
    expect(text.title).toBe("model quota exhausted");
    expect(text.action).toBeUndefined();
    expect(isKnownErrorCode("E8999")).toBe(false);
  });

  it("设计 06 §2 的码表已全量收口：55 个码位、每码两段", () => {
    // 收口前只有 21 个码有文案，其余冒到界面就直出壳层英文串（ADR-0061 的缺口）。
    // 这条断言把"码位总数"钉住：漏接一个码，这里就红。
    expect(ERROR_CODES.length).toBe(55);
    for (const segment of ["1", "2", "3", "4", "5", "6", "9"]) {
      expect(
        ERROR_CODES.filter((code) => code.startsWith(`E${segment}`)).length,
        `E${segment}xxx 段不应为空`,
      ).toBeGreaterThan(0);
    }
    for (const code of [
      "E1006",
      "E2003",
      "E2007",
      "E2008",
      "E3005",
      "E3006",
      "E4001",
      "E5008",
      "E6004",
      "E6011",
    ]) {
      expect(isKnownErrorCode(code), `${code} 应已接入`).toBe(true);
    }
  });

  it("每个收录码位在中英两个语言包里都有 summary 与 action，且文案不为空", () => {
    expect(ERROR_CODES.length).toBeGreaterThan(0);
    for (const code of ERROR_CODES) {
      const keys = ERROR_TEXT_KEYS[code];
      for (const bundle of [zhCN, enUS]) {
        const summary = bundle[keys.summary];
        const action = bundle[keys.action];
        expect(summary, `${code} 缺 ${keys.summary}`).toBeTruthy();
        expect(action, `${code} 缺 ${keys.action}`).toBeTruthy();
        // 占位符约定：这两类文案不带参数（数量/名称由调用方另行呈现）。
        expect(summary).not.toContain("{");
        expect(action).not.toContain("{");
      }
    }
  });
});

describe("错误横幅", () => {
  it("渲染码位、本地化摘要与建议动作，且关闭按钮可用", async () => {
    const onClose = { called: false };
    render(
      <I18nProvider initialLang="zh-CN" initialTheme="light">
        <ErrorBanner
          error={{
            code: "E1002",
            message: "invalid params: unknown key",
            retryable: false,
          }}
          onClose={() => {
            onClose.called = true;
          }}
        />
      </I18nProvider>,
    );

    const banner = screen.getByRole("alert");
    expect(banner).toHaveTextContent("E1002");
    expect(banner).toHaveTextContent(zh("error.E1002.summary"));
    expect(banner).toHaveTextContent(zh("error.E1002.action"));
    // 后端原文不进界面。
    expect(banner).not.toHaveTextContent("unknown key");

    await userEvent.click(
      screen.getByRole("button", { name: zh("common.close") }),
    );
    expect(onClose.called).toBe(true);
  });

  it("未收录码位时仍显示码位与后端原文，不出现 raw key", () => {
    render(
      <I18nProvider initialLang="en-US" initialTheme="light">
        <ErrorBanner
          error={{
            code: "E8999",
            message: "model quota exhausted",
            retryable: false,
          }}
          onClose={() => {}}
        />
      </I18nProvider>,
    );

    const banner = screen.getByRole("alert");
    expect(banner).toHaveTextContent("E8999");
    expect(banner).toHaveTextContent("model quota exhausted");
    expect(banner).not.toHaveTextContent("error.E8999");
  });
});
