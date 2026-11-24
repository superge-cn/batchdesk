/**
 * Web 形态的**计划通道**：把浏览器采到的 `File` 事实接到 WASM `plan_*`。
 *
 * 存在理由：A2 系列把 25 个 `plan_*` 都导出到了 WASM，但浏览器里功能页的
 * `planOperation` 只有两条路——Tauri IPC 或 **mock 假计划**。于是「导出了」与
 * 「有生产消费者」之间缺了一层：本模块就是那一层（A3-1 纵切定形，A3-2 横向铺开）。
 *
 * 边界（设计 05 §0 三条不变）：
 * - 只搬运事实与结果，不做授权/配额判定；
 * - WASM 不可用时**明确失败**（`E5002`），由调用方决定降级——不静默出示假计划；
 * - 不写用户磁盘：事实采集只读 `File`，产物生成属 `apply`（Web 尚无 OPFS 执行器）。
 *
 * ## 为什么有「能力矩阵」
 *
 * 25 个入口不是都能在浏览器里出**正确**计划：C09 要 blake3 全量哈希、F11 要
 * 目标目录已有内容、C07 要解析清单文件。对这些功能，浏览器能算出来的东西
 * **看着像计划、实际是错的**（例如 C09 拿不到哈希就会得出「没有重复」）。于是本模块
 * 显式声明每个视图的档位：`native` 才走 WASM，其余一律拒绝——拒绝比给一份错计划安全。
 */

import type { PlanDocument } from "../ipc/types";
import type { WasmCall, WasmCallResult } from "../wasm/client";
import { KernelError, parsePlanResult, type PlanOp } from "../wasm/contract";
import {
  c01FileFact,
  c02FileFact,
  c03FileFact,
  c08FileFact,
  csvFileFact,
  listingTrees,
  mergeFileFact,
  pickedRelativePath,
  pickedRootName,
  pickedRootOf,
  readAllBytes,
  splitFileFact
} from "./facts";
import { parseArchiveRules } from "./archiveRules";
import { readZipEntriesFromFile } from "./zipIndex";
import {
  isAffirmative,
  isNoHeader,
  nonNegativeInt,
  positiveInt,
  positiveNumber,
  splitList,
  splitNumbers,
  trimmed
} from "./formValues";

/** 头部采样窗口缺省值：与壳层 `read_head`／功能页 `|| 4096` 同口径（非方案 §5.2 数值）。 */
export const DEFAULT_SAMPLE_WINDOW = 4096;

/** WASM 调用入口的可注入形态（测试用假实现，生产用 `createWasmClient().call`）。 */
export type PlanCaller = (request: WasmCall) => Promise<WasmCallResult>;

/**
 * 按**字段名分槽**的浏览器输入。
 *
 * 大多数视图只有一份输入（槽名 `files`）。C07 需要两份互不相干的输入——清单文件
 * 与源目录——单槽 `File[]` 表达不了。**分槽而不是混在一个数组里**：混在一起就只能
 * 靠扩展名猜谁是清单，猜错会拿源目录里的某个 CSV 当清单解析，得出看似正常、实际
 * 全错的计划。
 */
export type PickedFiles = Readonly<Record<string, File[]>>;

/** 取某个字段槽（缺省空数组，调用方据此报「没选」）。 */
function slot(picked: PickedFiles, field: string): File[] {
  return picked[field] ?? [];
}

/** 单槽视图的输入：所有槽扁平化（这些视图本来就只有一份输入）。 */
function allPicked(picked: PickedFiles): File[] {
  return Object.values(picked).flat();
}

/**
 * WASM 运行环境不可用（设计 01 §E5xxx：`E5002` WASM 能力缺失）。
 *
 * 与 `E9001`（未预期错误）区分开：调用方据此走「降级到 mock／提示换桌面版」，
 * 而不是把浏览器能力缺失当成计划失败。
 */
export class WebPlanUnavailable extends Error {
  readonly code = "E5002";

  constructor(detail: string) {
    super(`E5002: ${detail}`);
    this.name = "WebPlanUnavailable";
  }
}

/**
 * 该视图在浏览器里的计划能力档位。
 *
 * - `native`：事实齐备，`planWithFiles` 能出**真计划**；
 * - `unsupported`：浏览器采不到关键事实，**必须拒绝**（`E5001`），不退回假计划。
 */
export type WebPlanLevel = "native" | "unsupported";

/**
 * 不支持的原因（界面按它选文案；不用自然语言，免得 lib 层依赖语言包）。
 *
 * - `directory-walk`：需要目录树（条目枚举、目标占用、现有目标探测）；
 * - `file-identity`：需要**文件系统身份**（卷序列号＋文件 ID）。哈希本身在 WASM 里
 *   能算（`blake3` 已是 wasm 依赖、`plan_c09` 已导出），缺的是身份：浏览器给不出，
 *   `plan_c09` 会把「身份未知」的重复组**整组排除**——直连只会得到一份「0 条待删」
 *   的计划，看着像「没有重复文件」，是假绿；
 * - `pending-wiring`：仅用于**未登记视图**的保守回退（已知视图都已有明确档位）。
 *
 * `archive-compress` 已于 A3-5 移除：C10 压缩侧接上了 `plan_c10_compress`
 * （ADR-0107），缺口枚举不再需要它。
 */
export type WebPlanGap = "directory-walk" | "file-identity" | "pending-wiring";

export interface WebPlanSupport {
  level: WebPlanLevel;
  gap?: WebPlanGap;
}

