import { describe, expect, it } from "vitest";

import {
  UserFileRegistry,
  c01FileFact,
  c02FileFact,
  c03FileFact,
  csvFileFact,
  sampleHead
} from "./facts";

/**
 * Web 事实采集层的契约（设计 02 §2/§3）。
 *
 * 钉住三件事：
 * ① `BlobRef` 只携带元数据，**不含内容**（文件内容不进 JSON）；
 * ② 采样只取头部窗口，且**不越界**（小文件取全长）；
 * ③ C01 事实与壳层 `plan_kinds.rs:331` 同形——字段缺一不可，否则 WASM 侧反序列化失败。
 */

function fakeFile(name: string, bytes: Uint8Array, type = "text/plain"): File {
  // jsdom 的 File 构造器接受 BlobPart 数组。
  return new File([bytes as unknown as BlobPart], name, { type });
}

describe("Web 事实采集层（设计 02 §2/§3）", () => {
  it("登记返回 BlobRef 句柄，且不含文件内容", () => {
    const registry = new UserFileRegistry();
    const file = fakeFile("合同.pdf", new TextEncoder().encode("hello"));
    const ref = registry.register(file);
    expect(ref.kind).toBe("user-file");
    expect(ref.size).toBe(5);
    expect(ref.mime).toBe("text/plain");
    expect(JSON.stringify(ref)).not.toContain("hello");
    expect(registry.get(ref.ref)).toBe(file);
  });

  it("句柄可释放；释放后取不到", () => {
    const registry = new UserFileRegistry();
    const ref = registry.register(fakeFile("a.txt", new Uint8Array([1])));
    registry.release(ref.ref);
    expect(registry.get(ref.ref)).toBeUndefined();
  });

  it("头部采样按窗口截断，小文件取全长", async () => {
    const content = new TextEncoder().encode("0123456789");
    const file = fakeFile("x.bin", content);
    expect(Array.from(await sampleHead(file, 4))).toEqual([48, 49, 50, 51]);
    expect((await sampleHead(file, 100)).length).toBe(10);
    await expect(sampleHead(file, 0)).rejects.toThrow(/positive/);
  });

  it("C01 事实与壳层同形：path/file_size/sample，canonical_path 置 null", async () => {
    // 壳层构造见 src-tauri/src/plan_kinds.rs:331；字段名与内核 PlanC01File 一致。
    const file = fakeFile("报告.txt", new TextEncoder().encode("中文内容"));
    const fact = await c01FileFact(file, 8);
    // camelCase 线协议（与 WASM `WebC01File` 一致）——改错键名 wasm 侧测试会红。
    expect(Object.keys(fact).sort()).toEqual([
      "canonicalPath",
      "fileSize",
      "path",
      "sample"
    ]);
    expect(fact.path).toBe("报告.txt");
    expect(fact.fileSize).toBe(12);
    expect(fact.canonicalPath).toBeNull();
    expect(fact.sample.length).toBe(8);
  });
});

describe("C02/C03 事实（ADR-0102 A2-2）", () => {
  function fakeFile(name: string, text: string): File {
    return new File([new TextEncoder().encode(text) as unknown as BlobPart], name);
  }

  it("C03：整段字节 + 只读引用（Web 不写用户磁盘）", async () => {
    const fact = await c03FileFact(fakeFile("note.txt", "alpha\r\nbeta"), 4096);
    expect(fact.path).toBe("note.txt");
    expect(fact.fileSize).toBe(11);
    expect(fact.readonly).toBe(false);
    expect(new TextDecoder().decode(Uint8Array.from(fact.data))).toBe("alpha\r\nbeta");
  });

  it("C02：样本按窗口截断，data 为整段", async () => {
    const fact = await c02FileFact(fakeFile("log.txt", "abcdef"), 3);
    expect(Array.from(fact.sample)).toEqual([97, 98, 99]);
    expect(new TextDecoder().decode(Uint8Array.from(fact.data))).toBe("abcdef");
  });
});

describe("CSV 家族共用事实（ADR-0102 A2-3）", () => {
  it("采样按窗口截断、整段完整，目标占用恒 false（Web 不写用户磁盘）", async () => {
    const file = new File([new TextEncoder().encode("name,age\nann,30") as unknown as BlobPart], "a.csv");
    const fact = await csvFileFact(file, 4, true);
    expect(Array.from(fact.sample)).toEqual([110, 97, 109, 101]); // "name"
    expect(new TextDecoder().decode(Uint8Array.from(fact.data))).toBe("name,age\nann,30");
    expect(fact.hasHeader).toBe(true);
    expect(fact.targetOccupied).toBe(false);
  });
});
