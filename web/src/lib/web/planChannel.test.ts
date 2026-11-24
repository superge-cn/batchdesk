import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";

import { describe, expect, it, vi } from "vitest";

import type { WasmCall, WasmCallResult } from "../wasm/client";
import { PLAN_OPS, KernelError } from "../wasm/contract";
import {
  DEFAULT_SAMPLE_WINDOW,
  WebPlanUnavailable,
  WebPlanUnsupported,
  planC02WithFiles,
  planWithFiles,
  planWithPickedInputs,
  webPlanSupport,
  type WebNativeView,
  type WebUnsupportedView
} from "./planChannel";

/**
 * Web 计划通道的契约。
 *
 * A3-1 建的纵切钉住四件事（①事实形状可被线协议反序列化 ②计划原样交回
 * ③WASM 缺失＝E5002 ④无文件在调用前拒绝）；A3-2 横向铺开后追加三条：
 * ⑤**每个视图 → 正确的 `plan_*` op 与字段映射**（F04/F06 的 target 交叉映射
 * 是易错点，写反不报错只出错计划）⑥事实不全的视图**在调用前**拒绝（E5001），
 * 绝不「先算一份错的再说」⑦25 个导出**逐个表态**：要么 native、要么 unsupported。
 */

function fakeFile(name: string, text: string): File {
  const bytes = new TextEncoder().encode(text);
  return new File([bytes as unknown as BlobPart], name, { type: "text/plain" });
}

/**
 * 目录选择得到的 `File`：`webkitRelativePath` 只有目录选择才有（形状是
 * `目录名/子路径`）。jsdom 的 `File` 不带这个字段，这里按浏览器形态补上。
 */
function dirFile(relative: string, text: string): File {
  const bytes = new TextEncoder().encode(text);
  const file = new File(
    [bytes as unknown as BlobPart],
    relative.split("/").pop() ?? relative,
    { type: "text/plain" }
  );
  Object.defineProperty(file, "webkitRelativePath", { value: relative });
  return file;
}

// C10 必须真的读中央目录：mock 出来的条目只能证明字段被搬运，证明不了读得对。
declare const process: { cwd(): string };

/** 真实语料路径（从 web/ 或仓库根跑都能找到）。 */
function fixturesDir(): string {
  const candidates = [
    resolve(process.cwd(), "..", "fixtures"),
    resolve(process.cwd(), "fixtures")
  ];
  const found = candidates.find((dir) => existsSync(dir));
  if (found === undefined) {
    throw new Error("fixtures directory not found; run vitest from web/");
  }
  return found;
}

function fixtureFile(relative: string): File {
  const bytes = readFileSync(join(fixturesDir(), relative));
  return new File([new Uint8Array(bytes)], relative.split("/").pop() ?? "fixture.zip");
}

function planDocument(kind = "c02"): string {
  return JSON.stringify({
    planId: "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8",
    kind,
    params: {},
    entries: [{ entryId: "e1", source: "a.txt", action: "edit-content" }],
    stats: { total: 1, conflict: 0, warning: 0, blocked: 0 },
    warnings: []
  });
}

interface Captured {
  op: string;
  payload: Record<string, unknown>;
}

/** 跑一次 `planWithFiles` 并抓回线协议；`ok` 时返回计划。 */
async function capture(
  view: string,
  form: Record<string, string>,
  files: File[]
): Promise<Captured> {
  const calls: WasmCall[] = [];
  const caller = async (request: WasmCall): Promise<WasmCallResult> => {
    calls.push(request);
    return { ok: true, result: planDocument(view) };
  };
  await planWithFiles(caller, view, files, form);
  expect(calls).toHaveLength(1);
  const request = calls[0] as { op: string; payload: string };
  return { op: request.op, payload: JSON.parse(request.payload) };
}

const CSV = "id,name\n1,a\n2,b\n";
const TEXT = "a\r\nb\r\n";

