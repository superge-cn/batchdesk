import { expect, test } from "@playwright/test";

// 真实浏览器里的功能视图可达性（ADR-0045 挂载缺陷的回归面）。
// jsdom 已断言同一件事；这里补的是真实布局（`display: contents` 容器与 grid）下
// 功能页确实占据主区、且 C09 工作台不会残留。
const VIEWS: Array<[string, string, string]> = [
  ["CSV 分割", "F05", "CSV 分割（F05）"],
  ["CSV 合并", "F06", "CSV 合并（F06）"],
  ["CSV 抽取列", "F07", "CSV 抽取指定列（F07）"],
  ["CSV 前后 N 行", "F08", "CSV 抽取前/后 N 行（F08）"],
  ["CSV 行区间", "F09", "CSV 抽取位置行区间（F09）"],
  ["目录清单", "F10", "生成目录清单（F10）"],
  ["清单建档", "F11", "清单建档（F11）"],
  ["批量改名", "F12", "批量改名（F12）"],
  ["TXT 按字节分割", "F01", "TXT 按字节分割（F01）"],
  ["TXT 按行数分割", "F02", "TXT 按行数分割（F02）"],
  ["TXT 按份数分割", "F03", "TXT 按份数分割（F03）"],
  ["TXT 合并", "F04", "TXT 合并（F04）"],
  ["编码转换", "C01", "编码转换（C01）"],
  ["换行转换", "C02", "换行符转换（C02）"],
  ["查找替换", "C03", "查找替换（C03）"],
  ["名称查找替换", "C04", "名称查找替换（C04）"],
  ["清单移动/复制", "C07", "清单驱动移动/复制（C07）"],
  ["按规则归档", "C08", "按规则归档（C08）"],
  ["ZIP 解压/压缩", "C10", "ZIP 解压/压缩（C10）"],
  ["CSV 去重", "C11", "CSV 去重（C11）"],
  ["CSV 过滤", "C12", "CSV 过滤（C12）"],
  ["CSV 排序", "C13", "CSV 排序（C13）"],
  ["CSV 列拆分/合并", "C14", "CSV 列拆分/合并（C14）"]
];

test.beforeEach(async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("navigation", { name: "功能导航" })).toBeVisible();
});

test("23 个功能视图在真实浏览器里逐个可达", async ({ page }) => {
  const nav = page.getByRole("navigation", { name: "功能导航" });
  for (const [label, code, title] of VIEWS) {
    const button = nav.getByRole("button", { name: new RegExp(`^${label}\\s*${code}$`) });
    await expect(button, `导航项「${label} ${code}」缺失`).toHaveCount(1);
    await button.click();
    const heading = page.getByRole("heading", { name: title, exact: true });
    await expect(heading, `点击「${label}」后功能页标题未渲染`).toBeVisible();
    await expect(
      page.getByRole("button", { name: /生成计划预览/ }),
      `${title} 缺生成计划入口`
    ).toBeVisible();
    await expect(page.getByRole("button", { name: /^撤销$/ })).toBeVisible();
    // 回到工作台再进下一个视图，避免状态串页
    await page.getByRole("button", { name: /返回 C09/ }).click();
    await expect(page.getByRole("button", { name: /生成计划预览/ })).toBeVisible();
  }
});

test("功能页不残留上一视图的计划与预览", async ({ page }) => {
  const nav = page.getByRole("navigation", { name: "功能导航" });
  await nav.getByRole("button", { name: /^CSV 合并\s*F06$/ }).click();
  await expect(page.getByRole("heading", { name: "CSV 合并（F06）", exact: true })).toBeVisible();
  await nav.getByRole("button", { name: /^CSV 排序\s*C13$/ }).click();
  await expect(page.getByRole("heading", { name: "CSV 排序（C13）", exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "计划预览" })).toHaveCount(0);
});
