import { describe, expect, it, vi } from "vitest";
import type { WasmCall, WasmCallResult } from "../wasm/client";
import {
  SANDBOX_FILE_GUIDANCE_BYTES,
  SANDBOX_TOTAL_GUIDANCE_BYTES,
  applyInSandbox,
  productFileName,
  productFileNames,
  resetSandbox,
  saveProducts,
  stageInputs,
  summarizeOutcome,
  undoInSandbox,
  webApplySupport,
  webSandboxFacts
} from "./applyChannel";
import type { PlanDocument } from "../ipc/types";
import type { PickedFiles } from "./planChannel";

const plan = {
  schemaVersion: 1,
  planId: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
  kind: "f03",
  createdAt: "2026-11-22T00:00:00Z",
  params: {},
  entries: [],
  warnings: [],
  summary: { total: 0, willProcess: 0, skipped: 0, conflicts: 0 }
} as unknown as PlanDocument;

function picked(files: File[]): PickedFiles {
  return { files };
}

function file(name: string, content: string): File {
  return new File([content], name, { type: "text/plain" });
}

/** 目录选择给的 File：`webkitRelativePath` 是相对路径的唯一来源。 */
function dirFile(relative: string, content: string): File {
  const name = relative.slice(relative.lastIndexOf("/") + 1);
  const picked = new File([content], name, { type: "text/plain" });
  Object.defineProperty(picked, "webkitRelativePath", { value: relative });
  return picked;
}

describe("webApplySupport", () => {
  it("23 个视图接入执行：分片/合并族 + 改写三兄弟 + CSV 行列变换 + 归档族 + 改名族 + 目录清单", () => {
    const wired = [
      "f01", "f02", "f03", "f04", "f05", "f06",
      "f07", "f08", "f09",
      "c01", "c02", "c03",
      "c07", "c08", "c10",
      "c04", "f12", "f13", "f10",
      "c11", "c12", "c13", "c14"
    ];
    for (const view of wired) {
      expect(webApplySupport(view)).toEqual({ level: "native" });
    }
    expect(wired).toHaveLength(23);
  });

  it("计划侧不支持的视图，执行侧也不谎称能执行", () => {
    // F11/C09 的**计划**都出不来（`webPlanSupport` 拒绝），执行入口不会出现；
    // 这里钉住档位表不把它们算成 native——两处表各自表态，但不互相矛盾。
    for (const view of ["f11", "c09"]) {
      expect(webApplySupport(view).level).toBe("unsupported");
    }
  });

  it("未知视图按最保守的「未接线」，不猜成能执行", () => {
    expect(webApplySupport("zzz")).toEqual({
      level: "unsupported",
      gap: "not-wired"
    });
  });
});

describe("webSandboxFacts", () => {
  it("按文件大小给出总量、单文件最大值与是否超建议档", () => {
    const facts = webSandboxFacts([
      { size: 10 } as File,
      { size: 25 } as File
    ]);
    expect(facts).toEqual({
      fileCount: 2,
      totalBytes: 35,
      maxFileBytes: 25,
      aboveGuidance: false
    });
  });

  it("单文件或总量任一超 §13.10.2 的本地建议档即置位", () => {
    expect(
      webSandboxFacts([{ size: SANDBOX_FILE_GUIDANCE_BYTES + 1 } as File])
        .aboveGuidance
    ).toBe(true);
    expect(
      webSandboxFacts([
        { size: SANDBOX_TOTAL_GUIDANCE_BYTES / 2 + 1 } as File,
        { size: SANDBOX_TOTAL_GUIDANCE_BYTES / 2 + 1 } as File
      ]).aboveGuidance
    ).toBe(true);
    // 恰好等于建议档不算超档（阈值是上限，不是排他区间）。
    expect(
      webSandboxFacts([{ size: SANDBOX_FILE_GUIDANCE_BYTES } as File])
        .aboveGuidance
    ).toBe(false);
  });
});

