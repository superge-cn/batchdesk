/// <reference lib="webworker" />
/**
 * WASM Worker（ADR-0100 续）——把内核调用移出主线程。
 *
 * 边界不变（设计 05 §0）：这里只做**调用与结果搬运**，不做任何授权/配额判定。
 * 产物未装载时，所有请求返回 `{ ok: false, reason: "wasm-unavailable" }`，
 * 主线程按能力降级处理（**不阻断**）。
 */

import init, * as wasm from "../../../wasm/batchdesk_wasm.js";
import { errorCodeFromText } from "../../errors";
import { planHandler, type ByteOp, type PlanOp } from "./contract";

export type WasmRequest =
  | { id: number; op: "browser_capabilities" }
  | { id: number; op: PlanOp; payload: string }
  /** 字节型入口：`bytes` 走结构化克隆，`payload` 只装该入口的 JSON 参数。 */
  | { id: number; op: ByteOp; payload: string; bytes: Uint8Array }
  /** 执行面（ADR-0108）：登记输入 / 执行 / 撤销 / 清空沙箱。 */
  | { id: number; op: "stage_inputs"; payload: string; bytes: Uint8Array }
  | { id: number; op: "stage_dirs"; payload: string }
  | { id: number; op: "stage_listing"; payload: string }
  | { id: number; op: "apply_plan"; payload: string }
  | { id: number; op: "undo_plan"; payload: string }
  | { id: number; op: "reset_sandbox" };

/** 产物（Worker 内已包成 Blob；跨线程传递不复制字节）。 */
export interface ExecProduct {
  path: string;
  size: number;
  blob: Blob;
}

/**
 * 失败响应只带**错误码**，不带后端原文（设计 06 §3／ADR-0061）：
 * 技术细节只进诊断日志，界面文案经 `describeError` 的码位映射出口。
 */
export type WasmResponse =
  | { id: number; ok: true; result: string; products?: ExecProduct[] }
  | { id: number; ok: false; reason: "wasm-unavailable" | "kernel-error"; code?: string };

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
    let products: ExecProduct[] | undefined;
    if (request.op === "browser_capabilities") {
      result = wasm.browser_capabilities();
    } else if (request.op === "stage_inputs") {
      result = wasm.web_exec_stage(request.payload, request.bytes);
    } else if (request.op === "stage_dirs") {
      result = wasm.web_exec_stage_dirs(request.payload);
    } else if (request.op === "stage_listing") {
      result = wasm.web_exec_stage_listing(request.payload);
    } else if (request.op === "apply_plan") {
      result = wasm.web_exec_apply(request.payload);
      // 产物字节在这里一次读齐并包成 Blob：主线程只建下载链接，不再逐文件往返。
      const parsed = JSON.parse(result) as { products?: { path: string; size: number }[] };
      products = (parsed.products ?? []).map((product) => {
        const bytes = wasm.web_exec_product(product.path);
        // wasm-bindgen 给的是 `Uint8Array<ArrayBufferLike>`；BlobPart 只接受
        // ArrayBuffer 支撑的视图。按 byteOffset/长度切一份 ArrayBuffer 再包 Blob，
        // 不整段复制（切片与 Blob 内部各持一份，随后 WASM 侧即可释放）。
        const buffer = bytes.buffer.slice(
          bytes.byteOffset,
          bytes.byteOffset + bytes.byteLength
        ) as ArrayBuffer;
        return {
          path: product.path,
          size: product.size,
          blob: new Blob([buffer])
        };
      });
    } else if (request.op === "undo_plan") {
      result = wasm.web_exec_undo(request.payload);
    } else if (request.op === "reset_sandbox") {
      result = wasm.web_exec_reset();
    } else if (request.op === "parse_c07_manifest") {
      // 字节型入口：参数从 payload 的 JSON 里取，字节单独传（见 `BYTE_OPS`）。
      const extras = JSON.parse(request.payload) as {
        delimiter?: string;
        source?: string;
      };
      result = wasm.parse_c07_manifest(
        request.bytes,
        extras.delimiter,
        extras.source
      );
    } else {
      const handler = planHandler(wasm, request.op);
      if (handler === undefined) {
        // 未登记的处理入口：回一个错误码，**不能静默 return**——
        // 那会让主线程的 Promise 永不 settle（调用方挂死且没有任何提示）。
        const response: WasmResponse = {
          id: request.id,
          ok: false,
          reason: "kernel-error",
          code: "E1002"
        };
        ctx.postMessage(response);
        return;
      }
      result = handler(request.payload);
    }
    const response: WasmResponse = { id: request.id, ok: true, result, products };
    ctx.postMessage(response);
  } catch (error) {
    const response: WasmResponse = {
      id: request.id,
      ok: false,
      reason: "kernel-error",
      code: errorCodeFromText(error)
    };
    ctx.postMessage(response);
  }
};