describe("Web 计划通道（C02 纵切）", () => {
  it("把 File 采成 plan_c02 的线协议事实", async () => {
    const calls: WasmCall[] = [];
    const caller = async (request: WasmCall): Promise<WasmCallResult> => {
      calls.push(request);
      return { ok: true, result: planDocument() };
    };

    await planC02WithFiles(
      caller,
      [fakeFile("a.txt", "a\r\nb"), fakeFile("b.log", "c\nd")],
      { target: "lf" }
    );

    expect(calls).toHaveLength(1);
    const request = calls[0];
    expect(request.op).toBe("plan_c02");
    const payload = JSON.parse((request as { payload: string }).payload);
    expect(payload.target).toBe("lf");
    // `structure` 未传时不出现——缺省由内核的 `default_structure` 决定（auto），
    // 前端不替内核写一份默认值。
    expect("structure" in payload).toBe(false);
    expect(payload.files).toHaveLength(2);
    expect(payload.files[0]).toMatchObject({
      path: "a.txt",
      canonicalPath: null,
      fileSize: 4
    });
    expect(payload.files[0].sample).toEqual([97, 13, 10, 98]);
    expect(payload.files[0].data).toEqual([97, 13, 10, 98]);
  });

  it("采样窗口大于文件时取全长，缺省窗口与功能页同口径", async () => {
    const calls: WasmCall[] = [];
    const caller = async (request: WasmCall): Promise<WasmCallResult> => {
      calls.push(request);
      return { ok: true, result: planDocument() };
    };
    const file = fakeFile("small.txt", "hi");
    await planC02WithFiles(caller, [file], { target: "crlf", sampleWindow: 0 });
    const payload = JSON.parse((calls[0] as { payload: string }).payload);
    expect(payload.files[0].sample).toEqual([104, 105]);
    expect(DEFAULT_SAMPLE_WINDOW).toBe(4096);
  });

  it("成功路径原样交回内核计划（不在本层改写 stats/entries）", async () => {
    const caller = async (): Promise<WasmCallResult> => ({
      ok: true,
      result: planDocument()
    });
    const plan = await planC02WithFiles(caller, [fakeFile("a.txt", "x")], {
      target: "lf"
    });
    expect(plan.kind).toBe("c02");
    expect(plan.entries).toHaveLength(1);
    expect(plan.stats.blocked).toBe(0);
  });

  it("WASM 不可用报 E5002，便于调用方降级", async () => {
    const caller = async (): Promise<WasmCallResult> => ({
      ok: false,
      reason: "wasm-unavailable"
    });
    await expect(
      planC02WithFiles(caller, [fakeFile("a.txt", "x")], { target: "lf" })
    ).rejects.toBeInstanceOf(WebPlanUnavailable);
    await expect(
      planC02WithFiles(caller, [fakeFile("a.txt", "x")], { target: "lf" })
    ).rejects.toMatchObject({ code: "E5002" });
  });

  it("内核错误码原样上抛（界面文案仍由码位映射决定）", async () => {
    const caller = async (): Promise<WasmCallResult> => ({
      ok: false,
      reason: "kernel-error",
      code: "E3001"
    });
    await expect(
      planC02WithFiles(caller, [fakeFile("a.txt", "x")], { target: "lf" })
    ).rejects.toBeInstanceOf(KernelError);
    await expect(
      planC02WithFiles(caller, [fakeFile("a.txt", "x")], { target: "lf" })
    ).rejects.toMatchObject({ code: "E3001" });
  });

  it("没有文件时在调用前拒绝（不产空壳计划）", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({
      ok: true,
      result: planDocument()
    }));
    await expect(
      planC02WithFiles(caller, [], { target: "lf" })
    ).rejects.toMatchObject({ code: "E1001" });
    expect(caller).not.toHaveBeenCalled();
  });
});

