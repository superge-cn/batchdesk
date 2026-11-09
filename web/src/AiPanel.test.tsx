import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AiPanel, type AiPanelProps } from "./AiPanel";
import { I18nProvider } from "./i18n";
import { mockAiStats, mockSetAiScenario, resetAiMock, type AiScenario } from "./lib/ipc/mock";
import type { AiContext } from "./lib/ipc/types";

// F15 Phase 1 组件级契约（docs/testing/AI助手前端面板-mock验证方案-v0.1.md §5.1）。
// 全部走浏览器 mock（isTauri()===false），不代表真实 Provider / WebView 结果。

const baseContext: AiContext = {
  fileCount: 3,
  kinds: ["f12"],
  locale: "zh-CN",
  includeFileNames: false
};

function renderPanel(
  overrides: Partial<AiPanelProps> = {},
  lang: "zh-CN" | "en-US" = "zh-CN"
) {
  const onOpenPlan = vi.fn();
  const onClose = vi.fn();
  const context = overrides.context ?? baseContext;
  const view = render(
    <I18nProvider initialLang={lang} initialTheme="light" persist={() => Promise.resolve({})}>
      <AiPanel open onClose={onClose} onOpenPlan={onOpenPlan} context={context} />
    </I18nProvider>
  );
  return { ...view, onOpenPlan, onClose };
}

async function typeAndSend(user: ReturnType<typeof userEvent.setup>, text: string) {
  await user.type(screen.getByTestId("ai-input"), text);
  await user.click(screen.getByTestId("ai-send"));
}

async function sendAndConfirm(user: ReturnType<typeof userEvent.setup>, text: string) {
  await typeAndSend(user, text);
  await user.click(await screen.findByTestId("ai-consent-confirm"));
}

beforeEach(() => {
  resetAiMock();
});