describe("stageInputs", () => {
  it("清单与拼接字节一致，路径取计划里出现的裸文件名", async () => {
    const calls: WasmCall[] = [];
    const caller = vi.fn(async (request: WasmCall): Promise<WasmCallResult> => {
      calls.push(request);
      return { ok: true, result: "{\"staged\":2}" };
    });
    const facts = await stageInputs(caller, "f03", picked([
      file("报告.txt", "aaa\n"),
      file("notes.txt", "bbb\n")
    ]));
    expect(facts.totalBytes).toBe(8);

    const request = calls[0];
    expect(request.op).toBe("stage_inputs");
    if (request.op !== "stage_inputs") throw new Error("unreachable");
    expect(JSON.parse(request.payload)).toEqual([
      { path: "报告.txt", length: 4 },
      { path: "notes.txt", length: 4 }
    ]);
    expect(new TextDecoder().decode(request.bytes)).toBe("aaa\nbbb\n");
  });

  it("未接线的视图在调用 WASM 之前就拒绝", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({ ok: true, result: "{}" }));
    await expect(
      stageInputs(caller, "f11", picked([file("a.txt", "a")]))
    ).rejects.toMatchObject({ code: "E5001" });
    expect(caller).not.toHaveBeenCalled();
  });

  it("F10 只登记目录与元数据：两次调用、零字节进沙箱", async () => {
    const caller = vi.fn(async (_request: WasmCall): Promise<WasmCallResult> => ({
      ok: true,
      result: "{}"
    }));
    const facts = await stageInputs(caller, "f10", {
      roots: [dirFile("素材/a.txt", "abc"), dirFile("素材/sub/b.txt", "hello")]
    });

    // 两次调用：先建目录（预检要 stat 到），再交目录事实（端口读它）。
    expect(caller).toHaveBeenCalledTimes(2);
    const [dirs, listing] = caller.mock.calls.map(([request]) => request);
    expect(dirs.op).toBe("stage_dirs");
    expect(JSON.parse(String((dirs as { payload?: string }).payload))).toEqual([
      { path: "素材" }
    ]);
    expect(listing.op).toBe("stage_listing");
    const staged = JSON.parse(String((listing as { payload?: string }).payload));
    expect(staged).toHaveLength(1);
    expect(staged[0].root).toBe("素材");
    expect(staged[0].files.map((row: { path: string }) => row.path)).toEqual([
      "a.txt",
      "sub/b.txt"
    ]);
    expect(staged[0].files[1].size).toBe(5);

    // 清单只读元数据：没有字节，建议档按 0 算（不该按选中文件总大小吓用户）。
    expect(facts).toEqual({
      fileCount: 2,
      totalBytes: 0,
      maxFileBytes: 0,
      aboveGuidance: false
    });
  });

  it("F10 的普通多选明确拒绝，不去 stage 一份没有根的假清单", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({ ok: true, result: "{}" }));
    await expect(
      stageInputs(caller, "f10", picked([file("a.txt", "a")]))
    ).rejects.toMatchObject({ code: "E1001" });
    expect(caller).not.toHaveBeenCalled();
  });

  it("C02 登记的是裸文件名——与计划事实同口径，否则执行器找不到源", async () => {
    const caller = vi.fn(async (_request: WasmCall): Promise<WasmCallResult> => ({
      ok: true,
      result: "{}"
    }));
    await stageInputs(caller, "c02", picked([file("报告.txt", "ab")]));
    const request = caller.mock.calls[0][0];
    expect(request.op).toBe("stage_inputs");
    const payload = "payload" in request ? request.payload : undefined;
    expect(JSON.parse(payload as string)).toEqual([
      { path: "报告.txt", length: 2 }
    ]);
  });

  it("没选文件时按 E1001 拒绝（不把空沙箱当成功）", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({ ok: true, result: "{}" }));
    await expect(stageInputs(caller, "f03", {})).rejects.toMatchObject({
      code: "E1001"
    });
    expect(caller).not.toHaveBeenCalled();
  });

  it("C07 只登记源目录槽，键是「源根/相对路径」——清单不参与执行", async () => {
    const caller = vi.fn(async (_request: WasmCall): Promise<WasmCallResult> => ({
      ok: true,
      result: "{}"
    }));
    const facts = await stageInputs(caller, "c07", {
      manifest: [file("清单.csv", "year,docx\n")],
      sourceRoot: [
        dirFile("资料/sub/年度报告.docx", "ab"),
        dirFile("资料/b.txt", "cd")
      ]
    });
    // 建议档按执行期真正读的文件算（清单已在计划期读完，不进沙箱）。
    expect(facts.totalBytes).toBe(4);

    const request = caller.mock.calls[0][0];
    if (request.op !== "stage_inputs") throw new Error("unreachable");
    expect(JSON.parse(request.payload)).toEqual([
      { path: "资料/sub/年度报告.docx", length: 2 },
      { path: "资料/b.txt", length: 2 }
    ]);
    expect(new TextDecoder().decode(request.bytes)).toBe("abcd");
  });

  it("C08 的键同样带源根，子目录层级不丢", async () => {
    const caller = vi.fn(async (_request: WasmCall): Promise<WasmCallResult> => ({
      ok: true,
      result: "{}"
    }));
    await stageInputs(caller, "c08", {
      sourceRoot: [dirFile("src/sub/a.jpg", "img")]
    });
    const request = caller.mock.calls[0][0];
    if (request.op !== "stage_inputs") throw new Error("unreachable");
    expect(JSON.parse(request.payload)).toEqual([
      { path: "src/sub/a.jpg", length: 3 }
    ]);
  });

  it("目录视图拿到普通多选（无相对路径）时先拒绝，不拿裸文件名凑数", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({ ok: true, result: "{}" }));
    await expect(
      stageInputs(caller, "c08", { sourceRoot: [file("a.jpg", "img")] })
    ).rejects.toMatchObject({ code: "E1001" });
    expect(caller).not.toHaveBeenCalled();
  });

  it("C10 解压登记的是包自己的文件名（计划里 archive 就是这个源）", async () => {
    const caller = vi.fn(async (_request: WasmCall): Promise<WasmCallResult> => ({
      ok: true,
      result: "{}"
    }));
    await stageInputs(caller, "c10", { archive: [file("包.zip", "PK")] });
    const request = caller.mock.calls[0][0];
    if (request.op !== "stage_inputs") throw new Error("unreachable");
    expect(JSON.parse(request.payload)).toEqual([{ path: "包.zip", length: 2 }]);
  });

  it("C10 压缩的源用相对路径——与 planC10Compress 的 sources 逐字对齐", async () => {
    const caller = vi.fn(async (_request: WasmCall): Promise<WasmCallResult> => ({
      ok: true,
      result: "{}"
    }));
    await stageInputs(caller, "c10", {
      sources: [dirFile("素材/a.txt", "aa"), dirFile("素材/sub/b.txt", "bbb")]
    });
    const request = caller.mock.calls[0][0];
    if (request.op !== "stage_inputs") throw new Error("unreachable");
    // 键＝`pickedRelativePath`（相对所选目录内部，不含根名）——与计划里的 sources
    // 逐字一致；裸文件名会让执行器逐条报源缺失（计划写的是 `sub/b.txt`）。
    expect(JSON.parse(request.payload)).toEqual([
      { path: "a.txt", length: 2 },
      { path: "sub/b.txt", length: 3 }
    ]);
  });

  it("C04/F12 登记整个目录：键＝「根名/相对路径」，子目录的子树一起进沙箱", async () => {
    const caller = vi.fn(async (_request: WasmCall): Promise<WasmCallResult> => ({
      ok: true,
      result: "{}"
    }));
    await stageInputs(caller, "f12", {
      dir: [dirFile("素材/a.txt", "A"), dirFile("素材/sub/b.txt", "BB")]
    });
    const request = caller.mock.calls[0][0];
    if (request.op !== "stage_inputs") throw new Error("unreachable");
    // 条目是 `素材/a.txt` 与 `素材/sub`（第一层），但目录改名会带走整棵子树，
    // 所以登记的是**所有选中的文件**；键与计划里的条目口径一致（根名 + 相对路径）。
    expect(JSON.parse(request.payload)).toEqual([
      { path: "素材/a.txt", length: 1 },
      { path: "素材/sub/b.txt", length: 2 }
    ]);
  });

  it("F13 与 F12 同一口径：dir 槽按「根名/相对路径」登记（含子树）", async () => {
    const caller = vi.fn(async (_request: WasmCall): Promise<WasmCallResult> => ({
      ok: true,
      result: "{}"
    }));
    await stageInputs(caller, "f13", {
      dir: [dirFile("素材/2026-a.txt", "A"), dirFile("素材/sub/b.txt", "BB")]
    });
    const request = caller.mock.calls[0][0];
    if (request.op !== "stage_inputs") throw new Error("unreachable");
    expect(JSON.parse(request.payload)).toEqual([
      { path: "素材/2026-a.txt", length: 1 },
      { path: "素材/sub/b.txt", length: 2 }
    ]);
  });
});

