/**
 * WASM Worker 的**主线程客户端**（ADR-0100 续）。
 *
 * 用法：
 * ```ts
 * const client = createWasmClient();
 * const caps = await client.call({ op: "browser_capabilities" });
 * ```
 *
 * 语义：
 * - 产物/Worker 不可用时，`call` 返回 `{ ok: false, reason: "wasm-unavailable" }`；
 * - 内核错误以 `{ ok: false, reason: "kernel-error", message }` 返回，
 *   调用方可用 `KernelError` 解析 E 码位——**是否阻断由调用方按 UI 语义决定**，
 *   本层**不做判定**（设计 05 §0 第 2 条）。
 */

import type { WasmRequest, WasmResponse } from "./worker";

export type WasmCall =
  | { op: "browser_capabilities" }
  | { op: "plan_c01"; payload: string }
  | { op: "plan_f12"; payload: string };

export type WasmCallResult =
  | { ok: true; result: string }
  | { ok: false; reason: "wasm-unavailable" | "kernel-error"; message?: string };

export interface WasmClient {
  call(request: WasmCall): Promise<WasmCallResult>;
  terminate(): void;
}

/** Worker 不可用（非浏览器环境、产物未装载）时返回 null——调用方降级。 */
export function createWasmClient(): WasmClient | null {
  if (typeof Worker === "undefined") return null;

  let worker: Worker;
  try {
    worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
  } catch {
    return null;
  }

  let nextId = 1;
  const pending = new Map<number, (result: WasmCallResult) => void>();

  worker.onmessage = (event: MessageEvent<WasmResponse>) => {
    const response = event.data;
    const resolve = pending.get(response.id);
    if (resolve === undefined) return;
    pending.delete(response.id);
    if (response.ok) {
      resolve({ ok: true, result: response.result });
    } else if (response.reason === "wasm-unavailable") {
      resolve({ ok: false, reason: "wasm-unavailable" });
    } else {
      resolve({
        ok: false,
        reason: "kernel-error",
        message: response.message ?? ""
      });
    }
  };

  worker.onerror = () => {
    // Worker 级失败：所有在途请求按不可用降级（不抛、不判定）。
    for (const resolve of pending.values()) {
      resolve({ ok: false, reason: "wasm-unavailable" });
    }
    pending.clear();
  };

  return {
    call(request: WasmCall): Promise<WasmCallResult> {
      const id = nextId++;
      const message: WasmRequest =
        "payload" in request
          ? { id, op: request.op, payload: request.payload }
          : { id, op: request.op };
      return new Promise((resolve) => {
        pending.set(id, resolve);
        worker.postMessage(message);
      });
    },
    terminate() {
      worker.terminate();
      pending.clear();
    }
  };
}