/** 能出真计划的视图（`Record` 穷尽，新增一个必须同时补实现）。 */
export type WebNativeView =
  | "c01"
  | "c02"
  | "c03"
  | "f01"
  | "f02"
  | "f03"
  | "f04"
  | "f05"
  | "f06"
  | "f07"
  | "f08"
  | "f09"
  | "c11"
  | "c12"
  | "c13"
  | "c14"
  /** C08：用户选**整个目录**时，相对路径/大小/mtime 齐备（见 `planC08`）。 */
  | "c08"
  /** C10 解压：条目事实来自中央目录（ADR-0103），不解压任何内容。 */
  | "c10"
  /**
   * C07：清单文件 + 源目录两份独立输入，清单解析走内核（ADR-0106）。
   *
   * 仍缺的事实：浏览器里没有**目标**文件系统，`targetOccupied` 恒 false——
   * 计划因此少一类「目标已存在」的冲突提示（不影响条目与动作的正确性）。
   */
  | "c07"
  /**
   * C04 名称替换 / F12 批量改名：条目来自**用户选整个目录**时的第一层名字
   * （`webkitdirectory` 的相对路径），与桌面 `read_dir` 同口径——不递归（ADR-0113）。
   *
   * 缺的事实：浏览器的条目只有「名字」，没有 inode/属性；符号链接、系统隐藏项
   * 拿不到（桌面会逐项 `symlink_metadata` 跳过链接）。计划因此少那几条，
   * 但对**被选进来的**文件与目录，改名结果与桌面逐字一致。
   */
  | "c04"
  | "f12"
  /**
   * F13 正则改名：与 C04/F12 同一套条目口径（目录第一层名字），规则是正则＋
   * 替换模板。正则编译与捕获引用校验都在内核计划期完成（`plan_f13`），
   * Web 侧只负责把目录事实送进去。
   */
  | "f13"
  /**
   * F10 目录清单（ADR-0114）：条目＝所选目录的根，产物是一份 UTF-8 BOM CSV。
   *
   * 有别于桌面的事实：浏览器不给**空目录**（`dirs` 只数得到非空目录）、不给目录
   * 修改时间（清单里该列留空）、分不出占位文件与符号链接（两类计数恒 0）。
   */
  | "f10";

/**
 * 浏览器事实不全、必须拒绝的视图。
 *
 * F13 于 2026-11-24 接入（ADR-0117）后从「无视图」变为 native——它的条目口径与
 * C04/F12 相同（`webkitdirectory` 的第一层名字），不需要额外事实。
 */
export type WebUnsupportedView = "c09" | "f11";

const UNSUPPORTED_GAPS: Record<WebUnsupportedView, WebPlanGap> = {
  // F11 的计划只需要 root＋清单文本，缺的是**目标目录已有什么**（冲突判定）；
  // 且执行产物是空目录，浏览器无法交付（下载不了目录）——两处都在计划侧挡下。
  f11: "directory-walk",
  // 归因（2026-11-23 纠正）：此前记作 content-hash，按名联想「重复文件＝要算哈希」。
  // 核对后：哈希能在 WASM 里算（blake3 已是依赖）、`plan_c09` 也已导出；真正的阻塞
  // 是 FileIdentity——`group_duplicates` 把身份未知的成员标 needsManualReview，
  // `plan_c09` 随即**整组排除**，直连会产出一份 0 条目的假绿计划。
  c09: "file-identity"
};

/** 查某视图在浏览器里的计划档位。未知视图按不支持处理（保守）。 */
export function webPlanSupport(view: string): WebPlanSupport {
  if (isNativeView(view)) return { level: "native" };
  if (view in UNSUPPORTED_GAPS) {
    return {
      level: "unsupported",
      gap: UNSUPPORTED_GAPS[view as WebUnsupportedView]
    };
  }
  return { level: "unsupported", gap: "pending-wiring" };
}

function isNativeView(view: string): view is WebNativeView {
  return view in NATIVE_PLANNERS;
}

/**
 * 浏览器里无法出真计划（`E5001`：当前运行环境不支持该操作）。
 *
 * 用 `E5001` 而不是 `E9001`：这是**能力边界**不是故障，界面据此提示「改用桌面版」。
 */
export class WebPlanUnsupported extends Error {
  readonly code = "E5001";
  readonly gap: WebPlanGap;

  constructor(view: string, gap: WebPlanGap) {
    super(`E5001: browser cannot plan ${view} (${gap})`);
    this.name = "WebPlanUnsupported";
    this.gap = gap;
  }
}

async function callPlan(
  caller: PlanCaller,
  op: PlanOp,
  payload: unknown
): Promise<PlanDocument> {
  return parsePlanResult(
    await invoke(caller, { op, payload: JSON.stringify(payload) })
  ) as PlanDocument;
}

/**
 * 调一次 WASM 并还原成功/失败语义（`callPlan` 与字节型入口共用）。
 *
 * 失败分两种：`wasm-unavailable` → `E5002`（能力缺失，调用方降级）；
 * 其余 → `KernelError`，码位以 Worker 回传的为准（设计 06 §3：不带后端原文）。
 */
async function invoke(caller: PlanCaller, request: WasmCall): Promise<string> {
  const result = await caller(request);
  if (result.ok) return result.result;
  if (result.reason === "wasm-unavailable") {
    throw new WebPlanUnavailable(
      "WASM runtime is not available in this environment"
    );
  }
  throw new KernelError(`${result.code ?? "E0000"}: wasm plan call failed`);
}

/** 采样窗口：表单没填就用缺省（各功能口径一致）。 */
function sampleWindow(
  form: Record<string, string>,
  field = "sampleWindow"
): number {
  return positiveInt(form, field) ?? DEFAULT_SAMPLE_WINDOW;
}

/** 列选择：名字列与序号列合并成 `WebColumnPick` 列表（两者可同时给，内核按序处理）。 */
function columnPicks(
  form: Record<string, string>,
  namesField: string,
  indexesField: string
): Array<{ name: string } | { index: number }> {
  return [
    ...splitList(form[namesField]).map((name) => ({ name })),
    ...splitNumbers(form[indexesField]).map((index) => ({ index }))
  ];
}

