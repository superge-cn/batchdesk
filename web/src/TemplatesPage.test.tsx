import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { TemplatesPage } from "./TemplatesPage";
import type { PlanDocument, RecipeApplySpec, RecipeDocument } from "./lib/ipc/types";

// 处理模板（C37）：存取半边（列表/删除/导出/导入）＋套用半边（OQ-40①/OQ-41②）。
// 套用判定走 describeRecipe（与 loadRecipe 同源，ADR-0080）；本文件把它 mock 成
// 两种回答——可套用（带槽位表）与不可套用（带原因）——来钉界面的两种义务：
// 可套用给真按钮＋填槽位出预览；不可套用只展示原因，绝不给假按钮。
const listed: RecipeDocument[] = [
  {
    recipeId: "r1",
    schemaVersion: 5,
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

const applicableSpec: RecipeApplySpec = {
  recipeId: "r1",
  applicable: true,
  feature: "f05",
  kind: "split",
  required: [{ key: "file", many: false }],
  outputs: [{ key: "outputDir", many: false }]
};

vi.mock("./lib/ipc", () => ({
  listRecipes: vi.fn(async () => ({ items: listed, total: listed.length })),
  describeRecipe: vi.fn(async () => applicableSpec),
  loadRecipe: vi.fn(async () => {
    throw new Error("loadRecipe 未在用例里定义预期");
  }),
  applyPlan: vi.fn(async () => ({ taskId: "task-1" })),
  deleteRecipe: vi.fn(async () => undefined),
  exportRecipe: vi.fn(async () => ({ payload: '{\n  "schemaVersion": 5\n}' })),
  importRecipe: vi.fn(async () => listed[0]),
  toIpcError: (value: unknown) => value
}));

import {
  applyPlan,
  deleteRecipe,
  describeRecipe,
  exportRecipe,
  importRecipe,
  listRecipes,
  loadRecipe
} from "./lib/ipc";

/** v5 的一步模板：功能码已绑定（OQ-36 裁决①落地后的形状）。 */
function boundRecipe(feature: string | undefined): RecipeDocument {
  return {
    recipeId: "r-bound",
    schemaVersion: 5,
    name: "按大小分片",
    steps: [
      {
        stepId: "step-1",
        kind: "split",
        ...(feature ? { feature } : {}),
        paramsSnapshot: { targetBytes: 1048576 },
        source: "original"
      }
    ],
    createdAt: "2026-11-06T09:00:00Z"
  };
}

const previewPlan: PlanDocument = {
  planId: "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
  kind: "f05",
  params: {},
  entries: [
    {
      entryId: "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8-0001",
      action: "split",
      source: "/in/a.csv",
      reason: "plan entry"
    }
  ],
  stats: { total: 1, conflict: 0, warning: 0, blocked: 0 },
  warnings: []
};

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
    expect((box as HTMLTextAreaElement).value).toContain('"schemaVersion": 5');
  });
})

describe("模板的功能码绑定（OQ-36 裁决①，schema v4）", () => {
  it("绑定过就写明是哪个功能，而不是只报动作名", async () => {
    vi.mocked(listRecipes).mockResolvedValueOnce({ items: [boundRecipe("f01")], total: 1 });
    renderPage();

    const line = await screen.findByTestId("template-binding");
    expect(line).toHaveTextContent("已绑定功能");
    expect(line).toHaveTextContent("f01");
  });

  it("没绑定的如实说「未绑定」，不假装可以套用", async () => {
    vi.mocked(listRecipes).mockResolvedValueOnce({ items: [boundRecipe(undefined)], total: 1 });
    vi.mocked(describeRecipe).mockResolvedValueOnce({
      recipeId: "r-bound",
      applicable: false,
      reason: "recipe step has no feature binding; re-save it from that feature page",
      required: [],
      outputs: []
    });
    renderPage();

    const line = await screen.findByTestId("template-binding");
    expect(line).toHaveTextContent("未绑定功能");
    expect(screen.queryByRole("button", { name: /一键套用|套用/ })).toBeNull();
  });
});

describe("模板一键套用（OQ-40①/OQ-41②，ADR-0077/0080）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("可套用的模板：填路径槽位生成预览计划，只出计划不执行", async () => {
    vi.mocked(loadRecipe).mockResolvedValueOnce(previewPlan);
    renderPage();
    const user = userEvent.setup();
    const applyButton = await screen.findByTestId("template-apply");
    await user.click(applyButton);

    // 槽位输入框按 describe 给的键渲染（必填 file ＋输出 outputDir）
    const fileInput = await screen.findByTestId("apply-slot-file");
    const outputInput = screen.getByTestId("apply-slot-outputDir");
    await user.type(fileInput, "/in/a.csv");
    await user.type(outputInput, "/out");

    await user.click(screen.getByTestId("apply-submit"));
    // 套用＝loadRecipe {recipeId, pathParams}（OQ-40①），路径是调用方给的现选值
    await waitFor(() =>
      expect(loadRecipe).toHaveBeenCalledWith("r1", { file: "/in/a.csv", outputDir: "/out" })
    );
    // 预览面板：统计＋条目列表＋"未执行"说明，执行是另一颗显式按钮
    expect(await screen.findByTestId("apply-preview-stats")).toBeVisible();
    expect(screen.getAllByText(/未执行/).length).toBeGreaterThan(0);
    expect(applyPlan).not.toHaveBeenCalled();
  });

  it("「执行此计划」显式走 applyPlan，不静默执行", async () => {
    vi.mocked(loadRecipe).mockResolvedValueOnce(previewPlan);
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId("template-apply"));
    await user.type(await screen.findByTestId("apply-slot-file"), "/in/a.csv");
    await user.type(screen.getByTestId("apply-slot-outputDir"), "/out");
    await user.click(screen.getByTestId("apply-submit"));

    await user.click(await screen.findByTestId("apply-execute"));
    await waitFor(() =>
      expect(applyPlan).toHaveBeenCalledWith("01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8", {
        onConflict: "skip",
        overwriteConfirmed: false
      })
    );
    expect(await screen.findByText(/task-1/)).toBeVisible();
  });

  it("正文缺失的模板：不给套用按钮，原因如实展示（OQ-43 的七类）", async () => {
    vi.mocked(listRecipes).mockResolvedValueOnce({ items: [boundRecipe("c08")], total: 1 });
    vi.mocked(describeRecipe).mockResolvedValueOnce({
      recipeId: "r-bound",
      applicable: false,
      feature: "c08",
      reason: "archive rules are not carried in plan params; reconfigure on the feature page",
      required: [],
      outputs: []
    });
    renderPage();

    await screen.findByTestId("template-binding");
    expect(screen.queryByTestId("template-apply")).toBeNull();
    const refused = await screen.findByTestId("template-refused");
    expect(refused).toHaveTextContent("不可套用");
    expect(refused).toHaveTextContent("archive rules are not carried in plan params");
  });
});