describe("applyInSandbox / undoInSandbox", () => {
  it("把产物路径与 Worker 带回的 Blob 一一对应", async () => {
    const blob = new Blob(["aaa\n"]);
    const caller = vi.fn(async (_request: WasmCall): Promise<WasmCallResult> => ({
      ok: true,
      result: JSON.stringify({
        outcome: {
          state: "completed",
          report: { entries: [{ status: "ok" }, { status: "ok" }] }
        },
        products: [{ path: "/out/a.1.txt", size: 4 }]
      }),
      products: [{ path: "/out/a.1.txt", size: 4, blob }]
    }));

    const result = await applyInSandbox(caller, plan, { now: "2026-11-22T00:00:00Z" });
    expect(result.state).toBe("completed");
    expect(result.products).toHaveLength(1);
    expect(result.products[0].blob).toBe(blob);
    expect(result.products[0].size).toBe(4);
    expect(summarizeOutcome(result.outcome)).toEqual({
      ok: 2,
      failed: 0,
      skipped: 0,
      total: 2
    });

    const request = caller.mock.calls[0][0] as WasmCall;
    expect(request.op).toBe("apply_plan");
    if (request.op !== "apply_plan") throw new Error("unreachable");
    const payload = JSON.parse(request.payload);
    expect(payload.options).toEqual({
      onConflict: "skip",
      overwriteConfirmed: false
    });
    expect(payload.now).toBe("2026-11-22T00:00:00Z");
  });

  it("解密密码随请求进沙箱；不填就不出现在请求里", async () => {
    const caller = vi.fn(async (_request: WasmCall): Promise<WasmCallResult> => ({
      ok: true,
      result: JSON.stringify({ outcome: { state: "completed" }, products: [] })
    }));
    await applyInSandbox(caller, plan, { password: "pw123" });
    const withPassword = caller.mock.calls[0][0] as WasmCall;
    if (withPassword.op !== "apply_plan") throw new Error("unreachable");
    expect(JSON.parse(withPassword.payload).password).toBe("pw123");

    caller.mockClear();
    await applyInSandbox(caller, plan);
    const without = caller.mock.calls[0][0] as WasmCall;
    if (without.op !== "apply_plan") throw new Error("unreachable");
    // 计划里只有 `passwordProvided` 这个事实；密码不填就不出现在请求里。
    expect("password" in JSON.parse(without.payload)).toBe(false);
  });

  it("内核报错原样上抛（码位不在这里改写）", async () => {    const caller = vi.fn(async (): Promise<WasmCallResult> => ({
      ok: false,
      reason: "kernel-error",
      code: "E2001"
    }));
    await expect(applyInSandbox(caller, plan)).rejects.toMatchObject({
      code: "E2001"
    });
  });

  it("WASM 不可用报 E5002（不是 E9001）", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({
      ok: false,
      reason: "wasm-unavailable"
    }));
    await expect(applyInSandbox(caller, plan)).rejects.toMatchObject({
      code: "E5002"
    });
  });

  it("撤销回传内核计数；失败条目按数量上报", async () => {
    const caller = vi.fn(async (_request: WasmCall): Promise<WasmCallResult> => ({
      ok: true,
      result: JSON.stringify({
        reverted: 2,
        failed: [{ entryId: "e3", error: { code: "E2001" } }]
      })
    }));
    await expect(undoInSandbox(caller, plan.planId)).resolves.toEqual({
      reverted: 2,
      failed: 1
    });
    const request = caller.mock.calls[0][0] as WasmCall;
    expect(request).toEqual({ op: "undo_plan", payload: plan.planId });
  });

  it("清空沙箱走独立入口", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({
      ok: true,
      result: "{\"reset\":true}"
    }));
    await resetSandbox(caller);
    expect(caller).toHaveBeenCalledWith({ op: "reset_sandbox" });
  });
});