describe("Web 计划通道（A3-2 横向铺开：每个视图的 op 与字段）", () => {
  it("C01 转码：target + sampleWindow + 头部样本", async () => {
    const { op, payload } = await capture(
      "c01",
      { target: "utf-8", sampleWindow: "8192" },
      [fakeFile("a.txt", TEXT)]
    );
    expect(op).toBe("plan_c01");
    expect(payload.target).toBe("utf-8");
    expect(payload.sampleWindow).toBe(8192);
    expect(payload.files).toHaveLength(1);
    expect((payload.files as Array<{ sample: number[] }>)[0].sample).toEqual(
      Array.from(new TextEncoder().encode(TEXT))
    );
  });

  it("C03 查找替换：mode/find/replacement 原样，files 带 data 与只读事实", async () => {
    const { op, payload } = await capture(
      "c03",
      { mode: "regex", find: "a+", replacement: "A" },
      [fakeFile("a.txt", TEXT)]
    );
    expect(op).toBe("plan_c03");
    expect(payload.mode).toBe("regex");
    expect(payload.find).toBe("a+");
    expect(payload.replacement).toBe("A");
    expect(payload.sampleBytes).toBe(DEFAULT_SAMPLE_WINDOW);
    const file = (payload.files as Array<Record<string, unknown>>)[0];
    expect(file).toMatchObject({ path: "a.txt", readonly: false, locked: false });
    expect(file.data).toHaveLength(new TextEncoder().encode(TEXT).length);
  });

  it.each([
    ["f01", "targetBytes", "8", "plan_f01"],
    ["f02", "linesPerPart", "100", "plan_f02"],
    ["f03", "parts", "4", "plan_f03"]
  ])("%s 分片：%s 落到同名线协议字段", async (view, field, value, op) => {
    const { op: actual, payload } = await capture(
      view,
      { [field]: value, outputDir: "/out" },
      [fakeFile("a.txt", TEXT)]
    );
    expect(actual).toBe(op);
    expect(payload[field]).toBe(Number(value));
    expect(payload.outputDir).toBe("/out");
    const file = (payload.files as Array<Record<string, unknown>>)[0];
    expect(file).toMatchObject({ path: "a.txt", canonicalPath: null, groupId: "" });
  });

  it("F04 合并：表单 output → 线协议 target，表单 target → targetEncoding（交叉映射）", async () => {
    const { op, payload } = await capture(
      "f04",
      { output: "/out/merged.txt", target: "utf-8" },
      [fakeFile("a.txt", "x"), fakeFile("b.txt", "y")]
    );
    expect(op).toBe("plan_f04");
    // 写反了不会报错，只会拿 /out/merged.txt 当编码、拿 utf-8 当输出名——
    // 所以这两条必须分别断言，不能只断言「字段存在」。
    expect(payload.target).toBe("/out/merged.txt");
    expect(payload.targetEncoding).toBe("utf-8");
    expect(payload.files).toHaveLength(2);
    const file = (payload.files as Array<Record<string, unknown>>)[0];
    expect(file.fullHash).toBeNull();
    expect(typeof file.mtimeNanos).toBe("number");
  });

  it("F05 CSV 分片：rows 优先成 basis.rows，keepHeader 跟随表头声明", async () => {
    const { op, payload } = await capture(
      "f05",
      { rows: "100", delimiter: ",", outputDir: "/out", header: "有" },
      [fakeFile("a.csv", CSV)]
    );
    expect(op).toBe("plan_f05");
    expect(payload.basis).toEqual({ rows: 100 });
    expect(payload.keepHeader).toBe(true);
    expect(payload.delimiter).toBe(",");
    const file = (payload.files as Array<Record<string, unknown>>)[0];
    expect(file.hasHeader).toBe(true);
    // Web 侧无目标文件概念：恒 false（线协议缺省是 true，必须显式传）。
    expect(file.targetOccupied).toBe(false);
  });

  it("F06 CSV 合并：output→target、target→targetEncoding，来源列开关由字段是否存在决定", async () => {
    const { op, payload } = await capture(
      "f06",
      {
        output: "/out/all.csv",
        target: "gbk",
        delimiter: ";",
        sourceColumnName: "origin",
        strict: "yes",
        header: "无"
      },
      [fakeFile("a.csv", CSV)]
    );
    expect(op).toBe("plan_f06");
    expect(payload.target).toBe("/out/all.csv");
    expect(payload.targetEncoding).toBe("gbk");
    expect(payload.outputDelimiter).toBe(";");
    expect(payload.addSourceColumn).toBe(true);
    expect(payload.sourceColumnName).toBe("origin");
    expect(payload.strict).toBe(true);
    expect((payload.files as Array<{ hasHeader: boolean }>)[0].hasHeader).toBe(false);
  });

  it("F07 列提取：名字列与序号列合并成 selections", async () => {
    const { op, payload } = await capture(
      "f07",
      { columns: "name", columnIndexes: "3", target: "utf-8" },
      [fakeFile("a.csv", CSV)]
    );
    expect(op).toBe("plan_f07");
    expect(payload.selections).toEqual([{ name: "name" }, { index: 3 }]);
  });

  it("F08 行提取：window 只认 last，其余按 first；count 缺省 0", async () => {
    const { op, payload } = await capture(
      "f08",
      { window: "last", count: "5", target: "utf-8" },
      [fakeFile("a.csv", CSV)]
    );
    expect(op).toBe("plan_f08");
    expect(payload.window).toBe("last");
    expect(payload.count).toBe(5);

    const fallback = await capture("f08", { target: "utf-8" }, [
      fakeFile("a.csv", CSV)
    ]);
    expect(fallback.payload.window).toBe("first");
    expect(fallback.payload.count).toBe(0);
  });

  it("F09 区间读取：startRow/endRow 原样（0 也照传，交给内核判）", async () => {
    const { op, payload } = await capture(
      "f09",
      { startRow: "2", endRow: "5", target: "utf-8" },
      [fakeFile("a.csv", CSV)]
    );
    expect(op).toBe("plan_f09");
    expect(payload.startRow).toBe(2);
    expect(payload.endRow).toBe(5);
  });

  it("C11 去重：key 同时收名字列与序号列", async () => {
    const { op, payload } = await capture(
      "c11",
      { keyColumns: "id,user", keyColumnIndexes: "1", target: "utf-8" },
      [fakeFile("a.csv", CSV)]
    );
    expect(op).toBe("plan_c11");
    expect(payload.key).toEqual([{ name: "id" }, { name: "user" }, { index: 1 }]);
  });

  it("C12 过滤：filterMode=drop 才反转 modeKeep，条件列二选一", async () => {
    const keep = await capture(
      "c12",
      { column: "amount", op: "gt", value: "100", target: "utf-8" },
      [fakeFile("a.csv", CSV)]
    );
    expect(keep.op).toBe("plan_c12");
    expect(keep.payload.modeKeep).toBe(true);
    expect(keep.payload.conditionColumn).toEqual({ name: "amount" });
    expect(keep.payload.conditionOp).toBe("gt");

    const drop = await capture(
      "c12",
      { columnIndex: "3", filterMode: "drop", value: "1", target: "utf-8" },
      [fakeFile("a.csv", CSV)]
    );
    expect(drop.payload.modeKeep).toBe(false);
    expect(drop.payload.conditionColumn).toEqual({ index: 3 });
  });

  it("C13 排序：key 列表 + order + 可选外排序分块", async () => {
    const { op, payload } = await capture(
      "c13",
      {
        sortColumns: "amount",
        order: "desc",
        externalChunkRows: "5000",
        target: "utf-8"
      },
      [fakeFile("a.csv", CSV)]
    );
    expect(op).toBe("plan_c13");
    expect(payload.keys).toEqual([{ name: "amount" }]);
    expect(payload.order).toBe("desc");
    expect(payload.externalChunkRows).toBe(5000);
  });

  it("C14 列拆分：填了拆分列即 mode=split，分隔符/正则一并带上", async () => {
    const { op, payload } = await capture(
      "c14",
      { splitColumn: "tags", splitSeparator: ";", target: "utf-8" },
      [fakeFile("a.csv", CSV)]
    );
    expect(op).toBe("plan_c14");
    expect(payload.mode).toBe("split");
    expect(payload.split).toEqual({
      source: { name: "tags" },
      separator: ";"
    });
  });

  it("C14 列合并：没填拆分列即 mode=merge，合并列走 mergePicks", async () => {
    const { op, payload } = await capture(
      "c14",
      { mergeColumns: "first,last", mergeJoinWith: "-", mergeName: "full", target: "utf-8" },
      [fakeFile("a.csv", CSV)]
    );
    expect(op).toBe("plan_c14");
    expect(payload.mode).toBe("merge");
    expect("split" in payload).toBe(false);
    expect(payload.mergePicks).toEqual([{ name: "first" }, { name: "last" }]);
    expect(payload.joiner).toBe("-");
    expect(payload.targetName).toBe("full");
  });
});

