/// <reference lib="webworker" />
/**
 * WASM Worker（ADR-0100 续）——把内核调用移出主线程。
 *
 * 边界不变（设计 05 §0）：这里只做**调用与结果搬运**，不做任何授权/配额判定。
 * 产物未装载时，所有请求返回 `{ ok: false, reason: "wasm-unavailable" }`，
 * 主线程按能力降级处理（**不阻断**）。
 */

import init, * as wasm from "../../../wasm/batchdesk_wasm.js";

export type WasmRequest =
  | { id: number; op: "browser_capabilities" }
  | { id: number; op: "plan_c01"; payload: string }
  | { id: number; op: "plan_f12"; payload: string };

export type WasmResponse =
  | { id: number; ok: true; result: string }
  | { id: number; ok: false; reason: "wasm-unavailable" | "kernel-error"; message?: string };

const ctx = self as unknown as DedicatedWorkerGlobalScope;

/** 装载 WASM；产物缺失时返回 false（降级，不抛）。 */
async function boot(): Promise<boolean> {
  try {
    await init(new URL("../../../wasm/batchdesk_wasm_bg.wasm", import.meta.url));
    return true;
  } catch {
    return false;
  }
}

let ready: Promise<boolean> | undefined;

function ensureReady(): Promise<boolean> {
  if (ready === undefined) ready = boot();
  return ready;
}

ctx.onmessage = async (event: MessageEvent<WasmRequest>) => {
  const request = event.data;
  const loaded = await ensureReady();
  if (!loaded) {
    const response: WasmResponse = {
      id: request.id,
      ok: false,
      reason: "wasm-unavailable"
    };
    ctx.postMessage(response);
    return;
  }

  try {
    let result: string;
    switch (request.op) {
      case "browser_capabilities":
        result = wasm.browser_capabilities();
        break;
      case "plan_c01":
        result = wasm.plan_c01(request.payload);
        break;
      case "plan_f12":
        result = wasm.plan_f12(request.payload);
        break;
      default:
        return;
    }
    const response: WasmResponse = { id: request.id, ok: true, result };
    ctx.postMessage(response);
  } catch (error) {
    const response: WasmResponse = {
      id: request.id,
      ok: false,
      reason: "kernel-error",
      message: error instanceof Error ? error.message : String(error)
    };
    ctx.postMessage(response);
  }
};
