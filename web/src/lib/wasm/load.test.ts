import { afterEach, describe, expect, it, vi } from "vitest";

/**
 * WASM 产物探测（ADR-0100 续）。
 *
 * 钉住「单一来源＝`web/wasm/ENABLED` 文件」：构建脚本产出后即有，未产出则无。
 * 此前只看构建期 env 快照，导致跑过 build-wasm 但没设环境变量时**永远降级**。
 */

afterEach(() => {
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
});

describe("WASM 产物探测", () => {
  it("env 显式为 1 时直接可用（不依赖网络探测）", async () => {
    vi.stubEnv("VITE_BATCHDESK_WASM", "1");
    vi.resetModules();
    const { wasmArtifactAvailable } = await import("./load");
    await expect(wasmArtifactAvailable()).resolves.toBe(true);
  });

  it("env 未设置时回退到标记文件探测；探测失败按不可用（保守降级）", async () => {
    vi.stubEnv("VITE_BATCHDESK_WASM", "");
    vi.resetModules();
    vi.spyOn(globalThis, "fetch").mockRejectedValue(new Error("no network"));
    const { wasmArtifactAvailable } = await import("./load");
    // jsdom 无真实产物文件：保守返回 false，不得抛给界面。
    await expect(wasmArtifactAvailable()).resolves.toBe(false);
  });

  it("标记文件可读时判定可用", async () => {
    vi.stubEnv("VITE_BATCHDESK_WASM", "");
    vi.resetModules();
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("1", { status: 200 }));
    const { wasmArtifactAvailable } = await import("./load");
    await expect(wasmArtifactAvailable()).resolves.toBe(true);
  });
});
