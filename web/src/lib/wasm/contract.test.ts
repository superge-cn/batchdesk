import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";
import type { BrowserCapabilities } from "./contract";
import {
  BYTE_OPS,
  EXEC_BINDINGS,
  EXTRA_PLAN_OPS,
  KernelError,
  PLAN_OPS,
  PLAN_OPS_COVERAGE,
  parsePlanResult,
  planHandler
} from "./contract";

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

/**
 * 处理入口清单的**反漂移**断言（ADR-0102 A2-7）。
 *
 * 背景：A2 系列分批下沉时，Rust 侧导出面由 `export_surface.rs` 把关，但 Web 侧
 * `contract.ts` 的接口与 worker 的 `switch` 是两份手工清单——于是出现过
 * 「24 个导出里只有 c01/f12 真的能从 Web 调到」。这里把清单收敛成
 * `PLAN_OPS` 一份，并直接读 Rust 源码对拍，避免再退化。
 */
// 测试在 Node 里跑：只用到 cwd()，不必为它引整套 @types/node。
declare const process: { cwd(): string };

const LIB_RS = [
  // vitest 从 web/ 跑（`pnpm test`）；也容忍从仓库根跑。
  resolve(process.cwd(), "..", "crates", "wasm", "src", "lib.rs"),
  resolve(process.cwd(), "crates", "wasm", "src", "lib.rs")
].find((path) => existsSync(path));

function rustExports(prefix: string): string[] {
  if (!LIB_RS) {
    throw new Error("crates/wasm/src/lib.rs not found; run vitest from web/");
  }
  const source = new TextDecoder().decode(readFileSync(LIB_RS));
  const pattern = new RegExp(
    `#\\[wasm_bindgen\\]\\s*\\npub fn (${prefix}\\w+)`,
    "g"
  );
  return [...source.matchAll(pattern)].map((match) => match[1]);
}

function rustPlanExports(): string[] {
  return rustExports("plan_");
}

describe("WASM 处理入口清单", () => {
  it("PLAN_OPS ∪ EXTRA_PLAN_OPS 与 lib.rs 的 plan_* 导出精确相等", () => {
    // 两份清单合起来才是完整导出面：`PLAN_OPS` 是「一功能一入口」的 25 项，
    // `EXTRA_PLAN_OPS` 是同视图的第二条计划路径（C10 压缩）。任一边漏登记，
    // 要么「导出了但调不到」，要么「登记了但 Rust 没有」。
    const exported = rustPlanExports();
    expect(exported.length).toBeGreaterThan(0);
    expect([...PLAN_OPS, ...EXTRA_PLAN_OPS].sort()).toEqual(exported.sort());
  });

  it("C10 双向：25 个功能入口 ＋ 1 条压缩侧入口", () => {
    expect(PLAN_OPS).toContain("plan_c10");
    expect(PLAN_OPS).toHaveLength(25);
    expect(EXTRA_PLAN_OPS).toEqual(["plan_c10_compress"]);
    // 编译期断言在运行时的取值（类型不成立时 tsc 先失败）。
    expect(PLAN_OPS_COVERAGE).toBe(true);
  });

  it("BYTE_OPS 与 lib.rs 的字节型导出精确相等", () => {
    // 字节型入口（`data: &[u8]`）装不进 JSON 协议，单列一份清单；两侧任一边
    // 增删而另一边没跟上，都会得到「登记了但调不到」或「能调但没登记」。
    const exported = rustExports("parse_");
    expect([...BYTE_OPS].sort()).toEqual(exported.sort());
    expect(BYTE_OPS).toEqual(["parse_c07_manifest"]);
  });

  it("EXEC_OPS 的绑定名与 lib.rs 的 web_exec_* 导出精确相等", () => {
    // 执行面（ADR-0108）不是 `plan_*`，`planHandler` 不管它；Worker 的 dispatch
    // 与 Rust 导出之间同样需要反漂移——名字写错不会报错，只会「执行按钮没反应」。
    const exported = rustExports("web_exec_");
    const expected = [...Object.values(EXEC_BINDINGS), "web_exec_product"].sort();
    expect(exported.sort()).toEqual(expected);
    expect(exported).toHaveLength(7);
  });

  it("执行面入口不会被 planHandler 当成处理入口分发", () => {
    const surface = { web_exec_apply: () => "{}" };
    expect(planHandler(surface, "web_exec_apply")).toBeUndefined();
  });

  it("字节型入口不会被 planHandler 当成处理入口分发", () => {
    const surface = { parse_c07_manifest: () => "rows" };
    expect(planHandler(surface, "parse_c07_manifest")).toBeUndefined();
  });

  it("planHandler 只分发 plan_* 前缀，未知名字返回 undefined", () => {
    const surface = {
      plan_c10: (json: string) => `c10:${json}`,
      browser_capabilities: () => "caps"
    };
    expect(planHandler(surface, "plan_c10")?.("{}")).toBe("c10:{}");
    expect(planHandler(surface, "plan_unknown")).toBeUndefined();
    expect(planHandler(surface, "browser_capabilities")).toBeUndefined();
    // 原型链上的名字不能被当成入口（动态查表的经典坑）。
    expect(planHandler(surface, "constructor")).toBeUndefined();
  });
});
