import { expect, test } from "@playwright/test";

// 支持包（设计 10 §5；方案 §9.7／§12.5）的浏览器面：设置页可达、先预览后导出、
// 导出写回刚预览的那份。数据走浏览器 mock 夹具（ADR-0046），不代表真机运行结果。
test("支持包：设置页可达，预览后才可导出，文案随语言切换", async ({ page }) => {
  await page.goto("/");

  const nav = page.getByRole("navigation", { name: "功能导航" });
  await nav.getByRole("button", { name: /设置/ }).click();

  const card = page.getByRole("region", { name: "支持包" });
  await expect(card).toBeVisible();

  // 未预览时导出不可用（不给"没内容可导"的死按钮）。
  const exportButton = card.getByRole("button", { name: "导出" });
  await expect(exportButton).toBeDisabled();

  await card.getByRole("button", { name: "预览" }).click();

  // 预览内容来自夹具：版本、错误码统计、最近诊断日志。
  await expect(card.getByText(/0\.1\.0/)).toBeVisible();
  await expect(card.getByText("E2002 × 1")).toBeVisible();
  await expect(card.getByText(/task finished with partial failures/)).toBeVisible();
  await expect(exportButton).toBeEnabled();

  await exportButton.click();
  await expect(card.getByText(/batchdesk-support-mock\.json/)).toBeVisible();

  // 语言切换后卡片标题同步换语言（不是只改了设置页其它部分）。
  await page.getByLabel("界面语言").selectOption("en-US");
  await expect(page.getByRole("region", { name: "Support package" })).toBeVisible();
});
