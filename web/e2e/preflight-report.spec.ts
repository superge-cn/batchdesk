import { expect, test } from "@playwright/test";

// TC-S01-PRE-06（报告可读）与 TC-S01-RBK-02（无"跳过预检"入口）的浏览器面断言。
// 预检数据来自 mock 夹具（mockPreflightPlan）：断言的是"逐项结论是否呈现"，
// 判定权威仍是 kernel::preflight（已由 9 项单测钉住）。
test.beforeEach(async ({ page }) => {
  await page.goto("/");
  await page.getByPlaceholder(/资料归档|Documents/).fill("/mock/tree");
  await page.getByRole("button", { name: /生成计划预览/ }).click();
});

test("预检报告给出逐项结论与三类计数", async ({ page }) => {
  const banner = page.getByRole("status").filter({ hasText: /预检 · 通过 \d+ · 警告 \d+ · 阻断 \d+/ });
  await expect(banner).toBeVisible();
});

test("界面上不存在跳过预检的入口（默认必检）", async ({ page }) => {
  await expect(page.getByRole("button", { name: /跳过预检|忽略预检|不预检/ })).toHaveCount(0);
});
