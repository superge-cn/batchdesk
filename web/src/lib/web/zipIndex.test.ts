import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";

import { describe, expect, it } from "vitest";

import { parseCentralDirectory, readZipEntriesFromFile, ZipIndexError } from "./zipIndex";

/**
 * ADR-0103 的**对拍测试**：浏览器侧的中央目录读取必须与桌面
 * `platform::archive::ZipArchiveService::inspect` 给出同一组事实。
 *
 * 期望值不是"看代码猜的"：Z01/Z02 来自语料生成脚本，四个夹具的条目事实用
 * `crates/platform` 的 `inspect` 实际跑过一遍（探针见 ADR-0103 §证据）。
 */
// 测试在 Node 里跑：只用到 cwd()，不必为它引整套 @types/node。
declare const process: { cwd(): string };

const FIXTURES = [
  // vitest 从 web/ 跑（`pnpm test`）；也容忍从仓库根跑。
  resolve(process.cwd(), "..", "fixtures"),
  resolve(process.cwd(), "fixtures")
].find((dir) => existsSync(dir));

function fixture(relative: string): File {
  if (!FIXTURES) {
    throw new Error("fixtures directory not found; run vitest from web/");
  }
  const bytes = readFileSync(join(FIXTURES, relative));
  return new File([new Uint8Array(bytes)], relative.split("/").pop() ?? "fixture.zip");
}

