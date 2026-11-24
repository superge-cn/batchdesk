/**
 * Web 执行通道：浏览器沙箱里的真实 apply / undo（ADR-0108）。
 *
 * 与 `planChannel` 的分工：那边只读产出计划，这边把计划**真执行**——但执行在
 * Worker 内的会话沙箱（内存文件系统）里，产物以 Blob 交回主线程下载，**不写用户
 * 磁盘**（浏览器也写不了）。撤销只回滚沙箱内产物，已下载的副本无法被页面收回，
 * 界面必须照实说明（设计 02 §3.4 的 `scope: products-only`）。
 *
 * 为什么不在前端模拟执行：`plan` 与 `apply` 必须是同一套内核代码，否则「预览看到
 * 的」与「执行做出来的」会各算各的（不变量 2）。本模块只搬运，不做任何判定。
 */

import type { PlanDocument } from "../ipc/types";
import { KernelError, parsePlanResult } from "../wasm/contract";
import type { WasmCall, WasmCallResult, WasmProduct } from "../wasm/client";
import { listingTrees, pickedRelativePath, pickedRootOf, readAllBytes } from "./facts";
import { WebPlanUnavailable, type PickedFiles } from "./planChannel";

/** 执行入口的调用形态（与 `PlanCaller` 同注入点，测试用假实现）。 */
export type ExecCaller = (request: WasmCall) => Promise<WasmCallResult>;

/**
 * 本地建议档（方案 §13.10.2：单文件 ≤100MB、任务总量 ≤500MB）。
 *
 * 超档不是阻断：按该表口径是**强提示＋用户确认后放行**。数值只此一处，
 * 界面与通道都读这里，不各自写一份。
 */
export const SANDBOX_FILE_GUIDANCE_BYTES = 100_000_000;
export const SANDBOX_TOTAL_GUIDANCE_BYTES = 500_000_000;

/**
 * 尚未接入 Web 执行的原因（lib 层内部口径，界面统一用 `feature.execute.webOnly`）。
 *
 * 现在只剩一种：**未知视图**的保守兜底。19 个视图已全部接上——C10 的缺口曾是
 * 「端口未装配」，`MemoryArchiveService`（WASM 内的真 ZIP 编解码）落地后不再存在
 * （ADR-0112）。保留这个枚举而不是直接断言「全都能执行」：加新视图时忘接线要能被
 * 说清，而不是猜成能执行。
 */
export type WebApplyGap = "not-wired";

export interface WebApplySupport {
  level: "native" | "unsupported";
  gap?: WebApplyGap;
}

/**
 * 视图 → 执行能力档位。
 *
 * 23 个视图接上（22 个原有 + F13 正则改名），
 * 按「执行臂需要什么」分七族：
 * - **只要读写**：分片三兄弟（F01/F02/F03）＋合并族（F04 合并文本、F05 CSV 分片、
 *   F06 CSV 合并）——动作是 `split`/`merge`/`csv`，走 `FsAdapter` 就够；
 * - **只要改写器**：C01 转码／C02 换行／C03 查找替换——三个改写器都在 kernel 里
 *   （`TranscodeRewriter`／`LineEndingRewriter`／`ReplaceRewriter`），与桌面同一实现，
 *   沙箱只是把它们交给执行器（ADR-0109）；
 * - **CSV 行/列变换**：F07–F09、C11–C14——动作是 `csv-extract`/`csv-rows`/
 *   `csv-dedup`/`csv-filter`/`csv-sort`/`csv-columns`，同样只要读写。它们此前被记作
 *   `target-in-place`，根因是 wire 层把输出路径写成源文件（`outputDir` 被丢掉）；
 *   目标路径按表单输出目录派生后即可执行（ADR-0110）；
 * - **按相对路径归档/同步**：C07 清单驱动移动、C08 规则归档——动作就是 `move`，
 *   但源路径带目录层级（`sourceRoot/相对路径`），staging 必须逐字对齐计划的源
 *   （ADR-0111）；
 * - **归档编解码**：C10 解压/压缩——执行臂要 `ArchiveService` 端口，Web 侧由
 *   `MemoryArchiveService` 提供（WASM 内真 ZIP 读写，与桌面同一个 `zip` crate、
 *   同一套错误码；ADR-0112）。解压的源是包本身，压缩的源是用户选中的文件；
 * - **按相对路径改名**：C04 名称替换、F12 批量改名、F13 正则改名——动作是 `rename`，条目是目录
 *   的**第一层名字**（与桌面 `read_dir` 同口径），子目录条目的子树随目录一起移动，
 *   所以 staging 要放进整个目录（ADR-0113）。产物是改名后的文件，可下载、可撤销。
 * - **只读元数据的清单**：F10 目录清单——动作是 `listing`，执行臂要 `DirectoryLister`
 *   端口。浏览器里没有目录遍历，端口由 `MemoryDirectoryLister` 承担，事实来自
 *   `webkitRelativePath`（ADR-0114）。**不登记任何文件字节**：清单本来就不读内容。
 */
