import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import App from "./App";
import { mockAiStats, mockResetUiState, mockSetAiScenario, resetAiMock } from "./lib/ipc/mock";

// F15 Phase 1 集成级契约（docs/testing/AI助手前端面板-mock验证方案-v0.1.md §5.2）。
// 走浏览器 mock：验证「面板 ↔ 既有预览」的交接，不代表真机文件结果。

async function openPanelAndAsk(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByTestId("ai-toggle"));
  await screen.findByTestId("ai-panel");
  await screen.findByText(/AI 已就绪/);
  await user.type(screen.getByTestId("ai-input"), "把发票按日期改名");
  await user.click(screen.getByTestId("ai-send"));
  await user.click(await screen.findByTestId("ai-consent-confirm"));
  await screen.findByTestId("ai-bundle");
}

beforeEach(() => {
  resetAiMock();
  mockResetUiState();
});

describe("F15 AI 面板（集成级）", () => {
  it("AI-INT-05 模板入口：切到处理模板视图并关闭面板，不产生 AI 请求", async () => {
    mockSetAiScenario("ready");
    const user = userEvent.setup();
    render(<App />);

    await user.click(screen.getByTestId("ai-toggle"));
    await screen.findByTestId("ai-panel");
    await user.click(screen.getByTestId("ai-open-templates"));

    expect(
      await screen.findByRole("heading", { level: 1, name: "处理模板" })
    ).toBeVisible();
    expect(screen.queryByTestId("ai-panel")).toBeNull();
    expect(mockAiStats().askCount).toBe(0);
  });

  it("AI-INT-01 面板开关：关闭再打开保留本次会话消息", async () => {
    mockSetAiScenario("ready");
    const user = userEvent.setup();
    render(<App />);

    await openPanelAndAsk(user);
    expect(screen.getByTestId("ai-panel")).toBeVisible();

    await user.click(screen.getByTestId("ai-close"));
    expect(screen.queryByTestId("ai-panel")).toBeNull();

    await user.click(screen.getByTestId("ai-toggle"));
    expect(await screen.findByTestId("ai-panel")).toBeVisible();
    expect(screen.getByTestId("ai-bundle")).toBeVisible();
  });

  it("AI-INT-02 计划束交接：切到 f12 功能视图，预览表出现 AI 计划条目", async () => {
    mockSetAiScenario("ready");
    const user = userEvent.setup();
    render(<App />);

    await openPanelAndAsk(user);
    await user.click(screen.getByTestId("ai-open-preview"));

    expect(
      await screen.findByRole("heading", { name: "批量改名（F12）" })
    ).toBeVisible();
    expect(screen.getByText(/已接收 AI 计划：1 条/)).toBeVisible();
    expect(await screen.findByText("/mock/ai/合同.pdf")).toBeVisible();
    expect(screen.getByText(/2026_合同.pdf/)).toBeVisible();
    expect(mockAiStats().applyCount).toBe(0);
  });

  it("AI-INT-03 交接后不自动执行：无运行中任务，执行仍待用户显式点击", async () => {
    mockSetAiScenario("ready");
    const user = userEvent.setup();
    render(<App />);

    await openPanelAndAsk(user);
    await user.click(screen.getByTestId("ai-open-preview"));
    await screen.findByRole("heading", { name: "批量改名（F12）" });

    expect(screen.queryByTestId("plan-stale")).toBeNull();
    expect(screen.queryByTestId("cancel-task")).toBeNull();
    expect(mockAiStats().applyCount).toBe(0);
    expect(screen.getByRole("button", { name: /^执行$/ })).toBeVisible();
  });

  it("AI-INT-04 无直接 apply：全流程 apply 计数恒为 0，面板内无执行入口", async () => {
    mockSetAiScenario("ready");
    const user = userEvent.setup();
    render(<App />);

    await openPanelAndAsk(user);
    const panel = screen.getByTestId("ai-panel");
    expect(within(panel).queryByRole("button", { name: /^执行$/ })).toBeNull();

    await user.click(screen.getByTestId("ai-open-preview"));
    await screen.findByRole("heading", { name: "批量改名（F12）" });

    expect(mockAiStats().applyCount).toBe(0);
    expect(screen.queryByTestId("plan-stale")).toBeNull();
  });

  it("AI-INT-06 上下文接线：功能页填了文件后，面板的「包含文件名」可选", async () => {
    // 修复前 App 传的是恒空的 `fileCount: 0, kinds: []`，导致同意弹窗里
    // 「包含文件名」永远 disabled、清单无从展示（设计 14 §9 的能力形同虚设）。
    mockSetAiScenario("ready");
    const user = userEvent.setup();
    render(<App />);

    await user.click(screen.getByRole("button", { name: /编码转换/ }));
    await screen.findByRole("heading", { name: "编码转换（C01）" });
    // C01 的文件字段（`FEATURE_FIELDS` 的 labelKey）。浏览器形态渲染的是 File
    // 选择器——桌面形态才是路径文本域，所以这里是 upload 而不是 type：
    // 文件名清单由 `FeaturePages` 把选中文件的 name 合进上报表单。
    const fileField = document.getElementById("c01-files") as HTMLInputElement;
    expect(fileField).not.toBeNull();
    expect(fileField.type).toBe("file");
    await user.upload(fileField, [
      new File(["contract"], "合同.pdf", { type: "application/pdf" }),
      new File(["invoice"], "发票.pdf", { type: "application/pdf" })
    ]);

    await user.click(screen.getByTestId("ai-toggle"));
    await screen.findByTestId("ai-panel");
    await screen.findByText(/AI 已就绪/);
    await user.type(screen.getByTestId("ai-input"), "转成 UTF-8");
    await user.click(screen.getByTestId("ai-send"));

    const dialog = await screen.findByTestId("ai-consent");
    const toggle = within(dialog).getByTestId("ai-consent-names");
    expect(toggle).toBeEnabled();
    await user.click(toggle);
    expect(within(dialog).getByText("合同.pdf")).toBeVisible();
    expect(within(dialog).getByText("发票.pdf")).toBeVisible();

    await user.click(within(dialog).getByTestId("ai-consent-confirm"));
    await screen.findByTestId("ai-bundle");
    expect(mockAiStats().lastAskInput?.context.fileNames).toEqual(["合同.pdf", "发票.pdf"]);
  });
});
