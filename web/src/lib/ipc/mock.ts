// Browser-only demo adapter. It never touches the filesystem and is used only
// when the Vite app runs outside Tauri; real desktop runs call Rust commands.
import type {
  ApplyEntryReport,
  ApplyReport,
  CleanupOutcome,
  IpcError,
  IpcMeta,
  PlanDocument,
  PlanEntry,
  PlanOperationInput,
  PreflightReport,
  ProgressEvent,
  ProgressPhase,
  StorageInfo,
  TaskDetail,
  TaskHandle,
  TaskPage,
  TaskState,
  TaskSummary
} from "./types";

const PLAN_ID = "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8Q8";
const TASK_EVENT = {
  progress: "task://progress",
  state: "task://state",
  done: "task://done"
} as const;
let lastPlan: PlanDocument | null = null;
let taskCounter = 0;

const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

type MockListener = (payload: unknown) => void;
const listeners = new Map<string, Set<MockListener>>();

export function mockListen(event: string, handler: MockListener): () => void {
  const bucket = listeners.get(event) ?? new Set<MockListener>();
  bucket.add(handler);
  listeners.set(event, bucket);
  return () => {
    bucket.delete(handler);
    if (bucket.size === 0) listeners.delete(event);
  };
}

function emit(event: string, payload: unknown) {
  for (const handler of listeners.get(event) ?? []) handler(payload);
}

function fail(code: string, message: string): never {
  const error: IpcError = { code, message, retryable: false };
  throw error;
}

export function mockMeta(): IpcMeta {
  return { contractVersion: 1, kernelVersion: "mock" };
}

export function mockGetStorageInfo(): StorageInfo {
  return {
    quarantineDir: "~/.local/share/com.batchdesk.desktop/quarantine",
    journalFile: "~/.local/share/com.batchdesk.desktop/journal.jsonl"
  };
}

export async function mockPlan(input: PlanOperationInput): Promise<PlanDocument> {
  await wait(320);
  const root = input.sourcePaths[0]?.replace(/\/+$/, "");
  if (!root) fail("E1001", "请填写扫描目录");
  const keep = input.params.keep ?? "earliest";
  const blocked = (input.params.protectedDirs ?? []).length > 0;
  const entries: PlanEntry[] = [
    {
      entryId: `${PLAN_ID}-0001`,
      source: `${root}/archive/report-copy.pdf`,
      action: "delete",
      reason: `duplicate of ${root}/report.pdf (group size 245760, keep by ${keep}); apply moves this file to the quarantine directory`
    },
    {
      entryId: `${PLAN_ID}-0002`,
      source: `${root}/archive/report-final.pdf`,
      action: "delete",
      reason: `duplicate of ${root}/report.pdf (group size 245760, keep by ${keep}); apply moves this file to the quarantine directory`
    }
  ];
  if (blocked) {
    entries.push({
      entryId: `${PLAN_ID}-0003`,
      source: `${root}/protected/locked-copy.pdf`,
      action: "delete",
      reason: `duplicate of ${root}/report.pdf (group size 245760, keep by ${keep}); apply moves this file to the quarantine directory`,
      blocked: {
        code: "E2009",
        reason: "path is under protected directory",
        hint: "从保护目录移除该路径，或改用其他来源"
      }
    });
  }
  lastPlan = {
    planId: PLAN_ID,
    kind: "c09",
    params: {
      keep,
      mode: "full",
      hash: "blake3",
      quickWindowBytes: input.params.quickWindowBytes ?? null,
      protectedDirs: input.params.protectedDirs ?? []
    },
    entries,
    stats: {
      total: entries.length,
      conflict: 0,
      warning: 1,
      blocked: blocked ? 1 : 0
    },
    warnings: [
      {
        kind: "hardlink-skipped",
        suggestion: "3 个硬链接入口按同一文件跳过，未纳入清理",
        riskLevel: "safe"
      }
    ]
  };
  return lastPlan;
}

interface MockTask {
  taskId: string;
  kind: "apply" | "undo";
  state: TaskState;
  phase: ProgressPhase;
  done: number;
  total: number;
  message?: string;
  startedAt?: string;
  finishedAt?: string;
  entries: ApplyEntryReport[];
  cursor: number;
  cancelled: boolean;
  paused: boolean;
  launched: boolean;
  timer?: ReturnType<typeof setTimeout>;
}

