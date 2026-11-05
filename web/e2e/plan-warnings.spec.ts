import { expect, test } from "@playwright/test";

// 计划级"集中报告"在功能页逐项点名（此前只有"警告 N"计数，用户看不到哪几行被排除、为什么）。
// 数据走浏览器 mock 夹具（ADR-0046 边界：只证 UI 语义，不证真机文件结果）。
test.describe("功能页的集中报告", () => {
  test("F11 的 parent-is-file 报告逐项列出类别与建议，而不只给计数", async ({ page }) => {
    await page.goto("/");
    await page
      .getByRole("navigation", { name: "功能导航" })
      .getByRole("button", { name: /^清单建档\s*F11$/ })
      .click();
    await page.getByLabel(/目标根目录/).fill("/mock/root");
    await page.getByLabel(/每行一个相对路径/).fill("folder-a/inner");
    await page.getByRole("button", { name: /生成计划预览/ }).click();

    await expect(page.getByRole("heading", { name: "计划预览" })).toBeVisible();
    // 类别标题走资源包（设计 06 §2A）；这里断的是界面文案，不是内核 slug。
    const report = page.getByText("父级位置是文件", { exact: true });
    await expect(report).toBeVisible();
    await expect(page.getByText(/父级 \/mock\/root\/folder-a 是文件/)).toBeVisible();
    // 计数与逐项点名同时存在：计数不能成为唯一出口。
    await expect(page.getByText(/警告 1/)).toBeVisible();
  });

  test("对照：夹具没有 warnings 的功能页不得凭空出现集中报告块", async ({ page }) => {
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
    await expect(page.locator(".warnings")).toHaveCount(0);
  });
});