const WEB_APPLY_VIEWS = new Set([
  "f01",
  "f02",
  "f03",
  "f04",
  "f05",
  "f06",
  "f07",
  "f08",
  "f09",
  "c01",
  "c02",
  "c03",
  "c07",
  "c08",
  "c10",
  "c11",
  "c12",
  "c13",
  "c14",
  "c04",
  "f12",
  "f13",
  "f10"
]);

export function webApplySupport(view: string): WebApplySupport {
  if (WEB_APPLY_VIEWS.has(view)) return { level: "native" };
  return { level: "unsupported", gap: "not-wired" };
}

export interface WebSandboxFacts {
  fileCount: number;
  totalBytes: number;
  maxFileBytes: number;
  /** 超出 §13.10.2 的本地建议档：调用方必须先取得用户确认再继续。 */
  aboveGuidance: boolean;
}

/** 按文件大小算沙箱事实（不读内容——确认框要在读之前弹）。 */
export function webSandboxFacts(files: File[]): WebSandboxFacts {
  const totalBytes = files.reduce((total, file) => total + file.size, 0);
  const maxFileBytes = files.reduce((max, file) => Math.max(max, file.size), 0);
  return {
    fileCount: files.length,
    totalBytes,
    maxFileBytes,
    aboveGuidance:
      totalBytes > SANDBOX_TOTAL_GUIDANCE_BYTES ||
      maxFileBytes > SANDBOX_FILE_GUIDANCE_BYTES
  };
}

/**
 * 按目录层级取源的视图（计划的源是 `sourceRoot/相对路径`，不是裸文件名）。
 *
 * 这三个事实必须一致，否则执行器报「源缺失」而用户在界面上明明选了文件：
 * ① 浏览器里 `sourceRoot` 字段是目录选择器（不写表单文本），`planC07`/`planC08`
 *    因此拿**所选目录名**当源根；② `webkitRelativePath` 的第一段就是该目录名；
 * ③ 计划里的源＝`join_root(源根, 相对路径)`。staging 键按同一规则拼。
 */
/**
 * 源键口径是「根名/相对路径」的视图。
 *
 * C07/C08 的计划里源是 `sourceRoot/相对路径`；C04/F12 的条目是目录第一层名字
 * （`根名/名字`），其子目录条目的子树也要一起进沙箱（否则 rename 目录时子树不在，
 * 产物只剩一个空目录）。两类形状相同，共用一条拼法。
 */
const RELATIVE_ROOT_VIEWS = new Set(["c07", "c08", "c04", "f12", "f13"]);

/**
 * 执行期要登记进沙箱的输入槽。
 *
 * 不是「选中什么就登记什么」：C07 的清单文件只参与**计划**（`parse_c07_manifest`
 * 在计划期已读进内存），执行时不需要它出现在沙箱里。把清单也 stage 进去会多一个
 * 可能重名的条目（`清单.csv` 与源目录里的同名文件撞键），而它对执行毫无用处。
 *
 * 未列出的视图按「全部槽」摊平——它们的多槽只有 C10（`archive`/`sources`，两者
 * 互斥使用，多 stage 的那一份对执行无害：执行器只读计划提到的那条）。
 */
