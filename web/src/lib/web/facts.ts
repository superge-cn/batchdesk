/**
 * Web 形态的「事实采集层」（设计 02 §2/§3、设计 05 §2）——A1 的第一块。
 *
 * 为什么需要这一层：kernel 的 `plan_*` 全是**纯函数**，输入是**事实 DTO**
 * （如 `PlanC01File{path, canonical_path, file_size, sample}`），不是路径。
 * 桌面壳在 `src-tauri/src/plan_kinds.rs` 用真实 FS 采集这些事实；浏览器里没有
 * 真实 FS，必须由本层从 **File API + BlobRef 句柄**采出同形事实，再交给 WASM。
 *
 * 边界（设计 02 §2、不变量 11）：
 * - `BlobRef` 只是**句柄**，文件内容不进 JSON；
 * - 只读用户文件——**不写用户磁盘**（Web 侧写产物一律走 OPFS/下载）；
 * - 本层**不做任何授权/配额判定**，只搬运事实。
 */

import { KernelError } from "../wasm/contract";

/** 设计 02 §2 的句柄形态。 */
export type BlobRefKind = "user-file" | "opfs" | "remote";

export interface BlobRef {
  kind: BlobRefKind;
  /** user-file：会话内句柄 id；opfs：`opfs://<bucket>/<path>`；remote：服务端对象 id。 */
  ref: string;
  size: number;
  mime: string;
  sha256?: string;
}

/**
 * 会话内的用户文件登记表（`registerUserFiles` 的客户端对应物，设计 02 §3.3）。
 *
 * 只存在内存里——刷新即失，不落 localStorage/IndexedDB（设计 14 §9 的同族口径：
 * 会话级数据不持久化）。File 对象本身也不能结构化克隆进 Worker，
 * 故句柄只留在主线程，Worker 拿到的是**事实**而非句柄（设计 02 §3.2）。
 */
export class UserFileRegistry {
  private readonly files = new Map<string, File>();
  private seq = 0;

  register(file: File): BlobRef {
    const ref = `uf-${++this.seq}`;
    this.files.set(ref, file);
    return { kind: "user-file", ref, size: file.size, mime: file.type };
  }

  registerAll(files: File[]): BlobRef[] {
    return files.map((file) => this.register(file));
  }

  get(ref: string): File | undefined {
    return this.files.get(ref);
  }

  /** 释放句柄（页面关闭或用户移除选择时调用；File 无显式释放 API，摘引用即可）。 */
  release(ref: string): void {
    this.files.delete(ref);
  }

  clear(): void {
    this.files.clear();
  }
}

/**
 * 按采样窗口取文件头部的字节事实（对应 `platform::text::sample_bytes` 的**头部窗口**）。
 *
 * 桌面侧采样是「头部 + 等距窗口」，浏览器侧 A1 只取头部——因为编码检测
 * （方案 §5.2 的四级流水线）用的就是头部采样；等距窗口用于长文件，
 * 留待 A2 与分块读取一起做（本层先如实标注口径差异，不假装等价）。
 */
export async function sampleHead(
  file: File,
  windowBytes: number
): Promise<Uint8Array> {
  if (windowBytes <= 0) {
    throw new Error("sample window must be positive");
  }
  const slice = file.slice(0, Math.min(windowBytes, file.size));
  return readBlobBytes(slice);
}

/**
 * 读 `Blob` 的全部字节。
 *
 * 优先用 `Blob.prototype.arrayBuffer()`（所有真实浏览器都有）；jsdom 的实现缺该方法，
 * 回退到 `FileReader`——这样同一份代码在浏览器与测试里行为一致，
*  不必让产品代码去迁就测试环境。
 */
async function readBlobBytes(blob: Blob): Promise<Uint8Array> {
  if (typeof blob.arrayBuffer === "function") {
    return new Uint8Array(await blob.arrayBuffer());
  }
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(new Uint8Array(reader.result as ArrayBuffer));
    reader.onerror = () => reject(reader.error);
    reader.readAsArrayBuffer(blob);
  });
}

/** `path` 字段在 Web 形态的取值：用户文件的显示名（不含目录，浏览器本就给不到目录）。 */
function displayPath(name: string): string {
  return name;
}

/** 读整个 `Blob` 的字节（Web 侧事实采集的唯一来源；不写用户磁盘）。 */
export async function readAllBytes(file: File): Promise<Uint8Array> {
  return readBlobBytes(file);
}

/**
 * 把一个用户文件采成 C01 的事实（对齐 WASM 侧 `WebC01File`，camelCase 线协议）。
 *
 * `canonical_path` 在浏览器里无从获得（File API 不暴露真实路径），按契约置 null——
 * 内核对该字段是 `Option`，缺失只影响 Windows 长路径等**平台事实**判定，
 * 不影响计划生成（设计 04 v1.13：能力值按平台注入，Web 侧由调用方声明）。
 */
