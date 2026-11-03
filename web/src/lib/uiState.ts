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

/** 功能页在 UiState 里统一记成 `lastSection: "feature"` ＋ `lastFeature: <功能 id>`。 */
export const FEATURE_SECTION = "feature";

/**
 * 记住当前所在的页面（设计 03 §11.2 的 `navigation.lastSection` / `lastFeature`）。
 *
 * 离开功能页时**不清** `lastFeature`：`restoreView` 只在 `lastSection === "feature"`
 * 时才看它，留着上一次的功能 id 反而能让"回到功能页"更像上次的延续。
 */
export function rememberView(
  navigation: UiNavigationState | undefined,
  section: string,
  feature?: string
): UiNavigationState {
  const base = { ...(navigation ?? {}) };
  if (section === FEATURE_SECTION) {
    if (!feature) return base;
    return { ...base, lastSection: FEATURE_SECTION, lastFeature: feature };
  }
  return { ...base, lastSection: section };
}

/**
 * 启动时该回到哪个页面。
 *
 * 两条保守规则：① 只有 `lastSection === "feature"` 时才认 `lastFeature`；
 * ② 功能 id 必须仍在当前导航表里——功能改名/下线后不会把用户带到一个空白页，
 * 而是回默认视图。返回 `null` 表示"没有可用记录，保持默认"。
 */
export function restoreView(
  navigation: UiNavigationState | undefined,
  featureIds: readonly string[],
  topLevelIds: readonly string[]
): string | null {
  const section = navigation?.lastSection?.trim();
  const feature = navigation?.lastFeature?.trim();
  if (section === FEATURE_SECTION && feature && featureIds.includes(feature)) {
    return feature;
  }
  if (section && topLevelIds.includes(section)) return section;
  return null;
}