const STAGED_SLOTS: Record<string, string[]> = {
  c07: ["sourceRoot"],
  c08: ["sourceRoot"],
  c04: ["dir"],
  f12: ["dir"],
  f13: ["dir"]
};

/**
 * 执行期要登记进沙箱的项目（槽名 + 文件）。
 *
 * 槽名要带着走，因为**同一个视图的不同槽可能有不同的路径口径**：C10 的压缩源在
 * 计划里是 `pickedRelativePath(source) || source.name`，而解压的源是包自己的文件名。
 */
function stagedPairs(
  view: string,
  picked: PickedFiles
): { slot: string; file: File }[] {
  const slots = STAGED_SLOTS[view] ?? Object.keys(picked);
  return slots.flatMap((slot) =>
    (picked[slot] ?? []).map((file) => ({ slot, file }))
  );
}

/**
 * 执行期真正会读进沙箱的文件（界面算 §13.10.2 建议档也用它）。
 *
 * 与「用户选了什么」不是一回事：见 `STAGED_SLOTS`。
 */
export function webStagedFiles(view: string, picked: PickedFiles): File[] {
  // F10 只登记元数据（清单不读内容），字节量为 0——建议档不该按选中文件的总大小算。
  if (view === "f10") return [];
  return stagedPairs(view, picked).map((pair) => pair.file);
}

/** 计划里出现过的源路径（沙箱登记的键必须与计划一致，否则执行器找不到文件）。 */
function stagedPath(view: string, slot: string, file: File): string {
  assertExecutable(view);
  if (RELATIVE_ROOT_VIEWS.has(view)) {
    const root = pickedRootOf(file);
    const relative = pickedRelativePath(file);
    if (root === undefined || relative === "") {
      // 普通多选没有相对路径：不能拿裸文件名凑数——计划里的源是
      // `sourceRoot/相对路径`，凑出来的键永远对不上，结果会是「每条都源缺失」。
      throw new KernelError(
        "E1001: this view needs a directory pick (relative paths are unavailable)"
      );
    }
    return `${root}/${relative}`;
  }
  if (view === "c10" && slot === "sources") {
    // 压缩计划的源逐字来自 `planC10Compress`：`pickedRelativePath(source) || source.name`。
    // 文件名没有层级，不能拿它当键——源在目录里时计划写的是 `sub/a.txt`。
    return pickedRelativePath(file) || file.name;
  }
  // 其余已接视图的计划事实都用 `displayPath(file.name)`：裸文件名（C10 的 `archive`
  // 槽也在此列：解压的源就是包自己的名字）。
  return file.name;
}

function assertExecutable(view: string): void {
  const support = webApplySupport(view);
  if (support.level !== "native") {
    throw new KernelError(
      `E5001: ${view} is not wired to the browser sandbox executor yet`
    );
  }
}

async function invokeResult(
  caller: ExecCaller,
  request: WasmCall
): Promise<{ result: string; products?: WasmProduct[] }> {
  const result = await caller(request);
  if (result.ok) return result;
  if (result.reason === "wasm-unavailable") {
    throw new WebPlanUnavailable(
      "WASM runtime is not available in this environment"
    );
  }
  throw new KernelError(`${result.code ?? "E0000"}: sandbox call failed`);
}

async function invoke(caller: ExecCaller, request: WasmCall): Promise<string> {
  return (await invokeResult(caller, request)).result;
}