export async function c01FileFact(
  file: File,
  windowBytes: number
): Promise<{
  path: string;
  canonicalPath: string | null;
  fileSize: number;
  sample: number[];
}> {
  const sample = await sampleHead(file, windowBytes);
  return {
    path: displayPath(file.name),
    canonicalPath: null,
    fileSize: file.size,
    sample: Array.from(sample)
  };
}

/**
 * C03 事实（ADR-0102 A2-2）：整段内容 + 头部样本。
 *
 * 桌面侧对大文件走流式扫描（`ReplaceScan::Streaming`），Web 侧先只支持**整段**
 * 语义——浏览器里读整段本就要把文件放进内存，与流式无关；但对超大文件应先做
 * 体积门槛提示（设计 02 §13.6 的内存建议档），不在本函数内判定。
 */
export async function c03FileFact(
  file: File,
  windowBytes: number
): Promise<{
  path: string;
  canonicalPath: string | null;
  fileSize: number;
  data: number[];
  readonly: boolean;
  locked: boolean;
}> {
  const all = await readAllBytes(file);
  const end = Math.min(windowBytes, all.length);
  return {
    path: displayPath(file.name),
    canonicalPath: null,
    fileSize: file.size,
    data: Array.from(all),
    // Web 侧用户文件恒为只读引用（设计 02 §3.1：不写用户磁盘）。
    readonly: false,
    locked: false
  };
}

/** C02 事实（ADR-0102 A2-2）：头部样本 + 整段字节（换行统计在 WASM 侧做）。 */
export async function c02FileFact(
  file: File,
  windowBytes: number
): Promise<{
  path: string;
  canonicalPath: string | null;
  fileSize: number;
  sample: number[];
  data: number[];
}> {
  const all = await readAllBytes(file);
  const end = Math.min(windowBytes, all.length);
  return {
    path: displayPath(file.name),
    canonicalPath: null,
    fileSize: file.size,
    sample: Array.from(all.slice(0, end)),
    data: Array.from(all)
  };
}


/**
 * CSV 家族共用事实（ADR-0102 A2-3）：头部采样 + 整段字节。
 *
 * 八个功能（F06–F09／C11–C14）的计划输入都需要同一组字段，故采一次即可复用；
 * 嗅探、列数、脏行由 WASM 侧的 kernel 入口算（桌面 platform 走同一条路径）。
 */
export async function csvFileFact(
  file: File,
  windowBytes: number,
  hasHeader: boolean
): Promise<{
  path: string;
  canonicalPath: string | null;
  fileSize: number;
  sample: number[];
  data: number[];
  hasHeader: boolean;
  targetOccupied: boolean;
}> {
  const all = await readAllBytes(file);
  const end = Math.min(windowBytes, all.length);
  return {
    path: displayPath(file.name),
    canonicalPath: null,
    fileSize: file.size,
    sample: Array.from(all.slice(0, end)),
    data: Array.from(all),
    hasHeader,
    // Web 侧无目标文件概念（不写用户磁盘），恒 false。
    targetOccupied: false
  };
}

/**
 * 分割类事实（ADR-0102 A2-4）：整段字节即可（分片布局按行边界计算）。
 * `groupId` 由 WASM 侧兜底生成（计划层要求 ULID 形态），此处留空。
 */
export async function splitFileFact(
  file: File
): Promise<{
  path: string;
  canonicalPath: string | null;
  groupId: string;
  fileSize: number;
  data: number[];
}> {
  const all = await readAllBytes(file);
  return {
    path: displayPath(file.name),
    canonicalPath: null,
    groupId: "",
    fileSize: file.size,
    data: Array.from(all)
  };
}

/**
 * C08 归档文件事实（ADR-0102 A2-5）：File API 可直接给出 name/size/lastModified。
 * `relPath` 由调用方按所选目录结构拼装（File API 只给扁平 name）。
 */
export async function c08FileFact(
  file: File,
  relPath: string
): Promise<{
  relPath: string;
  size: number;
  modifiedDate: string | null;
  exists: boolean;
}> {
  // lastModified 是毫秒时间戳 → `yyyy-mm-dd`（与桌面 RFC 3339 前 10 字符同形）。
  const modifiedDate = Number.isFinite(file.lastModified)
    ? new Date(file.lastModified).toISOString().slice(0, 10)
    : null;
  return { relPath, size: file.size, modifiedDate, exists: true };
}

/** C07 清单行的存在性事实（Web 侧只能由已选文件表推断；目标占用恒 false）。 */
export function c07Fact(exists: boolean, size: number): { exists: boolean; size: number } {
  return { exists, size };
}

/**
 * 目录选择（`<input type="file" webkitdirectory>`）给出的相对路径。
 *
 * 浏览器给的是 `目录名/子目录/文件`——**第一段是被选目录自身的名字**，而内核的
 * `rel_path` 是相对于源根的（`platform::listing::collect_tree_files(source_root)`）。
 * 所以这里剥掉第一段，否则规则里的 `ext:` 仍能命中、但计划里的路径会多一层目录，
 * 与桌面同参数产出的计划对不上。
 *
 * 非目录选择（普通多选）返回空串：调用方据此判定「拿不到相对路径」，
 * 不要拿扁平文件名凑数——那会把「子目录里的文件」当成「根目录下的文件」。
 */
