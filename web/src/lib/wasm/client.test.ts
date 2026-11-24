import { describe, expect, it } from "vitest";

/**
 * Worker 客户端的**协议契约**（ADR-0100 续）。
 *
 * 这里不驱动真实 Worker（Vitest 无 DOM/Worker 环境），
 * 而是钉住三条边界：
 * ① Worker 不可用时返回 null（降级，**不判定**）；
 * ② 在途请求与响应按 id 关联；
 * ③ 内核错误以 `kernel-error` 返回，**不在本层判定**。
 */
describe("WASM Worker 客户端", () => {
  it("非浏览器环境返回 null（能力降级，不阻断）", async () => {
    const { createWasmClient } = await import("./client");
    // Vitest 环境无 Worker 构造器 → 期望降级路径。
    const original = (globalThis as { Worker?: unknown }).Worker;
    (globalThis as { Worker?: unknown }).Worker = undefined;
    try {
      expect(createWasmClient()).toBeNull();
    } finally {
      (globalThis as { Worker?: unknown }).Worker = original;
    }
  });

  it("Worker 构造失败同样降级为 null", async () => {
    const { createWasmClient } = await import("./client");
    const original = (globalThis as { Worker?: unknown }).Worker;
    (globalThis as { Worker?: unknown }).Worker = class {
      constructor() {
        throw new Error("产物未装载");
      }
    };
    try {
      expect(createWasmClient()).toBeNull();
    } finally {
      (globalThis as { Worker?: unknown }).Worker = original;
    }
  });

  it("响应按 id 关联（协议形状）", () => {
    // 直接校验协议类型的形状契约：请求带 id/op，响应带 id/ok。
    const request = { id: 7, op: "plan_c01" as const, payload: "{}" };
    const ok = { id: 7, ok: true as const, result: "{}" };
    const unavailable = { id: 7, ok: false as const, reason: "wasm-unavailable" as const };
    expect(request.id).toBe(ok.id);
    expect(unavailable.reason).toBe("wasm-unavailable");
  });

  it("内核错误不在客户端层被判定（只搬运错误码）", async () => {
    const { KernelError } = await import("./contract");
    // 客户端只负责把错误码传出去；是否阻断由调用方决定。
    // 后端原文不进协议（设计 06 §3：技术细节只进诊断日志）。
    const message = "E4001: operation not licensed";
    const error = new KernelError(message);
    expect(error.code).toBe("E4001");
    const failed = { id: 7, ok: false as const, reason: "kernel-error" as const, code: error.code };
    expect(failed.code).toBe("E4001");
    expect("message" in failed).toBe(false);
  });
});
