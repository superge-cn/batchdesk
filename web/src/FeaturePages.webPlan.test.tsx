import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FeaturePages } from "./FeaturePages";
import { installFakeOpfs } from "./test/fakeOpfs";

/**
 * A3-1 纵切：浏览器形态（非 Tauri）下 C02 的**真计划**链路。
 *
 * 钉住三件事：
 * ① 浏览器里字段渲染成文件选择器（不是路径文本域）；
 * ② 选了文件 → `plan_c02` 走 WASM 通道，计划被真实展示；
 * ③ **真计划不配假执行**——早期 Web 没有执行器，执行/撤销/预演/存模板必须禁用并明示；
 *    A3-6 起执行走**会话内内存沙箱**（不是 OPFS 执行器），A3-13 起产物在执行后
 *    flush 到 OPFS 留一份（「执行后搬运」，不改内核同步契约）。
 *
 * 没有文件时仍走原 mock 路径（能力降级），由第二个用例钉住，防止本切片改变既有行为。
 */

const wasmPlan = {
  planId: "01WASMC02PLAN0000000000000",
  kind: "c02",
  params: { target: "lf" },
  entries: [
    {
      entryId: "e1",
      source: "a.txt",
      target: "a.txt",
      action: "edit-content",
      reason: "convert crlf to lf",
      blocked: null
    }
  ],
  stats: { total: 1, conflict: 0, warning: 0, blocked: 0 },
  warnings: []
};

const mockPlan = {
  ...wasmPlan,
  planId: "01MOCKC02PLAN0000000000000",
  kind: "c02"
};

const { callWasm, planOperationMock } = vi.hoisted(() => ({
  callWasm: vi.fn(),
  planOperationMock: vi.fn()
}));

vi.mock("./lib/ipc", () => ({
  // 关键：模拟**浏览器形态**（无 Tauri）。
  isTauri: () => false,
  getUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  setUiState: vi.fn(async () => ({ uiStateVersion: 1, updatedAt: "" })),
  getConfig: vi.fn(async () => ({ settings: {}, sources: {} })),
  planOperation: planOperationMock,
  applyPlan: vi.fn(async () => ({ taskId: "task-apply" })),
  undoPlan: vi.fn(async () => ({ taskId: "task-undo" })),
  cancelTask: vi.fn(async () => undefined),
  retryTask: vi.fn(async () => ({ taskId: "task-retry" })),
  pauseTask: vi.fn(async () => undefined),
  resumeTask: vi.fn(async () => undefined),
  saveRecipe: vi.fn(async () => ({ name: "t" })),
  getTask: vi.fn(async () => ({
    summary: { state: "queued", phase: "queued", done: 0, total: 1 },
    entries: []
  })),
  renamePreview: vi.fn(async () => ({
    rows: [],
    counts: { toProcess: 0, skipped: 0, conflicts: 0, blocked: 0 },
    highlights: []
  })),
  undoDryRun: vi.fn(async () => ({ entries: [], conflictCount: 0, missingCount: 0 })),
  listenTaskProgress: vi.fn(async () => () => undefined),
  listenTaskState: vi.fn(async () => () => undefined),
  listenTaskDone: vi.fn(async () => () => undefined),
  toIpcError: (value: unknown) => value
}));

vi.mock("./lib/wasm/client", () => ({
  createWasmClient: () => ({ call: callWasm, terminate: vi.fn() })
}));

function pickFile(name: string, text: string): File {
  const bytes = new TextEncoder().encode(text);
  return new File([bytes as unknown as BlobPart], name, { type: "text/plain" });
}

/** 目录选择得到的 File（`webkitRelativePath` 只有目录选择才有）。 */
function pickFromDirectory(relative: string, text: string): File {
  const bytes = new TextEncoder().encode(text);
  const file = new File(
    [bytes as unknown as BlobPart],
    relative.split("/").pop() ?? relative,
    { type: "text/plain" }
  );
  Object.defineProperty(file, "webkitRelativePath", { value: relative });
  return file;
}