export function pickedRelativePath(file: File): string {
  const raw = (file as File & { webkitRelativePath?: string }).webkitRelativePath ?? "";
  const normalized = raw.replace(/\\/g, "/");
  const at = normalized.indexOf("/");
  return at < 0 ? "" : normalized.slice(at + 1);
}

/** 目录选择的根名（`sourceRoot` 在浏览器里的现实来源；非目录选择时为 `undefined`）。 */
export function pickedRootOf(file: File): string | undefined {
  const raw = (file as File & { webkitRelativePath?: string }).webkitRelativePath ?? "";
  const normalized = raw.replace(/\\/g, "/");
  const at = normalized.indexOf("/");
  return at > 0 ? normalized.slice(0, at) : undefined;
}

/** 一组目录选择文件的共同根名（取第一个有根名的；混合由调用方另行判定）。 */
export function pickedRootName(files: File[]): string | undefined {
  for (const file of files) {
    const root = pickedRootOf(file);
    if (root !== undefined) return root;
  }
  return undefined;
}

/** F10 清单里的一行文件事实（相对根路径 + 大小 + 修改时间）。 */
export interface ListingFactFile {
  /** 相对根的路径（正斜杠、无前导 `/`）；目录行由内核按路径前缀补齐，不必单列。 */
  path: string;
  size: number;
  /** RFC 3339；浏览器拿不到时为空串（不编造时间）。 */
  mtime: string;
}

/** 一个根目录的清单事实（计划期算摘要、执行期交给 `MemoryDirectoryLister`）。 */
export interface ListingTree {
  /** 根名（浏览器里就是被选目录自身的名字；沙箱键与计划里的源都用它）。 */
  root: string;
  files: number;
  dirs: number;
  maxDepth: number;
  rows: ListingFactFile[];
}

/**
 * 从目录选择的文件表推出 F10 的目录树事实（ADR-0114）。
 *
 * 口径与桌面 `platform::listing::walk` 的**可采集部分**对齐：
 * - `files`＝文件条目数；`dirs`＝文件路径的各级前缀去重（桌面还会数空目录）；
 * - `maxDepth`＝最深层级（根的子项为 1）；
 * - 占位文件/符号链接在 File API 里不可分辨，本层不产出这两类计数（恒 0）。
 *
 * 拿不到根名（普通多选）时抛 `E1001`：计划里的源是「根名」，凑一个裸文件名
 * 会让清单的每一行都少一层目录——那比报错更糟。
 */
export function listingTrees(files: File[]): ListingTree[] {
  const byRoot = new Map<string, File[]>();
  for (const file of files) {
    const root = pickedRootOf(file);
    const relative = pickedRelativePath(file);
    if (root === undefined || relative === "") {
      throw new KernelError(
        "E1001: directory listing needs a directory pick (relative paths are unavailable)"
      );
    }
    const bucket = byRoot.get(root);
    if (bucket === undefined) byRoot.set(root, [file]);
    else bucket.push(file);
  }

  const trees: ListingTree[] = [];
  for (const [root, bucket] of byRoot) {
    const dirs = new Set<string>();
    const rows: ListingFactFile[] = [];
    let maxDepth = 0;
    for (const file of bucket) {
      const relative = pickedRelativePath(file);
      const parts = relative.split("/").filter((part) => part !== "");
      maxDepth = Math.max(maxDepth, parts.length);
      for (let index = 1; index < parts.length; index += 1) {
        dirs.add(parts.slice(0, index).join("/"));
      }
      rows.push({
        path: relative,
        size: file.size,
        mtime: Number.isFinite(file.lastModified)
          ? new Date(file.lastModified).toISOString()
          : ""
      });
    }
    rows.sort((left, right) => left.path.localeCompare(right.path));
    trees.push({
      root,
      files: rows.length,
      dirs: dirs.size,
      maxDepth,
      rows
    });
  }
  trees.sort((left, right) => left.root.localeCompare(right.root));
  return trees;
}

/**
 * F04/C09 的文件事实（ADR-0102 A2-6）。
 *
 * - `fullHash` 需调用方预算（算法须与桌面一致＝blake3）；File API 只给内容，
 *   哈希不在本模块内计算，避免前端引入与 Rust 侧不同的实现。
 * - `mtime` 用 `Date.getTime()`（毫秒）；内核字段是纳秒，故 ×1e6。
 */
export async function mergeFileFact(
  file: File,
  fullHash?: number[]
): Promise<{
  path: string;
  canonicalPath: string | null;
  fileSize: number;
  data: number[];
  fullHash: number[] | null;
  mtimeNanos: number;
}> {
  const all = await readAllBytes(file);
  return {
    path: displayPath(file.name),
    canonicalPath: null,
    fileSize: file.size,
    data: Array.from(all),
    fullHash: fullHash ?? null,
    mtimeNanos: Math.round(file.lastModified) * 1_000_000
  };
}

