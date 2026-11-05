import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { TemplatesPage } from "./TemplatesPage";
import type { RecipeDocument } from "./lib/ipc/types";

// 处理模板（C37）的存取半边：列表要能看懂、删得掉、导得出、导入被拒时说得清。
// **应用不在这一版的范围内**——把模板变成计划要 `loadRecipe`，卡在 OQ-36；
// 页面必须把这件事写成横幅，而不是给一个点了没反应的按钮（见 App.navigation 的
// 「不给假按钮」断言）。
const listed: RecipeDocument[] = [
  {
    recipeId: "r1",
    schemaVersion: 3,
    name: "CSV 分片（每片 5 万行）",
    description: "演示夹具",
    steps: [
      { stepId: "step-1", kind: "split", paramsSnapshot: { rows: 50000 }, source: "original" }
    ],
    match: { extensions: ["csv"] },
    createdAt: "2026-11-01T09:00:00Z",
    updatedAt: "2026-11-04T10:00:00Z",
    usageCount: 2
  }
];

vi.mock("./lib/ipc", () => ({
  listRecipes: vi.fn(async () => ({ items: listed, total: listed.length })),
  deleteRecipe: vi.fn(async () => undefined),
  exportRecipe: vi.fn(async () => ({ payload: '{\n  "schemaVersion": 3\n}' })),
  importRecipe: vi.fn(async () => listed[0]),
  toIpcError: (value: unknown) => value
}));

import { deleteRecipe, exportRecipe, importRecipe, listRecipes } from "./lib/ipc";

function renderPage() {
  return render(<TemplatesPage onBack={() => undefined} />);
}

describe("处理模板视图", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("列出模板：动作显示中文名而不是线协议枚举，步骤与建议扩展名可见", async () => {
    renderPage();
    // `split` 是内核枚举，界面上必须叫「分割」（设计 09 §2）
    expect(await screen.findByText("分割")).toBeVisible();
    expect(screen.queryByText("split")).toBeNull();
    expect(screen.getByText(/1 步 · 分割/)).toBeVisible();
    expect(screen.getByText(/csv/)).toBeVisible();
    expect(listRecipes).toHaveBeenCalled();
  });

  it("删除是两段式：第一下只进入确认态，确认后才真调用", async () => {
    renderPage();
    const user = userEvent.setup();
    await screen.findByText("分割");

    await user.click(screen.getByRole("button", { name: /^删除$/ }));
    expect(deleteRecipe).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: /确认删除/ }));
    await waitFor(() => expect(deleteRecipe).toHaveBeenCalledWith("r1"));
    expect(await screen.findByText("已删除。")).toBeVisible();
  });

  it("导入高版本模板：错误码与本地化摘要都要出现，不显示后端原文", async () => {
    vi.mocked(importRecipe).mockRejectedValueOnce({
      code: "E1007",
      message: "recipe schemaVersion 4 is newer than the supported 3",
      retryable: false
    });
    renderPage();
    const user = userEvent.setup();
    await screen.findByText("分割");

    // 粘贴而不是逐字 type：user-event 把 `{` 当按键描述符解析，粘贴才是真实路径
    await user.click(screen.getByPlaceholderText("粘贴导出的模板 JSON"));
    await user.paste('{"schemaVersion":4}');
    await user.click(screen.getByRole("button", { name: /^导入$/ }));

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("E1007");
    expect(alert.textContent).toContain("这项数据由更新版本的程序写入");
    // 技术原文不出界面（设计 06 §3）
    expect(alert.textContent).not.toContain("schemaVersion 4");
  });

  it("导出把模板 JSON 摊在可复制的文本框里", async () => {
    renderPage();
    const user = userEvent.setup();
    await screen.findByText("分割");

    await user.click(screen.getByRole("button", { name: /^导出$/ }));
    expect(exportRecipe).toHaveBeenCalledWith("r1");
    const box = await screen.findByRole("textbox", { name: /导出「CSV 分片/ });
    expect((box as HTMLTextAreaElement).value).toContain('"schemaVersion": 3');
  });
});
