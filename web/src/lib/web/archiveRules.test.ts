import { describe, expect, it } from "vitest";

import { ArchiveRuleError, parseArchiveCondition, parseArchiveRules } from "./archiveRules";

/**
 * C08 规则文本解析的契约（A3-3）。
 *
 * 这层之所以要单独测：WASM 入口收的是**结构化条件**，不是文本——解析在前端，
 * 一旦与 Rust 侧（`plan_manifest.rs::parse_archive_condition`，与 CLI 同语义）
 * 差一点，产物就是「规则看着对、归档结果不对」。重点钉三处易错：
 * ① 只按**第一个** `|` 切分（参数里可以有 `|`）；
 * ② `size` 只认十进制（`1e6` 在 JS 里能过 `Number()`，Rust `parse::<u64>()` 不认）；
 * ③ 端点为空＝不限，且**不写出**该字段（Rust 侧是 `Option` + `serde(default)`）。
 */

describe("C08 规则解析（与 CLI/桌面同语义）", () => {
  it("四种条件类型", () => {
    expect(parseArchiveCondition("ext:jpg, png,")).toEqual({
      kind: "extensions",
      value: ["jpg", "png"]
    });
    expect(parseArchiveCondition("mtime:2024-01-01..2024-12-31")).toEqual({
      kind: "modifiedDateRange",
      value: { from: "2024-01-01", to: "2024-12-31" }
    });
    expect(parseArchiveCondition("size:1048576..")).toEqual({
      kind: "sizeRange",
      value: { min: 1048576 }
    });
    expect(parseArchiveCondition("name:log")).toEqual({
      kind: "nameKeyword",
      value: "log"
    });
  });

  it("端点为空＝不限，字段不写出（交给内核的 Option）", () => {
    expect(parseArchiveCondition("mtime:..2024-12-31")).toEqual({
      kind: "modifiedDateRange",
      value: { to: "2024-12-31" }
    });
    expect(parseArchiveCondition("size:..")).toEqual({
      kind: "sizeRange",
      value: {}
    });
  });

  it("只按第一个 `|` 切分：参数里可以出现 `|`", () => {
    expect(parseArchiveRuleLine("a|name:x|y")).toEqual({
      subDir: "a",
      condition: { kind: "nameKeyword", value: "x|y" }
    });
  });

  it("空行跳过，错误信息带行号", () => {
    expect(
      parseArchiveRules("images|ext:jpg\n\n  \ndocs|name:doc\n")
    ).toHaveLength(2);
    expect(() => parseArchiveRules("images|ext:jpg\nbroken")).toThrowError(
      /line 2: c08: rule must be/
    );
  });

  it.each([
    ["ext:", /at least one extension/],
    ["mtime:2024-01-01", /requires from\.\.to/],
    ["size:1e6..", /must be a byte count/],
    ["size:-1..", /must be a byte count/],
    ["name:", /requires a keyword/],
    ["todo:x", /unknown rule type/]
  ])("非法条件 %s 明确报错", (params, message) => {
    expect(() => parseArchiveCondition(params)).toThrowError(message);
    expect(() => parseArchiveCondition(params)).toThrow(ArchiveRuleError);
  });
});

/** 与 `parseArchiveRule` 同义，单独包一层只为让上面的用例读起来短。 */
function parseArchiveRuleLine(line: string) {
  return parseArchiveRules(line)[0];
}
