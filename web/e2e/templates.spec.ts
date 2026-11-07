import { expect, test } from "@playwright/test";

// 处理模板（C37）的浏览器面。
//
// OQ-41② 落地后（ADR-0080）套用通道已接：夹具的 f05 模板可套用，走
// describe→填槽位→loadRecipe 出预览计划（浏览器里计划是夹具，同 mockPlan 先例），
// 执行是显式按钮。判定与 loadRecipe 同源（describeRecipe），拒绝的模板不给按钮。
test("模板库可达：列表出得来，导入高版本被拒并给出错误码", async ({ page }) => {
  await page.goto("/");

  const nav = page.getByRole("navigation", { name: "功能导航" });
  await nav.getByRole("button", { name: /处理模板/ }).click();

  await expect(page.getByRole("heading", { name: "处理模板", level: 2 })).toBeVisible();
  // 浏览器夹具里预置了一条模板（src/lib/ipc/mock.ts），列表不是空壳
  await expect(page.getByRole("heading", { name: "CSV 分片（每片 5 万行）" })).toBeVisible();
  // 动作要显示成人话，不是内核枚举
  await expect(page.getByText("分割", { exact: true })).toBeVisible();
  // 套用通道已接：可套用的模板给真按钮
  await expect(page.getByText(/OQ-41/)).toBeVisible();
  await expect(page.getByTestId("template-apply")).toBeVisible();

  // 导入一份高版本模板：报 E1007，且不显示后端原文（设计 06 §3）
  await page.getByPlaceholder("粘贴导出的模板 JSON").fill('{"schemaVersion":99,"name":"x"}');
  await page.getByRole("button", { name: /^导入$/ }).click();
  const alert = page.getByRole("alert");
  await expect(alert).toContainText("E1007");
  await expect(alert).not.toContainText("schemaVersion 99");
});

test("套用模板：填路径槽位生成预览计划，执行是显式按钮", async ({ page }) => {
  await page.goto("/");
  const nav = page.getByRole("navigation", { name: "功能导航" });
  await nav.getByRole("button", { name: /处理模板/ }).click();

  await page.getByTestId("template-apply").click();
  // 夹具 f05 的槽位：file（必填）＋outputDir（输出）
  await page.getByTestId("apply-slot-file").fill("/in/list.csv");
  await page.getByTestId("apply-slot-outputDir").fill("/out");
  await page.getByTestId("apply-submit").click();

  // 预览计划出得来（浏览器夹具同 mockPlan：形状一致，不代表真机文件结果）
  await expect(page.getByTestId("apply-preview-stats")).toContainText("共 1 条");
  // 只出计划不执行（方案 §15.2.2）：执行必须显式点
  await page.getByTestId("apply-execute").click();
  await expect(page.getByText(/已启动/)).toBeVisible();
});