describe("Web 计划通道（C08 归档：目录选择才给得出相对路径）", () => {
  it("目录选择 → 剥掉源根、规则解析成结构体、未命中走 move:", async () => {
    const files = [
      dirFile("src/a.jpg", "a"),
      dirFile("src/sub/b.png", "bb"),
      dirFile("src/readme.txt", "txt")
    ];
    const { op, payload } = await capture(
      "c08",
      {
        targetRoot: "/dst",
        rules: "images|ext:jpg,png\nbig|size:1048576..",
        recursive: "yes",
        unclassifiedDir: "other"
      },
      files
    );

    expect(op).toBe("plan_c08");
    // 源根由所选目录名派生（表单里 sourceRoot 在浏览器形态是目录选择器，不落文本）。
    expect(payload.sourceRoot).toBe("src");
    expect(payload.targetRoot).toBe("/dst");
    expect(payload.unmatched).toBe("move:other");
    expect(payload.rules).toEqual([
      { subDir: "images", condition: { kind: "extensions", value: ["jpg", "png"] } },
      { subDir: "big", condition: { kind: "sizeRange", value: { min: 1048576 } } }
    ]);
    // 相对路径**不含**被选目录自身那一段（桌面 rel_path 相对于 source_root）。
    expect(
      (payload.files as Array<{ relPath: string }>).map((file) => file.relPath)
    ).toEqual(["a.jpg", "sub/b.png", "readme.txt"]);
    expect(payload.files).toMatchObject([
      { relPath: "a.jpg", size: 1, exists: true },
      { relPath: "sub/b.png", size: 2, exists: true },
      { relPath: "readme.txt", size: 3, exists: true }
    ]);
  });

  it("recursive=no 只归档源根下的文件（子目录项不进计划）", async () => {
    const files = [dirFile("src/a.jpg", "a"), dirFile("src/sub/b.png", "bb")];
    const { payload } = await capture(
      "c08",
      { targetRoot: "/dst", rules: "images|ext:jpg,png", recursive: "no" },
      files
    );
    expect(
      (payload.files as Array<{ relPath: string }>).map((file) => file.relPath)
    ).toEqual(["a.jpg"]);
    expect(payload.unmatched).toBe("keep");
  });

  it("普通多选（没有相对路径）在调用前拒绝：不拿扁平文件名冒充相对路径", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({
      ok: true,
      result: planDocument("c08")
    }));
    await expect(
      planWithFiles(caller, "c08", [fakeFile("a.jpg", "a")], {
        targetRoot: "/dst",
        rules: "images|ext:jpg"
      })
    ).rejects.toMatchObject({ code: "E1001" });
    expect(caller).not.toHaveBeenCalled();
  });

  it("选了多个目录时拒绝：相对路径会互相覆盖，计划看起来正常但归属是错的", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({
      ok: true,
      result: planDocument("c08")
    }));
    await expect(
      planWithFiles(
        caller,
        "c08",
        [dirFile("src/a.jpg", "a"), dirFile("other/b.jpg", "b")],
        { targetRoot: "/dst", rules: "images|ext:jpg", recursive: "yes" }
      )
    ).rejects.toMatchObject({ code: "E1001" });
    expect(caller).not.toHaveBeenCalled();
  });

  it("规则写错在调用前拒绝（E1002），并指出是第几行", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({
      ok: true,
      result: planDocument("c08")
    }));
    await expect(
      planWithFiles(caller, "c08", [dirFile("src/a.jpg", "a")], {
        targetRoot: "/dst",
        rules: "images|ext:jpg\nbroken-line"
      })
    ).rejects.toMatchObject({
      code: "E1002",
      message: expect.stringContaining("line 2") as unknown as string
    });
    expect(caller).not.toHaveBeenCalled();
  });
});

