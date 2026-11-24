/**
 * Web 形态 WASM 契约（ADR-0098／ADR-0099、设计 05 §2）。
 *
 * 三条边界（设计 05 §0）：
 * 1. **Web 版强制力只在云端服务端**——本地模式对授权/配额零强制力；
 * 2. `web/` 不含判定逻辑——只按内核/服务端已给的结果做显示；
 * 3. **WASM 公开导出面不含授权/配额判定入口**（由 `crates/wasm` 的导出面断言把关）。
 *
 * 因此本模块只做「调用 + 结果搬运」，**不做任何能力/配额判断**。
 */

/** 能力事实（`browser_capabilities` 的返回形状）。 */
export interface BrowserCapabilities {
  form: string;
  localMode: boolean;
  exportVersion: string;
}

/** WASM 导出的加载接口（与 `crates/wasm/src/lib.rs` 一一对应）。 */
export interface BatchDeskWasm {
  /** 环境能力事实；不含授权/配额字段。 */
  browser_capabilities(): string;
  /** C01 转码计划（只读）。入参/出参均为 JSON 字符串。 */
  plan_c01(json: string): string;
  /** C02 换行转换计划（只读，ADR-0102 A2-2）。 */
  plan_c02(json: string): string;
  /** C03 查找替换计划（只读，ADR-0102 A2-2）。 */
  plan_c03(json: string): string;
  /** C11 CSV 去重（只读，ADR-0102 A2-3）。 */
  plan_c11(json: string): string;
  /** C12 CSV 过滤（只读，ADR-0102 A2-3）。 */
  plan_c12(json: string): string;
  /** F06 CSV 合并（只读，ADR-0102 A2-3）。 */
  plan_f06(json: string): string;
  /** F07 CSV 列提取（只读，ADR-0102 A2-3）。 */
  plan_f07(json: string): string;
  /** C14 CSV 列拆分（只读，ADR-0102 A2-3 收尾）。 */
  plan_c14(json: string): string;
  /** F08 CSV 行提取（只读，ADR-0102 A2-3）。 */
  plan_f08(json: string): string;
  /** F09 CSV 区间读取（只读，ADR-0102 A2-3 收尾）。 */
  plan_f09(json: string): string;
  /** F01 按字节分片（只读，ADR-0102 A2-4）。 */
  plan_f01(json: string): string;
  /** F02 按行分片（只读，ADR-0102 A2-4）。 */
  plan_f02(json: string): string;
  /** F03 按份数分片（只读，ADR-0102 A2-4）。 */
  plan_f03(json: string): string;
  /** F05 CSV 分片（只读，ADR-0102 A2-4）。 */
  plan_f05(json: string): string;
  /** C07 清单同步（只读，ADR-0102 A2-5）。 */
  plan_c07(json: string): string;
  /** C08 归档（只读，ADR-0102 A2-5）。 */
  plan_c08(json: string): string;
  /** F10 目录清单（只读，ADR-0102 A2-5）。 */
  plan_f10(json: string): string;
  /** F11 批量建目录（只读，ADR-0102 A2-5）。 */
  plan_f11(json: string): string;
  /** F13 正则改名（只读，ADR-0102 A2-5）。 */
  plan_f13(json: string): string;
  /** C04 名称替换（只读，ADR-0102 A2-6）。 */
  plan_c04(json: string): string;
  /** C09 重复文件清理（只读，ADR-0102 A2-6）。 */
  plan_c09(json: string): string;
  /** C13 CSV 排序（只读，ADR-0102 A2-6）。 */
  plan_c13(json: string): string;
  /** F04 文本合并（只读，ADR-0102 A2-6）。 */
  plan_f04(json: string): string;
  /** C10 ZIP 解压（只读，ADR-0102 A2-7／ADR-0103）。 */
  plan_c10(json: string): string;
  /** C10 ZIP 压缩（只读，ADR-0107 A3-5）：只出计划，不打包。 */
  plan_c10_compress(json: string): string;
  /** F12 改名计划（只读）。入参/出参均为 JSON 字符串。 */
  plan_f12(json: string): string;
  /** 沙箱登记输入（ADR-0108）：清单为 JSON，字节单独传（结构化克隆）。 */
  web_exec_stage(manifest: string, data: Uint8Array): string;
  /** 沙箱执行计划（会改变沙箱状态；请求 JSON = `{plan, options?, now?}`）。 */
  web_exec_apply(request: string): string;
  /** 只登记目录（F10 的清单根；`manifest` = `[{path}]`，无字节）。 */
  web_exec_stage_dirs(manifest: string): string;
  /** 登记 F10 的目录事实（`[{root, files:[{path,size,mtime}]}]`）。 */
  web_exec_stage_listing(json: string): string;
  /** 沙箱撤销（只回滚沙箱内产物）。 */
  web_exec_undo(planId: string): string;
  /** 读产物字节。 */
  web_exec_product(path: string): Uint8Array;
  /** 清空沙箱。 */
  web_exec_reset(): string;
  /**
   * C07 清单解析（只读，ADR-0106）：**原始字节** + 编码/分隔符提示 → 行数组 JSON。
   *
   * 为什么走字节而不是 JSON：编码检测必须在原始字节上做。`Blob.text()` 恒按
   * UTF-8 解，GB18030 清单会先乱码再检测——那正是要避免的。
   */
  parse_c07_manifest(data: Uint8Array, delimiter?: string, source?: string): string;
}

/**
 * 全部处理入口的名字——**唯一清单**。
 *
 * `worker.ts` 按名字动态分发，`client.ts` 的调用联合类型从这里派生，
 * 不再「新增一个导出要改 Rust、worker、client 三处」。
 *
 * 与 `crates/wasm/src/lib.rs` 的 `#[wasm_bindgen] pub fn plan_*` 必须**精确相等**：
 * 漏登记由 `PLAN_OPS_COVERAGE`（编译期）与 `contract.test.ts` 读 Rust 源码对拍
 * （运行期）双重把关。
 */
