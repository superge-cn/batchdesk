import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import App from "./App";

// 导航可达性回归（ADR-0040…0044）：功能视图必须真能被点出来。
// 抓到的历史缺陷：App import 了 FeaturePages 却从未渲染它——导航只换标题、主区渲染空白，
// 而 `pnpm build` 与 cargo 侧测试都无法发现（tsc 不报未使用的导入分支）。
const VIEWS: Array<[string, string]> = [
  ["CSV 分割", "CSV 分割（F05）"],
  ["CSV 合并", "CSV 合并（F06）"],
  ["CSV 抽取列", "CSV 抽取指定列（F07）"],
  ["CSV 前后 N 行", "CSV 抽取前/后 N 行（F08）"],
  ["CSV 行区间", "CSV 抽取位置行区间（F09）"],
  ["目录清单", "生成目录清单（F10）"],
  ["清单建档", "清单建档（F11）"],
  ["批量改名", "批量改名（F12）"],
  ["TXT 按字节分割", "TXT 按字节分割（F01）"],
  ["TXT 按行数分割", "TXT 按行数分割（F02）"],
  ["TXT 按份数分割", "TXT 按份数分割（F03）"],
  ["TXT 合并", "TXT 合并（F04）"],
  ["编码转换", "编码转换（C01）"],
  ["换行转换", "换行符转换（C02）"],
  ["查找替换", "查找替换（C03）"],
  ["名称查找替换", "名称查找替换（C04）"],
  ["清单移动/复制", "清单驱动移动/复制（C07）"],
  ["按规则归档", "按规则归档（C08）"],
  ["ZIP 解压/压缩", "ZIP 解压/压缩（C10）"],
  ["CSV 去重", "CSV 去重（C11）"],
  ["CSV 过滤", "CSV 过滤（C12）"],
  ["CSV 排序", "CSV 排序（C13）"],
  ["CSV 列拆分/合并", "CSV 列拆分/合并（C14）"]
];

function navButton(nav: HTMLElement, label: string, code: string) {
  const matches = within(nav)
    .getAllByRole("button")
    .filter(
      (button) => {
        const text = (button.textContent ?? "").replace(/\s+/g, " ").trim();
        return text.startsWith(label) && text.endsWith(code);
      }
    );
  expect(matches, `导航项「${label} ${code}」应存在且唯一`).toHaveLength(1);
  return matches[0];
}

describe("功能视图导航可达", () => {
  it("侧栏每一项都渲染出对应功能页的参数区与执行区", async () => {
    render(<App />);
    const nav = screen.getByRole("navigation", { name: "功能导航" });
    const main = screen.getByRole("main");
    for (const [label, title] of VIEWS) {
      const code = title.match(/[（(]([^)）]+)[)）]/)?.[1] ?? "";
      await userEvent.click(navButton(nav, label, code));
      expect(within(main).getByText(title), `${title} 未渲染`).toBeVisible();
      expect(
        within(main).getByRole("button", { name: /生成计划预览/ }),
        `${title} 缺生成计划按钮`
      ).toBeVisible();
      expect(
        within(main).getByRole("button", { name: /^撤销$/ }),
        `${title} 缺撤销按钮`
      ).toBeVisible();
    }
  });

  it("默认视图是 C09 工作台，且能回到它", async () => {
    render(<App />);
    const main = screen.getByRole("main");
    expect(main.textContent).toContain("重复文件查找");
    const nav = screen.getByRole("navigation", { name: "功能导航" });
    await userEvent.click(navButton(nav, "CSV 合并", "F06"));
    expect(within(main).getByText("CSV 合并（F06）")).toBeVisible();
    await userEvent.click(within(main).getByRole("button", { name: /返回 C09/ }));
    expect(main.textContent).toContain("重复文件查找");
  });

  it("「任务与撤销」是能点开的真实视图，不是 disabled 占位", async () => {
    // 与上面同一类回归：侧栏项存在 ≠ 视图被渲染。
    render(<App />);
    const nav = screen.getByRole("navigation", { name: "功能导航" });
    const main = screen.getByRole("main");
    const entry = within(nav).getByRole("button", { name: /任务与撤销/ });
    expect(entry).toBeEnabled();

    await userEvent.click(entry);
    expect(within(main).getByRole("heading", { name: "任务与撤销" })).toBeVisible();
    // 空队列也要给出下一步指引，而不是留一块空白
    await waitFor(() => expect(main.textContent).toContain("还没有任务记录"));

    await userEvent.click(within(main).getByRole("button", { name: /返回 C09/ }));
    expect(main.textContent).toContain("重复文件查找");
  });

  it("「设置」是能点开的真实视图：五组配置与语言选择都在", async () => {
    // 侧栏原先的 disabled 占位换成真页面后，要能证明它真的渲染了设置项。
    render(<App />);
    const nav = screen.getByRole("navigation", { name: "功能导航" });
    const main = screen.getByRole("main");
    const entry = within(nav).getByRole("button", { name: /设置/ });
    expect(entry).toBeEnabled();

    await userEvent.click(entry);
    for (const title of ["通用", "处理", "性能", "安全", "日志"]) {
      expect(await within(main).findByRole("heading", { name: title })).toBeVisible();
    }
    expect(within(main).getByLabelText("界面语言")).toBeVisible();
    expect(within(main).getByLabelText("界面语言")).toHaveValue("zh-CN");
  });

  it("「处理模板」是能点开的真实视图：列表出得来，可套用的模板给真按钮", async () => {
    // OQ-41② 落地后套用通道已接（ADR-0080）：夹具里预置的 f05 模板（已绑定、
    // 变换正文随计划保存）应当渲染「套用」按钮——判定来自 describeRecipe，
    // 与 loadRecipe 同源。这条用例同时钉两件事：
    // ① 视图真渲染（侧栏有名字 ≠ 页面在）；② 套用按钮只在可套用的模板上出现。
    render(<App />);
    const nav = screen.getByRole("navigation", { name: "功能导航" });
    const main = screen.getByRole("main");
    const entry = within(nav).getByRole("button", { name: /处理模板/ });
    expect(entry).toBeEnabled();

    await userEvent.click(entry);
    expect(await within(main).findByRole("heading", { name: "处理模板", level: 2 })).toBeVisible();
    // 浏览器夹具里预置了一条模板：列表不是空壳（空列表也会通过"页面渲染了"这种弱断言）
    await waitFor(() => expect(main.textContent).toContain("CSV 分片（每片 5 万行）"));
    expect(main.textContent).toContain("OQ-41");
    // 浏览器夹具现在有单步与多步两条可套用模板，这里只要求首条给真按钮。
    expect(within(main).getAllByTestId("template-apply").length).toBeGreaterThan(0);
  });
});