function assertFiles(files: File[]): void {
  if (files.length === 0) {
    throw new KernelError("E1001: at least one file is required");
  }
}

/**
 * C02 换行转换计划：浏览器 `File` → 事实 → `plan_c02` → 真实计划。
 *
 * 为什么先做 C02：它的计划输入只需「头部样本 + 整段字节」，不依赖目录遍历、
 * 目标占用、平台文件名规则等浏览器采不到的事实——是 25 个入口里**语义最完整**
 * 的一条，适合当纵切样板（与 A1 选 C01 同理）。
 */
export async function planC02WithFiles(
  caller: PlanCaller,
  files: File[],
  params: { target: string; structure?: string; sampleWindow?: number }
): Promise<PlanDocument> {
  assertFiles(files);
  const window =
    params.sampleWindow !== undefined && params.sampleWindow > 0
      ? params.sampleWindow
      : DEFAULT_SAMPLE_WINDOW;
  const facts = await Promise.all(
    files.map((file) => c02FileFact(file, window))
  );
  return callPlan(caller, "plan_c02", {
    target: params.target,
    ...(params.structure ? { structure: params.structure } : {}),
    files: facts
  });
}

// ── C 族：编码类 ────────────────────────────────────────────────────────────

/** C01 转码：样本 + 文件大小即可判定源编码，与桌面同一 kernel 入口。 */
async function planC01(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  assertFiles(files);
  const window = sampleWindow(form);
  const facts = await Promise.all(
    files.map((file) => c01FileFact(file, window))
  );
  const target = trimmed(form, "target");
  if (target === undefined) {
    throw new KernelError("E1002: target encoding is required");
  }
  const invalid = trimmed(form, "invalid");
  const source = trimmed(form, "source");
  return callPlan(caller, "plan_c01", {
    target,
    ...(invalid !== undefined ? { invalid } : {}),
    ...(source !== undefined ? { source } : {}),
    sampleWindow: window,
    files: facts
  });
}

/** C03 查找替换：整段字节 + 只读/锁定事实（Web 侧文件恒为只读引用）。 */
async function planC03(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  assertFiles(files);
  const find = trimmed(form, "find");
  if (find === undefined) {
    throw new KernelError("E1002: find must not be empty");
  }
  const window = sampleWindow(form, "sampleBytes");
  const facts = await Promise.all(
    files.map((file) => c03FileFact(file, window))
  );
  return callPlan(caller, "plan_c03", {
    mode: trimmed(form, "mode") ?? "plain",
    find,
    replacement: form.replacement ?? "",
    sampleBytes: window,
    files: facts
  });
}

// ── 分割类（F01/F02/F03）────────────────────────────────────────────────────

async function planSplit(
  caller: PlanCaller,
  op: "plan_f01" | "plan_f02" | "plan_f03",
  files: File[],
  form: Record<string, string>,
  params: Record<string, unknown>
): Promise<PlanDocument> {
  assertFiles(files);
  const facts = await Promise.all(files.map((file) => splitFileFact(file)));
  return callPlan(caller, op, {
    ...params,
    outputDir: trimmed(form, "outputDir") ?? "",
    files: facts
  });
}

/**
 * F01 按字节分片 / F02 按行分片 / F03 按份数分片。
 *
 * `targetBytes`／`linesPerPart`／`parts` 缺失时**不在这里补默认值**：交给内核报
 * E1002（参数不合法），免得用户看到一份按别的参数算出来的计划。
 */
async function planF01(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
) {
  return planSplit(caller, "plan_f01", files, form, {
    targetBytes: positiveInt(form, "targetBytes") ?? 0
  });
}

async function planF02(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
) {
  return planSplit(caller, "plan_f02", files, form, {
    linesPerPart: positiveInt(form, "linesPerPart") ?? 0
  });
}

async function planF03(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
) {
  return planSplit(caller, "plan_f03", files, form, {
    parts: positiveInt(form, "parts") ?? 0
  });
}

// ── 文本合并（F04）──────────────────────────────────────────────────────────

/**
 * F04 文本合并。
 *
 * **字段交叉映射（易错）**：表单里 `output` 是输出路径、`target` 是**目标编码**；
 * 而线协议里 `target` 是输出路径、`targetEncoding` 是编码。这里逐字段对调，
 * 写反了不会报错——只会把 `/path/merged.txt` 当成编码解析失败，或把 `utf-8`
 * 当输出名。`planChannel.test.ts` 有一条用例专门钉这个映射。
 */
async function planF04(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  assertFiles(files);
  const targetEncoding = trimmed(form, "target");
  if (targetEncoding === undefined) {
    throw new KernelError("E1002: target encoding is required");
  }
  const facts = await Promise.all(files.map((file) => mergeFileFact(file)));
  return callPlan(caller, "plan_f04", {
    target: trimmed(form, "output") ?? "",
    targetEncoding,
    files: facts
  });
}

// ── CSV 族（F05–F09／C11–C14）───────────────────────────────────────────────

/** CSV 事实的统一采集：窗口与表头声明必须与计划参数一致，否则列数会错位。 */
async function csvFacts(
  files: File[],
  form: Record<string, string>,
  hasHeader: boolean,
  windowField = "sampleWindow"
) {
  const window = sampleWindow(form, windowField);
  return Promise.all(
    files.map((file) => csvFileFact(file, window, hasHeader))
  );
}

function csvHeader(form: Record<string, string>): boolean {
  return !isNoHeader(form.header);
}

function requireEncoding(form: Record<string, string>): string {
  const encoding = trimmed(form, "target");
  if (encoding === undefined) {
    throw new KernelError("E1002: target encoding is required");
  }
  return encoding;
}