describe("Web 计划通道（C10 解压：条目来自真实中央目录）", () => {
  it("中央目录事实原样进线协议；限额必填；密码只传「会不会提供」", async () => {
    const archive = fixtureFile("corpus/Z01/out/gbk_name.zip");
    const { op, payload } = await capture(
      "c10",
      {
        outputDir: "/out",
        maxTotalBytes: "1048576",
        maxRatio: "100",
        password: "not-a-real-secret"
      },
      [archive]
    );

    expect(op).toBe("plan_c10");
    expect(payload.archive).toBe("gbk_name.zip");
    expect(payload.archiveSize).toBe(archive.size);
    // 与 `zipIndex.test.ts` 的 Z01 期望同一组事实：这里核的是「真的读了中央目录」。
    expect(payload.entries).toEqual([
      {
        index: 0,
        name: "中文资料.txt",
        kind: "file",
        size: 77,
        compressedSize: 77,
        gbkFallback: true,
        encrypted: false,
        supported: true
      }
    ]);
    expect(payload.maxTotalBytes).toBe(1048576);
    expect(payload.maxRatio).toBe(100);
    expect(payload.passwordProvided).toBe(true);
    // 密码本身不进请求（ADR-0074）：只传「执行期会不会提供」这一位。
    expect(JSON.stringify(payload)).not.toContain("not-a-real-secret");
    // 目标占用不预判（Web 无 OPFS 列举），字段缺省而不是空数组冒充「查过了」。
    expect("existingTargets" in payload).toBe(false);
  });

  it("maxTotalBytes／maxRatio 缺失时拒绝：不替用户放行未知解压量", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({
      ok: true,
      result: planDocument("c10")
    }));
    await expect(
      planWithFiles(
        caller,
        "c10",
        [fixtureFile("corpus/Z01/out/gbk_name.zip")],
        { outputDir: "/out", maxTotalBytes: "1048576" }
      )
    ).rejects.toMatchObject({ code: "E1001" });
    expect(caller).not.toHaveBeenCalled();
  });

  it("压缩侧（mode=compress）用选中的文件出真计划，不再按缺口拒绝", async () => {
    const calls: WasmCall[] = [];
    const caller = async (request: WasmCall): Promise<WasmCallResult> => {
      calls.push(request);
      return { ok: true, result: planDocument("c10") };
    };
    await planWithPickedInputs(
      caller,
      "c10",
      { sources: [fakeFile("a.txt", "AAAA"), fakeFile("b.txt", "BB")] },
      { mode: "compress", output: "/out/bundle.zip", compression: "store" }
    );
    expect(calls.map((call) => call.op)).toEqual(["plan_c10_compress"]);
    const payload = JSON.parse((calls[0] as { payload: string }).payload);
    // 路径取选中文件的名字（普通多选没有相对路径）；计数与字节数由浏览器统计——
    // 内核把「压缩几个文件、多少字节」原样写进计划理由，缺了会显示 0。
    expect(payload).toEqual({
      output: "/out/bundle.zip",
      sources: ["a.txt", "b.txt"],
      compression: "store",
      sourceFileCount: 2,
      sourceBytes: 6,
      targetExists: false
    });
  });

  it("压缩侧缺输出路径或没选文件时在调用前拒绝", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({
      ok: true,
      result: planDocument("c10")
    }));
    const sources = { sources: [fakeFile("a.txt", "AAAA")] };
    await expect(
      planWithPickedInputs(caller, "c10", sources, { mode: "compress" })
    ).rejects.toMatchObject({ code: "E1001" });
    await expect(
      planWithPickedInputs(caller, "c10", {}, { mode: "compress", output: "/out/a.zip" })
    ).rejects.toMatchObject({ code: "E1001" });
    expect(caller).not.toHaveBeenCalled();
  });

  it("解压侧不受 sources 槽影响：两份输入各归各槽", async () => {
    // `mode` 留空＝解压（与桌面缺省一致）。若 planner 把两份槽摊平，这里会看到
    // 2 个文件并报「一次只能解一个包」——那正是分槽要避免的。
    const calls: WasmCall[] = [];
    const caller = async (request: WasmCall): Promise<WasmCallResult> => {
      calls.push(request);
      return { ok: true, result: planDocument("c10") };
    };
    await planWithPickedInputs(
      caller,
      "c10",
      {
        archive: [fixtureFile("corpus/Z01/out/gbk_name.zip")],
        sources: [fakeFile("unrelated.txt", "x")]
      },
      { outputDir: "/out", maxTotalBytes: "1048576", maxRatio: "100" }
    );
    expect(calls.map((call) => call.op)).toEqual(["plan_c10"]);
    const payload = JSON.parse((calls[0] as { payload: string }).payload);
    expect(payload.archive).toBe("gbk_name.zip");
    expect(payload.entries).toHaveLength(1);
  });
});

// ── C07：清单与源目录是两份独立输入 ─────────────────────────────────────────

