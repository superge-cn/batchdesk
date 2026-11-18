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
  /** F12 改名计划（只读）。入参/出参均为 JSON 字符串。 */
  plan_f12(json: string): string;
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