/** F05 CSV 分片：按行数或字节数二选一（`basis` 只认其一，两者都给时内核报错）。 */
async function planF05(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  assertFiles(files);
  const rows = positiveInt(form, "rows");
  const bytes = positiveInt(form, "targetBytes");
  if (rows === undefined && bytes === undefined) {
    throw new KernelError("E1002: basis needs exactly one of bytes or rows");
  }
  const hasHeader = csvHeader(form);
  const facts = await csvFacts(files, form, hasHeader);
  const delimiter = trimmed(form, "delimiter");
  return callPlan(caller, "plan_f05", {
    basis: rows !== undefined ? { rows } : { bytes },
    keepHeader: hasHeader,
    ...(delimiter !== undefined ? { delimiter } : {}),
    outputDir: trimmed(form, "outputDir") ?? "",
    files: facts
  });
}

/** F06 CSV 合并：`output` → `target`（输出路径），`target` → `targetEncoding`。 */
async function planF06(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  assertFiles(files);
  const targetEncoding = requireEncoding(form);
  const hasHeader = csvHeader(form);
  const facts = await csvFacts(files, form, hasHeader);
  const sourceColumn = trimmed(form, "sourceColumnName");
  const delimiter = trimmed(form, "delimiter");
  return callPlan(caller, "plan_f06", {
    target: trimmed(form, "output") ?? "",
    targetEncoding,
    ...(delimiter !== undefined ? { outputDelimiter: delimiter } : {}),
    addSourceColumn: sourceColumn !== undefined,
    sourceColumnName: sourceColumn ?? "source",
    strict: isAffirmativeValue(form.strict),
    files: facts
  });
}

/** 「是」的宽松取值（表单里 strict/recursive 这类开关）。 */
function isAffirmativeValue(value: string | undefined): boolean {
  const normalized = (value ?? "").trim().toLowerCase();
  return normalized === "true" || normalized === "yes" || normalized === "1";
}

/** F07 CSV 列提取：目标路径 = 表单输出目录 + 源文件名（wire 层派生，ADR-0110）。 */
async function planF07(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  assertFiles(files);
  const selections = columnPicks(form, "columns", "columnIndexes");
  if (selections.length === 0) {
    throw new KernelError("E1002: at least one column is required");
  }
  const targetEncoding = requireEncoding(form);
  const facts = await csvFacts(files, form, csvHeader(form));
  return callPlan(caller, "plan_f07", {
    targetEncoding,
    outputDir: trimmed(form, "outputDir") ?? "",
    selections,
    files: facts
  });
}

/** F08 CSV 行提取：`window` 只认 first/last，其余一律按 first（与 IPC 同口径）。 */
async function planF08(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  assertFiles(files);
  const targetEncoding = requireEncoding(form);
  const facts = await csvFacts(files, form, csvHeader(form));
  return callPlan(caller, "plan_f08", {
    targetEncoding,
    outputDir: trimmed(form, "outputDir") ?? "",
    window: trimmed(form, "window") === "last" ? "last" : "first",
    count: nonNegativeInt(form, "count") ?? 0,
    files: facts
  });
}

/** F09 CSV 区间读取：行号 1-based，缺省 0 交给内核判定。 */
async function planF09(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  assertFiles(files);
  const targetEncoding = requireEncoding(form);
  const facts = await csvFacts(files, form, csvHeader(form));
  return callPlan(caller, "plan_f09", {
    targetEncoding,
    outputDir: trimmed(form, "outputDir") ?? "",
    startRow: nonNegativeInt(form, "startRow") ?? 0,
    endRow: nonNegativeInt(form, "endRow") ?? 0,
    files: facts
  });
}

/** C11 CSV 去重：键列可同时给名字与序号。 */
async function planC11(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  assertFiles(files);
  const target = requireEncoding(form);
  const key = columnPicks(form, "keyColumns", "keyColumnIndexes");
  if (key.length === 0) {
    throw new KernelError("E1002: at least one key column is required");
  }
  const facts = await csvFacts(files, form, csvHeader(form));
  return callPlan(caller, "plan_c11", {
    target,
    outputDir: trimmed(form, "outputDir") ?? "",
    key,
    files: facts
  });
}

/**
 * C12 CSV 过滤：条件列与比较值。
 *
 * `filterMode` 是「保留命中的行」还是「丢弃命中的行」；表单里的 `drop` 才反转，
 * 其余（含未填）按 `keep`——与 IPC 侧 `form.filterMode === "drop" ? "drop" : "keep"` 同口径。
 */
async function planC12(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  assertFiles(files);
  const target = requireEncoding(form);
  const columnName = trimmed(form, "column");
  const columnIndex = positiveInt(form, "columnIndex");
  if (columnName === undefined && columnIndex === undefined) {
    throw new KernelError("E1002: condition column is required");
  }
  const facts = await csvFacts(files, form, csvHeader(form));
  return callPlan(caller, "plan_c12", {
    target,
    outputDir: trimmed(form, "outputDir") ?? "",
    modeKeep: trimmed(form, "filterMode") !== "drop",
    conditionColumn:
      columnName !== undefined ? { name: columnName } : { index: columnIndex },
    conditionOp: trimmed(form, "op") ?? "contains",
    conditionValue: form.value ?? "",
    files: facts
  });
}

/** C13 CSV 排序：键列 + 升降序 + 可选外排序分块行数。 */
async function planC13(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  assertFiles(files);
  const targetEncoding = requireEncoding(form);
  const keys = columnPicks(form, "sortColumns", "sortColumnIndexes");
  if (keys.length === 0) {
    throw new KernelError("E1002: at least one sort key is required");
  }
  const externalChunkRows = positiveInt(form, "externalChunkRows");
  const facts = await csvFacts(files, form, csvHeader(form));
  return callPlan(caller, "plan_c13", {
    keys,
    outputDir: trimmed(form, "outputDir") ?? "",
    order: trimmed(form, "order") === "desc" ? "desc" : "asc",
    ...(externalChunkRows !== undefined ? { externalChunkRows } : {}),
    targetEncoding,
    files: facts
  });
}