const tasks = new Map<string, MockTask>();

function nextTaskId(kind: string): string {
  taskCounter += 1;
  return `01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8${kind.toUpperCase()}${String(taskCounter).padStart(2, "0")}`;
}

function emitState(task: MockTask) {
  emit(TASK_EVENT.state, { taskId: task.taskId, state: task.state });
}

function emitProgress(task: MockTask, message?: string) {
  const event: ProgressEvent = {
    taskId: task.taskId,
    phase: task.phase,
    done: task.done,
    total: task.total,
    message
  };
  emit(TASK_EVENT.progress, event);
}

function summary(task: MockTask): TaskSummary {
  return {
    taskId: task.taskId,
    kind: "c09",
    state: task.state,
    phase: task.phase,
    done: task.done,
    total: task.total,
    startedAt: task.startedAt,
    finishedAt: task.finishedAt,
    message: task.message
  };
}

function reportFor(task: MockTask): ApplyReport {
  const entries =
    task.state === "cancelled"
      ? task.entries.map((entry, index) =>
          index < task.cursor
            ? entry
            : { entryId: entry.entryId, status: "skipped" as const }
        )
      : task.entries;
  return {
    planId: PLAN_ID,
    taskId: task.taskId,
    entries,
    undoneBy: task.state === "cancelled" ? task.taskId : undefined
  };
}

function finish(task: MockTask, state: TaskState, message?: string) {
  if (task.timer) clearTimeout(task.timer);
  task.timer = undefined;
  task.state = state;
  task.phase = "finalizing";
  task.finishedAt = new Date().toISOString();
  task.message = message;
  emitState(task);
  emit(TASK_EVENT.done, { taskId: task.taskId, summary: reportFor(task) });
}

function tick(task: MockTask) {
  if (task.cancelled) {
    finish(task, "cancelled", "已取消，已执行条目已回滚");
    return;
  }
  if (task.paused) {
    task.timer = setTimeout(() => tick(task), 120);
    return;
  }
  if (task.cursor >= task.entries.length) {
    task.done = task.total;
    task.phase = "executing";
    emitProgress(task);
    finish(task, finalApplyState(task));
    return;
  }
  task.cursor += 1;
  task.done = task.cursor;
  task.phase = "executing";
  emitProgress(task);
  task.timer = setTimeout(() => tick(task), 110);
}

function finalApplyState(task: MockTask): TaskState {
  const failed = task.entries.some((entry) => entry.status === "failed");
  if (!failed) return "completed";
  const applied = task.entries.some(
    (entry) => entry.status === "ok" || entry.status === "overwritten"
  );
  return applied ? "partial-failed" : "failed";
}

function launch(task: MockTask) {
  if (task.launched || task.state === "cancelled" || task.finishedAt) return;
  if (task.paused || task.state === "paused") {
    task.timer = setTimeout(() => launch(task), 80);
    return;
  }
  task.launched = true;
  task.state = "running";
  task.phase = "executing";
  task.startedAt = new Date().toISOString();
  emitState(task);
  emitProgress(task);
  task.timer = setTimeout(() => tick(task), 110);
}

function createTask(
  kind: "apply" | "undo",
  entries: ApplyEntryReport[]
): MockTask {
  const task: MockTask = {
    taskId: nextTaskId(kind),
    kind,
    state: "queued",
    phase: "queued",
    done: 0,
    total: entries.length,
    entries,
    cursor: 0,
    cancelled: false,
    paused: false,
    launched: false
  };
  tasks.set(task.taskId, task);
  emitState(task);
  emitProgress(task);
  setTimeout(() => launch(task), 30);
  return task;
}