describe("浏览器形态 C02 真计划（A3-1 纵切）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // vi.mock 的工厂被提升到文件顶部，不能在工厂里引用顶层常量——
    // 实现放这里注入，工厂只负责交出同一个 vi.fn。
    callWasm.mockResolvedValue({ ok: true, result: JSON.stringify(wasmPlan) });
    planOperationMock.mockResolvedValue(mockPlan);
  });

  it("浏览器里渲染文件选择器，选文件后用 WASM 通道出真计划且执行链可用", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="c02" onBack={() => undefined} />);

    const input = screen.getByTestId("web-file-input") as HTMLInputElement;
    expect(input.type).toBe("file");
    expect(input.multiple).toBe(true);

    fireEvent.change(input, { target: { files: [pickFile("a.txt", "a\r\nb")] } });
    // 目标换行方式必须填；否则内核按 E1002 拒绝（不替用户选默认值）。
    const target = document.querySelector<HTMLInputElement>("#c02-target")!;
    await user.type(target, "lf");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("heading", { name: "计划预览" });

    // 走的是 WASM：外层 mock 计划从未被请求。
    expect(callWasm).toHaveBeenCalledTimes(1);
    expect(planOperationMock).not.toHaveBeenCalled();
    const request = callWasm.mock.calls[0][0] as { op: string; payload: string };
    expect(request.op).toBe("plan_c02");
    expect(JSON.parse(request.payload).files[0]).toMatchObject({
      path: "a.txt",
      fileSize: 4
    });

    // C02 的执行臂已接（改写器端口在沙箱内）：执行可用，撤销要等真执行过；
    // 回滚预演与存模板仍是桌面能力，浏览器继续禁用。
    expect(screen.queryByTestId("web-plan-readonly")).toBeNull();
    expect(screen.getByTestId("web-sandbox-note")).toBeVisible();
    expect(screen.getByRole("button", { name: /^执行$/ })).toBeEnabled();
    expect(screen.getByRole("button", { name: /^撤销$/ })).toBeDisabled();
    expect(screen.getByRole("button", { name: /回滚预演/ })).toBeDisabled();
    expect(screen.getByTestId("save-template")).toBeDisabled();
  });

  it("未选文件时保持原 mock 路径（不改既有降级行为），但必须自报是演示数据", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="c02" onBack={() => undefined} />);
    const target = document.querySelector<HTMLInputElement>("#c02-target")!;
    await user.type(target, "lf");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("heading", { name: "计划预览" });

    expect(planOperationMock).toHaveBeenCalledTimes(1);
    expect(callWasm).not.toHaveBeenCalled();
    expect(screen.queryByTestId("web-plan-readonly")).toBeNull();
    // 假计划可以存在，但不能冒充真计划：必须标出「以下为演示数据」并说明怎么得到真计划。
    const demo = screen.getByTestId("web-plan-demo");
    expect(demo).toBeVisible();
    expect(demo.textContent).toMatch(/演示数据/);
    expect(demo.textContent).toMatch(/选择文件/);
  });
});

