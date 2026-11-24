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
 * - 内核错误以 `{ ok: false, reason: "kernel-error", code }` 返回，
 *   调用方可用 `KernelError` 解析 E 码位——**是否阻断由调用方按 UI 语义决定**，
 *   本层**不做判定**（设计 05 §0 第 2 条）。
 */

import type { ByteOp, PlanOp } from "./contract";
import type { WasmRequest, WasmResponse } from "./worker";

/** 沙箱产物：Worker 里已把字节包成 Blob，主线程只需建下载链接（不再复制一次）。 */
export interface WasmProduct {
  path: string;
  size: number;
  blob: Blob;
}

export type WasmCall =
  | { op: "browser_capabilities" }
  | { op: PlanOp; payload: string }
  /** 字节型入口（见 `contract.ts` 的 `BYTE_OPS`）。 */
  | { op: ByteOp; payload: string; bytes: Uint8Array }
  /** 沙箱登记输入：清单走 `payload`，字节走结构化克隆。 */
  | { op: "stage_inputs"; payload: string; bytes: Uint8Array }
  /** 只登记目录／目录事实（F10 清单）：没有字节，只有 JSON。 */
  | { op: "stage_dirs"; payload: string }
  | { op: "stage_listing"; payload: string }
  | { op: "apply_plan"; payload: string }
  | { op: "undo_plan"; payload: string }
  | { op: "reset_sandbox" };

export type WasmCallResult =
  | { ok: true; result: string; products?: WasmProduct[] }
  | { ok: false; reason: "wasm-unavailable" | "kernel-error"; code?: string };

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
      resolve({ ok: true, result: response.result, products: response.products });
    } else if (response.reason === "wasm-unavailable") {
      resolve({ ok: false, reason: "wasm-unavailable" });
    } else {
      resolve({
        ok: false,
        reason: "kernel-error",
        code: response.code ?? "E0000"
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
      // 三种请求形态：无参 / JSON 单参 / JSON 参数 + 原始字节（结构化克隆直接过）。
      // 逐个 op 收窄而不是靠 `"payload" in request`：执行面的 op 与 `PlanOp` 联合后，
      // 属性判别收不到单一成员，TS 会把 `{op: A | B}` 判成不可赋值。
      let message: WasmRequest;
      if (request.op === "browser_capabilities" || request.op === "reset_sandbox") {
        message = { id, op: request.op };
      } else if ("bytes" in request) {
        message = {
          id,
          op: request.op,
          payload: request.payload,
          bytes: request.bytes
        };
      } else {
        message = { id, op: request.op, payload: request.payload } as WasmRequest;
      }
      return new Promise((resolve) => {
        pending.set(id, resolve);
        // 字节型请求把底层缓冲区**转移**给 Worker：结构化克隆会再复制一整块，
        // 500MB 量级下等于凭空多一份峰值内存。转移后调用方的这块内存失效——
        // 调用方（`stageInputs` / `parse_c07_manifest`）每次都新建并随即交出，
        // 不存在复用，因此这是安全的。
        const transfer =
          "bytes" in message ? ([message.bytes.buffer] as Transferable[]) : [];
        worker.postMessage(message, transfer);
      });
    },
    terminate() {
      worker.terminate();
      pending.clear();
    }
  };
}
