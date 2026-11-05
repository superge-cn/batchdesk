import type { MessageKey } from "./i18n";
import type {
  ApplyStatus,
  PlanAction,
  ProgressPhase,
  TaskState,
  UndoDryRunStatus
} from "./lib/ipc/types";
import { NAV_LABEL_BY_ID } from "./navCatalog";

/**
 * 任务状态与阶段的用户可见文案。
 * 抽到公共模块的原因：C09 工作台与「任务与撤销」视图都要显示同一套措辞，
 * 两处各写一份必然会漂移（同一个 state 在两个页面叫不同名字）。
 * 文案本体在设计 09 的资源文件里；这里只做枚举 → key 的映射。
 */
export type Translate = (key: MessageKey, params?: Record<string, string | number>) => string;

const PHASE_KEYS: Record<ProgressPhase, MessageKey> = {
  queued: "task.phase.queued",
  scanning: "task.phase.scanning",
  planning: "task.phase.planning",
  executing: "task.phase.executing",
  journaling: "task.phase.journaling",
  finalizing: "task.phase.finalizing"
};

const STATE_KEYS: Record<TaskState, MessageKey> = {
  queued: "task.state.queued",
  running: "task.state.running",
  paused: "task.state.paused",
  cancelling: "task.state.cancelling",
  interrupted: "task.state.interrupted",
  completed: "task.state.completed",
  "partial-failed": "task.state.partialFailed",
  failed: "task.state.failed",
  cancelled: "task.state.cancelled"
};

export function phaseKey(phase: ProgressPhase): MessageKey {
  return PHASE_KEYS[phase];
}

export function stateKey(state: TaskState): MessageKey {
  return STATE_KEYS[state];
}

export function phaseLabel(t: Translate, phase: ProgressPhase): string {
  return t(phaseKey(phase));
}

/** 状态徽章：语气色由状态决定，文案由资源包决定。 */
export function stateBadge(t: Translate, state: TaskState) {
  const label = t(stateKey(state));
  if (state === "completed") return <span className="badge badge-safe">{label}</span>;
  if (state === "failed") return <span className="badge badge-danger">{label}</span>;
  if (state === "running") return <span className="badge">{label}</span>;
  if (state === "queued") return <span className="badge">{label}</span>;
  return <span className="badge badge-warn">{label}</span>;
}

/**
 * 条目级状态的用户文案：内核线协议给的是 `ok`／`skipped`／`missing` 这类枚举字符串，
 * 直接印在界面上就是未本地化文案（09 §2）。两处消费点（C09 执行结果表、功能页回滚预演）
 * 共用同一份映射，避免同一个枚举在两个页面叫不同名字。
 */
const APPLY_STATUS_KEYS: Record<ApplyStatus, MessageKey> = {
  ok: "common.status.applied",
  overwritten: "common.status.overwritten",
  skipped: "common.status.skipped",
  failed: "common.status.failed"
};

const DRY_RUN_STATUS_KEYS: Record<UndoDryRunStatus, MessageKey> = {
  ok: "common.status.revertible",
  // 预演里的"冲突"与 C05 预览行同义，复用同一条文案，不再另起一词
  conflict: "feature.status.conflict",
  missing: "common.status.missing"
};

export function applyStatusLabel(t: Translate, status: ApplyStatus): string {
  return t(APPLY_STATUS_KEYS[status]);
}

/** 条目状态徽章：语气色由结果决定，文案由资源包决定。 */
export function applyStatusBadge(t: Translate, status: ApplyStatus) {
  const label = applyStatusLabel(t, status);
  if (status === "ok") return <span className="badge badge-safe">{label}</span>;
  if (status === "failed") return <span className="badge badge-danger">{label}</span>;
  return <span className="badge badge-warn">{label}</span>;
}

export function dryRunStatusLabel(t: Translate, status: UndoDryRunStatus): string {
  return t(DRY_RUN_STATUS_KEYS[status]);
}

/**
 * 任务列表里的功能码可读化。`TaskSummary.kind` 是"被处理计划的功能码"（设计 01 §2.1），
 * 列表原来把它原样印出来（`f12`）——那是内部编码不是文案（09 §2）。
 * 名字取侧栏那份表（`navCatalog` 唯一来源，不另立清单），**码位仍保留在括号里**：
 * 同 06 §3 对错误码位的处理口径，报障时"我在 F12"比"我在批量改名"更好对。
 * 查不到的编码（例如 F13 这类没有页面的功能）就只显示码位，不编名字。
 */
export function taskKindLabel(t: Translate, kind: string): string {
  const code = kind.toUpperCase();
  const key = NAV_LABEL_BY_ID[kind];
  // 名字本体来自资源包；全角括号只是排版（U+FF08/FF09），不是措辞。
  return key ? `${t(key)}（${code}）` : code; // i18n-exempt
}

/** 任务是否仍在推进（非终态）。 */
export const ACTIVE_STATES: TaskState[] = ["queued", "running", "paused", "cancelling"];

export function isActiveState(state: TaskState): boolean {
  return ACTIVE_STATES.includes(state);
}

/**
 * 动作级类型的用户文案（内核 `PlanAction`）。
 *
 * 处理模板存的是动作不是功能码（`split` 服务 f01/f02/f03/f05），列表里要给人看的是
 * "这个模板干什么"，不能印 `csv-extract` 这种线协议枚举（设计 09 §2）。
 */
const ACTION_KEYS: Record<PlanAction, MessageKey> = {
  rename: "feature.action.rename",
  move: "feature.action.move",
  copy: "feature.action.copy",
  archive: "feature.action.archive",
  extract: "feature.action.extract",
  "create-dir": "feature.action.createDir",
  transcode: "feature.action.transcode",
  "edit-content": "feature.action.editContent",
  delete: "feature.action.delete",
  split: "feature.action.split",
  merge: "feature.action.merge",
  "csv-extract": "feature.action.csvExtract",
  "csv-columns": "feature.action.csvColumns",
  "csv-sort": "feature.action.csvSort",
  "csv-rows": "feature.action.csvRows",
  listing: "feature.action.listing",
  "csv-dedup": "feature.action.csvDedup",
  "csv-filter": "feature.action.csvFilter"
};

export function actionLabel(t: Translate, action: PlanAction): string {
  return t(ACTION_KEYS[action]);
}
