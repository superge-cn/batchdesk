export type KeepPolicy = "earliest" | "shortest-path" | "newest";
export type RiskLevel = "safe" | "caution" | "danger";
export type PlanAction =
  | "rename"
  | "move"
  | "copy"
  | "archive"
  | "extract"
  | "create-dir"
  | "transcode"
  | "edit-content"
  | "delete";
export type ApplyStatus = "ok" | "overwritten" | "skipped" | "failed";
export type ApplyState = "completed" | "partial-failed" | "failed" | "cancelled";
export type ProgressPhase =
  | "queued"
  | "scanning"
  | "planning"
  | "executing"
  | "journaling"
  | "finalizing";
/** 任务类别：apply＝执行计划，undo＝按 Journal 回滚。kind 是"被处理计划的功能码"，不承担这个判别（设计 01 §2.1 v1.49）。 */
export type TaskJob = "apply" | "undo";

export type TaskState =
  | "queued"
  | "running"
  | "paused"
  | "cancelling"
  | "interrupted"
  | "completed"
  | "partial-failed"
  | "failed"
  | "cancelled";

export type PreflightLevel = "pass" | "warn" | "block";

export interface PreflightItem {
  kind: string;
  level: PreflightLevel;
  message: string;
  path?: string;
  code?: string;
  hint?: string;
}

export interface PreflightReport {
  items: PreflightItem[];
  level: PreflightLevel;
  pass: number;
  warn: number;
  block: number;
  taskBlocked: boolean;
}

export type RecoveryAction = "resume" | "rollback" | "discard";

export interface IpcError {
  code: string;
  message: string;
  detail?: string;
  retryable: boolean;
  taskId?: string;
}

export interface PlanBlocked {
  code: string;
  reason: string;
  hint?: string;
}

export interface PlanWarning {
  kind: string;
  suggestion?: string;
  riskLevel: RiskLevel;
}

export interface PlanEntry {
  entryId: string;
  source: string;
  target?: string;
  action: PlanAction;
  reason: string;
  blocked?: PlanBlocked;
  expected?: { size: number; algorithm: string; hash?: string };
}

export interface PlanDocument {
  planId: string;
  kind: string;
  params: Record<string, unknown>;
  entries: PlanEntry[];
  stats: { total: number; conflict: number; warning: number; blocked: number };
  warnings: PlanWarning[];
}

export interface ApplyEntryReport {
  entryId: string;
  status: ApplyStatus;
  error?: IpcError;
  /** 条目级明细（方案 §5.2 的替换计数上报等）；与 error 互不冲突，成功条目也可带。 */
  detail?: string;
}

export interface ApplyReport {
  planId: string;
  taskId: string;
  entries: ApplyEntryReport[];
  undoneBy?: string;
}

export interface TaskHandle {
  taskId: string;
}

export interface ProgressEvent {
  taskId: string;
  phase: ProgressPhase;
  done: number;
  total: number;
  message?: string;
}

export interface TaskStateEvent {
  taskId: string;
  state: TaskState;
}

export interface TaskSummary {
  taskId: string;
  /** 被处理计划的功能码（`f12`／`c09`…）。**不**用于区分执行与回滚——那用 `job`。 */
  kind: string;
  /** 任务类别：执行还是回滚。壳层过去只在 done 事件里带这个判别，列表拿不到，
   * 于是「撤销任务不再提供重试/二次撤销」在界面上是死分支（设计 01 §2.1 v1.49）。 */
  job: TaskJob;
  state: TaskState;
  phase: ProgressPhase;
  done: number;
  total: number;
  /** 计划 ID：任务列表据此走 undoPlan（撤销按 plan 走共享 Journal）。旧快照可能没有。 */
  planId?: string;
  startedAt?: string;
  finishedAt?: string;
  message?: string;
}

export interface TaskDetail {
  summary: TaskSummary;
  entries: ApplyEntryReport[];
}

export interface TaskPage {
  items: TaskSummary[];
  total: number;
}

export interface TaskDoneEvent {
  taskId: string;
  /** 任务类别：取消后的"是否一并撤销"只对 apply 成立。 */
  kind: "apply" | "undo";
  summary: ApplyReport;
}

export interface ApplyOutcome {
  state: ApplyState;
  report: ApplyReport;
}

export interface UndoFailure {
  entryId: string;
  error: IpcError;
}

export interface UndoOutcome {
  reverted: number;
  failed: UndoFailure[];
}

export interface CleanupFailure {
  entryId: string;
  path?: string;
  message: string;
}

export interface CleanupOutcome {
  cutoff: string;
  retentionDays: number;
  dryRun: boolean;
  expiredEntries: number;
  expiredQuarantineFiles: number;
  removedEntries: number;
  removedFiles: number;
  failed: CleanupFailure[];
}

export interface StorageInfo {
  quarantineDir: string;
  journalFile: string;
}

export interface IpcMeta {
  contractVersion: number;
  kernelVersion: string;
}

export interface PlanOperationInput {
  kind: "c09";
  params: {
    keep?: KeepPolicy;
    quickWindowBytes?: number;
    protectedDirs?: string[];
  };
  sourcePaths: string[];
}

export type PreviewStatus = "normal" | "conflict" | "invalid" | "too-long" | "protected";

export interface RenamePreviewRow {
  entryId: string;
  oldPath: string;
  newPath: string;
  status: PreviewStatus;
  twoPhase: boolean;
  detail?: string;
}

export interface RenamePreviewCounts {
  toProcess: number;
  skipped: number;
  conflicts: number;
  blocked: number;
}

export interface RenamePreview {
  planId: string;
  kind: string;
  counts: RenamePreviewCounts;
  rows: RenamePreviewRow[];
  highlights: string[];
}

export type UndoDryRunStatus = "ok" | "conflict" | "missing";

export interface UndoDryRunEntry {
  entryId: string;
  status: UndoDryRunStatus;
  code?: string;
  detail?: string;
}

export interface UndoDryRun {
  planId: string;
  entries: UndoDryRunEntry[];
  conflictCount: number;
  missingCount: number;
}

// ── 配置（设计 03 §2／§2.1／§4）───────────────────────────────────────
export type ConfigScope =
  | "general"
  | "processing"
  | "performance"
  | "security"
  | "logging";

export type SettingSource = "default" | "user" | "recipe" | "task";

/** 设置页"为什么是这个值"的数据支撑：生效值 + 来源 + 用户档 + 默认值。 */
export interface SettingValue<T = unknown> {
  value: T;
  effectiveSource: SettingSource;
  userValue?: T;
  defaultValue: T;
}

/** 单 scope 的逐项来源视图（getConfig({scope}) / setConfig / resetScope 出参）。 */
export interface ScopeView {
  scope: ConfigScope;
  values: Record<string, SettingValue>;
}

export interface ConfigDocument {
  configVersion: number;
  updatedAt: string;
  scopes: Partial<Record<ConfigScope, Record<string, unknown>>>;
}

/** 界面状态里的导航段（设计 03 §11.2）。前端目前只读写这一段。 */
export interface UiNavigationState {
  lastSection?: string;
  lastFeature?: string;
  lastDirectory?: string;
  recentDirectories?: string[];
}

/**
 * 本机、可重置的界面/工作区上下文（设计 03 §11）。
 *
 * 只声明前端真正读写的字段：`window`/`layout`/`tables`/`templatePrefs` 由壳层
 * 与后续的面板宽度/列宽联动消费，现在透传即可（patch 是浅合并，未触碰的分节原样保留）。
 */
export interface UiState {
  uiStateVersion: number;
  updatedAt: string;
  navigation?: UiNavigationState;
}