/** 把输入字节登记进 Worker 沙箱（清单 + 一段连续字节，只过一次边界）。 */
export async function stageInputs(
  caller: ExecCaller,
  view: string,
  picked: PickedFiles
): Promise<WebSandboxFacts> {
  // 先判档位再读内容：未接线的视图不该先把用户的文件读进内存。
  assertExecutable(view);
  // F10 走「目录 + 元数据」两条登记，不读任何文件字节（清单只读元数据，ADR-0114）。
  if (view === "f10") return stageListingFacts(caller, picked);
  const files = webStagedFiles(view, picked);
  if (files.length === 0) {
    throw new KernelError("E1001: pick at least one file before applying");
  }
  const facts = webSandboxFacts(files);
  const entries: { path: string; length: number; bytes: Uint8Array }[] = [];
  for (const { slot, file } of stagedPairs(view, picked)) {
    const bytes = await readAllBytes(file);
    entries.push({ path: stagedPath(view, slot, file), length: bytes.length, bytes });
  }
  const total = entries.reduce((sum, entry) => sum + entry.length, 0);
  const data = new Uint8Array(total);
  let offset = 0;
  for (const entry of entries) {
    data.set(entry.bytes, offset);
    offset += entry.length;
  }
  await invoke(caller, {
    op: "stage_inputs",
    payload: JSON.stringify(
      entries.map((entry) => ({ path: entry.path, length: entry.length }))
    ),
    bytes: data
  });
  return facts;
}

/**
 * F10 的两步登记：先建目录（执行器预检 `stat(源)` 要求源是目录），再交目录事实。
 *
 * 只建**根**目录：`MemoryDirectoryLister` 不读沙箱目录树，子目录不必存在——
 * 多建反而是假事实（沙箱里凭空多出目录，而浏览器里它们可能根本不存在）。
 */
async function stageListingFacts(
  caller: ExecCaller,
  picked: PickedFiles
): Promise<WebSandboxFacts> {
  const trees = listingTrees(Object.values(picked).flat());
  if (trees.length === 0) {
    throw new KernelError("E1001: pick at least one directory before applying");
  }
  await invoke(caller, {
    op: "stage_dirs",
    payload: JSON.stringify(trees.map((tree) => ({ path: tree.root })))
  });
  await invoke(caller, {
    op: "stage_listing",
    payload: JSON.stringify(
      trees.map((tree) => ({ root: tree.root, files: tree.rows }))
    )
  });
  return {
    fileCount: trees.reduce((total, tree) => total + tree.files, 0),
    // 没有字节进沙箱：清单只读元数据。
    totalBytes: 0,
    maxFileBytes: 0,
    aboveGuidance: false
  };
}

export interface WebApplyProduct {
  path: string;
  size: number;
  blob: Blob;
}

export interface WebApplyResult {
  /** 内核 `ApplyState` 的字符串形态（`completed`/`partial-failed`/`failed`/`cancelled`）。 */
  state: string;
  products: WebApplyProduct[];
  /** 内核 `ApplyOutcome` 原文（条目级状态与错误码都在里面）。 */
  outcome: unknown;
}

/**
 * 在沙箱里执行计划。
 *
 * `now` 由宿主给（内核不读时钟，不变量 1）；缺省值只用于测试/离线场景。
 */
export async function applyInSandbox(
  caller: ExecCaller,
  plan: PlanDocument,
  options: {
    onConflict?: "skip" | "sequence" | "overwrite";
    now?: string;
    /** 解密密码（执行期秘密，来自界面输入；计划的参数里只有「会不会提供」这个事实）。 */
    password?: string;
  } = {}
): Promise<WebApplyResult> {
  const response = await invokeResult(caller, {
    op: "apply_plan",
    payload: JSON.stringify({
      plan,
      options: {
        onConflict: options.onConflict ?? "skip",
        overwriteConfirmed: (options.onConflict ?? "skip") === "overwrite"
      },
      now: options.now ?? new Date().toISOString(),
      ...(options.password ? { password: options.password } : {})
    })
  });
  const raw = response.result;
  const parsed = parsePlanResult(raw) as {
    outcome?: { state?: string };
    products?: { path: string; size: number }[];
  };
  const state = parsed.outcome?.state;
  if (typeof state !== "string") {
    throw new KernelError("E9001: sandbox apply returned no outcome state");
  }
  const listed = parsed.products ?? [];
  const provided = response.products ?? [];
  return {
    state,
    // 产物字节随响应一起回（Worker 已包成 Blob）；测试里的假 caller 不给时用空
    // Blob 占位——界面按 `size` 显示真实大小，不会把占位当内容。
    products: listed.map((product) => ({
      path: product.path,
      size: product.size,
      blob:
        provided.find((entry) => entry.path === product.path)?.blob ?? new Blob([])
    })),
    outcome: parsed.outcome
  };
}