/**
 * C14 CSV 列拆分/合并：`mode` 由**填了哪组字段**决定，不让用户手选
 * ——手选会出现「选了 split 却只填了合并列」的空计划。
 */
async function planC14(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  assertFiles(files);
  const targetEncoding = requireEncoding(form);
  const splitName = trimmed(form, "splitColumn");
  const splitIndex = positiveInt(form, "splitColumnIndex");
  const mergePicks = columnPicks(form, "mergeColumns", "mergeColumnIndexes");
  const isSplit = splitName !== undefined || splitIndex !== undefined;
  if (!isSplit && mergePicks.length === 0) {
    throw new KernelError("E1002: c14 needs split source or merge columns");
  }
  const facts = await csvFacts(files, form, csvHeader(form));
  const separator = trimmed(form, "splitSeparator");
  const regex = trimmed(form, "splitRegex");
  return callPlan(caller, "plan_c14", {
    targetEncoding,
    outputDir: trimmed(form, "outputDir") ?? "",
    mode: isSplit ? "split" : "merge",
    ...(isSplit
      ? {
          split: {
            source:
              splitName !== undefined ? { name: splitName } : { index: splitIndex },
            ...(separator !== undefined ? { separator } : {}),
            ...(regex !== undefined ? { regex } : {})
          }
        }
      : {}),
    mergePicks,
    joiner: form.mergeJoinWith ?? "",
    ...(trimmed(form, "mergeName") ? { targetName: trimmed(form, "mergeName") } : {}),
    files: facts
  });
}

// ── 清单同步（C07）────────────────────────────────────────────────────────

/**
 * C07 清单驱动移动/复制：**两份独立输入** → 内核解析清单 → 逐行事实 → `plan_c07`。
 *
 * 三处关键：
 * - 清单解析走 WASM 的 `parse_c07_manifest`（内核 `c07::parse_manifest`）：编码检测、
 *   显式编码、分隔符嗅探、引号内换行、两列校验全在核心里，与 CLI/桌面同一实现；
 * - 逐行事实按**相对路径**精确匹配（`pickedRelativePath` 已剥掉被选目录名），与桌面
 *   `source_root.join(name)` 同口径。没命中＝不存在，交给 `missing` 策略，
 *   不拿文件名模糊匹配凑数（会静默匹配到同名不同目录的文件）；
 * - `targetOccupied` 恒 false：浏览器没有目标文件系统可查（`plan_c07` 因此少一类
 *   「目标已存在」冲突提示）。这是能力边界，不是「目标一定空闲」的断言。
 */
async function planC07(
  caller: PlanCaller,
  picked: PickedFiles,
  form: Record<string, string>
): Promise<PlanDocument> {
  const manifests = slot(picked, "manifest");
  if (manifests.length !== 1) {
    throw new KernelError("E1001: c07 takes exactly one manifest file");
  }
  const sources = slot(picked, "sourceRoot");
  if (sources.length === 0) {
    throw new KernelError("E1001: c07 needs the source directory pick");
  }
  // 一次只认一个源根：两个目录的相对路径会互相覆盖（与 C08 同理由）。
  const root = pickedRootName(sources);
  if (sources.some((file) => pickedRootOf(file) !== root)) {
    throw new KernelError("E1001: c07 takes one source directory at a time");
  }
  // 目标根必填（与 C08/C10 同口径：不替用户补一个「/out」式的默认值——计划里
  // 的每个目标路径都要落在这个根下，补默认等于替用户决定产物去哪）。
  const targetRoot = trimmed(form, "targetRoot");
  if (targetRoot === undefined) {
    throw new KernelError("E1001: c07 needs the target root");
  }
  const rows = await parseManifest(
    caller,
    await readAllBytes(manifests[0]),
    trimmed(form, "delimiter"),
    trimmed(form, "source")
  );
  // 相对路径 → 文件。`recursive` 之类的开关不参与：清单点名了谁就只看谁。
  const byPath = new Map<string, File>();
  for (const file of sources) {
    const relative = pickedRelativePath(file);
    if (relative !== "") byPath.set(relative, file);
  }
  const facts = rows.map((row) => {
    const hit = byPath.get(row.name);
    return { exists: hit !== undefined, size: hit?.size ?? 0 };
  });
  return callPlan(caller, "plan_c07", {
    operation: (trimmed(form, "operation") ?? "move").toLowerCase(),
    missing: (trimmed(form, "missing") ?? "skip").toLowerCase(),
    // 计划里的源/目标是**展示路径**，不是访问路径（浏览器不写用户磁盘）；
    // 缺省用所选目录名，让计划读起来是用户的目录而不是空白。
    sourceRoot: trimmed(form, "sourceRoot") ?? root ?? "",
    targetRoot,
    rows,
    facts
  });
}

/**
 * 目录选择的**第一层条目**（与桌面 `read_dir` 同口径：不递归）。
 *
 * 浏览器的目录选择只给「文件 + 相对路径」，所以：
 * - 根层文件 → `根名/文件名`；
 * - 子目录 → 取相对路径的第一段（`sub/b.txt` 贡献 `sub`），去重；
 * - 更深层的文件**不进来**：桌面的 C04/F12 只改一层，递归会让两侧结果不同。
 *
 * 空子目录拿不到（里面没有文件就没有相对路径）——这是 `webkitdirectory` 的能力边界，
 * 计划里因此少那几条；界面在提示里说明。
 */
function dirEntries(files: File[]): { path: string }[] {
  if (files.length === 0) {
    throw new KernelError("E1001: this view needs a directory pick");
  }
  const root = pickedRootName(files);
  if (root === undefined) {
    throw new KernelError(
      "E1001: this view needs a directory pick (relative paths are unavailable)"
    );
  }
  if (files.some((file) => pickedRootOf(file) !== root)) {
    throw new KernelError("E1001: this view takes one directory at a time");
  }
  const names = new Set<string>();
  for (const file of files) {
    const relative = pickedRelativePath(file);
    if (relative === "") continue;
    const at = relative.indexOf("/");
    names.add(at < 0 ? relative : relative.slice(0, at));
  }
  return [...names].sort().map((name) => ({ path: `${root}/${name}` }));
}

