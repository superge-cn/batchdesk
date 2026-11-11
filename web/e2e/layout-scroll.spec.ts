import { expect, test } from "@playwright/test";

// D9 回归（2026-11-10 实机验收查实）：.glass-shell 以 overflow:hidden 做圆角裁剪，
// 侧栏只有 min-height:0 而无内部滚动——折叠线以下导航被裁剪，鼠标滚轮与键盘都滚不动
// （仅 Tab/scrollIntoView 的程序化滚动能滚且滚不回），鼠标用户点不到「处理模板/设置」。
// 判定点：滚轮必须能滚到它们，且 brand/页脚保持固定（导航区独立滚动）。
test("侧栏下折导航可用鼠标滚轮到达（处理模板），brand 保持固定", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  const tpl = page.getByRole("button", { name: /处理模板/ });
  await expect(tpl).not.toBeInViewport();
  await page.mouse.move(120, 400);
  await page.mouse.wheel(0, 600);
  await expect(tpl).toBeInViewport();
  await expect(page.getByText("BatchDesk").first()).toBeInViewport();
  const shellScroll = await page.evaluate(
    () => document.querySelector(".glass-shell")?.scrollTop ?? -1
  );
  expect(shellScroll).toBe(0);
});