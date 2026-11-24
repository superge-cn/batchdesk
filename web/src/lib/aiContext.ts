/**
 * F15 面板上下文（设计 14 §4 DTO／§9 数据最小化）——**从当前功能页表单派生**。
 *
 * 边界（设计 14 §9）：
 * - `fileCount`／`kinds` 是**默认请求**就带的量（不触碰文件名）；
 * - `fileNames` **只在用户逐次同意后才出现**，且只放**文件名**（`Path.name`），
 *   不带目录、不带内容——同意弹窗据此列清单；
 * - 列表内容/路径一律不出端。
 *
 * 槽位口径复用 `mock.ts` 的 `APPLY_SLOTS`（与壳层 `recipe_apply::FEATURE_PATH_SLOTS`
 * 同源、由 check_docs.py 逐码核对），不另立一份功能→字段清单。
 */

import { APPLY_SLOTS } from "./ipc/mock";
import type { AiContext } from "./ipc/types";

/** 路径类参数名 → 取其中的"文件"而非"目录"（F15 上下文只列文件名）。 */
const DIRECTORY_SLOTS = new Set(["dir", "directory", "root", "roots", "sourceRoot", "outputDir"]);

/** 只保留文件名的末段（不含目录，避免把路径带出端）。 */
function baseName(value: string): string {
  const parts = value.split(/[\\/]/).filter(Boolean);
  return parts.length > 0 ? (parts[parts.length - 1] as string) : value;
}

/**
 * 从表单值里取出该功能的输入文件集（按 `APPLY_SLOTS` 的 required 槽位）。
 * `many=true` 的槽位（`files`/`sources`）已在表单里以换行分隔。
 */
export function fileNamesForFeature(
  feature: string,
  form: Record<string, string>
): string[] {
  const spec = APPLY_SLOTS[feature];
  if (!spec) return [];
  const names: string[] = [];
  for (const [slot, many] of spec.required) {
    if (DIRECTORY_SLOTS.has(slot)) continue;
    if (slot === "sourcePaths") continue;
    const raw = form[slot];
    if (!raw) continue;
    const values = many ? raw.split("\n") : [raw];
    for (const value of values) {
      const trimmed = value.trim();
      if (!trimmed) continue;
      names.push(baseName(trimmed));
    }
  }
  return [...new Set(names)];
}

/** 组装面板上下文：文件名只在调用方已获同意时带入。 */
export function buildAiContext(input: {
  feature: string;
  form: Record<string, string>;
  locale: string;
  includeFileNames: boolean;
}): AiContext {
  const names = fileNamesForFeature(input.feature, input.form);
  return {
    fileCount: names.length,
    kinds: input.feature ? [input.feature] : [],
    locale: input.locale,
    includeFileNames: input.includeFileNames,
    ...(input.includeFileNames && names.length > 0 ? { fileNames: names } : {})
  };
}
