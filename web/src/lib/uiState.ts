import type { UiLayoutState, UiNavigationState, UiSortDirection, UiTableState } from "./ipc/types";

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

/**
 * 面板宽度 clamp（设计 03 §11.3「面板宽度 clamp」）：越界贴边，不做比例换算。
 *
 * 上下界不在这里写死数值：视觉尺寸属 U 的射程，TS 里抄一份 CSS 已有的数值
 * 就是第二个权威（同 `VirtualList` 用 DOM 实测行高的理由）。
 */
export function clampPixels(px: number, min: number, max: number): number {
  if (!Number.isFinite(px)) return min;
  return Math.min(Math.max(px, min), max);
}

/** 读一个 `px` 形态的 CSS 自定义属性；读不到返回 `null`，调用方退回自身默认。 */
export function readPixelVar(styles: CSSStyleDeclaration, name: string): number | null {
  const raw = styles.getPropertyValue(name).trim();
  if (!raw.endsWith("px")) return null;
  const value = Number.parseFloat(raw);
  return Number.isFinite(value) ? value : null;
}

/** 侧栏拖宽后的 `layout` 分节（patch 整节替换，兄弟字段必须带上）。 */
export function rememberSidebarWidth(
  layout: UiLayoutState | undefined,
  px: number
): UiLayoutState {
  return { ...(layout ?? {}), sidebarWidth: px };
}

/** 按表与列写入列宽；其它表、其它列原样保留。 */
export function rememberColumnWidth(
  tables: Record<string, UiTableState> | undefined,
  tableId: string,
  column: string,
  px: number
): Record<string, UiTableState> {
  const table = tables?.[tableId] ?? {};
  return {
    ...(tables ?? {}),
    [tableId]: { ...table, columnWidths: { ...(table.columnWidths ?? {}), [column]: px } }
  };
}

/**
 * 列头排序（设计 03 §11.2 的 `tables[].sort`）。
 *
 * 口径＝常见电子表格：点同列翻转方向，换列回 `asc`。UiState 只存"最后一次排序"，
 * 没有"取消排序"这一态——回到计划序由 §11.3 的重置入口负责，不在这里发明第三种状态。
 */
export function rememberSort(
  tables: Record<string, UiTableState> | undefined,
  tableId: string,
  column: string
): Record<string, UiTableState> {
  const table = tables?.[tableId] ?? {};
  const current = table.sort;
  const direction: UiSortDirection =
    current && current.column === column && current.direction === "asc" ? "desc" : "asc";
  return { ...(tables ?? {}), [tableId]: { ...table, sort: { column, direction } } };
}

/** 该列当前的排序方向（不是当前排序列时返回 `null`）。 */
export function sortDirectionOf(
  tables: Record<string, UiTableState> | undefined,
  tableId: string,
  column: string
): UiSortDirection | null {
  const sort = tables?.[tableId]?.sort;
  return sort && sort.column === column ? sort.direction : null;
}

/**
 * 按列排序行的**显示顺序**。
 *
 * 只重排界面渲染的数组，不动 `PlanDocument.entries` 本身：派发顺序与"哪条被跳过"
 * 仍由内核的计划序决定（方案 §7.1），排序是视图态不是执行态。
 * 同值保持原序（`Array.prototype.sort` 自 ES2019 起规定稳定），取不到值的行恒在后。
 */
export function sortRows<T>(
  rows: readonly T[],
  sort: UiTableState["sort"],
  valueOf: (row: T, column: string) => string | number | undefined
): T[] {
  if (!sort) return [...rows];
  const direction = sort.direction === "desc" ? -1 : 1;
  return [...rows].sort((left, right) => {
    const a = valueOf(left, sort.column);
    const b = valueOf(right, sort.column);
    if (a === undefined && b === undefined) return 0;
    if (a === undefined) return 1;
    if (b === undefined) return -1;
    if (a === b) return 0;
    return (a < b ? -1 : 1) * direction;
  });
}
