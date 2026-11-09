// Browser-only demo adapter. It never touches the filesystem and is used only
// when the Vite app runs outside Tauri; real desktop runs call Rust commands.
import type {
  AiAskInput,
  AiAskOutput,
  AiStatus,
  ApplyEntryReport,
  PlanBundle,
  RenamePreviewCounts,
  RenamePreviewRow,
  ApplyReport,
  CleanupOutcome,
  ConfigDocument,
  ConfigScope,
  IpcError,
  IpcMeta,
  PlanDocument,
  PlanEntry,
  PlanOperationInput,
  PlanWarning,
  PreflightReport,
  ProgressEvent,
  ProgressPhase,
  RecipeDocument,
  RecipeExportPayload,
  RecipeApplySpec,
  RecipePage,
  SaveRecipeInput,
  ScopeView,
  SettingValue,
  StorageInfo,
  TaskDetail,
  TaskHandle,
  TaskPage,
  TaskJob,
  TaskState,
  TaskSummary,
  TemplateStep,
  UiState
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

// ── F15 AI 助手（设计 14 §3／§4；Phase 1）────────────────────────────
// 浏览器 mock 只验证 UI 契约，不代表真实 Provider；场景开关只在 isTauri()===false
// 分支生效（Playwright 用 ?aiScenario=，Vitest 用 mockSetAiScenario）。
export const AI_EVENT = {
  delta: "ai://delta",
  tool: "ai://tool",
  bundle: "ai://bundle",
  done: "ai://done",
  error: "ai://error"
} as const;

export type AiScenario =
  | "unconfigured"
  | "ready"
  | "clarify"
  | "forbidden-tool"
  | "bad-schema"
  | "multi-step"
  | "blocked-content"
  | "cycle"
  | "timeout"
  | "cancelled";

const AI_PLAN_ID = "01JAI0PLAN0000000000000000";
const AI_BUNDLE_ID = "01JAIBUNDLE000000000000000";
let aiScenario: AiScenario | null = null;
let aiAskCount = 0;
let aiCancelCount = 0;
let aiApplyCount = 0;
let aiMessageCounter = 0;
let lastAiAskInput: AiAskInput | null = null;
const aiCancelled = new Set<string>();
const aiEmitted: string[] = [];

/** 发事件前记名：测试据此断言事件顺序（delta → tool → bundle → done）。 */
function emitAi(event: keyof typeof AI_EVENT, payload: unknown) {
  aiEmitted.push(AI_EVENT[event]);
  emit(AI_EVENT[event], payload);
}

function scenarioFromQuery(): AiScenario | null {
  if (typeof window === "undefined") return null;
  const value = new URLSearchParams(window.location.search).get("aiScenario");
  return value ? (value as AiScenario) : null;
}

function currentAiScenario(): AiScenario {
  return aiScenario ?? scenarioFromQuery() ?? "ready";
}

/** Vitest 用：显式指定场景（优先于 ?aiScenario=）。 */
export function mockSetAiScenario(scene: AiScenario | null) {
  aiScenario = scene;
}

/** 每个用例前重置场景、计数、取消标记与 AI 事件订阅。 */
export function resetAiMock() {
  aiScenario = null;
  aiAskCount = 0;
  aiCancelCount = 0;
  aiApplyCount = 0;
  aiMessageCounter = 0;
  lastAiAskInput = null;
  aiCancelled.clear();
  aiEmitted.length = 0;
  for (const event of Object.values(AI_EVENT)) listeners.delete(event);
}

/** 测试断言用：mock 实际收到的请求与调用次数。 */
export function mockAiStats() {
  return {
    scenario: currentAiScenario(),
    askCount: aiAskCount,
    cancelCount: aiCancelCount,
    applyCount: aiApplyCount,
    lastAskInput: lastAiAskInput,
    emitted: [...aiEmitted]
  };
}

function aiError(code: string, message: string): IpcError {
  return { code, message, retryable: false };
}

function aiPlanFixture(): PlanDocument {
  const plan: PlanDocument = {
    planId: AI_PLAN_ID,
    kind: "f12",
    params: { prefix: "2026_" },
    entries: [
      {
        entryId: `${AI_PLAN_ID}-0001`,
        source: "/mock/ai/合同.pdf",
        target: "/mock/ai/2026_合同.pdf",
        action: "rename",
        reason: "AI 预览夹具：真实执行与撤销需桌面壳"
      }
    ],
    stats: { total: 1, conflict: 0, warning: 0, blocked: 0 },
    warnings: []
  };
  lastPlan = plan;
  return plan;
}

export function mockAiStatus(): AiStatus {
  const configured = currentAiScenario() !== "unconfigured";
  return {
    available: configured,
    configured,
    phase: 1,
    catalogVersion: "mock.p1",
    supportedKinds: ["f12", "f05", "f07", "c07"],
    provider: configured ? "mock" : undefined,
    model: configured ? "mock-phase1" : undefined,
    consentRequired: true
  };
}

export function mockAiAsk(input: AiAskInput): AiAskOutput {
  aiAskCount += 1;
  lastAiAskInput = input;
  const messageId = `ai-msg-${++aiMessageCounter}`;
  const scenario = currentAiScenario();
  let finished = false;
  const cancelled = () => aiCancelled.has(messageId);
  const finishError = (code: string, message: string) => {
    if (finished) return;
    finished = true;
    emitAi("error", { messageId, error: aiError(code, message) });
  };
  const finishDone = (message: string, bundle?: PlanBundle) => {
    if (finished) return;
    finished = true;
    emitAi("done", { messageId, message, ...(bundle ? { bundle } : {}) });
  };
  const guard = () => {
    if (finished) return true;
    if (!cancelled()) return false;
    finishError("E6007", "cancelled by the user");
    return true;
  };

  void (async () => {
    await wait(10);
    if (guard()) return;
    if (scenario === "unconfigured") return finishError("E6001", "AI provider is not configured");
    if (scenario === "timeout") return finishError("E6006", "model request timed out");
    if (scenario === "bad-schema") return finishError("E6003", "model output failed schema validation");
    if (scenario === "multi-step") return finishError("E6011", "multi-step intents are not supported");
    if (scenario === "blocked-content") return finishError("E6005", "content policy rejected the request");
    if (scenario === "cycle") return finishError("E6010", "plan bundle has a dependency cycle");
    if (scenario === "forbidden-tool") {
      emitAi("delta", { messageId, text: "正在尝试直接执行…" });
      emitAi("tool", {
        messageId,
        call: { callId: "c1", name: "applyPlan", arguments: {} },
        allowed: false
      });
      return finishError("E6004", "tool is forbidden for the AI assistant");
    }
    emitAi("delta", { messageId, text: "已理解你的目标。" });
    // cancelled 场景留出足够窗口让用户点到「取消」：其余场景保持短时序。
    await wait(scenario === "cancelled" ? 300 : 10);
    if (guard()) return;
    if (scenario === "clarify") {
      return finishDone("要把哪些文件改名？");
    }
    emitAi("delta", { messageId, text: "正在生成计划…" });
    emitAi("tool", {
      messageId,
      call: { callId: "c1", name: "planOperation", arguments: { kind: "f12" } },
      allowed: true
    });
    const plan = aiPlanFixture();
    const bundle: PlanBundle = {
      bundleId: AI_BUNDLE_ID,
      steps: [{ stepId: "s1", plan, dependsOn: [], status: "ready" }],
      conflicts: [],
      status: "ready"
    };
    emitAi("bundle", { messageId, bundle });
    finishDone("已生成计划，请在预览里核对。", bundle);
  })();

  return { messageId };
}

export function mockAiCancel(messageId: string): boolean {
  aiCancelCount += 1;
  aiCancelled.add(messageId);
  return true;
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
  // 非 C09 功能切片：给出同形状的最小夹具（只是界面预览，不代表文件系统真实结果）。
  // 没有这一段时，功能页在浏览器预览里会拿到 C09 专属的"E1001 请填写扫描目录"。
  if (input.kind !== "c09") {
    const firstPath = Object.values(input.params).find(
      (value) => typeof value === "string" && value.startsWith("/")
    ) as string | undefined;
    const base = (firstPath ?? "/mock").replace(/\/+$/, "");
    // F11 夹具按 kernel 的同形状给一条集中报告：多级路径的父级位置若是文件，该分支在计划期
    // 就 skip 并进 warnings。只证面板"逐项点名"的 UI 语义，不代表真机文件结果（ADR-0046）。
    // 参数面按值形态取：PlanOperationInput 的类型是 C09 形状，不含功能页自己的字段。
    const listValue = Object.values(input.params).find(
      (value) => typeof value === "string" && value.includes("/") && !value.startsWith("/")
    );
    const branch =
      typeof listValue === "string"
        ? listValue
            .split(/\r?\n/)
            .map((line) => line.trim())
            .find((line) => line.includes("/"))
        : undefined;
    const warnings: PlanWarning[] =
      input.kind === "f11" && branch
        ? [
            {
              kind: "parent-is-file",
              suggestion: `${base}/${branch} (父级 ${base}/${branch.split("/")[0]} 是文件，本分支已跳过)`,
              riskLevel: "caution"
            }
          ]
        : [];
    lastPlan = {
      planId: PLAN_ID,
      kind: input.kind,
      params: { ...input.params },
      entries: [
        {
          entryId: `${PLAN_ID}-0001`,
          source: `${base}/sample-a.txt`,
          target: `${base}/sample-b.txt`,
          action: "rename",
          reason: `${input.kind} 预览夹具：真实执行与撤销需桌面壳`
        }
      ],
      stats: { total: 1, conflict: 0, warning: warnings.length, blocked: 0 },
      warnings
    };
    return lastPlan;
  }
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
  /** 任务类别（执行/回滚）。列表与 done 事件都用它判别，不再借用计划功能码。 */
  kind: TaskJob;
  /** 被处理计划的功能码：与壳层一致，`summary.kind` 取这个值。 */
  planKind: string;
  state: TaskState;
  phase: ProgressPhase;
  done: number;
  total: number;
  message?: string;
  startedAt?: string;
  finishedAt?: string;
  entries: ApplyEntryReport[];
  /** 任务对应的计划：任务列表据此提供「撤销」。 */
  planId?: string;
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
    kind: task.planKind,
    job: task.kind,
    state: task.state,
    phase: task.phase,
    done: task.done,
    total: task.total,
    planId: task.planId,
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
  emit(TASK_EVENT.done, { taskId: task.taskId, kind: task.kind, summary: reportFor(task) });
}

function tick(task: MockTask) {
  if (task.cancelled) {
    // 方案 §7.7：取消只停派发；已完成条目保留，由界面询问是否一并撤销。
    finish(task, "cancelled", "已取消后续条目；已完成项保留在磁盘，可在提示中一并撤销");
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
  entries: ApplyEntryReport[],
  planId?: string
): MockTask {
  const task: MockTask = {
    taskId: nextTaskId(kind),
    kind,
    planKind: lastPlan?.kind ?? "c09",
    planId,
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
  // AI 验证用：任何 AI 流程都不得走到 apply（测试断言此计数恒为 0）。
  aiApplyCount += 1;
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
  const task = createTask("apply", entries, planId);
  return { taskId: task.taskId };
}

export async function mockUndo(planId: string): Promise<TaskHandle> {
  if (!lastPlan || lastPlan.planId !== planId) fail("E1001", "计划不存在，请重新生成预览");
  const entries: ApplyEntryReport[] = lastPlan.entries
    .filter((entry) => !entry.blocked)
    .map((entry) => ({ entryId: entry.entryId, status: "ok" }));
  const task = createTask("undo", entries, planId);
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
  const retry = createTask("apply", entries, task.planId);
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

// C05/C06 mock：浏览器预览用（无真实计划存储）。
let lastRenamePreview: {
  planId: string;
  kind: string;
  counts: RenamePreviewCounts;
  rows: RenamePreviewRow[];
  highlights: string[];
} | null = null;

export function mockRenamePreview(planId: string) {
  lastRenamePreview = {
    planId,
    kind: "f12",
    counts: { toProcess: 1, skipped: 0, conflicts: 0, blocked: 0 },
    rows: [
      {
        entryId: `${planId}-0001`,
        oldPath: "/mock/dup.txt",
        newPath: "/mock/uniq.txt",
        status: "normal",
        twoPhase: false
      }
    ],
    highlights: []
  };
  return lastRenamePreview;
}

export function mockUndoDryRun(planId: string) {
  return {
    planId,
    entries: [
      { entryId: `${planId}-0001`, status: "ok" as const }
    ],
    conflictCount: 0,
    missingCount: 0
  };
}

// ── 配置（设计 03 §4.1 工程默认值／§4.2 安全默认值）────────────────────
// 浏览器预览没有真实配置文件：用户档只放内存，刷新即回默认。
const CONFIG_DEFAULTS: Record<ConfigScope, Record<string, unknown>> = {
  general: {
    language: "zh-CN",
    theme: "light",
    defaultOutputDir: null,
    launchBehavior: "normal"
  },
  processing: {
    onConflict: "skip",
    encodingPolicy: "auto",
    newlinePolicy: "keep",
    deletionMode: "move-to-quarantine",
    keepBackup: true
  },
  performance: {
    concurrency: "auto",
    memoryBudget: "auto",
    largeFileThreshold: "auto"
  },
  security: {
    protectedDirs: [],
    quarantineDir: null,
    quarantineRetentionDays: null,
    placeholderPolicy: "skip",
    diskSpaceMarginFloorBytes: null,
    diskSpaceMarginRatio: null
  },
  logging: {
    retentionDays: null,
    logFullPaths: false
  }
};

const configUser: Record<ConfigScope, Record<string, unknown>> = {
  general: {},
  processing: {},
  performance: {},
  security: {},
  logging: {}
};

function scopeDefaults(scope: string): Record<string, unknown> {
  const defaults = CONFIG_DEFAULTS[scope as ConfigScope];
  if (!defaults) fail("E1002", `unknown config scope: ${scope}`);
  return defaults;
}

function scopeView(scope: string): ScopeView {
  const defaults = scopeDefaults(scope);
  const user = configUser[scope as ConfigScope];
  const values: Record<string, SettingValue> = {};
  for (const [key, defaultValue] of Object.entries(defaults)) {
    const hasUser = Object.prototype.hasOwnProperty.call(user, key);
    values[key] = {
      value: hasUser ? user[key] : defaultValue,
      effectiveSource: hasUser ? "user" : "default",
      ...(hasUser ? { userValue: user[key] } : {}),
      defaultValue
    };
  }
  return { scope: scope as ConfigScope, values };
}

export function mockGetConfig(scope?: string): ConfigDocument | ScopeView {
  if (scope) return scopeView(scope);
  const scopes: ConfigDocument["scopes"] = {};
  for (const key of Object.keys(CONFIG_DEFAULTS) as ConfigScope[]) {
    const user = configUser[key];
    if (Object.keys(user).length > 0) scopes[key] = { ...user };
  }
  return { configVersion: 1, updatedAt: new Date().toISOString(), scopes };
}

export function mockSetConfig(scope: string, patch: Record<string, unknown>): ScopeView {
  const defaults = scopeDefaults(scope);
  for (const [key, value] of Object.entries(patch)) {
    if (!Object.prototype.hasOwnProperty.call(defaults, key)) {
      fail("E1002", `unknown config key: ${scope}.${key}`);
    }
    if (value === null || value === undefined) delete configUser[scope as ConfigScope][key];
    else configUser[scope as ConfigScope][key] = value;
  }
  return scopeView(scope);
}

export function mockResetScope(scope: string): ScopeView {
  scopeDefaults(scope);
  configUser[scope as ConfigScope] = {};
  return scopeView(scope);
}

// —— UiState（设计 03 §11）：读写全部已声明分节，透传口径与壳层一致 ——
let uiState: UiState = { uiStateVersion: 1, updatedAt: "" };

export function mockGetUiState(): UiState {
  return {
    uiStateVersion: uiState.uiStateVersion,
    updatedAt: uiState.updatedAt,
    navigation: uiState.navigation ? { ...uiState.navigation } : undefined,
    layout: uiState.layout ? { ...uiState.layout } : undefined,
    tables: uiState.tables
      ? Object.fromEntries(
          Object.entries(uiState.tables).map(([id, table]) => [id, { ...table }])
        )
      : undefined
  };
}

export function mockSetUiState(patch: Record<string, unknown>): UiState {
  // 与壳层 `UiState::apply_patch` 同口径：这两个字段由壳层管，调用方不得写入。
  if ("uiStateVersion" in patch || "updatedAt" in patch) {
    fail("E1002", "uiStateVersion/updatedAt are managed by the shell");
  }
  const next = { ...uiState } as UiState & Record<string, unknown>;
  for (const [key, value] of Object.entries(patch)) next[key] = value;
  next.updatedAt = new Date().toISOString();
  uiState = next as UiState;
  return mockGetUiState();
}

export function mockResetUiState(): UiState {
  uiState = { uiStateVersion: 1, updatedAt: "" };
  return mockGetUiState();
}

// —— 处理模板（C37；设计 03 §7）——
// 只是一份同形状夹具：不碰磁盘、不做版本迁移（迁移与拒绝高版本由内核/壳层负责），
// 但**保留两条语义**——缺 schemaVersion 报 E1001、高版本报 E1007，
// 否则浏览器里永远看不到「导入被拒」那条界面路径。
let recipeCounter = 0;
let recipes: RecipeDocument[] = [
  {
    recipeId: "01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8R1",
    schemaVersion: 5,
    name: "CSV 分片（每片 5 万行）",
    description: "演示夹具：真实模板由桌面壳落盘",
    steps: [
      {
        stepId: "step-1",
        kind: "split",
        // `split` 是**一对多**动作（f01/f02/f03/f05），这里刻意留绑定值 f05：
        // 夹具的作用是演示 v4 的形状，不是证明界面能反推——反推不出来时该显示「未绑定」。
        feature: "f05",
        paramsSnapshot: { rows: 50000 },
        source: "original"
      }
    ],
    match: { extensions: ["csv"], tags: ["分割"] },
    createdAt: "2026-11-01T09:00:00Z",
    updatedAt: "2026-11-04T10:00:00Z",
    usageCount: 3
  }
];

const PATH_KEYS = new Set([
  "file",
  "files",
  "path",
  "paths",
  "sources",
  "sourcepath",
  "sourcepaths",
  "output",
  "outputdir",
  "outputdirectory",
  "dir",
  "directory",
  "folder",
  "root",
  "roots",
  "sourceroot",
  "targetroot",
  "unclassifieddir",
  "list",
  "manifest",
  "archive"
]);

function stripPaths(params: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(params).filter(([key]) => !PATH_KEYS.has(key.toLowerCase()))
  );
}

export function mockListRecipes(offset = 0, limit = 50): RecipePage {
  return { items: recipes.slice(offset, offset + limit), total: recipes.length };
}

export function mockGetRecipe(recipeId: string): RecipeDocument {
  const found = recipes.find((item) => item.recipeId === recipeId);
  if (!found) fail("E2001", `recipe not found: ${recipeId}`);
  return found;
}

/**
 * 与内核 `feature_for_action` 同一张表：只登记**动作→功能码唯一**的项。
 * 一对多的六个动作族必须留 `undefined`，浏览器侧不许比壳层更宽松。
 */
const ONE_TO_ONE_FEATURES: Record<string, string> = {
  delete: "c09",
  "csv-extract": "f07",
  listing: "f10",
  "create-dir": "f11",
  archive: "c10",
  extract: "c10",
  "csv-dedup": "c11",
  "csv-filter": "c12",
  "csv-sort": "c13",
  "csv-columns": "c14",
  copy: "c07"
};

function inferFeature(kind: string): string | undefined {
  return ONE_TO_ONE_FEATURES[kind];
}

export function mockSaveRecipe(input: SaveRecipeInput): RecipeDocument {
  const steps: TemplateStep[] = input.steps
    ? input.steps.map((step) => ({
        ...step,
        feature: step.feature ?? inferFeature(step.kind),
        paramsSnapshot: stripPaths(step.paramsSnapshot)
      }))
    : [
        {
          stepId: "step-1",
          kind: input.kind ?? "rename",
          feature: input.feature ?? inferFeature(input.kind ?? "rename"),
          sampling: input.sampling,
          content: input.content,
          paramsSnapshot: stripPaths(input.paramsSnapshot ?? {}),
          source: "original"
        }
      ];
  const existing = input.recipeId
    ? recipes.find((item) => item.recipeId === input.recipeId)
    : undefined;
  recipeCounter += 1;
  const document: RecipeDocument = {
    recipeId: input.recipeId ?? `01J9Z3K8Y6Q8Q8Q8Q8Q8Q8Q8S${recipeCounter}`,
    schemaVersion: 5,
    name: input.name,
    description: input.description,
    steps,
    match: input.match,
    createdAt: existing?.createdAt ?? new Date().toISOString(),
    updatedAt: new Date().toISOString(),
    usageCount: existing?.usageCount
  };
  recipes = existing
    ? recipes.map((item) => (item.recipeId === document.recipeId ? document : item))
    : [...recipes, document];
  return document;
}

export function mockDeleteRecipe(recipeId: string): void {
  recipes = recipes.filter((item) => item.recipeId !== recipeId);
}

export function mockExportRecipe(recipeId: string): RecipeExportPayload {
  return { payload: JSON.stringify(mockGetRecipe(recipeId), null, 2) };
}

/**
 * 需要变换正文的功能码与键集（v6，OQ-43 裁决②，ADR-0081）：这些功能的请求侧
 * 语义（规则/列选择/条件/排序键/拆分合并规格/来源列开关）从不进 plan.params，
 * 模板没有正文就回放不了——describe 据此判定可套用与否（缺正文＝提示重存）。
 * 键集与壳层 `recipe_apply::FEATURE_PARAM_MAP` 的 content 声明同源，
 * 由 check_docs.py 逐码核对。f11 不在清单：清单**文件**是路径槽位、路由重读
 * 其内容（ADR-0081 §2 纠偏，OQ-43 登记时误列）。
 */
const CONTENT_KEYS: Record<string, { required: string[]; optional: string[]; any: string[][] }> = {
  f06: { required: ["sourceColumn"], optional: ["sourceColumnName", "sourceColumnRelative"], any: [] },
  f07: { required: [], optional: [], any: [["columns", "columnIndexes"]] },
  c08: { required: ["rules"], optional: ["recursive"], any: [] },
  c12: { required: ["value"], optional: ["column", "op", "filterMode"], any: [["column", "columnIndex"]] },
  c13: { required: [], optional: ["order", "externalChunkRows"], any: [["sortColumns", "sortColumnIndexes"]] },
  c14: { required: [], optional: ["splitColumnIndex", "splitSeparator", "splitRegex", "mergeColumnIndexes", "mergeJoinWith", "mergeName"], any: [["splitColumn", "mergeColumns"]] }
};

/**
 * 浏览器夹具的槽位表（`describeRecipe` 出参用）。与壳层
 * `recipe_apply::FEATURE_PATH_SLOTS` 同源，由 check_docs.py 逐码核对
 * （键名/单复数/多值标记，含 c09 的 sourcePaths 通道）。
 * 只登记演示与测试会用到的功能；未登记的功能如实报"夹具没有槽位"。
 */
const APPLY_SLOTS: Record<string, { required: [string, boolean][]; outputs: [string, boolean][] }> = {
  c09: { required: [["sourcePaths", true]], outputs: [] },
  f01: { required: [["file", false]], outputs: [["outputDir", false]] },
  f02: { required: [["file", false]], outputs: [["outputDir", false]] },
  f03: { required: [["file", false]], outputs: [["outputDir", false]] },
  f04: { required: [["files", true]], outputs: [["output", false]] },
  f05: { required: [["file", false]], outputs: [["outputDir", false]] },
  f06: { required: [["files", true]], outputs: [["output", false]] },
  f07: { required: [["files", true]], outputs: [["output", false], ["outputDir", false]] },
  f08: { required: [["files", true]], outputs: [["output", false], ["outputDir", false]] },
  f09: { required: [["files", true]], outputs: [["output", false], ["outputDir", false]] },
  f10: { required: [["roots", true]], outputs: [["output", false], ["outputDir", false]] },
  f11: { required: [["list", false], ["root", false]], outputs: [] },
  f12: { required: [["dir", false]], outputs: [] },
  c01: { required: [["files", true]], outputs: [] },
  c02: { required: [["files", true]], outputs: [] },
  c03: { required: [["files", true]], outputs: [] },
  c04: { required: [["dir", false]], outputs: [] },
  c07: { required: [["manifest", false], ["sourceRoot", false]], outputs: [["targetRoot", false]] },
  c08: { required: [["sourceRoot", false]], outputs: [["targetRoot", false]] },
  c10: { required: [["archive", false]], outputs: [["outputDir", false]] },
  c11: { required: [["files", true]], outputs: [["output", false], ["outputDir", false]] },
  c12: { required: [["files", true]], outputs: [["output", false], ["outputDir", false]] },
  c13: { required: [["files", true]], outputs: [["output", false], ["outputDir", false]] },
  c14: { required: [["files", true]], outputs: [["output", false], ["outputDir", false]] }
};

/** 浏览器夹具的 loadRecipe：槽位校验与壳层同语义（缺/多/空都报），事实采集是夹具（同 mockPlan），返回同形状的预览计划。 */
export function mockLoadRecipe(recipeId: string, pathParams: Record<string, unknown>): Promise<PlanDocument> {
  const spec = mockDescribeRecipe(recipeId);
  if (!spec.applicable || !spec.feature) {
    fail("E1002", `loadRecipe: ${spec.reason ?? "recipe is not applicable"}`);
  }
  const declared = new Set([...spec.required, ...spec.outputs].map((slot) => slot.key));
  for (const key of Object.keys(pathParams)) {
    if (!declared.has(key)) {
      fail(
        "E1002",
        `loadRecipe: ${spec.feature} has no path slot ${key}; declared slots are ${[...declared].join(", ")}`
      );
    }
  }
  const missing = spec.required.filter((slot) => {
    const value = pathParams[slot.key];
    return value === undefined || (typeof value === "string" && value.trim().length === 0);
  });
  if (missing.length > 0) {
    fail(
      "E1002",
      `loadRecipe: ${spec.feature} needs the path slots: ${missing.map((slot) => slot.key).join(", ")}`
    );
  }
  if (
    spec.outputs.length > 0 &&
    !spec.outputs.some((slot) => {
      const value = pathParams[slot.key];
      return typeof value === "string" ? value.trim().length > 0 : value !== undefined;
    })
  ) {
    fail(
      "E1002",
      `loadRecipe: ${spec.feature} needs one of the output slots: ${spec.outputs.map((slot) => slot.key).join(", ")}`
    );
  }
  const step = mockGetRecipe(recipeId).steps[0];
  // 正文白名单外的键拒绝（与壳层同语义，方案 §15.2.4 不静默丢）
  const contentSpec2 = CONTENT_KEYS[spec.feature];
  if (contentSpec2 && step.content) {
    const declared = new Set([...contentSpec2.required, ...contentSpec2.any.flat()]);
    for (const key of Object.keys(step.content)) {
      if (!declared.has(key)) {
        fail("E1002", `loadRecipe: ${spec.feature} has no content key ${key}`);
      }
    }
  }
  // PlanOperationInput 的 kind 字段在类型上只写了 "c09"（C09 形状先行），
  // mockPlan 运行时本就按非 c09 分支给最小夹具——与既有用法一致，这里显式收窄。
  return mockPlan({
    kind: step.kind as "c09",
    params: { ...step.paramsSnapshot, ...(step.content ?? {}), ...pathParams } as PlanOperationInput["params"],
    sourcePaths: []
  });
}

export function mockDescribeRecipe(recipeId: string): RecipeApplySpec {
  const document = mockGetRecipe(recipeId);
  const step = document.steps[0];
  const base = {
    recipeId: document.recipeId,
    feature: step?.feature,
    kind: step?.kind
  };
  if (!step || document.steps.length > 1 || !step.feature) {
    return {
      ...base,
      applicable: false,
      reason: step
        ? document.steps.length > 1
          ? "multi-step recipe is not applied in one click; plan each step separately"
          : "recipe step has no feature binding; re-save it from that feature page"
        : "recipe has no step to apply",
      required: [],
      outputs: []
    };
  }
  const feature = step.feature;
  const contentSpec = CONTENT_KEYS[feature];
  if (contentSpec) {
    // 缺正文＝回放不了该功能的变换语义：如实拒绝并提示重存（v6 起保存通道采集正文）
    const content = (step.content ?? {}) as Record<string, unknown>;
    const has = (key: string) =>
      content[key] !== undefined && content[key] !== null;
    const missingRequired = contentSpec.required.filter((key) => !has(key));
    const anyMissed = contentSpec.any.some((group) => !group.some(has));
    if (missingRequired.length > 0 || anyMissed) {
      return {
        ...base,
        applicable: false,
        reason:
          "transformation content is not stored in this template; re-save it from the feature page (schema v6)",
        required: [],
        outputs: []
      };
    }
  }
  const slots = APPLY_SLOTS[feature];
  if (!slots) {
    return {
      ...base,
      applicable: false,
      reason: `browser fixture has no slots registered for ${feature}`,
      required: [],
      outputs: []
    };
  }
  return {
    ...base,
    applicable: true,
    required: slots.required.map(([key, many]) => ({ key, many })),
    outputs: slots.outputs.map(([key, many]) => ({ key, many }))
  };
}

export function mockImportRecipe(payload: string): RecipeDocument {
  let parsed: unknown;
  try {
    parsed = JSON.parse(payload);
  } catch {
    fail("E1002", "invalid recipe payload");
  }
  const value = parsed as Partial<RecipeDocument> & {
    paramsSnapshot?: Record<string, unknown>;
    kind?: TemplateStep["kind"];
  };
  const version = value.schemaVersion;
  if (typeof version !== "number") fail("E1001", "schemaVersion is required");
  if (version > 3) fail("E1007", `模板版本 ${version} 高于本版支持的 3`);
  const steps: TemplateStep[] = value.steps
    ? value.steps.map((step) => ({ ...step, paramsSnapshot: stripPaths(step.paramsSnapshot) }))
    : [
        {
          stepId: "step-1",
          kind: value.kind ?? "rename",
          paramsSnapshot: stripPaths(value.paramsSnapshot ?? {}),
          source: "original"
        }
      ];
  return mockSaveRecipe({
    name: value.name ?? "导入的模板",
    description: value.description,
    match: value.match,
    steps
  });
}
