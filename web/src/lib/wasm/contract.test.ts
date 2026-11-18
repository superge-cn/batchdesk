import { describe, expect, it } from "vitest";
import type { BrowserCapabilities } from "./contract";
import { KernelError, parsePlanResult } from "./contract";

/**
 * WASM 契约（ADR-0098／设计 05 §2）——前端侧的**边界断言**：
 * 只搬运结果，不判定；错误按 E 码位分段解析。
 */
describe("Web WASM 契约", () => {
  it("按 E 码位解析内核错误", () => {
    const error = new KernelError("E1002: plan not found");
    expect(error.code).toBe("E1002");
    expect(error.message).toBe("plan not found");
  });

  it("非 E 码位形式不误判为内核错误码", () => {
    expect(new KernelError("网络异常").code).toBe("E0000");
  });

  it("解析 JSON 结果", () => {
    expect(parsePlanResult('{"planId":"01"}')).toEqual({ planId: "01" });
  });

  it("非 JSON 的 WASM 返回按内核错误抛出", () => {
    expect(() => parsePlanResult("E3001: encoding undetermined")).toThrowError(
      expect.objectContaining({ code: "E3001" })
    );
  });

  it("能力事实的形状只含环境字段（不含授权/配额）", () => {
    // 形状即契约：browser_capabilities 只返回 form/localMode/exportVersion。
    // 授权/配额由服务端下发，不经此入口（设计 05 §2）。
    const capabilities: BrowserCapabilities = {
      form: "web",
      localMode: true,
      exportVersion: "0.1.0"
    };
    expect(Object.keys(capabilities).sort()).toEqual([
      "exportVersion",
      "form",
      "localMode"
    ]);
  });
});