describe("saveProducts", () => {
  it("只取文件名，重名按序号前缀区分", () => {
    const saved: string[] = [];
    const count = saveProducts(
      [
        { path: "/out/a/part.txt", size: 1, blob: new Blob(["1"]) },
        { path: "/out/b/part.txt", size: 1, blob: new Blob(["2"]) },
        { path: "notes.1.txt", size: 1, blob: new Blob(["3"]) }
      ],
      (name) => saved.push(name)
    );
    expect(count).toBe(3);
    expect(saved).toEqual(["part.txt", "2-part.txt", "notes.1.txt"]);
    expect(productFileName("/a/b/c.txt")).toBe("c.txt");
  });

  it("产物本身叫「2-x.txt」时不能与去重前缀撞名", () => {
    // 旧实现按「出现次数」算序号：`x.txt`、`x.txt`、`2-x.txt` 会得到
    // `["x.txt", "2-x.txt", "2-x.txt"]`——第三条把第二条覆盖掉，用户静默丢一个产物。
    const names = productFileNames([
      { path: "/out/a/x.txt", size: 1, blob: new Blob(["1"]) },
      { path: "/out/b/x.txt", size: 1, blob: new Blob(["2"]) },
      { path: "/out/c/2-x.txt", size: 1, blob: new Blob(["3"]) }
    ]);
    expect(new Set(names).size).toBe(3);
  });
});