describe("浏览器形态横向铺开（A3-2：不只 C02）", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    callWasm.mockResolvedValue({ ok: true, result: JSON.stringify(wasmPlan) });
    planOperationMock.mockResolvedValue(mockPlan);
  });

  it("C01 多文件视图同样走 WASM 真通道", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="c01" onBack={() => undefined} />);

    const input = screen.getByTestId("web-file-input") as HTMLInputElement;
    expect(input.type).toBe("file");
    fireEvent.change(input, {
      target: { files: [pickFile("a.txt", "a"), pickFile("b.txt", "b")] }
    });
    const target = document.querySelector<HTMLInputElement>("#c01-target")!;
    await user.type(target, "utf-8");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("heading", { name: "计划预览" });

    expect(planOperationMock).not.toHaveBeenCalled();
    expect(callWasm).toHaveBeenCalledTimes(1);
    const request = callWasm.mock.calls[0][0] as { op: string; payload: string };
    expect(request.op).toBe("plan_c01");
    const payload = JSON.parse(request.payload);
    expect(payload.target).toBe("utf-8");
    expect(payload.files).toHaveLength(2);
    // C01 转码同样接上了执行臂（`TranscodeRewriter` 在 kernel 里）。
    expect(screen.getByTestId("web-sandbox-note")).toBeVisible();
    expect(screen.getByRole("button", { name: /^执行$/ })).toBeEnabled();
  });

  it("C08 在浏览器里渲染目录选择器，选目录后走 plan_c08", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="c08" onBack={() => undefined} />);

    // 目录选择器而不是普通文件多选：归档规则要按相对路径判定归属。
    const input = screen.getByTestId("web-directory-input") as HTMLInputElement;
    expect(input.type).toBe("file");
    expect(input.hasAttribute("webkitdirectory")).toBe(true);
    fireEvent.change(input, {
      target: {
        files: [
          pickFromDirectory("src/a.jpg", "a"),
          pickFromDirectory("src/sub/b.png", "bb")
        ]
      }
    });

    await user.type(document.querySelector<HTMLInputElement>("#c08-targetRoot")!, "/dst");
    await user.type(
      document.querySelector<HTMLTextAreaElement>("#c08-rules")!,
      "images|ext:jpg,png"
    );
    await user.type(
      document.querySelector<HTMLInputElement>("#c08-recursive")!,
      "yes"
    );
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("heading", { name: "计划预览" });

    expect(planOperationMock).not.toHaveBeenCalled();
    const request = callWasm.mock.calls[0][0] as { op: string; payload: string };
    expect(request.op).toBe("plan_c08");
    const payload = JSON.parse(request.payload);
    expect(payload.sourceRoot).toBe("src");
    expect(payload.rules).toEqual([
      { subDir: "images", condition: { kind: "extensions", value: ["jpg", "png"] } }
    ]);
    expect(
      (payload.files as Array<{ relPath: string }>).map((file) => file.relPath)
    ).toEqual(["a.jpg", "sub/b.png"]);
    // A3-9：源根就是所选目录名，staging 键按同一规则拼（`src/相对路径`），
    // 所以这个视图从「只能看」升级为可真执行。
    expect(screen.getByTestId("web-sandbox-note")).toBeVisible();
  });

  it("F10 在浏览器里渲染目录选择器，选目录后走 plan_f10 且执行入口可用（A3-12）", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="f10" onBack={() => undefined} />);

    // 桌面的 roots 是多行路径；浏览器里必须是目录选择——清单的源是根目录，
    // 只有目录选择才给得出根名与相对路径。
    const input = screen.getByTestId("web-directory-input") as HTMLInputElement;
    expect(input.type).toBe("file");
    expect(input.hasAttribute("webkitdirectory")).toBe(true);
    fireEvent.change(input, {
      target: {
        files: [
          pickFromDirectory("素材/a.txt", "aaa"),
          pickFromDirectory("素材/sub/b.txt", "hello")
        ]
      }
    });

    await user.type(document.querySelector<HTMLInputElement>("#f10-outputDir")!, "/out");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("heading", { name: "计划预览" });

    const request = callWasm.mock.calls[0][0] as { op: string; payload: string };
    expect(request.op).toBe("plan_f10");
    expect(JSON.parse(request.payload)).toEqual({
      roots: [
        {
          path: "素材",
          target: "/out/素材.csv",
          files: 2,
          dirs: 1,
          maxDepth: 2
        }
      ]
    });
    // A3-12：从「只能看」升级为可真执行（staging 走 stage_dirs + stage_listing）。
    expect(screen.getByTestId("web-sandbox-note")).toBeVisible();
  });

  it("C10 在浏览器里渲染单文件选择器；非 ZIP 直接报 E3003，不退回演示计划", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="c10" onBack={() => undefined} />);

    const input = screen.getByTestId("web-single-file-input") as HTMLInputElement;
    expect(input.type).toBe("file");
    // 解压一次只有一个包：不是多选。
    expect(input.multiple).toBe(false);

    fireEvent.change(input, {
      target: { files: [pickFile("not-a.zip", "this is plain text")] }
    });
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("alert");

    // 关键：**没有**退回 mock 演示计划——压缩包读不出来就明确失败。
    // 之前的隐患是「表单里没有文件选择器」，导致 C10 永远走不到真通道。
    expect(planOperationMock).not.toHaveBeenCalled();
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("E3003");
  });


  it("C10 压缩侧也能从用户入口走通：计划 → 沙箱执行（两次边界跳）", async () => {
    const user = userEvent.setup();
    // 计划与执行按 op 分别回包：C10 的执行臂要经过 stage → apply 两跳。
    callWasm.mockImplementation(async (request: { op: string }) => {
      switch (request.op) {
        case "plan_c10_compress":
          return { ok: true, result: JSON.stringify({ ...wasmPlan, kind: "c10" }) };
        case "stage_inputs":
          return { ok: true, result: '{"staged":2}' };
        case "apply_plan":
          return {
            ok: true,
            result: JSON.stringify({
              outcome: { state: "completed", report: { entries: [{ status: "ok" }] } },
              products: [{ path: "/out/bundle.zip", size: 12 }]
            })
          };
        case "reset_sandbox":
          return { ok: true, result: '{"reset":true}' };
        default:
          throw new Error(`unexpected op ${request.op}`);
      }
    });
    render(<FeaturePages view="c10" onBack={() => undefined} />);

    // 压缩侧选择器是**多选文件**（没有目录树，不能整目录压缩）。
    const sources = screen.getByTestId("web-file-input") as HTMLInputElement;
    expect(sources.multiple).toBe(true);
    fireEvent.change(sources, {
      target: { files: [pickFile("a.txt", "AAAA"), pickFile("b.txt", "BB")] }
    });
    await user.type(document.querySelector<HTMLInputElement>("#c10-mode")!, "compress");
    await user.type(document.querySelector<HTMLInputElement>("#c10-output")!, "/out/bundle.zip");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("heading", { name: "计划预览" });

    // 压缩侧不再按缺口拒绝：走的是真入口，且两槽各归各的。
    expect(planOperationMock).not.toHaveBeenCalled();
    expect(callWasm).toHaveBeenCalledTimes(1);
    const request = callWasm.mock.calls[0][0] as { op: string; payload: string };
    expect(request.op).toBe("plan_c10_compress");
    const payload = JSON.parse(request.payload);
    expect(payload.output).toBe("/out/bundle.zip");
    expect(payload.sources).toEqual(["a.txt", "b.txt"]);
    expect(payload.sourceFileCount).toBe(2);
    expect(payload.sourceBytes).toBe(6);
    // C10 已接执行面（ADR-0112）：横幅说明的是「产物留在浏览器、需自行下载」，
    // 不再是「只能看不能执行」。
    expect(screen.getByTestId("web-sandbox-note")).toBeVisible();

    // 执行：压缩源按计划里的 sources 逐字登记（裸名会让执行器报源缺失）。
    await user.click(screen.getByRole("button", { name: /^执行$/ }));
    const panel = await screen.findByTestId("web-sandbox-result");
    expect(panel.textContent).toContain("/out/bundle.zip");

    const ops = callWasm.mock.calls.map(([call]) => (call as { op: string }).op);
    expect(ops).toEqual(["plan_c10_compress", "stage_inputs", "apply_plan"]);
    const stage = callWasm.mock.calls.find(
      ([call]) => (call as { op: string }).op === "stage_inputs"
    )![0] as { payload: string };
    expect(JSON.parse(stage.payload)).toEqual([
      { path: "a.txt", length: 4 },
      { path: "b.txt", length: 2 }
    ]);
    const applied = callWasm.mock.calls.find(
      ([call]) => (call as { op: string }).op === "apply_plan"
    )![0] as { payload: string };
    expect(JSON.parse(applied.payload).plan.kind).toBe("c10");
  });


  it("执行成功后把产物留一份到浏览器（OPFS），面板如实报数量（A3-13）", async () => {
    const user = userEvent.setup();
    const { root, restore } = installFakeOpfs();
    try {
      callWasm.mockImplementation(async (request: { op: string }) => {
        switch (request.op) {
          case "plan_c02":
            return { ok: true, result: JSON.stringify({ ...wasmPlan, kind: "c02" }) };
          case "stage_inputs":
            return { ok: true, result: '{"staged":1}' };
          case "apply_plan":
            return {
              ok: true,
              result: JSON.stringify({
                outcome: { state: "completed", report: { entries: [{ status: "ok" }] } },
                products: [{ path: "/out/a.txt", size: 3 }]
              }),
              // 产物字节随响应回来（Worker 会包成 Blob）；落盘写的就是这份。
              products: [{ path: "/out/a.txt", blob: new Blob(["lf"]) }]
            };
          default:
            throw new Error(`unexpected op ${request.op}`);
        }
      });
      render(<FeaturePages view="c02" onBack={() => undefined} />);

      const input = screen.getByTestId("web-file-input") as HTMLInputElement;
      fireEvent.change(input, { target: { files: [pickFile("a.txt", "crlf")] } });
      await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
      await screen.findByRole("heading", { name: "计划预览" });
      await user.click(screen.getByRole("button", { name: /^执行$/ }));

      // 落盘是执行后的异步搬运：面板必须等它回来才显示数量。
      const note = await screen.findByTestId("web-sandbox-persisted");
      expect(note.textContent).toContain("已保存到浏览器：1 个");

      // 不是「口头上保存」：OPFS 里真有这次计划的目录与产物。
      const products = root.dirs.get("batchdesk")!.dirs.get("products")!;
      const planDir = products.dirs.get(wasmPlan.planId)!;
      expect([...planDir.files.keys()]).toEqual(["a.txt"]);
    } finally {
      restore();
    }
  });

  it("C07 在浏览器里分别选清单与源目录，字节交给内核解析后出 plan_c07", async () => {
    const user = userEvent.setup();
    // C07 先解析清单（字节型）再出计划：按 op 分别回包。
    callWasm.mockImplementation(async (request: { op: string }) => {
      if (request.op === "parse_c07_manifest") {
        return {
          ok: true,
          result: JSON.stringify([{ name: "a.jpg", subDir: "photos" }])
        };
      }
      return { ok: true, result: JSON.stringify({ ...wasmPlan, kind: "c07" }) };
    });
    render(<FeaturePages view="c07" onBack={() => undefined} />);

    const manifestInput = document.querySelector<HTMLInputElement>("#c07-manifest")!;
    const sourceInput = document.querySelector<HTMLInputElement>("#c07-sourceRoot")!;
    // 两份独立输入各占一个选择器：清单单选；源目录必须是整目录选择
    // （`webkitRelativePath` 是相对路径的唯一来源）。
    expect(manifestInput.multiple).toBe(false);
    expect(sourceInput.hasAttribute("webkitdirectory")).toBe(true);

    fireEvent.change(manifestInput, {
      target: { files: [pickFile("manifest.csv", "a.jpg,photos\n")] }
    });
    fireEvent.change(sourceInput, {
      target: { files: [pickFromDirectory("src/a.jpg", "AAAA")] }
    });
    await user.type(document.querySelector<HTMLInputElement>("#c07-targetRoot")!, "/dst");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("heading", { name: "计划预览" });

    expect(planOperationMock).not.toHaveBeenCalled();
    expect(
      callWasm.mock.calls.map(([request]) => (request as { op: string }).op)
    ).toEqual(["parse_c07_manifest", "plan_c07"]);

    // 交给内核的必须是**清单**的字节，不是源目录里那张图片——分槽传错会在这里暴露。
    const bytes = (callWasm.mock.calls[0][0] as { bytes: Uint8Array }).bytes;
    expect(new TextDecoder().decode(bytes)).toBe("a.jpg,photos\n");

    const payload = JSON.parse(
      (callWasm.mock.calls[1][0] as { payload: string }).payload
    );
    expect(payload.rows).toEqual([{ name: "a.jpg", subDir: "photos" }]);
    expect(payload.facts).toEqual([{ exists: true, size: 4 }]);
    // 计划里的源根是**所选目录名**（浏览器里该字段是目录选择器，没有文本值）：
    // 执行期 staging 的键 `src/a.jpg` 必须由同一条规则推出。
    expect(payload.sourceRoot).toBe("src");
    expect(screen.getByTestId("web-sandbox-note")).toBeVisible();
  });

  it("C07 的执行链：只把源目录登记进沙箱，键是「源根/相对路径」", async () => {
    const user = userEvent.setup();
    const c07Plan = {
      ...wasmPlan,
      kind: "c07",
      entries: [
        {
          entryId: "e1",
          source: "src/a.jpg",
          target: "/dst/photos/a.jpg",
          action: "move",
          reason: "manifest move src/a.jpg into /dst/photos",
          blocked: null
        }
      ]
    };
    callWasm.mockImplementation(async (request: { op: string }) => {
      switch (request.op) {
        case "parse_c07_manifest":
          return {
            ok: true,
            result: JSON.stringify([{ name: "a.jpg", subDir: "photos" }])
          };
        case "plan_c07":
          return { ok: true, result: JSON.stringify(c07Plan) };
        case "stage_inputs":
          return { ok: true, result: '{"staged":1}' };
        case "apply_plan":
          return {
            ok: true,
            result: JSON.stringify({
              outcome: {
                state: "completed",
                report: { entries: [{ status: "ok" }] }
              },
              products: [{ path: "/dst/photos/a.jpg", size: 4 }]
            }),
            products: [
              { path: "/dst/photos/a.jpg", size: 4, blob: new Blob(["AAAA"]) }
            ]
          };
        default:
          throw new Error(`unexpected op ${request.op}`);
      }
    });
    render(<FeaturePages view="c07" onBack={() => undefined} />);

    fireEvent.change(document.querySelector<HTMLInputElement>("#c07-manifest")!, {
      target: { files: [pickFile("manifest.csv", "a.jpg,photos\n")] }
    });
    fireEvent.change(document.querySelector<HTMLInputElement>("#c07-sourceRoot")!, {
      target: { files: [pickFromDirectory("src/a.jpg", "AAAA")] }
    });
    await user.type(document.querySelector<HTMLInputElement>("#c07-targetRoot")!, "/dst");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("heading", { name: "计划预览" });

    await user.click(screen.getByRole("button", { name: /^执行$/ }));
    await screen.findByTestId("web-sandbox-result");

    const stage = callWasm.mock.calls.find(
      ([request]) => (request as { op: string }).op === "stage_inputs"
    )![0] as { payload: string; bytes: Uint8Array };
    // 清单只参与计划、不进沙箱；源目录的键带源根，与计划的 `source` 逐字一致。
    expect(JSON.parse(stage.payload)).toEqual([{ path: "src/a.jpg", length: 4 }]);
    expect(new TextDecoder().decode(stage.bytes)).toBe("AAAA");
  });

  it("浏览器算不了的视图（F11）显示演示数据并说明缺什么", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="f11" onBack={() => undefined} />);

    // F11 要「目标目录已有什么」这份事实，浏览器拿不到，只能出演示数据。
    const list = document.querySelector<HTMLTextAreaElement>("#f11-list")!;
    await user.type(list, "a/");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("heading", { name: "计划预览" });

    expect(callWasm).not.toHaveBeenCalled();
    const demo = screen.getByTestId("web-plan-demo");
    expect(demo.textContent).toMatch(/演示数据/);
    // 原因必须具体到「需要读取整个目录」，不能只说一句「不支持」。
    expect(demo.textContent).toMatch(/整个目录/);
  });
});

