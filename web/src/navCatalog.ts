import type { MessageKey } from "./i18n";

/**
 * 视图与导航目录的单一来源。
 *
 * 这里放的是"有哪些视图、各自叫什么、顶着哪个功能码"——此前同一件事在 App 里抄了三份
 * （`NAV_ITEMS`、`VIEW_TITLES`、由 `NAV_ITEMS` 派生的 `FEATURE_VIEWS`），"加一个功能"要改
 * 多处，漏一处就出现侧栏有名字而顶栏空白。图标不在这里：那是渲染层的资源，
 * 留在 App 里按 id 绑定。
 *
 * 任务列表的功能码可读化（`labels.tsx` 的 `taskKindLabel`）也吃这份表，
 * 不再另立一张 kind→名称 的清单。
 */

/**
 * 23 个功能视图＝侧栏里除 C09 工作台与两个顶层项之外的全部页面。
 * F13 不在其中：它的登记是「切片完成（CLI，无 UI）」——见 `docs/DEVELOPMENT-PLAN.md`
 * §4 阶段 0 清单分组实现第 3 条，属既定范围而不是遗漏。
 */
export type FeatureView =
  | "f01"
  | "f02"
  | "f03"
  | "f04"
  | "f05"
  | "f06"
  | "f07"
  | "f08"
  | "f09"
  | "f10"
  | "f11"
  | "f12"
  | "c01"
  | "c02"
  | "c03"
  | "c04"
  | "c07"
  | "c08"
  | "c10"
  | "c11"
  | "c12"
  | "c13"
  | "c14";

/** 顶层视图（与功能页区分）：`lastSection` 直接记它们的 id。 */
export type TopLevelView = "c09" | "tasks" | "settings";

export type View = FeatureView | TopLevelView;

export const TOP_LEVEL_VIEWS: TopLevelView[] = ["c09", "tasks", "settings"];

/** 侧栏顺序即数组顺序（此前也如此，未改行为）。 */
export const NAV_ENTRIES: Array<{ id: View; label: MessageKey; tag: string }> = [
  { id: "c09", label: "nav.c09", tag: "C09" },
  { id: "f05", label: "nav.f05", tag: "F05" },
  { id: "f06", label: "nav.f06", tag: "F06" },
  { id: "f07", label: "nav.f07", tag: "F07" },
  { id: "f08", label: "nav.f08", tag: "F08" },
  { id: "f09", label: "nav.f09", tag: "F09" },
  { id: "f10", label: "nav.f10", tag: "F10" },
  { id: "c04", label: "nav.c04", tag: "C04" },
  { id: "c11", label: "nav.c11", tag: "C11" },
  { id: "c12", label: "nav.c12", tag: "C12" },
  { id: "c13", label: "nav.c13", tag: "C13" },
  { id: "c14", label: "nav.c14", tag: "C14" },
  { id: "c07", label: "nav.c07", tag: "C07" },
  { id: "c08", label: "nav.c08", tag: "C08" },
  { id: "c10", label: "nav.c10", tag: "C10" },
  { id: "f11", label: "nav.f11", tag: "F11" },
  { id: "f12", label: "nav.f12", tag: "F12" },
  { id: "f01", label: "nav.f01", tag: "F01" },
  { id: "f02", label: "nav.f02", tag: "F02" },
  { id: "f03", label: "nav.f03", tag: "F03" },
  { id: "f04", label: "nav.f04", tag: "F04" },
  { id: "c01", label: "nav.c01", tag: "C01" },
  { id: "c02", label: "nav.c02", tag: "C02" },
  { id: "c03", label: "nav.c03", tag: "C03" },
  { id: "tasks", label: "nav.tasks", tag: "QUEUE" },
  { id: "settings", label: "nav.settings", tag: "SET" }
];

export const FEATURE_VIEWS: FeatureView[] = NAV_ENTRIES.map((entry) => entry.id).filter(
  (id): id is FeatureView => !TOP_LEVEL_VIEWS.includes(id as TopLevelView)
);

/** 判别是否功能视图（比 `array.includes(view)` 的类型安全写法：收窄到 FeatureView）。 */
const FEATURE_VIEW_SET: ReadonlySet<string> = new Set<string>(FEATURE_VIEWS);

export function isFeatureView(view: View): view is FeatureView {
  return FEATURE_VIEW_SET.has(view);
}

/** id（含功能码）→ 导航名。任务列表把 `TaskSummary.kind` 可读化时吃这张表。 */
export const NAV_LABEL_BY_ID: Record<string, MessageKey> = Object.fromEntries(
  NAV_ENTRIES.map((entry) => [entry.id, entry.label])
);
