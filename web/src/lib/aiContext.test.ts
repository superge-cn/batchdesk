import { describe, expect, it } from "vitest";

import { buildAiContext, fileNamesForFeature } from "./aiContext";

/**
 * F15 面板上下文的**数据最小化**边界（设计 14 §9）。
 *
 * 这里钉住三件事：
 * ① 文件名只取末段——目录与路径一律不出端；
 * ② `fileCount`／`kinds` 只描述量与功能，不含文件名；
 * ③ `includeFileNames=false` 时**不带** `fileNames` 键（不是空数组）。
 */
describe("AI 面板上下文（设计 14 §9）", () => {
  const form = {
    files: ["D:/资料/合同.pdf", "D:/资料/发票.pdf", "C:/tmp/备注.txt"].join("\n"),
    target: "utf-8",
    outputDir: "D:/输出"
  };

  it("多文件槽位取文件名末段，去重且不带目录", () => {
    expect(fileNamesForFeature("c01", form)).toEqual([
      "合同.pdf",
      "发票.pdf",
      "备注.txt"
    ]);
    // 单文件槽位同样只取末段。
    expect(fileNamesForFeature("f01", { file: "/home/a/b/报告.docx" })).toEqual(["报告.docx"]);
  });

  it("目录类槽位不当作文件（dir/root/outputDir 不进清单）", () => {
    expect(fileNamesForFeature("f12", { dir: "D:/工作目录" })).toEqual([]);
    expect(fileNamesForFeature("f01", { file: "a.txt", outputDir: "D:/输出" })).toEqual(["a.txt"]);
  });

  it("未登记槽位的功能如实返回空清单（不猜）", () => {
    expect(fileNamesForFeature("unknown-feature", form)).toEqual([]);
  });

  it("默认请求不带 fileNames 键（只在同意后由面板加入）", () => {
    const context = buildAiContext({
      feature: "c01",
      form,
      locale: "zh-CN",
      includeFileNames: false
    });
    expect(context.fileCount).toBe(3);
    expect(context.kinds).toEqual(["c01"]);
    expect("fileNames" in context).toBe(false);
  });

  it("同意后才带上清单，且仍是文件名而非路径", () => {
    const context = buildAiContext({
      feature: "c01",
      form,
      locale: "zh-CN",
      includeFileNames: true
    });
    expect(context.includeFileNames).toBe(true);
    expect(context.fileNames).toEqual(["合同.pdf", "发票.pdf", "备注.txt"]);
    expect(JSON.stringify(context)).not.toContain("D:/");
  });
});
