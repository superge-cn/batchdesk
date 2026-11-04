import { expect, test } from "@playwright/test";

// TC-C05-PRE-03 / PRE-04 / PRE-06（映射表 D 类）：重命名页的 C05 预览面板语义。
// 数据来自浏览器 mock 夹具（src/lib/ipc/mock.ts 的 mockRenamePreview），
// 因此这里断言的是"面板如何呈现与失效"，不是 shell 的判定结果。
async function fillRenameForm(page: import("@playwright/test").Page, scope: string) {
  await page.getByLabel(/改名目录/).fill("/mock/dir");
  await page.getByLabel(/前缀/).fill("2026-");
  await page.getByLabel(/替换为/).fill("x");
  const scopeInput = page.getByLabel(/作用范围/);
  await scopeInput.fill(scope);
}

test.beforeEach(async ({ page }) => {
  await page.goto("/");
  await page
    .getByRole("navigation", { name: "功能导航" })
    .getByRole("button", { name: /^批量改名\s*F12$/ })
    .click();
});

test("无计划时执行入口被禁用（PRE-03 的入口阻断面）", async ({ page }) => {
  await expect(page.getByRole("button", { name: /^执行$/ })).toBeDisabled();
});

test("生成计划后渲染计划预览与 C05 对照面板", async ({ page }) => {
  await fillRenameForm(page, "full");
  await page.getByRole("button", { name: /生成计划预览/ }).click();
  await expect(page.getByRole("heading", { name: "计划预览" })).toBeVisible();
  const panel = page.getByRole("heading", { name: /C05 预览与冲突检测/ });
  await expect(panel).toBeVisible();
  await expect(panel.locator("xpath=ancestor::*[self::section][1]")).toContainText("将处理 1");
  await expect(panel.locator("xpath=ancestor::*[self::section][1]")).toContainText("冲突 0");
  await expect(page.getByText("/mock/dup.txt → /mock/uniq.txt")).toBeVisible();
});

test("改参数后重新预览：旧面板被新面板替换（PRE-04 / PRE-06）", async ({ page }) => {
  await fillRenameForm(page, "full");
  await page.getByRole("button", { name: /生成计划预览/ }).click();
  await expect(page.getByRole("heading", { name: /C05 预览与冲突检测/ })).toBeVisible();

  // 改变规则参数 → 重新预览；旧计划的对照表不得继续留在界面上
  await fillRenameForm(page, "stem");
  await page.getByRole("button", { name: /生成计划预览/ }).click();
  await expect(page.getByRole("heading", { name: "计划预览" })).toHaveCount(1);
  await expect(page.getByRole("heading", { name: /C05 预览与冲突检测/ })).toHaveCount(1);
  await expect(page.getByText(/计划已生成/)).toBeVisible();
});

test("回滚预演面板呈现零冲突判定（C06 的 UI 面）", async ({ page }) => {
  await fillRenameForm(page, "full");
  await page.getByRole("button", { name: /生成计划预览/ }).click();
  await page.getByRole("button", { name: /回滚预演/ }).click();
  await expect(page.getByText(/回滚预演/).first()).toBeVisible();
  await expect(page.getByText(/冲突 0/).first()).toBeVisible();
});

test("预览→执行链跑通：C05 标注逐项可见，确认后执行进入终态（EXE-01）", async ({ page }) => {
  // 入口顺序本身就是判定对象：没有计划时执行不可点（另一条用例已断），生成预览后
  // 才可执行——这就是「预览→确认→执行」这条链在界面上的形状。
  const apply = page.getByRole("button", { name: /^执行$/ });
  await expect(apply).toBeDisabled();

  await fillRenameForm(page, "full");
  await page.getByRole("button", { name: /生成计划预览/ }).click();

  const panel = page.getByRole("heading", { name: /C05 预览与冲突检测/ });
  await expect(panel).toBeVisible();
  const card = panel.locator("xpath=ancestor::*[self::section][1]");
  await expect(card).toContainText("将处理 1");
  // 对照行逐项标注（旧名 → 新名），这是"标注链"里给用户核对的那一份
  await expect(card.getByText("/mock/dup.txt → /mock/uniq.txt")).toBeVisible();

  await expect(apply).toBeEnabled();
  await apply.click();

  // 派发即入队，进度按 task:// 事件推进到全部条目收尾（1/1）。
  await expect(page.getByText(/任务已加入队列/)).toBeVisible();
  await expect(page.getByText(/1\/1/)).toBeVisible();
  // 终态徽标必须是本地化文案（labels.tsx 是状态文案的单一来源）：mock 夹具刻意留一条
  // E2003 失败项以演示 C16 重试，所以这里的终态是"失败"，同时给出重试入口。
  await expect(page.getByRole("button", { name: /^重试失败项$/ })).toBeVisible();
  const badge = page.locator(".task-panel .badge");
  await expect(badge).toHaveText(/失败|部分失败|已完成/);
});