/**
 * C07 走**两次** WASM 调用：先解析清单字节（`parse_c07_manifest`），再出计划
 * （`plan_c07`）。这里只在解析结果上做注入，其余原样抓回来比对。
 */
async function captureC07(
  picked: Record<string, File[]>,
  form: Record<string, string>,
  rows: Array<{ name: string; subDir: string }> = [
    { name: "a.jpg", subDir: "photos" },
    { name: "missing.jpg", subDir: "" }
  ]
) {
  const calls: WasmCall[] = [];
  const caller = async (request: WasmCall): Promise<WasmCallResult> => {
    calls.push(request);
    if (request.op === "parse_c07_manifest") {
      return { ok: true, result: JSON.stringify(rows) };
    }
    return { ok: true, result: planDocument("c07") };
  };
  const plan = await planWithPickedInputs(caller, "c07", picked, form);
  return { calls, plan };
}

describe("Web 计划通道（C07 清单同步：两份独立输入）", () => {
  /** 「年度,归」加换行的 GBK 字节：含非 ASCII，正是必须先检测编码的形态。 */
  const GBK_MANIFEST = new Uint8Array([
    0xc4, 0xea, 0xb6, 0xc8, 0x2c, 0xb9, 0xe9, 0x0a
  ]);

  function manifestFile(bytes: Uint8Array = GBK_MANIFEST): File {
    return new File([bytes as unknown as BlobPart], "manifest.csv", {
      type: "text/csv"
    });
  }

  it("清单原始字节原样交给内核解析，计划用解析出的行与源目录事实", async () => {
    const { calls } = await captureC07(
      {
        manifest: [manifestFile()],
        sourceRoot: [dirFile("src/a.jpg", "AAAA"), dirFile("src/sub/b.jpg", "B")]
      },
      {
        targetRoot: "/dst",
        operation: "copy",
        delimiter: ",",
        source: "gb18030"
      }
    );

    expect(calls.map((call) => call.op)).toEqual([
      "parse_c07_manifest",
      "plan_c07"
    ]);

    const parse = calls[0] as { payload: string; bytes: Uint8Array };
    // 逐字节相等：中间若过一次 `File.text()`，GBK 会先被按 UTF-8 解成替换字符，
    // 内核再怎么检测也救不回来——这正是「必须走字节」的理由。
    expect(Array.from(parse.bytes)).toEqual(Array.from(GBK_MANIFEST));
    expect(JSON.parse(parse.payload)).toEqual({
      delimiter: ",",
      source: "gb18030"
    });

    const plan = calls[1] as { payload: string };
    const payload = JSON.parse(plan.payload);
    expect(payload.rows).toEqual([
      { name: "a.jpg", subDir: "photos" },
      { name: "missing.jpg", subDir: "" }
    ]);
    // 事实按**相对路径**匹配：a.jpg 在源目录根下 → 存在且大小 4；
    // 清单点名而目录里没有 → 不存在（交给 missing 策略，不在这里替内核决定）。
    expect(payload.facts).toEqual([
      { exists: true, size: 4 },
      { exists: false, size: 0 }
    ]);
    expect(payload.operation).toBe("copy");
    expect(payload.missing).toBe("skip");
    // 计划的源根缺省用选中目录名（浏览器没有真实绝对路径）。
    expect(payload.sourceRoot).toBe("src");
    expect(payload.targetRoot).toBe("/dst");
  });

  it("匹配是精确相对路径，不做文件名模糊匹配", async () => {
    // 清单写 `a.jpg`，目录里只有 `src/sub/a.jpg`：桌面按 source_root/a.jpg 找不到，
    // Web 也必须判为不存在——否则会把子目录里的同名文件当成根目录那个。
    const { calls } = await captureC07(
      {
        manifest: [manifestFile()],
        sourceRoot: [dirFile("src/sub/a.jpg", "AAAA")]
      },
      { targetRoot: "/dst" },
      [{ name: "a.jpg", subDir: "" }]
    );
    const payload = JSON.parse((calls[1] as { payload: string }).payload);
    expect(payload.facts).toEqual([{ exists: false, size: 0 }]);

    // 反过来：清单写 `sub/a.jpg` 时能命中同一文件。
    const second = await captureC07(
      {
        manifest: [manifestFile()],
        sourceRoot: [dirFile("src/sub/a.jpg", "AAAA")]
      },
      { targetRoot: "/dst" },
      [{ name: "sub/a.jpg", subDir: "" }]
    );
    const payload2 = JSON.parse((second.calls[1] as { payload: string }).payload);
    expect(payload2.facts).toEqual([{ exists: true, size: 4 }]);
  });

  it("缺清单、源目录或目标根时在调用前拒绝", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({
      ok: true,
      result: planDocument("c07")
    }));
    await expect(
      planWithPickedInputs(
        caller,
        "c07",
        { sourceRoot: [dirFile("src/a.jpg", "x")] },
        { targetRoot: "/dst" }
      )
    ).rejects.toMatchObject({ code: "E1001" });
    await expect(
      planWithPickedInputs(
        caller,
        "c07",
        { manifest: [manifestFile()] },
        { targetRoot: "/dst" }
      )
    ).rejects.toMatchObject({ code: "E1001" });
    // 目标根必填：不替用户补默认值（产物归宿不能由前端决定）。
    await expect(
      planWithPickedInputs(
        caller,
        "c07",
        { manifest: [manifestFile()], sourceRoot: [dirFile("src/a.jpg", "x")] },
        {}
      )
    ).rejects.toMatchObject({ code: "E1001" });
    expect(caller).not.toHaveBeenCalled();
  });

  it("一次只认一个源目录（两个根会让相对路径互相覆盖）", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({
      ok: true,
      result: planDocument("c07")
    }));
    await expect(
      planWithPickedInputs(
        caller,
        "c07",
        {
          manifest: [manifestFile()],
          sourceRoot: [dirFile("src/a.jpg", "x"), dirFile("other/b.jpg", "y")]
        },
        { targetRoot: "/dst" }
      )
    ).rejects.toMatchObject({ code: "E1001" });
    expect(caller).not.toHaveBeenCalled();
  });

  it("清单解析失败（如 E3001 编码无法判定）原样上抛，不退回假计划", async () => {
    const calls: WasmCall[] = [];
    const caller = async (request: WasmCall): Promise<WasmCallResult> => {
      calls.push(request);
      return { ok: false, reason: "kernel-error", code: "E3001" };
    };
    await expect(
      planWithPickedInputs(
        caller,
        "c07",
        { manifest: [manifestFile()], sourceRoot: [dirFile("src/a.jpg", "x")] },
        { targetRoot: "/dst" }
      )
    ).rejects.toMatchObject({ code: "E3001" });
    // 解析都失败就不该去算计划：一次调用都不该发到 plan_c07。
    expect(calls.map((call) => call.op)).toEqual(["parse_c07_manifest"]);
  });
});

