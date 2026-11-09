import { expect, test, type Page } from "@playwright/test";

// F15 Phase 1 真浏览器契约（docs/testing/AI助手前端面板-mock验证方案-v0.1.md §5.3）。
// Chromium ＋ 浏览器 mock IPC（?aiScenario=）；不覆盖真实 Provider 与 WebView 文件结果。

async function openPanel(page: Page) {
  await page.getByTestId("ai-toggle").click();
  await expect(page.getByTestId("ai-panel")).toBeVisible();
}

async function ask(page: Page, text: string) {
  await page.getByTestId("ai-input").fill(text);
  await page.getByTestId("ai-send").click();
  await page.getByTestId("ai-consent-confirm").click();
}

test("AI-E2E-01 未配置：状态行与手动引导可见，输入禁用", async ({ page }) => {
  await page.goto("/?aiScenario=unconfigured");
  await openPanel(page);

  await expect(page.getByTestId("ai-status")).toContainText("AI 未配置");
  await expect(page.getByTestId("ai-input")).toBeDisabled();
  await expect(page.getByTestId("ai-send")).toBeDisabled();
});

test("AI-E2E-02 happy path：同意→流式→工具卡→计划束→去预览，面板内无执行入口", async ({
  page
}) => {
  await page.goto("/?aiScenario=ready");
  await openPanel(page);
  await expect(page.getByTestId("ai-status")).toContainText("AI 已就绪");

  await ask(page, "把发票按日期改名");

  await expect(page.getByTestId("ai-tool-card")).toContainText("planOperation");
  const bundle = page.getByTestId("ai-bundle");
  await expect(bundle).toContainText("1 步 · 1 条");
  await expect(
    page.getByTestId("ai-panel").getByRole("button", { name: "执行", exact: true })
  ).toHaveCount(0);

  await page.getByTestId("ai-open-preview").click();
  await expect(page.getByRole("heading", { name: "批量改名（F12）" })).toBeVisible();
  await expect(page.getByText("已接收 AI 计划：1 条。核对预览后再执行。")).toBeVisible();
  await expect(page.getByText("/mock/ai/合同.pdf")).toBeVisible();
});

test("AI-E2E-03 禁止工具：工具卡标红，E6004 错误条，无计划束", async ({ page }) => {
  await page.goto("/?aiScenario=forbidden-tool");
  await openPanel(page);
  await ask(page, "直接帮我执行");

  await expect(page.getByTestId("ai-tool-card")).toContainText("已拦截的工具");
  const alert = page.getByTestId("ai-error").getByRole("alert");
  await expect(alert).toContainText("E6004");
  await expect(alert).toContainText("模型请求了一次不被允许的操作");
  await expect(page.getByTestId("ai-bundle")).toHaveCount(0);
});

test("AI-E2E-04 多步：E6011，无计划束与去预览", async ({ page }) => {
  await page.goto("/?aiScenario=multi-step");
  await openPanel(page);
  await ask(page, "分三步处理");

  const alert = page.getByTestId("ai-error").getByRole("alert");
  await expect(alert).toContainText("E6011");
  await expect(alert).toContainText("当前版本不支持多计划组合");
  await expect(page.getByTestId("ai-bundle")).toHaveCount(0);
  await expect(page.getByTestId("ai-open-preview")).toHaveCount(0);
});

test("AI-E2E-05 取消：E6007 收尾，无计划束，输入恢复可用", async ({ page }) => {
  await page.goto("/?aiScenario=cancelled");
  await openPanel(page);
  await ask(page, "把发票按日期改名");

  await page.getByTestId("ai-cancel").click();

  const alert = page.getByTestId("ai-error").getByRole("alert");
  await expect(alert).toContainText("E6007");
  await expect(alert).toContainText("这次 AI 请求已取消");
  await expect(page.getByTestId("ai-bundle")).toHaveCount(0);
  await expect(page.getByTestId("ai-cancel")).toHaveCount(0);
  await expect(page.getByTestId("ai-input")).toBeEnabled();
  await page.getByTestId("ai-input").fill("再来一次");
  await expect(page.getByTestId("ai-send")).toBeEnabled();
});

test("AI-E2E-06 英文：关键路径在 en-US 下成立", async ({ page }) => {
  await page.goto("/?aiScenario=ready");

  const nav = page.getByRole("navigation", { name: "功能导航" });
  await nav.getByRole("button", { name: /设置/ }).click();
  await page.getByLabel("界面语言").selectOption("en-US");
  await expect(page.locator("html")).toHaveAttribute("lang", "en-US");

  await page.getByTestId("ai-toggle").click();
  await expect(page.getByTestId("ai-status")).toContainText("AI ready");
  await page.getByTestId("ai-input").fill("rename these invoices by date");
  await page.getByTestId("ai-send").click();
  await expect(page.getByTestId("ai-consent")).toContainText("Not sent");
  await page.getByTestId("ai-consent-confirm").click();

  await expect(page.getByTestId("ai-bundle")).toContainText("Plan ready for review");
  await page.getByTestId("ai-open-preview").click();
  await expect(page.getByRole("heading", { name: "Batch rename (F12)" })).toBeVisible();
  await expect(
    page.getByText("AI plan handed over: 1 entries. Review the preview before executing.")
  ).toBeVisible();
});