export const PLAN_OPS = [
  "plan_c01",
  "plan_c02",
  "plan_c03",
  "plan_c11",
  "plan_c12",
  "plan_f06",
  "plan_f07",
  "plan_f08",
  "plan_f12",
  "plan_c14",
  "plan_f09",
  "plan_f01",
  "plan_f02",
  "plan_f03",
  "plan_f05",
  "plan_c07",
  "plan_c08",
  "plan_f10",
  "plan_f11",
  "plan_f13",
  "plan_c04",
  "plan_c09",
  "plan_c13",
  "plan_f04",
  "plan_c10"
] as const;

/**
 * **同视图的第二个**计划入口（不属于「一功能一入口」的 25 项）。
 *
 * C10 的压缩与解压是两条完全不同的计划路径（`mode=extract|compress`，ADR-0080），
 * 各自有独立的入参事实：解压读中央目录、压缩读用户选中的文件。把它们硬塞进
 * `PLAN_OPS` 会让「25 个功能入口」的计数失真，所以单列——但**仍与 `PLAN_OPS`
 * 一起**参与导出面覆盖断言（漏登记同样编译不过）。
 */
export const EXTRA_PLAN_OPS = ["plan_c10_compress"] as const;

export type ExtraPlanOp = (typeof EXTRA_PLAN_OPS)[number];

export type PlanOp = (typeof PLAN_OPS)[number] | ExtraPlanOp;

/**
 * **字节型**处理入口（与 `PLAN_OPS` 分开登记）。
 *
 * 这类型入口的输入是原始字节，JSON 字符串装不下（若转 base64 或数字数组，
 * 要么引入编解码依赖、要么体积翻数倍，且都要在两侧各写一份）。所以协议上单列一
 * 支：`payload` 仍放 JSON 参数，字节走结构化克隆的 `bytes`。
 */
export const BYTE_OPS = ["parse_c07_manifest"] as const;

export type ByteOp = (typeof BYTE_OPS)[number];

/**
 * **执行面**入口（与只读 `PLAN_OPS` 分开登记，ADR-0108）。
 *
 * 这些入口会改变 Worker 内的会话沙箱：登记输入、执行计划、撤销、清空。
 * `apply`/`undo` 属设计 05 §2 允许的处理入口（不是授权/配额判定），产物只以
 * 字节回传、由主线程下载——浏览器始终不写用户磁盘。
 */
export const EXEC_OPS = [
  "stage_inputs",
  "stage_dirs",
  "stage_listing",
  "apply_plan",
  "undo_plan",
  "reset_sandbox"
] as const;

export type ExecOp = (typeof EXEC_OPS)[number];

/**
 * 执行入口 → Rust 绑定名（唯一映射；Worker 的 dispatch 与导出面断言都读这里）。
 *
 * `web_exec_product` 不在表里：它不是一次「操作」，而是读某个产物字节的取值器，
 * 由 Worker 在 `apply_plan` 的响应里顺带调用。
 */
export const EXEC_BINDINGS: Record<ExecOp, string> = {
  stage_inputs: "web_exec_stage",
  stage_dirs: "web_exec_stage_dirs",
  stage_listing: "web_exec_stage_listing",
  apply_plan: "web_exec_apply",
  undo_plan: "web_exec_undo",
  reset_sandbox: "web_exec_reset"
};

type MissingPlanOp = Exclude<
  Exclude<
    keyof BatchDeskWasm,
    | "browser_capabilities"
    | ByteOp
    | "web_exec_stage"
    | "web_exec_stage_dirs"
    | "web_exec_stage_listing"
    | "web_exec_apply"
    | "web_exec_undo"
    | "web_exec_product"
    | "web_exec_reset"
  >,
  PlanOp
>;

/** 编译期断言：`PLAN_OPS` 覆盖接口里的每个处理入口；漏一个则本常量类型不成立。 */
export type PlanOpsCoverage = MissingPlanOp extends never ? true : ["missing", MissingPlanOp];
export const PLAN_OPS_COVERAGE: PlanOpsCoverage = true;

/**
 * 按名字取处理入口（`worker.ts` 分发用）。只接受 `plan_` 前缀：
 * `browser_capabilities` 是无入参的能力查询，不走这条通路。
 *
 * 用动态查表而不是 `switch`：新增导出只要在 Rust 与 `PLAN_OPS` 登记就自动可达，
 * 不会再出现「导出了但 Web 侧调不到」的第三种状态。
 */
export function planHandler(
  surface: object,
  op: string
): ((json: string) => string) | undefined {
  if (!op.startsWith("plan_")) return undefined;
  // 只认自有属性：动态查表若走原型链，`constructor` 一类名字会被当成入口。
  if (!Object.prototype.hasOwnProperty.call(surface, op)) return undefined;
  const candidate = (surface as Record<string, unknown>)[op];
  return typeof candidate === "function" ? (candidate as (json: string) => string) : undefined;
}

/** 内核错误在 WASM 侧的形态：`E<码位>: <消息>`（设计 06 码位分段）。 */
export class KernelError extends Error {
  readonly code: string;

  constructor(raw: string) {
    const match = /^([E][0-9]{4}):\s*(.*)$/s.exec(raw);
    super(match ? match[2] : raw);
    this.code = match ? match[1] : "E0000";
  }
}

/** 解析 WASM 返回的 JSON；非 `E<码位>` 形式按原样抛出。 */
export function parsePlanResult(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    throw new KernelError(raw);
  }
}