describe("C04/F12：目录第一层条目（ADR-0113）", () => {
  it("C04 条目取第一层：根层文件 + 子目录名，深层文件不单独成条目", async () => {
    const calls: WasmCall[] = [];
    const caller = async (request: WasmCall): Promise<WasmCallResult> => {
      calls.push(request);
      return { ok: true, result: planDocument("c04") };
    };
    await planWithPickedInputs(
      caller,
      "c04",
      {
        dir: [
          dirFile("素材/a.txt", "A"),
          dirFile("素材/sub/b.txt", "B"),
          dirFile("素材/sub/deep/c.txt", "C")
        ]
      },
      { find: "a", replace: "z", scope: "stem" }
    );
    expect(calls.map((call) => call.op)).toEqual(["plan_c04"]);
    const payload = JSON.parse((calls[0] as { payload: string }).payload);
    // `sub` 只算一条（b.txt 与 deep/c.txt 都贡献第一段）；深层文件不单独成条目——
    // 桌面的 C04/F12 是 `read_dir` 单层，递归会让两端结果不同。
    expect(payload.items).toEqual([{ path: "素材/a.txt" }, { path: "素材/sub" }]);
    expect(payload.rule).toEqual({
      mode: "plain",
      find: "a",
      replace: "z",
      scope: "stem"
    });
  });

  it("F12 的 planId 由调用方给（26 字符 ULID），scope 翻成 PascalCase", async () => {
    const calls: WasmCall[] = [];
    const caller = async (request: WasmCall): Promise<WasmCallResult> => {
      calls.push(request);
      return { ok: true, result: planDocument("f12") };
    };
    await planWithPickedInputs(
      caller,
      "f12",
      { dir: [dirFile("素材/a.txt", "A")] },
      { prefix: "new_" }
    );
    expect(calls.map((call) => call.op)).toEqual(["plan_f12"]);
    const payload = JSON.parse((calls[0] as { payload: string }).payload);
    // ULID：首字符 ≤ 7（时间前缀只在 128 bit 的高 3 bit 里），字符集不含 I/L/O/U。
    expect(payload.planId).toMatch(/^[0-7][0-9A-HJKMNP-TV-Z]{25}$/);
    // 界面留空 → `full`；`PlanF12Rule.scope` 直接反序列化成枚举，必须是 PascalCase。
    expect(payload.rule).toEqual({ prefix: "new_", replace: "", scope: "Full" });
    // 浏览器没有目标文件系统：目标占用事实只能是空（沙箱执行前本来就是空的）。
    expect(payload.existingTargets).toEqual([]);
    expect(payload.longPathSupported).toBe(true);
  });

  it("普通多选（无相对路径）在调 WASM 前拒绝，不拿裸文件名凑数", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({
      ok: true,
      result: planDocument("f12")
    }));
    await expect(
      planWithPickedInputs(caller, "f12", { dir: [fakeFile("a.txt", "A")] }, {})
    ).rejects.toMatchObject({ code: "E1001" });
    expect(caller).not.toHaveBeenCalled();
  });

  it("两个不同目录一起选时拒绝（一次只认一个源根）", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({
      ok: true,
      result: planDocument("c04")
    }));
    await expect(
      planWithPickedInputs(
        caller,
        "c04",
        { dir: [dirFile("甲/a.txt", "A"), dirFile("乙/b.txt", "B")] },
        { find: "a" }
      )
    ).rejects.toMatchObject({ code: "E1001" });
    expect(caller).not.toHaveBeenCalled();
  });

  it("作用域非法值在调 WASM 前拒绝（不把错值送进内核）", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({
      ok: true,
      result: planDocument("c04")
    }));
    await expect(
      planWithPickedInputs(
        caller,
        "c04",
        { dir: [dirFile("素材/a.txt", "A")] },
        { find: "a", scope: "bogus" }
      )
    ).rejects.toMatchObject({ code: "E1002" });
    expect(caller).not.toHaveBeenCalled();
  });
});