/** 撤销沙箱内的一次执行；返回内核回滚条目数。 */
export async function undoInSandbox(
  caller: ExecCaller,
  planId: string
): Promise<{ reverted: number; failed: number }> {
  const raw = await invoke(caller, { op: "undo_plan", payload: planId });
  const parsed = parsePlanResult(raw) as { reverted?: number; failed?: unknown[] };
  if (typeof parsed.reverted !== "number") {
    throw new KernelError("E9001: sandbox undo returned no reverted count");
  }
  return { reverted: parsed.reverted, failed: parsed.failed?.length ?? 0 };
}

/** 清空沙箱（换视图／换输入时调用，旧产物随即消失）。 */
export async function resetSandbox(caller: ExecCaller): Promise<void> {
  await invoke(caller, { op: "reset_sandbox" });
}

/** 产物下载名：只取沙箱路径的文件名（浏览器不允许 `<a download>` 带目录）。 */
export function productFileName(path: string, index = 0): string {
  const base = path.split(/[\\/]/).filter(Boolean).pop() ?? "product";
  return index > 0 ? `${index + 1}-${base}` : base;
}

export type ProductSaver = (name: string, blob: Blob) => void;

/**
 * 默认的产物保存器（触发浏览器下载）。
 *
 * 导出是为了让「浏览器存储」卡片复用同一份实现——下载逻辑两处各写一遍必然漂移
 * （先 revoke 还是后 revoke、要不要 noopener 这类细节）。
 */
export const defaultProductSaver: ProductSaver = (name, blob) => {
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = name;
  anchor.rel = "noopener";
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  // 立即 revoke 会让部分浏览器取消下载；下一拍再回收。
  setTimeout(() => URL.revokeObjectURL(url), 0);
};

/**
 * 产物的落盘名（下载与 OPFS 共用同一套算法）。
 *
 * 按**文件名**去重：不同目录的同名产物在浏览器里只能存成同一个名字，
 * 第二个起加序号前缀。两处各写一份必然漂移，所以只有这一份。
 *
 * 判重按**已分配的名字集合**，不是按「同名出现次数」：若按次数算，产物本身
 * 叫 `2-x.txt` 时会与前面 `x.txt` 的去重前缀撞车（`x.txt`、`x.txt`、`2-x.txt`
 * → `2-x.txt` 出现两次），后写的那份直接覆盖前一份，用户静默丢产物。
 */
export function productFileNames(products: WebApplyProduct[]): string[] {
  const assigned = new Set<string>();
  return products.map((product) => {
    const base = productFileName(product.path);
    let name = base;
    // `productFileName(path, n)` 的第 n 档是 `<n+1>-<基名>`；撞了就继续往上挪，
    // 直到拿到一个没被占用的名字（总数有限，循环必然收敛）。
    for (let index = 1; assigned.has(name); index += 1) {
      name = productFileName(product.path, index);
    }
    assigned.add(name);
    return name;
  });
}

/** 逐个保存产物；同名（不同目录）按序号前缀区分。 */
export function saveProducts(
  products: WebApplyProduct[],
  save: ProductSaver = defaultProductSaver
): number {
  const names = productFileNames(products);
  products.forEach((product, index) => save(names[index], product.blob));
  return products.length;
}

/** 一次执行的条目级摘要（界面显示「成功 N / 失败 M」）。 */
export function summarizeOutcome(outcome: unknown): {
  ok: number;
  failed: number;
  skipped: number;
  total: number;
} {
  const entries =
    (outcome as { report?: { entries?: { status?: string }[] } })?.report?.entries ?? [];
  let ok = 0;
  let failed = 0;
  let skipped = 0;
  for (const entry of entries) {
    if (entry.status === "ok") ok += 1;
    else if (entry.status === "failed") failed += 1;
    else if (entry.status === "skipped") skipped += 1;
  }
  return { ok, failed, skipped, total: entries.length };
}
