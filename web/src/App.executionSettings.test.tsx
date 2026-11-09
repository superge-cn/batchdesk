import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import App from "./App";

// C15-PRE-04（方案 §7.10）：并发是执行期设置——改它就是「新设置将在下一批生效」，
// 已生成的 C09 预览与下一批的执行参数不再一致，必须失效并阻断执行入口。
// 浏览器夹具里性能组的并发初始为 auto；改动走真实的 setConfig → 设置页提示 → App 递增
// 执行期版本号 → planSignature 变化 → plan-stale 出现、执行按钮关闭。

const ROOT = "/mock/tree";

describe("运行中调参与面板重算（C15-PRE-04）", () => {
  it("设置页改并发后提示下一批生效，旧预览失效且执行入口关闭", async () => {
    const user = userEvent.setup();
    render(<App />);
    const main = screen.getByRole("main");

    await user.type(screen.getByPlaceholderText(/资料归档|Documents/), ROOT);
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByText(/计划已生成/, undefined, { timeout: 3000 });
    expect(screen.queryByTestId("plan-stale")).toBeNull();

    await user.click(screen.getByLabelText(/我已核对预览/));
    expect(screen.getByRole("button", { name: /^移入隔离目录$/ })).toBeEnabled();

    const nav = screen.getByRole("navigation", { name: "功能导航" });
    await user.click(within(nav).getByRole("button", { name: /设置/ }));
    const input = await screen.findByLabelText("并发数");
    // 受控输入：一次 change 完成整值替换（auto → 4），不逐键触发中间值保存。
    fireEvent.change(input, { target: { value: "4" } });
    expect(await screen.findByText(/新设置将在下一批生效/)).toBeVisible();

    await user.click(within(main).getByRole("button", { name: /返回 C09/ }));
    expect(await screen.findByTestId("plan-stale")).toBeVisible();
    expect(screen.getByRole("button", { name: /^移入隔离目录$/ })).toBeDisabled();
  });
});
