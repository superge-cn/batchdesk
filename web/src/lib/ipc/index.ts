import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  mockAiAsk,
  mockAiCancel,
  mockAiStatus,
  mockApply,
  mockBuildSupportPackage,
  mockCancel,
  mockCleanup,
  mockDeleteRecipe,
  mockDescribeRecipe,
  mockExportRecipe,
  mockLoadRecipe,
  mockGetConfig,
  mockGetRecipe,
  mockGetStorageInfo,
  mockGetTask,
  mockGetUiState,
  mockImportRecipe,
  mockListen,
  mockListRecipes,
  mockListTasks,
  mockMeta,
  mockPause,
  mockPlan,
  mockPreflightPlan,
  mockRecoverTask,
  mockRenamePreview,
  mockResetScope,
  mockResetUiState,
  mockResume,
  mockRetry,
  mockSaveRecipe,
  mockSetConfig,
  mockSetUiState,
  mockUndo,
  mockUndoDryRun,
  mockWriteSupportPackage
} from "./mock";
import type {
  AiAskInput,
  AiAskOutput,
  AiBundleEvent,
  AiDeltaEvent,
  AiDoneEvent,
  AiErrorEvent,
  AiStatus,
  AiToolEvent,
  CleanupOutcome,
  ConfigDocument,
  ConfigScope,
  RenamePreview,
  ScopeView,
  SupportPackage,
  SupportPackageInput,
  UndoDryRun,
  IpcMeta,
  PreflightReport,
  ProgressEvent,
  PlanDocument,
  PlanOperationInput,
  RecoveryAction,
  RecipeApplySpec,
  RecipeDocument,
  RecipeExportPayload,
  RecipePage,
  SaveRecipeInput,
  StorageInfo,
  TaskDetail,
  TaskDoneEvent,
  TaskHandle,
  TaskPage,
  TaskStateEvent,
  UiState
} from "./types";

export const TASK_PROGRESS = "task://progress";
export const TASK_STATE = "task://state";
export const TASK_DONE = "task://done";
export const AI_DELTA = "ai://delta";
export const AI_TOOL = "ai://tool";
export const AI_BUNDLE = "ai://bundle";
export const AI_DONE = "ai://done";
export const AI_ERROR = "ai://error";

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

export async function renamePreview(planId: string): Promise<RenamePreview> {
  return isTauri()
    ? invoke<RenamePreview>("renamePreview", { planId })
    : mockRenamePreview(planId);
}

