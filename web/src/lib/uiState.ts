import type { UiNavigationState } from "./ipc/types";

/**
 * 记录一次使用的目录（设计 03 §11.2 的 `navigation.lastDirectory` / `recentDirectories`）。
 *
 * 语义**刻意与内核 `UiState::remember_directory` 保持一致**：去重后置顶、不设上限
 * （内核没设上限，前端不擅自加一个）；只动这两个字段，`lastSection`/`lastFeature`
 * 等兄弟字段原样保留——patch 是顶层浅合并，漏掉兄弟字段就等于把它们擦掉。
 */
export function rememberDirectory(
  navigation: UiNavigationState | undefined,
  directory: string
): UiNavigationState {
  const trimmed = directory.trim();
  const base = { ...(navigation ?? {}) };
  if (!trimmed) return base;
  const recent = (base.recentDirectories ?? []).filter((entry) => entry !== trimmed);
  recent.unshift(trimmed);
  return { ...base, lastDirectory: trimmed, recentDirectories: recent };
}

/**
 * 启动时该预填哪个目录。
 *
 * 只做"给个默认值"，不做存在性校验：目录被删/改名时以空输入开始（设计 03 §11.3
 * 要求不阻塞启动），用户重新选一次即可；真选了不存在的目录，预览会以 E2001 报出来。
 */
export function preferredDirectory(navigation: UiNavigationState | undefined): string | null {
  const last = navigation?.lastDirectory?.trim();
  return last ? last : null;
}