describe("F15 AI 面板（组件级）", () => {
  it("AI-UI-01 未配置模型：状态行给出引导，输入与发送禁用且不发请求", async () => {
    mockSetAiScenario("unconfigured");
    renderPanel();

    expect(
      await screen.findByText(/AI 未配置；可在设置里配置模型/)
    ).toBeVisible();
    expect(screen.getByTestId("ai-input")).toBeDisabled();
    expect(screen.getByTestId("ai-send")).toBeDisabled();
    expect(mockAiStats().askCount).toBe(0);
  });

  it("AI-UI-02 首次发送弹同意：列出将发送与不发送的字段，未确认前不发请求", async () => {
    mockSetAiScenario("ready");
    const user = userEvent.setup();
    renderPanel();
    await screen.findByText(/AI 已就绪/);

    await typeAndSend(user, "把发票按日期改名");

    const dialog = await screen.findByTestId("ai-consent");
    expect(dialog).toHaveAttribute("role", "alertdialog");
    expect(dialog).toHaveTextContent(/将发送/);
    expect(dialog).toHaveTextContent(/文件数量 3/);
    expect(dialog).toHaveTextContent(/类型 f12/);
    expect(dialog).toHaveTextContent(/不发送/);
    expect(dialog).toHaveTextContent(/完整路径/);
    expect(dialog).toHaveTextContent(/文件内容/);
    expect(mockAiStats().askCount).toBe(0);
  });

  it("AI-UI-03 同意弹窗取消：关闭弹窗、保留输入、不发请求", async () => {
    mockSetAiScenario("ready");
    const user = userEvent.setup();
    renderPanel();
    await screen.findByText(/AI 已就绪/);
    await typeAndSend(user, "把发票按日期改名");

    await user.click(await screen.findByTestId("ai-consent-cancel"));

    expect(screen.queryByTestId("ai-consent")).toBeNull();
    expect(screen.getByTestId("ai-input")).toHaveValue("把发票按日期改名");
    expect(mockAiStats().askCount).toBe(0);
  });

  it("AI-UI-04 确认后发送：默认不带文件名，consentToken 非空，且无路径泄漏", async () => {
    mockSetAiScenario("ready");
    const user = userEvent.setup();
    renderPanel();
    await screen.findByText(/AI 已就绪/);

    await sendAndConfirm(user, "把发票按日期改名");
    await screen.findByTestId("ai-bundle");

    const input = mockAiStats().lastAskInput;
    expect(mockAiStats().askCount).toBe(1);
    expect(input?.context.includeFileNames).toBe(false);
    expect(input?.context.fileNames).toBeUndefined();
    expect(input?.consentToken).toBeTruthy();
    expect(JSON.stringify(input)).not.toContain("/data/");
  });

  it("AI-UI-05 文件名 opt-in：勾选后列出清单，请求里才出现文件名", async () => {
    mockSetAiScenario("ready");
    const user = userEvent.setup();
    renderPanel({
      context: {
        ...baseContext,
        includeFileNames: false,
        fileNames: ["合同.pdf", "发票.pdf"]
      }
    });
    await screen.findByText(/AI 已就绪/);
    await typeAndSend(user, "按日期改名");

    const dialog = await screen.findByTestId("ai-consent");
    const toggle = within(dialog).getByTestId("ai-consent-names");
    expect(toggle).toBeEnabled();
    await user.click(toggle);
    expect(within(dialog).getByText("合同.pdf")).toBeVisible();
    expect(within(dialog).getByText("发票.pdf")).toBeVisible();

    await user.click(within(dialog).getByTestId("ai-consent-confirm"));
    await screen.findByTestId("ai-bundle");

    expect(mockAiStats().lastAskInput?.context.includeFileNames).toBe(true);
    expect(mockAiStats().lastAskInput?.context.fileNames).toEqual(["合同.pdf", "发票.pdf"]);
  });

  it("AI-UI-06 流式文本：delta 按序拼接，done 只补未流出的收尾文案", async () => {
    mockSetAiScenario("ready");
    const user = userEvent.setup();
    renderPanel();
    await screen.findByText(/AI 已就绪/);
    await sendAndConfirm(user, "把发票按日期改名");

    const log = await screen.findByTestId("ai-log");
    await screen.findByText(/已生成计划，请在预览里核对/);
    expect(log).toHaveTextContent(
      "已理解你的目标。正在生成计划…已生成计划，请在预览里核对。"
    );
    expect(mockAiStats().emitted).toEqual([
      "ai://delta",
      "ai://delta",
      "ai://tool",
      "ai://bundle",
      "ai://done"
    ]);
  });

  it("AI-UI-07 工具卡（允许）：显示工具名与允许态，无错误条", async () => {
    mockSetAiScenario("ready");
    const user = userEvent.setup();
    renderPanel();
    await screen.findByText(/AI 已就绪/);
    await sendAndConfirm(user, "把发票按日期改名");

    const card = await screen.findByTestId("ai-tool-card");
    expect(card).toHaveTextContent("允许的工具");
    expect(card).toHaveTextContent("planOperation");
    expect(screen.queryByTestId("ai-error")).toBeNull();
  });

  it("AI-UI-08 工具卡（禁止）：标红并以 E6004 本地化文案收尾，不产计划束", async () => {
    mockSetAiScenario("forbidden-tool");
    const user = userEvent.setup();
    renderPanel();
    await screen.findByText(/AI 已就绪/);
    await sendAndConfirm(user, "直接帮我执行");

    const card = await screen.findByTestId("ai-tool-card");
    expect(card).toHaveTextContent("已拦截的工具");
    expect(card).toHaveTextContent("applyPlan");
    const error = await screen.findByTestId("ai-error");
    expect(error).toHaveTextContent("E6004");
    expect(error).toHaveTextContent(/模型请求了一次不被允许的操作/);
    expect(error).not.toHaveTextContent("tool is forbidden");
    expect(screen.queryByTestId("ai-bundle")).toBeNull();
  });

  it("AI-UI-09 计划束与唯一出口：只提供「去预览」，不存在执行按钮", async () => {
    mockSetAiScenario("ready");
    const user = userEvent.setup();
    const { onOpenPlan } = renderPanel();
    await screen.findByText(/AI 已就绪/);
    await sendAndConfirm(user, "把发票按日期改名");

    const bundle = await screen.findByTestId("ai-bundle");
    expect(bundle).toHaveTextContent("计划已生成，待核对");
    expect(bundle).toHaveTextContent("1 步 · 1 条");
    expect(screen.queryByRole("button", { name: "执行" })).toBeNull();
    expect(mockAiStats().applyCount).toBe(0);

    await user.click(screen.getByTestId("ai-open-preview"));
    expect(onOpenPlan).toHaveBeenCalledTimes(1);
    expect(onOpenPlan.mock.calls[0][0].entries[0].source).toContain("合同.pdf");
  });

  it("AI-UI-10 多步拒绝：E6011 本地化文案，无计划束与「去预览」", async () => {
    mockSetAiScenario("multi-step");
    const user = userEvent.setup();
    renderPanel();
    await screen.findByText(/AI 已就绪/);
    await sendAndConfirm(user, "分三步处理");

    const error = await screen.findByTestId("ai-error");
    expect(error).toHaveTextContent("E6011");
    expect(error).toHaveTextContent(/当前版本不支持多计划组合/);
    expect(screen.queryByTestId("ai-bundle")).toBeNull();
    expect(screen.queryByTestId("ai-open-preview")).toBeNull();
  });

  it("AI-UI-11 非法 schema：E6003 本地化文案，无计划束", async () => {
    mockSetAiScenario("bad-schema");
    const user = userEvent.setup();
    renderPanel();
    await screen.findByText(/AI 已就绪/);
    await sendAndConfirm(user, "随便试试");

    const error = await screen.findByTestId("ai-error");
    expect(error).toHaveTextContent("E6003");
    expect(error).toHaveTextContent(/模型返回的内容不符合格式要求/);
    expect(error).not.toHaveTextContent("schema validation");
    expect(screen.queryByTestId("ai-bundle")).toBeNull();
  });

  it("AI-UI-12 超时：E6006 文案带重试建议，无计划束", async () => {
    mockSetAiScenario("timeout");
    const user = userEvent.setup();
    renderPanel();
    await screen.findByText(/AI 已就绪/);
    await sendAndConfirm(user, "把发票按日期改名");

    const error = await screen.findByTestId("ai-error");
    expect(error).toHaveTextContent("E6006");
    expect(error).toHaveTextContent(/模型响应超时/);
    expect(error).toHaveTextContent(/重试即可/);
    expect(screen.queryByTestId("ai-bundle")).toBeNull();
  });

  it("AI-UI-13 取消：cancel 只调一次，收尾 E6007，取消按钮消失且无计划束", async () => {
    mockSetAiScenario("cancelled");
    const user = userEvent.setup();
    renderPanel();
    await screen.findByText(/AI 已就绪/);
    await sendAndConfirm(user, "把发票按日期改名");

    await user.click(await screen.findByTestId("ai-cancel"));

    const error = await screen.findByTestId("ai-error");
    expect(error).toHaveTextContent("E6007");
    expect(error).toHaveTextContent(/这次 AI 请求已取消/);
    expect(mockAiStats().cancelCount).toBe(1);
    expect(mockAiStats().emitted).not.toContain("ai://bundle");
    expect(screen.queryByTestId("ai-bundle")).toBeNull();
    expect(screen.queryByTestId("ai-cancel")).toBeNull();
  });

  it("AI-UI-14 事件幂等与卸载安全：bundle 只落一条消息，卸载后迟到事件不报错", async () => {
    mockSetAiScenario("ready");
    const user = userEvent.setup();
    const { unmount } = renderPanel();
    await screen.findByText(/AI 已就绪/);
    await sendAndConfirm(user, "把发票按日期改名");
    await screen.findByText(/已生成计划，请在预览里核对/);

    expect(screen.getAllByText("助手")).toHaveLength(1);
    expect(screen.getAllByTestId("ai-bundle")).toHaveLength(1);

    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => undefined);
    unmount();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(errorSpy).not.toHaveBeenCalled();
    errorSpy.mockRestore();
  });

  it("AI-UI-15 双语：en-US 的状态、禁止工具与多步文案均走资源包", async () => {
    mockSetAiScenario("unconfigured");
    const first = renderPanel({}, "en-US");
    expect(await screen.findByText(/AI is not configured/)).toBeVisible();
    expect(screen.getByTestId("ai-input")).toBeDisabled();
    first.unmount();

    resetAiMock();
    mockSetAiScenario("forbidden-tool");
    const user = userEvent.setup();
    const second = renderPanel({}, "en-US");
    await screen.findByText(/AI ready/);
    await sendAndConfirm(user, "just run it");
    const error = await screen.findByTestId("ai-error");
    expect(error).toHaveTextContent("E6004");
    expect(error).toHaveTextContent(/not on the allowlist/);
    expect(error).not.toHaveTextContent(/模型/);
    second.unmount();

    resetAiMock();
    mockSetAiScenario("multi-step");
    renderPanel({}, "en-US");
    await screen.findByText(/AI ready/);
    await sendAndConfirm(user, "three steps");
    const multi = await screen.findByTestId("ai-error");
    expect(multi).toHaveTextContent(/Multi-plan bundles are not supported/);
  });

  it("AI-UI-16 可访问性：面板/日志语义正确，同意弹窗初始焦点在「取消」", async () => {
    mockSetAiScenario("ready");
    const user = userEvent.setup();
    renderPanel();
    await screen.findByText(/AI 已就绪/);

    expect(screen.getByTestId("ai-panel")).toHaveAttribute("role", "complementary");
    expect(screen.getByTestId("ai-panel")).toHaveAttribute("aria-label", "AI 助手");
    const log = screen.getByTestId("ai-log");
    expect(log).toHaveAttribute("role", "log");
    expect(log).toHaveAttribute("aria-live", "polite");

    await typeAndSend(user, "把发票按日期改名");
    const dialog = await screen.findByTestId("ai-consent");
    expect(dialog).toHaveAttribute("role", "alertdialog");
    expect(screen.getByTestId("ai-consent-cancel")).toHaveFocus();
  });

  const errorCases: Array<[AiScenario, string, RegExp]> = [
    ["blocked-content", "E6005", /内容安全策略拒绝了这次请求/],
    ["cycle", "E6010", /计划束的依赖不成立/]
  ];

  it.each(errorCases)("AI-UI-错误码 %s：%s 走本地化文案", async (scenario, code, summary) => {
    mockSetAiScenario(scenario);
    const user = userEvent.setup();
    renderPanel();
    await screen.findByText(/AI 已就绪/);
    await sendAndConfirm(user, "试一下");

    const error = await screen.findByTestId("ai-error");
    expect(error).toHaveTextContent(code);
    expect(error).toHaveTextContent(summary);
    expect(screen.queryByTestId("ai-bundle")).toBeNull();
  });
});