describe("Web 计划通道（能力矩阵：25 个入口逐个表态）", () => {
  it("每个 plan_* 导出都映射到一个明确的视图档位", () => {
    // `PLAN_OPS` 是导出面的唯一清单；漏一个视图在这里就会红。
    const views = PLAN_OPS.map((op) => op.replace(/^plan_/, ""));
    expect(views).toHaveLength(25);
    const native = views.filter((view) => webPlanSupport(view).level === "native");
    const unsupported = views.filter(
      (view) => webPlanSupport(view).level === "unsupported"
    );
    expect(native).toHaveLength(22);
    expect(unsupported).toHaveLength(3);
    // 每个不支持的视图都必须说明原因（否则界面只能给一句空话）。
    for (const view of unsupported) {
      expect(webPlanSupport(view).gap).toBeDefined();
    }
  });

  it.each<[WebUnsupportedView, string]>([
    ["f11", "directory-walk"],
    ["c09", "file-identity"]
  ])("%s 在浏览器里拒绝并给出原因（%s）", async (view, gap) => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({
      ok: true,
      result: planDocument(view)
    }));
    await expect(
      planWithFiles(caller, view, [fakeFile("a.bin", "x")], {})
    ).rejects.toBeInstanceOf(WebPlanUnsupported);
    await expect(
      planWithFiles(caller, view, [fakeFile("a.bin", "x")], {})
    ).rejects.toMatchObject({ code: "E5001", gap });
    // 关键：**一次都没调 WASM**——不是「先算一份错的再报错」。
    expect(caller).not.toHaveBeenCalled();
  });

  it.each<[WebNativeView]>([
    ["c01"],
    ["c02"],
    ["c03"],
    ["f01"],
    ["f02"],
    ["f03"],
    ["f04"],
    ["f05"],
    ["f06"],
    ["f07"],
    ["f08"],
    ["f09"],
    ["c11"],
    ["c12"],
    ["c13"],
    ["c14"],
    ["c08"],
    ["c10"],
    ["c07"],
    ["c04"],
    ["f12"],
    ["f10"]
  ])("%s 声明为 native 就一定有实现（不会走到 E5001）", async (view) => {
    const caller = async (): Promise<WasmCallResult> => ({
      ok: true,
      result: planDocument(view)
    });
    // 参数不全时抛 E1002 也算「有实现」——只有 E5001 才说明档位与实现对不上。
    await planWithFiles(caller, view, [fakeFile("a.csv", CSV)], {}).catch(
      (error: unknown) => {
        expect((error as { code?: string }).code).not.toBe("E5001");
      }
    );
  });
});

describe("F10 目录清单（ADR-0114）", () => {
  it("按所选目录出计划：条目＝根名，target＝输出目录下的同名 CSV", async () => {
    let sent: unknown;
    const caller = async (request: WasmCall): Promise<WasmCallResult> => {
      sent = JSON.parse(String((request as { payload?: string }).payload ?? "{}"));
      return { ok: true, result: planDocument("f10") };
    };
    const files = [
      dirFile("素材/a.txt", "abc"),
      dirFile("素材/sub/b.txt", "hello"),
      dirFile("素材/sub/deep/c.txt", "x")
    ];
    await planWithFiles(caller, "f10", files, { outputDir: "/out/" });
    expect(sent).toEqual({
      roots: [
        {
          path: "素材",
          target: "/out/素材.csv",
          files: 3,
          dirs: 2,
          maxDepth: 3
        }
      ]
    });
  });

  it("输出目录留空时清单落在沙箱根（target 不加前导目录）", async () => {
    let sent: unknown;
    const caller = async (request: WasmCall): Promise<WasmCallResult> => {
      sent = JSON.parse(String((request as { payload?: string }).payload ?? "{}"));
      return { ok: true, result: planDocument("f10") };
    };
    await planWithFiles(caller, "f10", [dirFile("素材/a.txt", "a")], {});
    expect(sent).toMatchObject({ roots: [{ path: "素材", target: "素材.csv" }] });
  });

  it("普通多选（没有相对路径）明确拒绝，不编造一份扁平清单", async () => {
    const caller = vi.fn(async (): Promise<WasmCallResult> => ({
      ok: true,
      result: planDocument("f10")
    }));
    await expect(
      planWithFiles(caller, "f10", [fakeFile("a.txt", "x")], {})
    ).rejects.toMatchObject({ code: "E1001" });
    expect(caller).not.toHaveBeenCalled();
  });
});