/**
 * 改名作用域（三种口径共用）：界面留空＝`full`，非法值在调 WASM 前拒绝。
 *
 * `style` 是线协议形态而不是口味：C04/F13 的规则里是字符串（内核自己 `parse`），
 * F12 的规则直接反序列化成 `RenameScope` 枚举（`Full`/`Stem`/`Ext`）。
 */
function renameScope(raw: string | undefined, style: "lower" | "pascal"): string {
  const value = (raw ?? "").trim().toLowerCase();
  const scope = value === "" ? "full" : value;
  if (scope !== "full" && scope !== "stem" && scope !== "ext") {
    throw new KernelError(`E1002: unknown rename scope: ${scope}`);
  }
  return style === "pascal" ? scope.charAt(0).toUpperCase() + scope.slice(1) : scope;
}

/**
 * 生成 26 字符 ULID（时间 48 bit ＋ 随机 80 bit，Crockford base32）。
 *
 * F12 的 Web 输入要求 `planId` 由调用方给（其余入口在 WASM 内生成）；格式与
 * `kernel::plan::is_ulid` 同口径：首字符 ≤ `7`，字符集不含 I/L/O/U。
 */
function newPlanId(): string {
  const alphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  const random = new Uint8Array(10);
  globalThis.crypto.getRandomValues(random);
  let value = BigInt(Date.now());
  for (const byte of random) value = (value << 8n) | BigInt(byte);
  let id = "";
  for (let index = 0; index < 26; index++) {
    id = alphabet[Number(value & 31n)] + id;
    value >>= 5n;
  }
  return id;
}

/** C04 名称替换：条目＝目录第一层名字，规则＝查找/替换（ADR-0113）。 */
async function planC04(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  const find = trimmed(form, "find");
  if (find === undefined) {
    throw new KernelError("E1001: c04 needs the find text");
  }
  return callPlan(caller, "plan_c04", {
    items: dirEntries(files),
    rule: {
      mode: (trimmed(form, "mode") ?? "plain").toLowerCase(),
      find,
      replace: form.replace ?? "",
      scope: renameScope(form.scope, "lower")
    }
  });
}

/**
 * F12 批量改名。
 *
 * 与 C04 同一套条目口径（目录第一层），差别只在规则：前缀/后缀/查找替换三件套，
 * 且 `scope` 走 PascalCase（见 `renameScope`）。
 */
async function planF12(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  const prefix = trimmed(form, "prefix");
  const suffix = trimmed(form, "suffix");
  const find = trimmed(form, "find");
  return callPlan(caller, "plan_f12", {
    planId: newPlanId(),
    items: dirEntries(files),
    // 浏览器没有目标文件系统可探测：这两份事实只能是空。沙箱执行前本来就是空的，
    // 所以「计划不报目标冲突」与执行环境一致——不是弱化，是如实。
    existingTargets: [],
    protectedDirs: [],
    caseSense: "sensitive",
    longPathSupported: true,
    accessFacts: [],
    rule: {
      ...(prefix !== undefined ? { prefix } : {}),
      ...(suffix !== undefined ? { suffix } : {}),
      ...(find !== undefined ? { find } : {}),
      replace: form.replace ?? "",
      scope: renameScope(form.scope, "pascal")
    }
  });
}

/**
 * F13 正则改名。
 *
 * 与 C04/F12 同一套条目口径（目录第一层），差别是规则形态：内核收 `pattern`／
 * `replacement`／`scope`，正则编译与捕获引用校验在计划期完成——非法正则不会
 * 走到执行。`scope` 在这里是小写字符串（与 C04 同），F12 那个入口收 PascalCase
 * 枚举，`renameScope` 负责翻译。
 */
async function planF13(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  const pattern = trimmed(form, "pattern");
  if (pattern === undefined) {
    throw new KernelError("E1001: f13 needs the regex pattern");
  }
  return callPlan(caller, "plan_f13", {
    items: dirEntries(files),
    // 与 planF12 同一处置：浏览器没有目标文件系统，这两份事实只能是空。
    existingTargets: [],
    protectedDirs: [],
    caseSense: "sensitive",
    longPathSupported: true,
    accessFacts: [],
    rule: {
      pattern,
      replacement: form.replace ?? "",
      scope: renameScope(form.scope, "lower")
    }
  });
}

/**
 * 清单解析（字节型入口）：返回内核解析好的两列行。
 *
 * 不走 `callPlan`：那条路只装 JSON，而这里必须把**原始字节**交给内核做编码检测
 * （`Blob.text()` 恒按 UTF-8 解，GB18030 清单会先乱码）。
 */
async function parseManifest(
  caller: PlanCaller,
  bytes: Uint8Array,
  delimiter: string | undefined,
  source: string | undefined
): Promise<Array<{ name: string; subDir: string }>> {
  const raw = await invoke(caller, {
    op: "parse_c07_manifest",
    payload: JSON.stringify({ delimiter, source }),
    bytes
  });
  return JSON.parse(raw) as Array<{ name: string; subDir: string }>;
}

// ── 归档（C08）与解压（C10）──────────────────────────────────────────────────