/**
 * A3-6：浏览器沙箱执行（ADR-0108）。
 *
 * 与上面 C02 那条的分工：C02 证明「未接执行臂的视图仍禁用」；这里证明**接了执行
 * 臂的分片视图**真的把计划交给内核执行，并把产物列出来、能撤销。执行器在 Worker
 * 里跑（WASM），本用例用同形的假 caller 钉住协议与界面语义。
 */
describe("浏览器沙箱执行（A3-6 纵切）", () => {
  const sandboxPlan = {
    planId: "01WASMF03PLAN0000000000000",
    kind: "f03",
    params: { parts: 2, outputDir: "/out" },
    entries: [
      {
        entryId: "e1",
        source: "a.txt",
        target: "/out/a.1.txt",
        action: "split",
        reason: "part 1/2 of a.txt",
        blocked: null,
        range: { start: 0, end: 4 }
      },
      {
        entryId: "e2",
        source: "a.txt",
        target: "/out/a.2.txt",
        action: "split",
        reason: "part 2/2 of a.txt",
        blocked: null,
        range: { start: 4, end: 8 }
      }
    ],
    stats: { total: 2, conflict: 0, warning: 0, blocked: 0 },
    warnings: []
  };

  const productBlob = new Blob(["aaa\n"]);

  function mockSandbox(): void {
    callWasm.mockImplementation(async (request: { op: string }) => {
      switch (request.op) {
        case "plan_f03":
          return { ok: true, result: JSON.stringify(sandboxPlan) };
        case "stage_inputs":
          return { ok: true, result: '{"staged":1}' };
        case "apply_plan":
          return {
            ok: true,
            result: JSON.stringify({
              outcome: {
                state: "completed",
                report: { entries: [{ status: "ok" }, { status: "ok" }] }
              },
              products: [
                { path: "/out/a.1.txt", size: 4 },
                { path: "/out/a.2.txt", size: 4 }
              ]
            }),
            products: [
              { path: "/out/a.1.txt", size: 4, blob: productBlob },
              { path: "/out/a.2.txt", size: 4, blob: productBlob }
            ]
          };
        case "undo_plan":
          return { ok: true, result: JSON.stringify({ reverted: 2, failed: [] }) };
        case "reset_sandbox":
          return { ok: true, result: '{"reset":true}' };
        default:
          throw new Error(`unexpected op ${request.op}`);
      }
    });
  }

  beforeEach(() => {
    vi.clearAllMocks();
    mockSandbox();
  });

  it("执行 → 产物列表 → 撤销：走真实的 stage/apply/undo 协议", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="f03" onBack={() => undefined} />);

    const input = screen.getByTestId("web-file-input") as HTMLInputElement;
    fireEvent.change(input, {
      target: { files: [pickFile("a.txt", "aaa\nbbb\n")] }
    });
    await user.type(document.querySelector<HTMLInputElement>("#f03-parts")!, "2");
    await user.type(
      document.querySelector<HTMLInputElement>("#f03-outputDir")!,
      "/out"
    );
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("heading", { name: "计划预览" });

    // 接了执行臂：不再是「只能看」的横幅，而是沙箱执行说明。
    expect(screen.queryByTestId("web-plan-readonly")).toBeNull();
    expect(screen.getByTestId("web-sandbox-note")).toBeVisible();
    const apply = screen.getByRole("button", { name: /^执行$/ });
    expect(apply).toBeEnabled();

    await user.click(apply);
    const panel = await screen.findByTestId("web-sandbox-result");
    expect(panel.textContent).toContain("/out/a.1.txt");
    expect(panel.textContent).toContain("下载全部（2）"); // 产物计数
    expect(panel.textContent).toMatch(/撤销只回滚本页沙箱/);

    // 协议：先登记输入（清单＋拼接字节），再执行，最后才可能撤销。
    const ops = callWasm.mock.calls.map(([request]) => (request as { op: string }).op);
    expect(ops).toEqual(["plan_f03", "stage_inputs", "apply_plan"]);
    const stage = callWasm.mock.calls.find(
      ([request]) => (request as { op: string }).op === "stage_inputs"
    )![0] as { payload: string; bytes: Uint8Array };
    expect(JSON.parse(stage.payload)).toEqual([{ path: "a.txt", length: 8 }]);
    expect(new TextDecoder().decode(stage.bytes)).toBe("aaa\nbbb\n");

    const applied = callWasm.mock.calls.find(
      ([request]) => (request as { op: string }).op === "apply_plan"
    )![0] as { payload: string };
    const request = JSON.parse(applied.payload);
    expect(request.plan.planId).toBe(sandboxPlan.planId);
    expect(request.options.onConflict).toBe("skip");

    // 撤销：沙箱内产物收回，面板消失、按钮回到禁用（没有可撤销的东西）。
    await user.click(screen.getByRole("button", { name: /^撤销$/ }));
    await waitFor(() => {
      expect(screen.queryByTestId("web-sandbox-result")).toBeNull();
    });
    expect(
      callWasm.mock.calls.map(([entry]) => (entry as { op: string }).op)
    ).toContain("undo_plan");
    expect(screen.getByRole("button", { name: /^撤销$/ })).toBeDisabled();
  });

  it("超出 §13.10.2 本地建议档时先确认，确认后才读内容并执行", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="f03" onBack={() => undefined} />);

    const big = pickFile("big.txt", "x");
    // 只改 size 事实（不真造 100MB）：确认框必须在**读内容之前**拦住。
    Object.defineProperty(big, "size", { value: 100_000_001 });
    fireEvent.change(screen.getByTestId("web-file-input"), {
      target: { files: [big] }
    });
    await user.type(document.querySelector<HTMLInputElement>("#f03-parts")!, "2");
    await user.type(
      document.querySelector<HTMLInputElement>("#f03-outputDir")!,
      "/out"
    );
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("heading", { name: "计划预览" });

    await user.click(screen.getByRole("button", { name: /^执行$/ }));
    expect(screen.getByTestId("danger-confirm")).toBeVisible();
    // 确认前不得读内容（stage 是读全部字节的那一步）。
    expect(
      callWasm.mock.calls.some(
        ([request]) => (request as { op: string }).op === "stage_inputs"
      )
    ).toBe(false);

    await user.click(screen.getByRole("button", { name: "继续执行" }));
    await screen.findByTestId("web-sandbox-result");
    expect(
      callWasm.mock.calls.map(([request]) => (request as { op: string }).op)
    ).toContain("stage_inputs");
  });
});

