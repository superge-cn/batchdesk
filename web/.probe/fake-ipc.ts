// `./lib/ipc` 的测试替身：只实现 App / FeaturePages / SettingsPage 用到的导出，
// 并记录调用（对应 committed 测试里的"零调用/被调用"断言）。
// 与真实壳层的差异都写在这里，不假装完整。

export const PLAN = {
  planId: "01UISTATE",
  kind: "c09",
  entries: [{ entryId: "e1", source: "/data/last/a.pdf", action: "delete", reason: "duplicate" }],
  stats: { total: 1, conflict: 0, warning: 0, blocked: 0 },
  warnings: [],
  params: {}
};

export const PASS_REPORT = {
  items: [],
  level: "pass",
  pass: 9,
  warn: 0,
  block: 0,
  taskBlocked: false
};

type UiNavigation = {
  lastSection?: string;
  lastFeature?: string;
  lastDirectory?: string;
  recentDirectories?: string[];
};

export const state = {
  uiState: { uiStateVersion: 1, updatedAt: "" } as {
    uiStateVersion: number;
    updatedAt: string;
    navigation?: UiNavigation;
  },
  setUiStateCalls: [] as Array<Record<string, unknown>>,
  resetUiStateCalls: 0,
  planCalls: 0,
  settledState: "cancelled" as string,
  planNull: false,
  planOverride: null as Record<string, unknown> | null,
  getTaskNull: false,
  undoCalls: [] as string[],
  calls: { getConfig: 0, setConfig: 0, resetScope: 0 }
};

// 兼容设置页探针里的旧名字
export const calls = state.calls;

export function resetState(navigation?: UiNavigation) {
  state.uiState = {
    uiStateVersion: 1,
    updatedAt: navigation ? "2026-11-03T00:00:00Z" : "",
    navigation
  };
  state.setUiStateCalls = [];
  state.resetUiStateCalls = 0;
  state.planCalls = 0;
  state.undoCalls = [];
  state.settledState = "cancelled";
  state.planNull = false;
  state.planOverride = null;
  state.getTaskNull = false;
  state.calls = { getConfig: 0, setConfig: 0, resetScope: 0 };
}

export const isTauri = () => true;

export async function getStorageInfo() {
  return { quarantineDir: "/q", journalFile: "/j", quarantineBytes: 0, journalBytes: 0, logBytes: 0 };
}
export async function getIpcMeta() {
  return { contractVersion: 1, kernelVersion: "0.0.0", version: "test", commands: [], events: [] };
}
export async function listTasks() {
  return { items: [], total: 0 };
}
export async function getTask(taskId: string) {
  if (state.getTaskNull) return null;
  return {
    taskId,
    summary: { state: state.settledState, phase: "finalizing", done: 2, total: 3, message: "" },
    entries: []
  };
}
export async function listenTaskProgress() {
  return () => undefined;
}
export async function listenTaskState() {
  return () => undefined;
}
export const doneListeners: Array<(event: unknown) => void> = [];
export function listenTaskDone(callback: (event: unknown) => void) {
  doneListeners.push(callback);
  return Promise.resolve(() => undefined);
}
export async function planOperation() {
  state.planCalls += 1;
  if (state.planOverride) return state.planOverride;
  return state.planNull ? null : PLAN;
}
export async function preflightPlan() {
  return PASS_REPORT;
}
export async function applyPlan() {
  return { taskId: "task-apply" };
}
export async function undoPlan(planId: string) {
  state.undoCalls.push(planId);
  return { taskId: "task-undo" };
}
export async function undoDryRun() {
  return { entries: [] };
}
export async function cancelTask() {
  return undefined;
}
export async function retryTask() {
  return { taskId: "task-retry" };
}
export async function pauseTask() {
  return undefined;
}
export async function resumeTask() {
  return undefined;
}
export async function recoverTask() {
  return { taskId: "task-recover" };
}
export async function cleanupRetention() {
  return { removedEntries: 0, removedLogRecords: 0 };
}
export async function renamePreview() {
  return { rows: [], counts: {} };
}
export async function saveRecipe() {
  return { recipeId: "r1" };
}

export async function getUiState() {
  return state.uiState;
}
export async function setUiState(patch: Record<string, unknown>) {
  state.setUiStateCalls.push(patch);
  state.uiState = { ...state.uiState, ...(patch as object) } as typeof state.uiState;
  return state.uiState;
}
export async function resetUiState() {
  state.resetUiStateCalls += 1;
  state.uiState = { uiStateVersion: 1, updatedAt: "" };
  return state.uiState;
}

export async function getConfig(scope: string) {
  state.calls.getConfig += 1;
  return { scope, values: {} };
}
export async function setConfig() {
  state.calls.setConfig += 1;
  return { scope: "general", values: {} };
}
export async function resetScope(scope: string) {
  state.calls.resetScope += 1;
  return { scope, values: {} };
}

export function toIpcError(value: unknown) {
  return value;
}

// TemplatesPage（顶层视图 templates）用到的配方命令
// 与 web/src/lib/ipc/mock.ts 的预置夹具同名同形，让"模板列表非空"这类断言可复刻。
const FIXTURE_RECIPE = {
  recipeId: "01FIXTURE",
  schemaVersion: 3,
  name: "CSV 分片（每片 5 万行）",
  description: "演示夹具：真实模板由桌面壳落盘",
  steps: [
    {
      stepId: "step-1",
      kind: "split",
      paramsSnapshot: { rows: 50000 },
      source: "original"
    }
  ],
  match: { extensions: ["csv"], tags: ["分割"] },
  createdAt: "2026-11-01T09:00:00Z",
  updatedAt: "2026-11-04T10:00:00Z"
};

export async function listRecipes(offset = 0, limit = 200) {
  void offset;
  void limit;
  return { items: [FIXTURE_RECIPE], total: 1 };
}
export async function deleteRecipe() {
  return undefined;
}
export async function exportRecipe() {
  return { json: "{}" };
}
export async function importRecipe() {
  return { recipeId: "r1" };
}
