/// <reference types="vite/client" />

/**
 * WASM 模块加载（懒加载 + 降级）。
 *
 * 加载失败（产物未生成、浏览器不支持、Worker 未装载）时返回 `null`，
 * 调用方按「能力不可用」处理——**前端不做任何判定**（设计 05 §0 第 2 条）。
 */

import { createWasmClient } from "./client";
import type { WasmClient } from "./client";
import type { BatchDeskWasm } from "./contract";

let cached: BatchDeskWasm | null | undefined;

/** 产物目录标记文件的 URL（`web/wasm/ENABLED`，由 scripts/build-wasm.mjs 生成）。 */
const ARTIFACT_MARKER = new URL("../../../wasm/ENABLED", import.meta.url).href;

/**
 * 生成产物是否已就位。
 *
 * 单一来源＝**`web/wasm/ENABLED` 文件本身**：构建脚本产出后即有，未产出则无。
 * 不再只看构建期 env 快照——那样「跑过 build-wasm 但没设环境变量」会永远降级。
 *
 * 探测失败按**不可用**处理（保守），调用方据此降级；本函数**不做判定**
 * （设计 05 §0 第 2 条：能力不可用≠拦截）。
 */
export async function wasmArtifactAvailable(): Promise<boolean> {
  if (import.meta.env.VITE_BATCHDESK_WASM === "1") return true;
  try {
    const response = await fetch(ARTIFACT_MARKER, { method: "GET" });
    return response.ok;
  } catch {
    return false;
  }
}

/**
 * 载入 WASM 模块；未装载或失败返回 `null`（调用方降级，不判定）。
 *
 * 产物由 wasm-bindgen 生成（`web/wasm/batchdesk_wasm.js`），
 * 构建脚本见 `scripts/build-wasm.mjs`。
 */
export async function loadBatchDeskWasm(): Promise<BatchDeskWasm | null> {
  if (cached !== undefined) return cached;
  if (!(await wasmArtifactAvailable())) {
    cached = null;
    return cached;
  }
  try {
    const module = await import("../../../wasm/batchdesk_wasm.js");
    await module.default();
    cached = module as unknown as BatchDeskWasm;
  } catch {
    // 产物缺失/加载失败：按不可用降级，不抛给界面（零强制力＝不阻断）。
    cached = null;
  }
  return cached;
}

/** 测试用：清缓存。 */
export function resetWasmCacheForTests(): void {
  cached = undefined;
}

/**
 * Worker 客户端（ADR-0100 续）：内核调用走独立线程。
 *
 * Worker 不可用时返回 `null`——调用方按「能力不可用」降级，**不判定**。
 */
export function createWorkerClient(): WasmClient | null {
  return createWasmClient();
}
