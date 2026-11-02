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
  kind: string;
  state: TaskState;
  phase: ProgressPhase;
  done: number;
  total: number;
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