/**
 * C08 按规则归档。
 *
 * **相对路径从哪来**：桌面是 `collect_tree_files(source_root)` 扫目录扫出来的；
 * 浏览器没有目录枚举 API，只有**用户主动选择整个目录**时（`webkitdirectory`）
 * 才拿得到 `webkitRelativePath`。因此：
 * - 目录选择 → 每个文件的相对路径齐备，计划与桌面同口径；
 * - 普通多选 → 相对路径为空，此时**拒绝**（`E1001`）而不是拿扁平文件名顶上：
 *   那会把「子目录里的 a.txt」当成「根目录下的 a.txt」，扩展名规则照样命中、
 *   产物路径却是错的。
 *
 * `sourceRoot` 在 Web 里只是计划里的字符串（不扫描、不落盘），缺省取所选目录名；
 * `targetRoot` 与桌面一样必须由用户填（计划要把目标路径写出来）。
 */
async function planC08(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  assertFiles(files);
  const rules = parseArchiveRules(form.rules ?? "");
  if (rules.length === 0) {
    throw new KernelError("E1001: c08 needs at least one rule");
  }
  const recursive = isAffirmative(form.recursive);
  const facts = await Promise.all(
    files.map(async (file) => {
      const relPath = pickedRelativePath(file);
      if (relPath === "") {
        throw new KernelError(
          "E1001: c08 needs a directory pick (relative paths are unavailable for plain multi-select)"
        );
      }
      return c08FileFact(file, relPath);
    })
  );
  // `recursive=no` 与桌面同口径：只归档源根下的文件（`rel_path` 不含分隔符）。
  const kept = recursive
    ? facts
    : facts.filter((fact) => !fact.relPath.includes("/"));
  // 一次只归档**一个**源根：选了多个目录时相对路径会互相覆盖，
  // 计划看起来正常、文件归属却是错的（浏览器允许多选目录，必须在这里挡住）。
  const root = pickedRootName(files);
  if (files.some((file) => pickedRootOf(file) !== root)) {
    throw new KernelError("E1001: c08 takes one directory at a time");
  }
  const unclassified = trimmed(form, "unclassifiedDir");
  return callPlan(caller, "plan_c08", {
    sourceRoot: trimmed(form, "sourceRoot") ?? root ?? "",
    targetRoot: trimmed(form, "targetRoot") ?? "",
    rules,
    unmatched: unclassified !== undefined ? `move:${unclassified}` : "keep",
    files: kept
  });
}

/**
 * C10 ZIP 解压计划：条目事实来自**中央目录**（ADR-0103 的 `zipIndex.ts`）。
 *
 * 三处必须显式：
 * - 只解压侧可算——`mode=compress` 走**压缩**入口（`plan_c10_compress` 未导出，
 *   且浏览器没有「本地目录打包」的事实源），如实拒绝（`E5001`）而不是当成解压；
 * - `maxTotalBytes`/`maxRatio` **必填**：两者是用户可见的解压量上限，替用户补
 *   默认值等于放行未知解压量（与 CLI/桌面同口径）；
 * - 密码**不进请求**，只传「执行期会不会提供」这一位事实（ADR-0074）。
 *
 * `existingTargets` 不传：Web 侧 OPFS 目录列举尚未实现，计划期不预判「目标已存在」，
 * 这是已记入 TRACEABILITY 的能力边界（不会因此产出错误结论，只是少一类冲突提示）。
 */
/**
 * C10 双向：`mode=compress` 走压缩计划，其余走解压计划。
 *
 * 两侧的**输入槽不同**（`sources` 是被压缩的文件、`archive` 是待解压的包），
 * 所以本函数直接收分槽的 `PickedFiles`，不经过 `viaFiles` 摊平——摊平后两份输入
 * 会混成一个数组，只能靠扩展名猜谁是包，猜错就解错对象。
 */
async function planC10(
  caller: PlanCaller,
  picked: PickedFiles,
  form: Record<string, string>
): Promise<PlanDocument> {
  if ((trimmed(form, "mode") ?? "").toLowerCase() === "compress") {
    return planC10Compress(caller, slot(picked, "sources"), form);
  }
  const archives = slot(picked, "archive");
  assertFiles(archives);
  if (archives.length !== 1) {
    throw new KernelError("E1001: c10 takes exactly one archive");
  }
  const archive = archives[0];
  const entries = await readZipEntriesFromFile(archive);
  const maxTotalBytes = positiveInt(form, "maxTotalBytes");
  const maxRatio = positiveNumber(form, "maxRatio");
  if (maxTotalBytes === undefined || maxRatio === undefined) {
    throw new KernelError("E1001: maxTotalBytes and maxRatio are required");
  }
  return callPlan(caller, "plan_c10", {
    archive: trimmed(form, "archive") ?? archive.name,
    archiveSize: archive.size,
    entries,
    outputDir: trimmed(form, "outputDir") ?? "",
    maxTotalBytes,
    maxRatio,
    passwordProvided: (form.password ?? "").length > 0
  });
}

/**
 * C10 压缩计划：**只出计划，不打包**。
 *
 * 事实来源是用户选中的文件（相对路径＋逐个 size），不是目录树——浏览器拿不到
 * 目录里还有什么，所以「整目录压缩」在 Web 不在能力内（UI 用多文件选择器）。
 *
 * 边界：`targetExists` 恒 false（没有产物文件系统可查），计划因此不预判
 * 「目标已存在」；`sourceFileCount`/`sourceBytes` 是**必填**事实，不由内核默认。
 */
async function planC10Compress(
  caller: PlanCaller,
  sources: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  assertFiles(sources);
  const output = trimmed(form, "output");
  if (output === undefined) {
    throw new KernelError("E1001: c10 compression needs the output archive path");
  }
  const paths = sources.map((source) => pickedRelativePath(source) || source.name);
  return callPlan(caller, "plan_c10_compress", {
    output,
    sources: paths,
    // 与 CLI 缺省一致；未知值交给内核报 E1002（这里不替用户改写成 deflate）。
    compression: (trimmed(form, "compression") ?? "deflate").toLowerCase(),
    sourceFileCount: sources.length,
    sourceBytes: sources.reduce((total, source) => total + source.size, 0),
    targetExists: false
  });
}