describe("readZipEntriesFromFile：真实语料对拍", () => {
  it("Z01：GBK 名回退（bit 11 未置位）", async () => {
    const entries = await readZipEntriesFromFile(fixture("corpus/Z01/out/gbk_name.zip"));
    expect(entries).toEqual([
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
  });

  it("Z02：UTF-8 名（bit 11 置位）且穿越条目名原样保留", async () => {
    const entries = await readZipEntriesFromFile(fixture("corpus/Z02/out/traversal.zip"));
    expect(entries.map((entry) => entry.name)).toEqual([
      "safe/ok.txt",
      "../escape.txt",
      "safe/../../escape2.txt"
    ]);
    expect(entries.map((entry) => entry.gbkFallback)).toEqual([false, false, false]);
    expect(entries.map((entry) => entry.index)).toEqual([0, 1, 2]);
  });

  it("ZipCrypto：加密条目标记 encrypted，压缩方法仍受支持", async () => {
    const entries = await readZipEntriesFromFile(fixture("archive/password/zipcrypto.zip"));
    expect(entries).toEqual([
      {
        index: 0,
        name: "a.txt",
        kind: "file",
        size: 10,
        compressedSize: 22,
        gbkFallback: false,
        encrypted: true,
        supported: true
      },
      {
        index: 1,
        name: "sub/b.txt",
        kind: "file",
        size: 7,
        compressedSize: 19,
        gbkFallback: false,
        encrypted: true,
        supported: true
      }
    ]);
  });

  it("AES-256：方法 99 还原成扩展里的真实方法（桌面 inspect 实测为 Stored）", async () => {
    const entries = await readZipEntriesFromFile(fixture("archive/password/aes256.zip"));
    // 桌面 `zip` crate 把 AES 的 99 还原为实际方法；夹具是"存储"，故 supported=true。
    // 这正是对拍抓出来的偏差：只看 99 会误判为不支持，把可解条目全部跳过。
    expect(entries.map((entry) => entry.supported)).toEqual([true, true]);
    expect(entries.map((entry) => entry.encrypted)).toEqual([true, true]);
    expect(entries.map((entry) => entry.size)).toEqual([10, 7]);
  });
});

describe("readZipEntriesFromFile：边界与损坏输入", () => {
  it("目录条目按原始名末字节判定（`/` 与 `\\`）", async () => {
    const archive = buildArchive([
      { name: latin1("sub/"), method: 0, size: 0, compressedSize: 0 },
      { name: latin1("win\\"), method: 0, size: 0, compressedSize: 0 },
      { name: latin1("file.txt"), method: 8, size: 3, compressedSize: 2 }
    ]);
    const entries = await readZipEntriesFromFile(archive);
    expect(entries.map((entry) => entry.kind)).toEqual(["dir", "dir", "file"]);
  });

  it("非 ZIP 字节：报 E3003（与桌面「archive cannot be read」同码）", async () => {
    const notZip = new File([new Uint8Array(64).fill(0x41)], "no.zip");
    await expect(readZipEntriesFromFile(notZip)).rejects.toBeInstanceOf(ZipIndexError);
    await expect(readZipEntriesFromFile(notZip)).rejects.toThrow(/E3003/);
  });

  it("短于结束记录的文件直接拒绝", async () => {
    await expect(readZipEntriesFromFile(new File([new Uint8Array(4)], "tiny.zip"))).rejects.toThrow(
      /E3003/
    );
  });
});

describe("parseCentralDirectory：Zip64 扩展字段", () => {
  it("size/compressedSize 哨兵值改读 0x0001 扩展（64 位不截断）", () => {
    const name = latin1("big.bin");
    const zip64Body = new Uint8Array(16);
    const zip64View = new DataView(zip64Body.buffer);
    zip64View.setBigUint64(0, 5_000_000_000n, true); // 未压缩
    zip64View.setBigUint64(8, 4_800_000_000n, true); // 压缩后
    const directory = buildCentralOnly([
      {
        name,
        flags: 0x0800,
        method: 8,
        size: 0xffffffff,
        compressedSize: 0xffffffff,
        extra: concat(latin1Bytes([0x01, 0x00, 0x10, 0x00]), zip64Body)
      }
    ]);
    const entries = parseCentralDirectory(directory, 1);
    expect(entries[0]?.size).toBe(5_000_000_000);
    expect(entries[0]?.compressedSize).toBe(4_800_000_000);
  });

  it("条目数与结束记录不符：拒绝（不静默产出半截清单）", () => {
    const directory = buildCentralOnly([
      { name: latin1("a.txt"), flags: 0x0800, method: 0, size: 1, compressedSize: 1, extra: null }
    ]);
    expect(() => parseCentralDirectory(directory, 2)).toThrow(/E3003/);
  });
});

describe("parseCentralDirectory：AES 方法还原", () => {
  it("0x9901 里的真实方法决定 supported（deflate 支持 / bzip2 与缺失扩展不支持）", () => {
    const directory = buildCentralOnly([
      { name: latin1("a.bin"), flags: 0x0001, method: 99, size: 4, compressedSize: 9, extra: aesExtra(8) },
      { name: latin1("b.bin"), flags: 0x0001, method: 99, size: 4, compressedSize: 9, extra: aesExtra(12) },
      { name: latin1("c.bin"), flags: 0x0001, method: 99, size: 4, compressedSize: 9, extra: null }
    ]);
    expect(parseCentralDirectory(directory, 3).map((entry) => entry.supported)).toEqual([
      true,
      false,
      false
    ]);
  });
});

/** AES 扩展（0x9901）：version(2) + "AE"(2) + 强度(1) + 真实方法(2)。 */
function aesExtra(method: number): Uint8Array {
  const body = new Uint8Array(7);
  const view = new DataView(body.buffer);
  view.setUint16(0, 2, true);
  view.setUint16(2, 0x4541, true);
  view.setUint8(4, 3);
  view.setUint16(5, method, true);
  return concat(latin1Bytes([0x01, 0x99, 0x07, 0x00]), body);
}

interface CentralEntry {
  name: Uint8Array;
  flags?: number;
  method: number;
  size: number;
  compressedSize: number;
  extra?: Uint8Array | null;
}

/** 只造「中央目录 + 结束记录」的最小结构（本模块只读这两段，不碰本地头）。 */
function buildArchive(entries: CentralEntry[]): File {
  const directory = buildCentralOnly(entries.map((entry) => ({ ...entry, flags: entry.flags ?? 0 })));
  const eocd = new Uint8Array(22);
  const view = new DataView(eocd.buffer);
  view.setUint32(0, 0x06054b50, true);
  view.setUint16(8, entries.length, true);
  view.setUint16(10, entries.length, true);
  view.setUint32(12, directory.byteLength, true);
  view.setUint32(16, 0, true);
  return new File([directory, eocd], "synthetic.zip");
}

function buildCentralOnly(entries: CentralEntry[]): Uint8Array<ArrayBuffer> {
  const chunks: Uint8Array[] = [];
  let offset = 0;
  for (const entry of entries) {
    const extra = entry.extra ?? new Uint8Array(0);
    const header = new Uint8Array(46 + entry.name.byteLength + extra.byteLength);
    const view = new DataView(header.buffer);
    view.setUint32(0, 0x02014b50, true);
    view.setUint16(4, 20, true);
    view.setUint16(6, 20, true);
    view.setUint16(8, entry.flags ?? 0, true);
    view.setUint16(10, entry.method, true);
    view.setUint32(20, entry.compressedSize, true);
    view.setUint32(24, entry.size, true);
    view.setUint16(28, entry.name.byteLength, true);
    view.setUint16(30, extra.byteLength, true);
    header.set(entry.name, 46);
    header.set(extra, 46 + entry.name.byteLength);
    chunks.push(header);
    offset += header.byteLength;
  }
  return concat(...chunks);
}

function concat(...chunks: Uint8Array[]): Uint8Array<ArrayBuffer> {
  const total = chunks.reduce((sum, chunk) => sum + chunk.byteLength, 0);
  const out = new Uint8Array(total);
  let at = 0;
  for (const chunk of chunks) {
    out.set(chunk, at);
    at += chunk.byteLength;
  }
  return out;
}

function latin1Bytes(values: number[]): Uint8Array<ArrayBuffer> {
  return new Uint8Array(values);
}

function latin1(text: string): Uint8Array {
  return latin1Bytes(Array.from(text, (char) => char.charCodeAt(0)));
}
