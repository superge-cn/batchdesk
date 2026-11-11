import { expect, test } from "@playwright/test";

// D9 回归（2026-11-10 实机验收查实）：.glass-shell 以 overflow:hidden 做圆角裁剪，
// 侧栏曾只有 min-height:0 而无内部滚动——折叠线以下导航被裁剪，鼠标滚轮与键盘都滚不动
// （仅 Tab/scrollIntoView 的程序化滚动能滚且滚不回），鼠标用户点不到「处理模板/设置」。
// 判定点：在导航区上滚轮必须能滚动它（scrollTop>0），且「处理模板」进入视口、
// brand 保持固定（导航区独立滚动，brand/页脚不随走）。
test("侧栏下折导航可用鼠标滚轮到达（处理模板），brand 保持固定", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  const tpl = page.getByRole("button", { name: /处理模板/ });
  await expect(tpl).not.toBeInViewport();
  const navRegion = page.locator(".sidebar-scroll");
  const box = await navRegion.boundingBox();
  expect(box, "导航区 .sidebar-scroll 应存在").not.toBeNull();
  if (!box) return;
  // 在导航区盒内滚动（避开边界与项间隙；不依赖固定坐标，字体度量差异不影响）
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  for (let i = 0; i < 3; i++) {
    await page.mouse.wheel(0, 600);
    if ((await navRegion.evaluate((el) => el.scrollTop)) > 0) break;
  }
  await expect
    .poll(() => navRegion.evaluate((el) => el.scrollTop))
    .toBeGreaterThan(0);
  await expect(tpl).toBeInViewport();
  await expect(page.getByText("BatchDesk").first()).toBeInViewport();
  const shellScroll = await page.evaluate(
    () => document.querySelector(".glass-shell")?.scrollTop ?? -1
  );
  expect(shellScroll).toBe(0);
});