import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  mockApply,
  mockCancel,
  mockCleanup,
  mockGetStorageInfo,
  mockGetTask,
  mockListen,
  mockListTasks,
  mockMeta,
  mockPause,
  mockPlan,
  mockPreflightPlan,
  mockRecoverTask,
  mockResume,
  mockRetry,
  mockUndo
} from "./mock";
import type {
  CleanupOutcome,
  IpcMeta,
  PreflightReport,
  ProgressEvent,
  PlanDocument,
  PlanOperationInput,
  RecoveryAction,
  StorageInfo,
  TaskDetail,
  TaskDoneEvent,
  TaskHandle,
  TaskPage,
  TaskStateEvent
} from "./types";

export const TASK_PROGRESS = "task://progress";
export const TASK_STATE = "task://state";
export const TASK_DONE = "task://done";

export const isTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export async function getIpcMeta(): Promise<IpcMeta> {
  return isTauri() ? invoke<IpcMeta>("getIpcMeta") : mockMeta();
}

export async function getStorageInfo(): Promise<StorageInfo> {
  return isTauri() ? invoke<StorageInfo>("getStorageInfo") : mockGetStorageInfo();
}

export async function planOperation(input: PlanOperationInput): Promise<PlanDocument> {
  return isTauri()
    ? invoke<PlanDocument>("planOperation", {
        kind: input.kind,
        params: input.params,
        sourcePaths: input.sourcePaths
      })
    : mockPlan(input);
}

export async function applyPlan(
  planId: string,
  options: { onConflict: "skip" | "sequence" | "overwrite" | "ask"; overwriteConfirmed: boolean }
): Promise<TaskHandle> {
  return isTauri()
    ? invoke<TaskHandle>("applyPlan", { planId, options })
    : mockApply(planId);
}

export async function undoPlan(planId: string): Promise<TaskHandle> {
  return isTauri() ? invoke<TaskHandle>("undoPlan", { planId }) : mockUndo(planId);
}

export async function preflightPlan(planId: string): Promise<PreflightReport> {
  return isTauri()
    ? invoke<PreflightReport>("preflightPlan", { planId })
    : mockPreflightPlan();
}

export async function listTasks(offset = 0, limit = 50): Promise<TaskPage> {
  return isTauri()
    ? invoke<TaskPage>("listTasks", { offset, limit })
    : mockListTasks();
}

export async function recoverTask(
  taskId: string,
  action: RecoveryAction
): Promise<TaskHandle> {
  return isTauri()
    ? invoke<TaskHandle>("recoverTask", { taskId, action })
    : mockRecoverTask(taskId);
}

export async function retryTask(taskId: string): Promise<TaskHandle> {
  return isTauri() ? invoke<TaskHandle>("retryTask", { taskId }) : mockRetry(taskId);
}

export async function cancelTask(taskId: string): Promise<void> {
  if (isTauri()) {
    await invoke("cancelTask", { taskId });
    return;
  }
  mockCancel(taskId);
}

export async function pauseTask(taskId: string): Promise<void> {
  if (isTauri()) {
    await invoke("pauseTask", { taskId });
    return;
  }
  mockPause(taskId);
}

export async function resumeTask(taskId: string): Promise<void> {
  if (isTauri()) {
    await invoke("resumeTask", { taskId });
    return;
  }
  mockResume(taskId);
}

export async function getTask(taskId: string): Promise<TaskDetail> {
  return isTauri() ? invoke<TaskDetail>("getTask", { taskId }) : mockGetTask(taskId);
}

export function listenTaskProgress(
  handler: (event: ProgressEvent) => void
): Promise<UnlistenFn> {
  if (!isTauri()) {
    return Promise.resolve(
      mockListen(TASK_PROGRESS, (payload) => handler(payload as ProgressEvent))
    );
  }
  return listen<ProgressEvent>(TASK_PROGRESS, (event) => handler(event.payload));
}

export function listenTaskState(handler: (event: TaskStateEvent) => void): Promise<UnlistenFn> {
  if (!isTauri()) {
    return Promise.resolve(
      mockListen(TASK_STATE, (payload) => handler(payload as TaskStateEvent))
    );
  }
  return listen<TaskStateEvent>(TASK_STATE, (event) => handler(event.payload));
}

export function listenTaskDone(handler: (event: TaskDoneEvent) => void): Promise<UnlistenFn> {
  if (!isTauri()) {
    return Promise.resolve(
      mockListen(TASK_DONE, (payload) => handler(payload as TaskDoneEvent))
    );
  }
  return listen<TaskDoneEvent>(TASK_DONE, (event) => handler(event.payload));
}

export async function cleanupRetention(
  retentionDays: number,
  apply: boolean
): Promise<CleanupOutcome> {
  return isTauri()
    ? invoke<CleanupOutcome>("cleanupRetention", { retentionDays, apply })
    : mockCleanup(retentionDays, apply);
}

export function toIpcError(value: unknown): {
  code: string;
  message: string;
  detail?: string;
  retryable: boolean;
  taskId?: string;
} {
  if (typeof value === "object" && value !== null && "code" in value && "message" in value) {
    const error = value as {
      code: string;
      message: string;
      detail?: string;
      retryable?: boolean;
      taskId?: string;
    };
    return {
      code: error.code,
      message: error.message,
      detail: error.detail,
      retryable: error.retryable ?? false,
      taskId: error.taskId
    };
  }
  return {
    code: "E9001",
    message: value instanceof Error ? value.message : String(value),
    retryable: false
  };
}