/**
 * A3-11：改名族（C04/F12）的浏览器执行。
 *
 * 与 A3-6 的分片用例分工相同：这里钉的是**改名特有的口径**——条目是目录第一层，
 * 但 staging 要放进整棵子树（目录改名会带走它），产物是改名后的文件（可下载）。
 */
describe("改名族的浏览器执行（A3-11）", () => {
  const renamePlan = {
    planId: "01RENAMEF12PLAN00000000000",
    kind: "f12",
    params: { prefix: "new_", scope: "Full" },
    entries: [
      {
        entryId: "e1",
        source: "素材/a.txt",
        target: "素材/new_a.txt",
        action: "rename",
        reason: "rename to target",
        blocked: null,
        range: null
      },
      {
        entryId: "e2",
        source: "素材/sub",
        target: "素材/new_sub",
        action: "rename",
        reason: "rename to target",
        blocked: null,
        range: null
      }
    ],
    stats: { total: 2, conflict: 0, warning: 0, blocked: 0 },
    warnings: []
  };

  beforeEach(() => {
    vi.clearAllMocks();
    callWasm.mockImplementation(async (request: { op: string }) => {
      switch (request.op) {
        case "plan_f12":
          return { ok: true, result: JSON.stringify(renamePlan) };
        case "stage_inputs":
          return { ok: true, result: '{"staged":2}' };
        case "apply_plan":
          return {
            ok: true,
            result: JSON.stringify({
              outcome: {
                state: "completed",
                report: { entries: [{ status: "ok" }, { status: "ok" }] }
              },
              products: [{ path: "/素材/new_a.txt", size: 3 }]
            }),
            products: [
              { path: "/素材/new_a.txt", size: 3, blob: new Blob(["AAA"]) }
            ]
          };
        case "reset_sandbox":
          return { ok: true, result: '{"reset":true}' };
        default:
          throw new Error(`unexpected op ${request.op}`);
      }
    });
  });

  it("选目录 → plan_f12 → stage（根名/相对路径）→ apply，产物可下载", async () => {
    const user = userEvent.setup();
    render(<FeaturePages view="f12" onBack={() => undefined} />);

    // F12 的 dir 在浏览器里是目录选择器（条目来自每个文件的相对路径）。
    const input = screen.getByTestId("web-directory-input") as HTMLInputElement;
    expect(input.hasAttribute("webkitdirectory")).toBe(true);
    fireEvent.change(input, {
      target: {
        files: [
          pickFromDirectory("素材/a.txt", "AAA"),
          pickFromDirectory("素材/sub/b.txt", "BB")
        ]
      }
    });
    await user.type(document.querySelector<HTMLInputElement>("#f12-prefix")!, "new_");
    await user.click(screen.getByRole("button", { name: /生成计划预览/ }));
    await screen.findByRole("heading", { name: "计划预览" });

    // 目录里的条目能执行：横幅说明的是「产物留在浏览器、需自行下载」。
    expect(screen.getByTestId("web-sandbox-note")).toBeVisible();
    await user.click(screen.getByRole("button", { name: /^执行$/ }));

    const panel = await screen.findByTestId("web-sandbox-result");
    expect(panel.textContent).toContain("/素材/new_a.txt");

    const ops = callWasm.mock.calls.map(([call]) => (call as { op: string }).op);
    expect(ops).toEqual(["plan_f12", "stage_inputs", "apply_plan"]);
    const stage = callWasm.mock.calls.find(
      ([call]) => (call as { op: string }).op === "stage_inputs"
    )![0] as { payload: string };
    // 条目只有第一层（a.txt 与 sub），但子树里的 b.txt 必须一起进沙箱：
    // 目录改名的执行臂会把整棵子树搬走，缺了它产物就成空目录。
    expect(JSON.parse(stage.payload)).toEqual([
      { path: "素材/a.txt", length: 3 },
      { path: "素材/sub/b.txt", length: 2 }
    ]);
  });
});
