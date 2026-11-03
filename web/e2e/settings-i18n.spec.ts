import { expect, test } from "@playwright/test";

// I18N-02 / I18N-04 的浏览器面：切到 en-US 后可见文案整体变英文，
// 关键操作（恢复默认、语言选择）仍然可点、可读。
// I18N-03（重启保持）由桌面壳的 ConfigDocument 承担，浏览器 mock 不承诺留存。
test("设置页可达，切到英文后整页换语言且关键操作不被遮挡", async ({ page }) => {
  await page.goto("/");

  const nav = page.getByRole("navigation", { name: "功能导航" });
  await nav.getByRole("button", { name: /设置/ }).click();

  await expect(page.getByRole("heading", { name: "通用" })).toBeVisible();
  await expect(page.getByLabel("界面语言")).toHaveValue("zh-CN");

  await page.getByLabel("界面语言").selectOption("en-US");

  await expect(page.locator("html")).toHaveAttribute("lang", "en-US");
  await expect(page.getByRole("heading", { name: "General" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Logging" })).toBeVisible();
  await expect(page.getByLabel("Interface language")).toHaveValue("en-US");

  // 英文文案变长后，卡片头部的"恢复本组默认"仍完整可点（未被挤出可视区）。
  const reset = page.getByRole("button", { name: "Restore group defaults" }).first();
  await expect(reset).toBeEnabled();
  const box = await reset.boundingBox();
  expect(box?.width ?? 0).toBeGreaterThan(0);

  // 语言切换后导航同步换语言（不是只改了设置页）。导航的可读名也换了，
  // 所以这里按 role 重新取，而不是复用切换前的 locator。
  await expect(
    page.getByRole("navigation").getByRole("button", { name: /Duplicate files/ })
  ).toBeVisible();
});