export async function undoDryRun(planId: string): Promise<UndoDryRun> {
  return isTauri()
    ? invoke<UndoDryRun>("undoDryRun", { planId })
    : mockUndoDryRun(planId);
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

// ── 配置（设计 03 §7；命令由壳层 `lib.rs` 提供，Web 形态走尽力而为 mock）──
export async function getConfig(): Promise<ConfigDocument>;
export async function getConfig(scope: ConfigScope): Promise<ScopeView>;
export async function getConfig(scope?: ConfigScope): Promise<ConfigDocument | ScopeView> {
  if (isTauri()) {
    return scope
      ? invoke<ScopeView>("getConfig", { scope })
      : invoke<ConfigDocument>("getConfig", {});
  }
  return mockGetConfig(scope);
}

export async function setConfig(
  scope: ConfigScope,
  patch: Record<string, unknown>
): Promise<ScopeView> {
  return isTauri()
    ? invoke<ScopeView>("setConfig", { scope, patch })
    : mockSetConfig(scope, patch);
}

export async function resetScope(scope: ConfigScope): Promise<ScopeView> {
  return isTauri()
    ? invoke<ScopeView>("resetScope", { scope })
    : mockResetScope(scope);
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

/** 读本机界面状态（设计 03 §11）：只用来恢复上次目录与最近目录。 */
export async function getUiState(): Promise<UiState> {
  if (!isTauri()) return mockGetUiState();
  return invoke<UiState>("getUiState");
}

/**
 * 写界面状态（浅合并：只替换 patch 里的顶层分节）。
 *
 * 调用方必须读改写——`navigation` 是整个对象替换，不能只传一个字段。
 */
export async function setUiState(patch: Record<string, unknown>): Promise<UiState> {
  if (!isTauri()) return mockSetUiState(patch);
  return invoke<UiState>("setUiState", { patch });
}

/** 重置界面状态（只影响布局/最近位置，不动配置与任务，设计 03 §11.3）。 */
export async function resetUiState(): Promise<UiState> {
  if (!isTauri()) return mockResetUiState();
  return invoke<UiState>("resetUiState");
}

// ── 处理模板（C37；设计 03 §7）────────────────────────────────────────
//
// 套用通道两条（OQ-40①/OQ-41②，ADR-0077/0080）：`describeRecipe` 静态评估一份
// 模板能不能套、要填哪些路径槽位（判定与 `loadRecipe` 同源）；`loadRecipe
// {recipeId, pathParams}` 按槽位注入文件集后走与 planOperation 同一条出口出
// **预览计划**——只出计划不执行（方案 §15.2.2），执行由调用方拿 planId 显式走 applyPlan。

export async function listRecipes(offset = 0, limit = 50): Promise<RecipePage> {
  return isTauri()
    ? invoke<RecipePage>("listRecipes", { offset, limit })
    : mockListRecipes(offset, limit);
}

export async function getRecipe(recipeId: string): Promise<RecipeDocument> {
  return isTauri() ? invoke<RecipeDocument>("getRecipe", { recipeId }) : mockGetRecipe(recipeId);
}

export async function describeRecipe(recipeId: string): Promise<RecipeApplySpec> {
  return isTauri()
    ? invoke<RecipeApplySpec>("describeRecipe", { recipeId })
    : mockDescribeRecipe(recipeId);
}

/**
 * 生成第 `stepIndex` 步的预览计划（Phase B：缺省第 1 步）。
 *
 * 多步模板**按步调用**：第 2 步起的输入由调用方给（`source: previous`＝上一步
 * 实际产物），壳层不预生成所有步骤——第 2 步的输入在第 1 步执行前不存在
 * （方案 §15.2.3）。只出计划不执行（§15.2.2）。
 */
export async function loadRecipe(
  recipeId: string,
  pathParams: Record<string, unknown>,
  stepIndex?: number
): Promise<PlanDocument> {
  return isTauri()
    ? // 缺省第 1 步：显式传 null（`Option<usize>` ← null），不依赖"缺 key 即 None"的序列化细节。
      invoke<PlanDocument>("loadRecipe", {
        recipeId,
        pathParams,
        stepIndex: stepIndex ?? null
      })
    : mockLoadRecipe(recipeId, pathParams, stepIndex);
}

export async function saveRecipe(input: SaveRecipeInput): Promise<RecipeDocument> {
  return isTauri()
    ? invoke<RecipeDocument>("saveRecipe", { recipe: input })
    : mockSaveRecipe(input);
}

export async function deleteRecipe(recipeId: string): Promise<void> {
  if (!isTauri()) return mockDeleteRecipe(recipeId);
  await invoke<void>("deleteRecipe", { recipeId });
}

export async function exportRecipe(recipeId: string): Promise<RecipeExportPayload> {
  return isTauri()
    ? invoke<RecipeExportPayload>("exportRecipe", { recipeId })
    : mockExportRecipe(recipeId);
}

/** 导入即低版本迁移、高版本 E1007；**导入后必须重新预览**（方案 §15.2.2）。 */
export async function importRecipe(payload: string): Promise<RecipeDocument> {
  return isTauri()
    ? invoke<RecipeDocument>("importRecipe", { payload })
    : mockImportRecipe(payload);
}

// ── F15 AI 助手（设计 14 §3；Phase 1）────────────────────────────────
export async function getAiStatus(): Promise<AiStatus> {
  return isTauri() ? invoke<AiStatus>("getAiStatus") : mockAiStatus();
}

export async function aiAsk(input: AiAskInput): Promise<AiAskOutput> {
  return isTauri() ? invoke<AiAskOutput>("aiAsk", { input }) : mockAiAsk(input);
}

/** 取消进行中的 AI 请求；幂等（壳层按 messageId 找取消标记）。 */
export async function aiCancel(messageId: string): Promise<boolean> {
  return isTauri() ? invoke<boolean>("aiCancel", { messageId }) : mockAiCancel(messageId);
}

/** 把 API key 写入系统凭据库；浏览器/非 Windows 不提供该入口（OQ-44）。 */
export async function setAiApiKey(apiKey: string): Promise<void> {
  if (!isTauri()) return;
  await invoke<void>("setAiApiKey", { apiKey });
}

// ── 支持包（设计 10 §5；方案 §9.7／§12.5）──────────────────────────────
//
// 两步是刻意的：`buildSupportPackage` 只读，给界面做**预览**（方案 §9.7「用户可在界面
// 预览将上传的内容」）；`writeSupportPackage` 写回的就是刚预览过的那份，不重新组装。
// 落点由壳层定（`<app_data>/exports/`），界面拿到的只是路径。

export async function buildSupportPackage(
  input: SupportPackageInput
): Promise<SupportPackage> {
  return isTauri()
    ? invoke<SupportPackage>("buildSupportPackage", { input })
    : mockBuildSupportPackage(input);
}

/** 把预览过的支持包落盘，返回落盘路径（壳层 `<app_data>/exports/`）。 */
export async function writeSupportPackage(pkg: SupportPackage): Promise<string> {
  return isTauri() ? invoke<string>("writeSupportPackage", { package: pkg }) : mockWriteSupportPackage(pkg);
}

export function listenAiDelta(handler: (event: AiDeltaEvent) => void): Promise<UnlistenFn> {
  if (!isTauri()) {
    return Promise.resolve(mockListen(AI_DELTA, (payload) => handler(payload as AiDeltaEvent)));
  }
  return listen<AiDeltaEvent>(AI_DELTA, (event) => handler(event.payload));
}

export function listenAiTool(handler: (event: AiToolEvent) => void): Promise<UnlistenFn> {
  if (!isTauri()) {
    return Promise.resolve(mockListen(AI_TOOL, (payload) => handler(payload as AiToolEvent)));
  }
  return listen<AiToolEvent>(AI_TOOL, (event) => handler(event.payload));
}

export function listenAiBundle(handler: (event: AiBundleEvent) => void): Promise<UnlistenFn> {
  if (!isTauri()) {
    return Promise.resolve(mockListen(AI_BUNDLE, (payload) => handler(payload as AiBundleEvent)));
  }
  return listen<AiBundleEvent>(AI_BUNDLE, (event) => handler(event.payload));
}

export function listenAiDone(handler: (event: AiDoneEvent) => void): Promise<UnlistenFn> {
  if (!isTauri()) {
    return Promise.resolve(mockListen(AI_DONE, (payload) => handler(payload as AiDoneEvent)));
  }
  return listen<AiDoneEvent>(AI_DONE, (event) => handler(event.payload));
}

export function listenAiError(handler: (event: AiErrorEvent) => void): Promise<UnlistenFn> {
  if (!isTauri()) {
    return Promise.resolve(mockListen(AI_ERROR, (payload) => handler(payload as AiErrorEvent)));
  }
  return listen<AiErrorEvent>(AI_ERROR, (event) => handler(event.payload));
}
