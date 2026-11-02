import { expect, test } from "@playwright/test";

// 方案 §3.3（S02：保护目录命中即禁止写入·含二次确认）与 §7.11（危险级必须二次确认、
// 确认按钮不带默认焦点）在真实浏览器下的呈现语义；U v1.3 §3.5 的对话框形态。
// 边界同其他 e2e：数据走浏览器 mock 夹具，只证界面语义，不证真机文件结果。
test.describe("保护目录命中的二次确认", () => {
  test("C09 工作台：执行先出危险对话框，取消即零派发", async ({ page }) => {
    await page.goto("/");
    await page.getByPlaceholder(/资料归档|Documents/).fill("/mock/tree");
    await page.getByLabel(/保护目录/).fill("C:/Windows");
    await page.getByRole("button", { name: /生成计划预览/ }).click();
    await page.getByLabel(/我已核对预览/).check();

    const apply = page.getByRole("button", { name: /^移入隔离目录$/ });
    const undo = page.getByRole("button", { name: /^撤销本次计划$/ });
    await expect(apply).toBeEnabled();

    await apply.click();
    const dialog = page.getByTestId("danger-confirm");
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText("命中保护目录");
    await expect(dialog).toContainText("/mock/tree/protected/locked-copy.pdf");
    await expect(dialog).toContainText("不会执行");

    // 回车即确认的误操作路径必须在视觉层被消除：焦点落「取消」，危险按钮不带默认焦点
    await expect(page.getByTestId("danger-cancel")).toBeFocused();
    await expect(page.getByTestId("danger-confirm-button")).not.toBeFocused();

    await page.getByTestId("danger-cancel").click();
    await expect(dialog).toHaveCount(0);
    // 未确认前绝不派发任务（撤销入口仍禁用＝没有 apply 任务存在）
    await expect(undo).toBeDisabled();

    await apply.click();
    await page.getByTestId("danger-confirm-button").click();
    await expect(page.getByText("任务已加入队列")).toBeVisible();
  });

  test("保护名单为空时不出现对话框（默认空名单不误拦）", async ({ page }) => {
    await page.goto("/");
    await page.getByPlaceholder(/资料归档|Documents/).fill("/mock/tree");
    await page.getByRole("button", { name: /生成计划预览/ }).click();
    await page.getByLabel(/我已核对预览/).check();
    await page.getByRole("button", { name: /^移入隔离目录$/ }).click();
    await expect(page.getByTestId("danger-confirm")).toHaveCount(0);
    await expect(page.getByText("任务已加入队列")).toBeVisible();
  });
});
