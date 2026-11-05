import { expect, test } from "@playwright/test";

// 处理模板（C37）的浏览器面。
//
// 只断言"这一版真的做了什么"：列表、导入被拒的界面路径、以及**没有**假的应用按钮。
// 不做"套用模板生成计划"的断言——那要 `loadRecipe`，卡在 OQ-36（模板存动作、
// 壳层按功能码路由，一对多）；把未裁决的契约写成通过的用例，等于给缺口盖章。
test("模板库可达：列表出得来，导入高版本被拒并给出错误码，不提供假的套用入口", async ({ page }) => {
  await page.goto("/");

  const nav = page.getByRole("navigation", { name: "功能导航" });
  await nav.getByRole("button", { name: /处理模板/ }).click();

  await expect(page.getByRole("heading", { name: "处理模板", level: 2 })).toBeVisible();
  // 浏览器夹具里预置了一条模板（src/lib/ipc/mock.ts），列表不是空壳
  await expect(page.getByRole("heading", { name: "CSV 分片（每片 5 万行）" })).toBeVisible();
  // 动作要显示成人话，不是内核枚举
  await expect(page.getByText("分割", { exact: true })).toBeVisible();

  // 未决事项写成横幅，而不是一个点了没反应的按钮
  await expect(page.getByText(/OQ-36/)).toBeVisible();
  await expect(page.getByRole("button", { name: /^应用/ })).toHaveCount(0);

  // 导入一份高版本模板：报 E1007，且不显示后端原文（设计 06 §3）
  await page.getByPlaceholder("粘贴导出的模板 JSON").fill('{"schemaVersion":99,"name":"x"}');
  await page.getByRole("button", { name: /^导入$/ }).click();
  const alert = page.getByRole("alert");
  await expect(alert).toContainText("E1007");
  await expect(alert).not.toContainText("schemaVersion 99");
});