/**
 * F10 目录清单（ADR-0114）：每个所选目录一条 `listing` 条目，产物是一份 CSV。
 *
 * 摘要事实由 `listingTrees` 从**文件路径**推出（浏览器不给空目录，见该函数的边界）；
 * `targetOccupied` 恒 false —— 沙箱里本来没有这份 CSV，不是把计划打折。
 */
async function planF10(
  caller: PlanCaller,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  assertFiles(files);
  const trees = listingTrees(files);
  const outputDir = trimmed(form, "outputDir");
  const dir = outputDir === undefined ? "" : outputDir.replace(/[\\/]+$/, "");
  return callPlan(caller, "plan_f10", {
    roots: trees.map((tree) => ({
      // 计划里的源＝根名（沙箱键 `/<根名>`）；target 落在输出目录下，
      // 与 CLI 的 `--output-dir` 同命名（`<root>.csv`）。
      path: tree.root,
      target: dir === "" ? `${tree.root}.csv` : `${dir}/${tree.root}.csv`,
      files: tree.files,
      dirs: tree.dirs,
      maxDepth: tree.maxDepth
    }))
  });
}

/**
 * 视图 → 计划函数。`Record<WebNativeView, ...>` 保证「声明为 native 却没实现」
 * 编译不过（与 `PLAN_OPS` 的覆盖断言同一手法）。
 *
 * 入参是**分槽**的 `PickedFiles`：只有 C07 用多槽（`manifest` + `sourceRoot`），
 * 其余视图只有一份输入，用 `viaFiles` 摊平回 `File[]`，planner 签名保持简单。
 */
const NATIVE_PLANNERS: Record<
  WebNativeView,
  (
    caller: PlanCaller,
    picked: PickedFiles,
    form: Record<string, string>
  ) => Promise<PlanDocument>
> = {
  c01: viaFiles(planC01),
  c02: viaFiles((caller, files, form) => {
    const window = positiveInt(form, "sampleBytes");
    return planC02WithFiles(caller, files, {
      target: trimmed(form, "target") ?? "",
      ...(trimmed(form, "structure") ? { structure: trimmed(form, "structure") } : {}),
      ...(window !== undefined ? { sampleWindow: window } : {})
    });
  }),
  c03: viaFiles(planC03),
  f01: viaFiles(planF01),
  f02: viaFiles(planF02),
  f03: viaFiles(planF03),
  f04: viaFiles(planF04),
  f05: viaFiles(planF05),
  f06: viaFiles(planF06),
  f07: viaFiles(planF07),
  f08: viaFiles(planF08),
  f09: viaFiles(planF09),
  c11: viaFiles(planC11),
  c12: viaFiles(planC12),
  c13: viaFiles(planC13),
  c14: viaFiles(planC14),
  c07: planC07,
  c08: viaFiles(planC08),
  c10: planC10,
  c04: viaFiles(planC04),
  f12: viaFiles(planF12),
  f13: viaFiles(planF13),
  f10: viaFiles(planF10)
};

/**
 * 单槽视图的适配：把分槽输入摊平回 `File[]`。
 *
 * 这些视图本来就只有一份输入，摊平是安全且等价的（槽名由功能页决定，
 * 现行为 `files`）。
 */
function viaFiles(
  planner: (
    caller: PlanCaller,
    files: File[],
    form: Record<string, string>
  ) => Promise<PlanDocument>
) {
  return (caller: PlanCaller, picked: PickedFiles, form: Record<string, string>) =>
    planner(caller, allPicked(picked), form);
}

/**
 * 单输入视图的槽名（与 `FeaturePages` 的 `FieldSpec.name` 对齐）。
 *
 * 大多数视图是 `files`；C10 的**解压**侧输入叫 `archive`、**压缩**侧叫 `sources`
 * ——两个槽名都不是 `files`，写错会得到「至少需要一个文件」这种与用户操作无关的
 * 报错。C07 有**两份互相独立**的输入，单槽入口表达不了，直接拒绝。
 */
function singleSlot(view: string, form: Record<string, string>): string {
  if (view === "c07") {
    throw new KernelError(
      "E1001: c07 needs two separate inputs; use planWithPickedInputs"
    );
  }
  if (view === "c10") {
    return (trimmed(form, "mode") ?? "").toLowerCase() === "compress"
      ? "sources"
      : "archive";
  }
  return "files";
}

/**
 * 浏览器里的**统一计划入口**：`File[] + 表单` → WASM 真计划。
 *
 * 单输入便捷形态，等价于 `planWithPickedInputs(caller, view, { <该视图的槽>: files }, form)`。
 * 多槽视图（C07）必须用 `planWithPickedInputs`。
 */
export async function planWithFiles(
  caller: PlanCaller,
  view: string,
  files: File[],
  form: Record<string, string>
): Promise<PlanDocument> {
  return planWithPickedInputs(caller, view, { [singleSlot(view, form)]: files }, form);
}

/**
 * 浏览器里的**统一计划入口**（分槽形态）：按字段名分槽的输入 → WASM 真计划。
 *
 * 调用方（功能页）只需判断两件事：是不是浏览器形态、有没有选输入；
 * 至于「这个视图能不能算」由 `webPlanSupport` 回答，不能算就抛 `E5001`
 * ——**绝不**在这里悄悄退回 mock。
 */
export async function planWithPickedInputs(
  caller: PlanCaller,
  view: string,
  picked: PickedFiles,
  form: Record<string, string>
): Promise<PlanDocument> {
  const support = webPlanSupport(view);
  if (support.level !== "native") {
    throw new WebPlanUnsupported(view, support.gap ?? "pending-wiring");
  }
  // `webPlanSupport` 已经用 `view in NATIVE_PLANNERS` 判过，这里的收窄是安全的。
  return NATIVE_PLANNERS[view as WebNativeView](caller, picked, form);
}