export async function mockApply(planId: string): Promise<TaskHandle> {
  if (!lastPlan || lastPlan.planId !== planId) fail("E1001", "计划不存在，请重新生成预览");
  const entries: ApplyEntryReport[] = lastPlan.entries.map((entry) => {
    if (entry.blocked) {
      return {
        entryId: entry.entryId,
        status: "skipped",
        error: {
          code: entry.blocked.code,
          message: entry.blocked.reason,
          retryable: false
        }
      };
    }
    return { entryId: entry.entryId, status: "ok" };
  });
  // 演示 C16 重试链路：最后一个未受阻条目模拟“文件被占用”失败，重试后成功。
  const demoFailure = [...lastPlan.entries].reverse().find((entry) => !entry.blocked);
  const target = demoFailure
    ? entries.find((entry) => entry.entryId === demoFailure.entryId)
    : undefined;
  if (target) {
    target.status = "failed";
    target.error = {
      code: "E2003",
      message: "mock: file was locked by another process",
      retryable: true
    };
  }
  const task = createTask("apply", entries);
  return { taskId: task.taskId };
}

export async function mockUndo(planId: string): Promise<TaskHandle> {
  if (!lastPlan || lastPlan.planId !== planId) fail("E1001", "计划不存在，请重新生成预览");
  const entries: ApplyEntryReport[] = lastPlan.entries
    .filter((entry) => !entry.blocked)
    .map((entry) => ({ entryId: entry.entryId, status: "ok" }));
  const task = createTask("undo", entries);
  return { taskId: task.taskId };
}

export function mockPreflightPlan(): PreflightReport {
  return {
    items: [
      {
        kind: "disk-space",
        level: "pass",
        message: "mock preflight: sufficient disk space"
      }
    ],
    level: "pass",
    pass: 1,
    warn: 0,
    block: 0,
    taskBlocked: false
  };
}

export function mockListTasks(): TaskPage {
  const items = Array.from(tasks.values()).map(summary);
  return { items, total: items.length };
}

export function mockRecoverTask(taskId: string): TaskHandle {
  const task = tasks.get(taskId);
  if (!task) fail("E1001", "任务不存在");
  finish(task, "cancelled", "mock recovery: discarded");
  return { taskId };
}

export function mockRetry(taskId: string): TaskHandle {
  const task = tasks.get(taskId);
  if (!task) fail("E1001", "任务不存在");
  if (task.kind !== "apply" || !["partial-failed", "failed"].includes(task.state)) {
    fail("E1002", "只有失败的应用任务可重试");
  }
  const failed = task.entries.filter((entry) => entry.status === "failed");
  if (failed.length === 0) fail("E1002", "没有失败条目可重试");
  const entries: ApplyEntryReport[] = failed.map((entry) => ({
    entryId: entry.entryId,
    status: "ok"
  }));
  const retry = createTask("apply", entries);
  retry.message = `重试任务 ${taskId}`;
  return { taskId: retry.taskId };
}

export function mockCancel(taskId: string) {
  const task = tasks.get(taskId);
  if (!task || ["completed", "partial-failed", "failed", "cancelled"].includes(task.state)) {
    return;
  }
  task.cancelled = true;
  if (task.state === "queued") {
    finish(task, "cancelled", "任务尚未开始，已取消");
    return;
  }
  task.state = "cancelling";
  emitState(task);
  task.phase = "journaling";
  emitProgress(task, "正在回滚已执行条目");
}

export function mockPause(taskId: string) {
  const task = tasks.get(taskId);
  if (!task || !["queued", "running"].includes(task.state)) return;
  task.paused = true;
  task.state = "paused";
  emitState(task);
}

export function mockResume(taskId: string) {
  const task = tasks.get(taskId);
  if (!task || task.state !== "paused") return;
  task.paused = false;
  task.state = "running";
  emitState(task);
}

export async function mockGetTask(taskId: string): Promise<TaskDetail> {
  const task = tasks.get(taskId);
  if (!task) fail("E1001", "task not found");
  return { summary: summary(task), entries: task.entries };
}

export async function mockCleanup(
  retentionDays: number,
  apply: boolean
): Promise<CleanupOutcome> {
  await wait(300);
  return {
    cutoff: "2026-10-22T00:00:00Z",
    retentionDays,
    dryRun: !apply,
    expiredEntries: 2,
    expiredQuarantineFiles: 2,
    removedEntries: apply ? 2 : 0,
    removedFiles: apply ? 2 : 0,
    failed: []
  };
}
