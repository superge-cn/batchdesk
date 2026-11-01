import { expect, test } from "@playwright/test";

// TC-C05-PRE-04（旧预览失效）与 B1 预览三件套的入口阻断面，真实浏览器版本。
// 修的问题：参数改过之后界面仍拿着旧 planId，点执行就按旧计划落盘。
// 现在：任一参数变化 → 提示"旧计划预览失效"，执行/撤销入口关闭；重新预览后恢复。
test.describe("参数变更后旧计划失效", () => {
  test("C09 工作台：改保留策略后执行入口关闭（勾选确认后仍关闭）", async ({ page }) => {
    await page.goto("/");
    await page.getByPlaceholder(/资料归档|Documents/).fill("/mock/tree");
    await page.getByRole("button", { name: /生成计划预览/ }).click();
    const confirm = page.getByLabel(/我已核对预览/);
    const apply = page.getByRole("button", { name: /^移入隔离目录$/ });
    const undo = page.getByRole("button", { name: /^撤销本次计划$/ });

    await expect(page.getByTestId("plan-stale")).toHaveCount(0);
    await confirm.check();
    await expect(apply).toBeEnabled();

    await page.getByLabel(/保留策略/).selectOption("shortest-path");
    const banner = page.getByTestId("plan-stale");
    await expect(banner).toBeVisible();
    await expect(banner).toContainText("旧计划预览失效");
    await expect(apply).toBeDisabled();
    await expect(undo).toBeDisabled();
    // 确认勾选本身也被禁用：不得带着旧计划继续执行
    await expect(confirm).toBeDisabled();

    await page.getByRole("button", { name: /生成计划预览/ }).click();
    await expect(page.getByTestId("plan-stale")).toHaveCount(0);
    await expect(confirm).toBeEnabled();
    await confirm.check();
    await expect(apply).toBeEnabled();
  });

  test("F12 功能页：改前缀后执行/撤销/回滚预演全部关闭", async ({ page }) => {
    await page.goto("/");
    await page
      .getByRole("navigation", { name: "功能导航" })
      .getByRole("button", { name: /^批量改名\s*F12$/ })
      .click();
    await page.getByLabel(/改名目录/).fill("/mock/dir");
    await page.getByLabel(/前缀/).fill("2026-");
    await page.getByLabel(/替换为/).fill("x");
    await page.getByLabel(/作用范围/).fill("full");
    await page.getByRole("button", { name: /生成计划预览/ }).click();
    await expect(page.getByRole("heading", { name: "计划预览" })).toBeVisible();

    await page.getByLabel(/前缀/).fill("2027-");
    await expect(page.getByTestId("plan-stale")).toBeVisible();
    await expect(page.getByRole("button", { name: /^执行$/ })).toBeDisabled();
    await expect(page.getByRole("button", { name: /^撤销$/ })).toBeDisabled();
    await expect(page.getByRole("button", { name: /回滚预演/ })).toBeDisabled();
  });
});
