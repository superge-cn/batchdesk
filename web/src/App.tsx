import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  Files,
  FolderSearch,
  HardDrive,
  History,
  Info,
  Loader2,
  Moon,
  Pause,
  Play,
  RefreshCw,
  Settings,
  ShieldCheck,
  Sun,
  Trash2,
  Undo2,
  X
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import {
  applyPlan,
  cancelTask,
  cleanupRetention,
  getIpcMeta,
  getStorageInfo,
  getTask,
  isTauri,
  getUiState,
  listTasks,
  listenTaskDone,
  listenTaskProgress,
  listenTaskState,
  pauseTask,
  planOperation,
  preflightPlan,
  recoverTask,
  resumeTask,
  retryTask,
  setUiState,
  toIpcError,
  undoPlan
} from "./lib/ipc";
import { FeaturePages } from "./FeaturePages";
import { DangerConfirm } from "./DangerConfirm";
import { ErrorBanner } from "./ErrorBanner";
import { VirtualList } from "./VirtualList";
import { phaseLabel, stateBadge, type Translate } from "./labels";
import { TaskHistory } from "./TaskHistory";
import { SettingsPage } from "./SettingsPage";
import { I18nProvider, useI18n, type MessageKey } from "./i18n";
import { preferredDirectory, rememberDirectory } from "./lib/uiState";
import type {
  ApplyReport,
  CleanupOutcome,
  IpcError,
  IpcMeta,
  KeepPolicy,
  PlanDocument,
  PreflightReport,
  ProgressPhase,
  UiNavigationState,
  RecoveryAction,
  StorageInfo,
  TaskState,
  TaskSummary
} from "./lib/ipc/types";

type View =
  | "c09"
  | "tasks"
  | "settings"
  | "f01"
  | "f02"
  | "f03"
  | "f04"
  | "c01"
  | "c02"
  | "c03"
  | "f05"
  | "f06"
  | "f07"
  | "f08"
  | "f09"
  | "f10"
  | "f11"
  | "f12"
  | "c04"
  | "c07"
  | "c08"
  | "c10"
  | "c11"
  | "c12"
  | "c13"
  | "c14";

type Busy = "plan" | "cleanup" | null;

interface TaskView {
  taskId: string;
  state: TaskState;
  phase: ProgressPhase;
  done: number;
  total: number;
  message?: string;
  report?: ApplyReport;
}

const TERMINAL_STATES: TaskState[] = [
  "completed",
  "partial-failed",
  "failed",
  "cancelled"
];

/** 侧栏导航：可见名走资源包，右侧短码是功能标识（不翻译）。 */
const NAV_ITEMS: Array<{ id: View; label: MessageKey; tag: string; Icon: LucideIcon }> = [
  { id: "c09", label: "nav.c09", tag: "C09", Icon: FolderSearch },
  { id: "f05", label: "nav.f05", tag: "F05", Icon: Files },
  { id: "f06", label: "nav.f06", tag: "F06", Icon: Files },
  { id: "f07", label: "nav.f07", tag: "F07", Icon: Files },
  { id: "f08", label: "nav.f08", tag: "F08", Icon: Files },
  { id: "f09", label: "nav.f09", tag: "F09", Icon: Files },
  { id: "f10", label: "nav.f10", tag: "F10", Icon: Files },
  { id: "c04", label: "nav.c04", tag: "C04", Icon: Files },
  { id: "c11", label: "nav.c11", tag: "C11", Icon: Files },
  { id: "c12", label: "nav.c12", tag: "C12", Icon: Files },
  { id: "c13", label: "nav.c13", tag: "C13", Icon: Files },
  { id: "c14", label: "nav.c14", tag: "C14", Icon: Files },
  { id: "c07", label: "nav.c07", tag: "C07", Icon: Files },
  { id: "c08", label: "nav.c08", tag: "C08", Icon: Files },
  { id: "c10", label: "nav.c10", tag: "C10", Icon: Files },
  { id: "f11", label: "nav.f11", tag: "F11", Icon: Files },
  { id: "f12", label: "nav.f12", tag: "F12", Icon: Files },
  { id: "f01", label: "nav.f01", tag: "F01", Icon: Files },
  { id: "f02", label: "nav.f02", tag: "F02", Icon: Files },
  { id: "f03", label: "nav.f03", tag: "F03", Icon: Files },
  { id: "f04", label: "nav.f04", tag: "F04", Icon: Files },
  { id: "c01", label: "nav.c01", tag: "C01", Icon: Files },
  { id: "c02", label: "nav.c02", tag: "C02", Icon: Files },
  { id: "c03", label: "nav.c03", tag: "C03", Icon: Files },
  { id: "tasks", label: "nav.tasks", tag: "QUEUE", Icon: History },
  { id: "settings", label: "nav.settings", tag: "SET", Icon: Settings }
];

/** 顶栏标题：C09 用页面自己的两行说明，其余视图用导航名（卡片内另有区块标题）。 */
const VIEW_TITLES: Partial<Record<View, MessageKey>> = {
  tasks: "nav.tasks",
  settings: "nav.settings",
  f01: "nav.f01",
  f02: "nav.f02",
  f03: "nav.f03",
  f04: "nav.f04",
  c01: "nav.c01",
  c02: "nav.c02",
  c03: "nav.c03",
  f05: "nav.f05",
  f06: "nav.f06",
  f07: "nav.f07",
  f08: "nav.f08",
  f09: "nav.f09",
  f10: "nav.f10",
  c04: "nav.c04",
  c07: "nav.c07",
  c08: "nav.c08",
  c10: "nav.c10",
  c11: "nav.c11",
  c12: "nav.c12",
  c13: "nav.c13",
  c14: "nav.c14",
  f11: "nav.f11",
  f12: "nav.f12"
};

function isActive(task: TaskView | null): boolean {
  return Boolean(task && !TERMINAL_STATES.includes(task.state));
}

function statusBadge(t: Translate, plan: PlanDocument, entryId: string) {
  const entry = plan.entries.find((item) => item.entryId === entryId);
  if (entry?.blocked) {
    return (
      <span className="badge badge-danger">{t("c09.badge.blocked", { code: entry.blocked.code })}</span>
    );
  }
  return <span className="badge">{t("c09.badge.pending")}</span>;
}

function TaskPanel({
  role,
  task,
  onCancel,
  onPause,
  onResume
}: {
  role: "apply" | "undo";
  task: TaskView;
  onCancel: (taskId: string) => void;
  onPause: (taskId: string) => void;
  onResume: (taskId: string) => void;
}) {
  const active = isActive(task);
  const { t } = useI18n();
  const percent =
    task.total > 0
      ? Math.min(100, Math.round((task.done / task.total) * 100))
      : task.state === "completed"
        ? 100
        : 0;
  return (
    <div className="task-panel">
      <div className="task-head">
        <span className="task-role">
          {role === "apply" ? t("task.role.apply") : t("task.role.undo")}
        </span>
        {stateBadge(t, task.state)}
        <span className="task-id" title={task.taskId}>{task.taskId}</span>
      </div>
      <div
        className="progress"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={task.total}
        aria-valuenow={task.done}
        aria-label={role === "apply" ? t("task.progress.apply") : t("task.progress.undo")}
      >
        <div
          className={`progress-fill${task.state === "failed" ? " danger" : ""}`}
          style={{ width: `${percent}%` }}
        />
      </div>
      <div className="hint">
        {phaseLabel(t, task.phase)} · {task.done}/{task.total}
        {task.message ? ` · ${task.message}` : ""}
      </div>
      {active && (
        <div className="button-row task-actions">
          {task.state === "paused" ? (
            <button className="button button-secondary" type="button" onClick={() => onResume(task.taskId)}>
              <Play size={14} /> {t("common.resume")}
            </button>
          ) : (
            <button
              className="button button-secondary"
              type="button"
              onClick={() => onPause(task.taskId)}
              disabled={task.state !== "running" && task.state !== "queued"}
            >
              <Pause size={14} /> {t("common.pause")}
            </button>
          )}
          <button className="button button-danger" type="button" onClick={() => onCancel(task.taskId)}>
            <X size={14} /> {t("common.cancelTask")}
          </button>
        </div>
      )}
    </div>
  );
}

function AppShell() {
  const { t, resolvedTheme, setTheme } = useI18n();
  const [root, setRoot] = useState("");
  const [keep, setKeep] = useState<KeepPolicy>("earliest");
  const [quickWindow, setQuickWindow] = useState("");
  const [protectedText, setProtectedText] = useState("");
  const [plan, setPlan] = useState<PlanDocument | null>(null);
  /** 上次成功预览所依据的参数快照；任一参数变化即判定旧计划失效（T 配方 P4）。 */
  const [plannedSnapshot, setPlannedSnapshot] = useState<string | null>(null);
  /** 保护目录命中的危险二次确认（方案 §3.3＋§7.11）；仅在有待执行条目时出现。 */
  const [dangerOpen, setDangerOpen] = useState(false);
  /**
   * 取消后的处置询问（方案 §7.7）：取消只停止派发，已完成条目默认保留；
   * N 项已落盘时问「是否一并撤销」，答复撤销再走 undoPlan（同一本 Journal）。
   */
  const [cancelReview, setCancelReview] = useState<{ planId: string; completed: number } | null>(
    null
  );
  const [taskViews, setTaskViews] = useState<Record<string, TaskView>>({});
  const [applyTaskId, setApplyTaskId] = useState<string | null>(null);
  const [undoTaskId, setUndoTaskId] = useState<string | null>(null);
  const [cleanupOutcome, setCleanupOutcome] = useState<CleanupOutcome | null>(null);
  const [error, setError] = useState<IpcError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState<Busy>(null);
  const [confirmed, setConfirmed] = useState(false);
  const [retentionDays, setRetentionDays] = useState(7);
  const [purge, setPurge] = useState(false);
  const [storage, setStorage] = useState<StorageInfo | null>(null);
  const [meta, setMeta] = useState<IpcMeta | null>(null);
  const [preflight, setPreflight] = useState<PreflightReport | null>(null);
  const [recoverableTasks, setRecoverableTasks] = useState<TaskSummary[]>([]);
  const [activeView, setActiveView] = useState<View>("c09");
  // 设计 03 §11.2：最近目录只用于"预填 + 候选"，不参与任何业务判定。
  const [recentDirs, setRecentDirs] = useState<string[]>([]);
  const navigationRef = useRef<UiNavigationState>({});

  useEffect(() => {
    getStorageInfo().then(setStorage).catch((value) => setError(toIpcError(value)));
    getIpcMeta().then(setMeta).catch(() => undefined);
  }, []);

  // 上次用的目录：读 UiState 后预填输入框（设计 03 §11.2/§11.3）。读失败按"没有记录"处理，
  // 不打扰用户——UiState 是可丢的本机状态，丢了只回默认。
  useEffect(() => {
    let disposed = false;
    getUiState()
      .then((state) => {
        if (disposed) return;
        const navigation = state.navigation ?? {};
        navigationRef.current = navigation;
        setRecentDirs(navigation.recentDirectories ?? []);
        const last = preferredDirectory(navigation);
        // 只填空输入框：用户已经在敲的话不覆盖。
        if (last) setRoot((current) => (current.trim() ? current : last));
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    listTasks()
      .then((page) =>
        setRecoverableTasks(page.items.filter((item) => item.state === "interrupted"))
      )
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    const patchTask = (taskId: string, update: (current: TaskView) => TaskView) => {
      setTaskViews((previous) => {
        const current =
          previous[taskId] ?? {
            taskId,
            state: "queued" as TaskState,
            phase: "queued" as ProgressPhase,
            done: 0,
            total: 0
          };
        return { ...previous, [taskId]: update(current) };
      });
    };
    const track = (subscription: Promise<() => void>) => {
      subscription
        .then((unlisten) => {
          if (disposed) unlisten();
          else unlisteners.push(unlisten);
        })
        .catch((value) => setError(toIpcError(value)));
    };

    track(
      listenTaskProgress((event) => {
        const entryProgress =
          event.phase === "queued" || event.phase === "executing"
            ? { done: event.done, total: event.total }
            : {};
        patchTask(event.taskId, (current) => ({
          ...current,
          phase: event.phase,
          ...entryProgress,
          message: event.message
        }));
      })
    );
    track(
      listenTaskState((event) => {
        patchTask(event.taskId, (current) => ({ ...current, state: event.state }));
      })
    );
    track(
      listenTaskDone((event) => {
        patchTask(event.taskId, (current) => ({ ...current, report: event.summary }));
        getTask(event.taskId)
          .then((detail) => {
            patchTask(event.taskId, (current) => ({
              ...current,
              state: detail.summary.state,
              phase: detail.summary.phase,
              done: detail.summary.done,
              total: detail.summary.total,
              message: detail.summary.message
            }));
            // 方案 §7.7：取消后已完成条目默认保留在磁盘上并留在 Journal 里。
            // 只在"确实有已完成的写入"时才问，空取消不打扰用户。
            const completed = event.summary.entries.filter(
              (entry) => entry.status === "ok" || entry.status === "overwritten"
            ).length;
            if (event.kind === "apply" && detail.summary.state === "cancelled" && completed > 0) {
              setCancelReview({ planId: event.summary.planId, completed });
            }
          })
          .catch(() => undefined);
      })
    );

    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, []);

  const protectedDirs = useMemo(
    () =>
      protectedText
        .split(/\r?\n/)
        .map((line) => line.trim())
        .filter(Boolean),
    [protectedText]
  );

  const applyTask = applyTaskId ? taskViews[applyTaskId] ?? null : null;
  const undoTask = undoTaskId ? taskViews[undoTaskId] ?? null : null;
  const taskActive = isActive(applyTask) || isActive(undoTask);
  const hasBlocked = (plan?.stats.blocked ?? 0) > 0;
  // E2009 由 kernel 判定；界面只做危险确认，既不放宽也不改变该判定（强制力在执行层）。
  const protectionHits = (plan?.entries ?? []).filter((entry) => entry.blocked?.code === "E2009");
  const deletableCount = plan?.entries.filter((entry) => !entry.blocked).length ?? 0;
  const planSignature = JSON.stringify({
    root: root.trim(),
    keep,
    quickWindowBytes: quickWindow.trim(),
    protectedDirs
  });
  // 预检是异步的：等待期间用户可能又改了参数，落地后须按当前签名复核（方案 §7.3）。
  const latestSignatureRef = useRef(planSignature);
  latestSignatureRef.current = planSignature;

  // 参数改动 → 旧计划立即失效：执行/撤销入口全部关闭，须重新生成预览。
  // 内核侧仍有 plan→apply 指纹（E2010）与保护名单变更（E1002）兜底，这里是入口层阻断（方案 §7.3）。
  const planStale =
    plan !== null && plannedSnapshot !== null && plannedSnapshot !== planSignature;
  const canApply =
    Boolean(plan) &&
    deletableCount > 0 &&
    !applyTask &&
    !undoTask &&
    !taskActive &&
    busy === null &&
    !planStale &&
    confirmed;
  const applyHasApplied = Boolean(
    applyTask?.report?.entries.some(
      (entry) => entry.status === "ok" || entry.status === "overwritten"
    )
  );
  const canUndo =
    Boolean(applyTask) &&
    !planStale &&
    !undoTask &&
    !taskActive &&
    busy === null &&
    ["completed", "partial-failed", "failed"].includes(applyTask?.state ?? "") &&
    applyHasApplied;
  const canRetry =
    Boolean(applyTask) &&
    !undoTask &&
    !taskActive &&
    busy === null &&
    ["partial-failed", "failed"].includes(applyTask?.state ?? "");
  const undoReverted =
    undoTask?.report?.entries.filter((entry) => entry.status === "ok").length ?? 0;
  const undoFailures =
    undoTask?.report?.entries.filter((entry) => entry.status === "failed").length ?? 0;

  /** 记住这次用的目录；写失败只影响"下次预填"，不打扰用户（设计 03 §11.1）。 */
  const rememberRoot = useCallback(async (directory: string) => {
    const navigation = rememberDirectory(navigationRef.current, directory);
    navigationRef.current = navigation;
    setRecentDirs(navigation.recentDirectories ?? []);
    try {
      await setUiState({ navigation });
    } catch {
      // 刻意静默：UiState 可丢，且此刻用户关心的是预览结果而不是"记住目录"是否成功。
    }
  }, []);

  async function onPlan() {
    if (!root.trim()) {
      // 表单还差一步：用提示条而不是错误横幅——它没有码位，也不该占用错误码空间。
      setNotice(t("c09.error.rootRequired"));
      return;
    }
    const quick = quickWindow.trim();
    if (quick && (!Number.isInteger(Number(quick)) || Number(quick) <= 0)) {
      setNotice(t("c09.error.quickWindow"));
      return;
    }
    setBusy("plan");
    setError(null);
    setNotice(null);
    setPreflight(null);
    setTaskViews({});
    setApplyTaskId(null);
    setUndoTaskId(null);
    setCleanupOutcome(null);
    setConfirmed(false);
    setPlannedSnapshot(null);
    setDangerOpen(false);
    try {
      const next = await planOperation({
        kind: "c09",
        params: {
          keep,
          quickWindowBytes: quick ? Number(quick) : undefined,
          protectedDirs
        },
        sourcePaths: [root.trim()]
      });
      setPlan(next);
      setPlannedSnapshot(planSignature);
      void rememberRoot(root.trim());
      setNotice(
        t("c09.notice.planDone", {
          count: next.entries.length,
          blocked: next.stats.blocked
        })
      );
      try {
        setPreflight(await preflightPlan(next.planId));
      } catch (value) {
        setPreflight(null);
        setError(toIpcError(value));
      }
    } catch (value) {
      setPlan(null);
      setError(toIpcError(value));
    } finally {
      setBusy(null);
    }
  }

  async function onApply() {
    if (!plan || !confirmed) return;
    setError(null);
    setNotice(null);
    try {
      const report = await preflightPlan(plan.planId);
      setPreflight(report);
      if (plannedSnapshot !== null && latestSignatureRef.current !== plannedSnapshot) {
        setError({
          code: "E1002",
          message: t("c09.error.stale"),
          detail: t("c09.error.staleDetail"),
          retryable: false
        });
        return;
      }
      if (report.taskBlocked) {
        const blocking = report.items.find(
          (item) => item.level === "block" && item.kind === "disk-space"
        );
        setError({
          code: blocking?.code ?? "E2004",
          message: t("c09.error.diskBlocked"),
          detail: blocking?.message,
          retryable: true
        });
        return;
      }
      const handle = await applyPlan(plan.planId, {
        onConflict: "skip",
        overwriteConfirmed: false
      });
      setApplyTaskId(handle.taskId);
      setTaskViews((previous) =>
        previous[handle.taskId]
          ? previous
          : {
              ...previous,
              [handle.taskId]: {
                taskId: handle.taskId,
                state: "queued",
                phase: "queued",
                done: 0,
                total: deletableCount
              }
            }
      );
      setConfirmed(false);
      setNotice(t("c09.notice.queued"));
    } catch (value) {
      setError(toIpcError(value));
    }
  }

  /** 取消后的「一并撤销」（方案 §7.7）：与普通撤销同一条 undoPlan/Journal 通道。 */
  async function onCancelReviewConfirm() {
    const pending = cancelReview;
    if (!pending) return;
    setCancelReview(null);
    setError(null);
    setNotice(null);
    try {
      const handle = await undoPlan(pending.planId);
      setUndoTaskId(handle.taskId);
      setTaskViews((previous) =>
        previous[handle.taskId]
          ? previous
          : {
              ...previous,
              [handle.taskId]: {
                taskId: handle.taskId,
                state: "queued",
                phase: "queued",
                done: 0,
                total: pending.completed
              }
            }
      );
      setNotice(t("c09.notice.cancelUndo", { count: pending.completed }));
    } catch (value) {
      setError(toIpcError(value));
    }
  }

  async function onUndo() {
    if (!plan) return;
    setError(null);
    setNotice(null);
    try {
      const handle = await undoPlan(plan.planId);
      setUndoTaskId(handle.taskId);
      setTaskViews((previous) =>
        previous[handle.taskId]
          ? previous
          : {
              ...previous,
              [handle.taskId]: {
                taskId: handle.taskId,
                state: "queued",
                phase: "queued",
                done: 0,
                total:
                  applyTask?.report?.entries.filter((entry) => entry.status === "ok").length ?? 0
              }
            }
      );
      // 撤销总数以壳层按 Journal 统计为准（重试后 Journal 条目多于单次报告）。
      getTask(handle.taskId)
        .then((detail) =>
          setTaskViews((previous) => {
            const current = previous[handle.taskId];
            if (!current) return previous;
            return {
              ...previous,
              [handle.taskId]: {
                ...current,
                total: Math.max(current.total, detail.summary.total)
              }
            };
          })
        )
        .catch(() => undefined);
      setNotice(t("c09.notice.undoQueued"));
    } catch (value) {
      setError(toIpcError(value));
    }
  }

  async function onRetry() {
    if (!applyTask) return;
    setError(null);
    setNotice(null);
    try {
      const handle = await retryTask(applyTask.taskId);
      setApplyTaskId(handle.taskId);
      setTaskViews((previous) =>
        previous[handle.taskId]
          ? previous
          : {
              ...previous,
              [handle.taskId]: {
                taskId: handle.taskId,
                state: "queued",
                phase: "queued",
                done: 0,
                total: 0
              }
            }
      );
      setNotice(t("c09.notice.retryQueued"));
    } catch (value) {
      setError(toIpcError(value));
    }
  }

  async function onCancel(taskId: string) {
    try {
      await cancelTask(taskId);
    } catch (value) {
      setError(toIpcError(value));
    }
  }

  async function onPause(taskId: string) {
    try {
      await pauseTask(taskId);
    } catch (value) {
      setError(toIpcError(value));
    }
  }

  async function onResume(taskId: string) {
    try {
      await resumeTask(taskId);
    } catch (value) {
      setError(toIpcError(value));
    }
  }

  async function onRecover(taskId: string, action: RecoveryAction) {
    setError(null);
    setNotice(null);
    try {
      const handle = await recoverTask(taskId, action);
      setRecoverableTasks((previous) =>
        previous.filter((task) => task.taskId !== taskId)
      );
      if (action === "resume") {
        setApplyTaskId(handle.taskId);
        setTaskViews((previous) =>
          previous[handle.taskId]
            ? previous
            : {
                ...previous,
                [handle.taskId]: {
                  taskId: handle.taskId,
                  state: "queued",
                  phase: "queued",
                  done: 0,
                  total: 0
                }
              }
        );
        setNotice(t("c09.notice.recoverResume"));
      } else if (action === "rollback") {
        setUndoTaskId(handle.taskId);
        setNotice(t("c09.notice.recoverRollback"));
      } else {
        setNotice(t("c09.notice.recoverDiscard"));
      }
    } catch (value) {
      setError(toIpcError(value));
    }
  }

  async function onCleanup() {
    setBusy("cleanup");
    setError(null);
    setNotice(null);
    try {
      const outcome = await cleanupRetention(retentionDays, purge);
      setCleanupOutcome(outcome);
      setNotice(
        outcome.dryRun
          ? t("c09.notice.cleanupDry")
          : t("c09.notice.cleanupDone", {
              entries: outcome.removedEntries,
              files: outcome.removedFiles
            })
      );
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="app-shell">
      <div className="glass-shell">
        <aside className="sidebar">
          <div className="brand">
            <div className="brand-mark">B</div>
            <div>
              <div className="brand-name">BatchDesk</div>
              <div className="brand-sub">{t("app.brand.sub")}</div>
            </div>
          </div>

          <div>
            <div className="nav-label">{t("app.nav.workspace")}</div>
            <nav className="nav" aria-label={t("app.nav.aria")}>
              {NAV_ITEMS.map(({ id, label, tag, Icon }) => (
                <button
                  key={id}
                  className={`nav-item${activeView === id ? " active" : ""}`}
                  type="button"
                  onClick={() => setActiveView(id)}
                >
                  <span>
                    <Icon size={16} /> {t(label)}
                  </span>
                  <span className="soon">{tag}</span>
                </button>
              ))}
            </nav>
          </div>

          <div className="side-spacer" />

          <div className="storage">
            <div>
              <ShieldCheck size={14} /> {t("app.storage.title")}
            </div>
            <div className="storage-path" title={storage?.quarantineDir}>
              {storage?.quarantineDir ?? t("app.storage.loading")}
            </div>
            <div className="storage-path" title={storage?.journalFile}>
              {storage?.journalFile ?? ""}
            </div>
            <div>{t("app.storage.retention")}</div>
          </div>
        </aside>

        <main className="main">
          <header className="topbar">
            <div className="title-group">
              <div className="eyebrow">
                {activeView === "c09" ? t("c09.eyebrow") : "BatchDesk"}
              </div>
              <h1 className="title">
                {activeView === "c09" ? t("c09.title") : t(VIEW_TITLES[activeView] ?? "nav.c09")}
              </h1>
              {activeView === "c09" && <p className="subtitle">{t("c09.subtitle")}</p>}
            </div>
            <div className="top-actions">
              {!isTauri() && <span className="mock-banner">{t("app.mock")}</span>}
              <span className="badge">P0★ · G1</span>
              <button
                className="theme-toggle"
                type="button"
                onClick={() => setTheme(resolvedTheme === "light" ? "dark" : "light")}
                aria-label={t("app.theme.toggle")}
              >
                {resolvedTheme === "light" ? <Moon size={14} /> : <Sun size={14} />}
                {resolvedTheme === "light" ? t("app.theme.toDark") : t("app.theme.toLight")}
              </button>
            </div>
          </header>

          <div className="content">
            {activeView === "c09" ? (
            <div style={{ display: "contents" }}>
            {error && <ErrorBanner error={error} onClose={() => setError(null)} />}
            {notice && (
              <div className="banner banner-info" role="status">
                <div>{notice}</div>
              </div>
            )}
            {recoverableTasks.length > 0 && (
              <div className="banner banner-info" role="status">
                <div>
                  <strong>{t("c09.recover.title")}</strong>
                  <div className="muted">
                    {t("c09.recover.body")}
                  </div>
                  {recoverableTasks.map((task) => (
                    <div className="button-row" key={task.taskId}>
                      <span className="task-id" title={task.taskId}>{task.taskId}</span>
                      <button
                        className="button button-secondary"
                        type="button"
                        onClick={() => onRecover(task.taskId, "resume")}
                      >
                        <Play size={14} /> {t("common.resumeRun")}
                      </button>
                      <button
                        className="button button-secondary"
                        type="button"
                        onClick={() => onRecover(task.taskId, "rollback")}
                      >
                        <Undo2 size={14} /> {t("common.rollback")}
                      </button>
                      <button
                        className="button button-ghost"
                        type="button"
                        onClick={() => onRecover(task.taskId, "discard")}
                      >
                        {t("common.discard")}
                      </button>
                    </div>
                  ))}
                </div>
              </div>
            )}
            {preflight && (
              <div
                className={`banner ${preflight.taskBlocked ? "banner-danger" : "banner-info"}`}
                role="status"
              >
                <div>
                  {t("c09.preflight.summary", {
                    pass: preflight.pass,
                    warn: preflight.warn,
                    block: preflight.block
                  })}
                  {preflight.taskBlocked && (
                    <div className="muted">{t("c09.preflight.blocked")}</div>
                  )}
                  {preflight.items
                    .filter((item) => item.level !== "pass")
                    .slice(0, 5)
                    .map((item, index) => (
                      <div className="muted" key={`${item.kind}-${index}`}>
                        {item.level === "block"
                          ? t("c09.preflight.levelBlock")
                          : t("c09.preflight.levelWarn")}{" "}
                        · {item.kind}
                        {item.code ? ` (${item.code})` : ""} · {item.message}
                      </div>
                    ))}
                </div>
              </div>
            )}

            <div className="content-grid">
              <div className="stack">
                <section className="card" aria-labelledby="params-title">
                  <div className="card-header">
                    <div>
                      <h2 className="card-title" id="params-title">{t("c09.params.title")}</h2>
                      <p className="card-sub">{t("c09.params.sub")}</p>
                    </div>
                    <span className="badge">{t("c09.params.badge")}</span>
                  </div>

                  <div className="field-grid">
                    <div className="field wide">
                      <label htmlFor="root">{t("c09.root.label")}</label>
                      <input
                        id="root"
                        className="input"
                        value={root}
                        onChange={(event) => setRoot(event.target.value)}
                        placeholder={t("c09.root.placeholder")}
                        spellCheck={false}
                        list={recentDirs.length > 0 ? "c09-recent-dirs" : undefined}
                      />
                      {recentDirs.length > 0 && (
                        <datalist id="c09-recent-dirs">
                          {recentDirs.map((directory) => (
                            <option key={directory} value={directory} />
                          ))}
                        </datalist>
                      )}
                    </div>
                    <div className="field">
                      <label htmlFor="keep">{t("c09.keep.label")}</label>
                      <select
                        id="keep"
                        className="select"
                        value={keep}
                        onChange={(event) => setKeep(event.target.value as KeepPolicy)}
                      >
                        <option value="earliest">{t("c09.keep.earliest")}</option>
                        <option value="shortest-path">{t("c09.keep.shortest")}</option>
                        <option value="newest">{t("c09.keep.newest")}</option>
                      </select>
                    </div>
                    <div className="field">
                      <label htmlFor="quick">{t("c09.quick.label")}</label>
                      <input
                        id="quick"
                        className="input"
                        inputMode="numeric"
                        value={quickWindow}
                        onChange={(event) => setQuickWindow(event.target.value)}
                        placeholder={t("c09.quick.placeholder")}
                      />
                    </div>
                    <div className="field wide">
                      <label htmlFor="protected">{t("c09.protected.label")}</label>
                      <textarea
                        id="protected"
                        className="textarea"
                        value={protectedText}
                        onChange={(event) => setProtectedText(event.target.value)}
                        placeholder={t("c09.protected.placeholder")}
                        spellCheck={false}
                      />
                      <div className="hint">{t("c09.protected.hint")}</div>
                    </div>
                  </div>

                  <div className="toolbar">
                    <span className="hint">{t("c09.plan.hint")}</span>
                    <button
                      className="button button-primary"
                      type="button"
                      onClick={onPlan}
                      disabled={busy !== null || taskActive}
                    >
                      {busy === "plan" ? <Loader2 size={16} className="spin" /> : <FolderSearch size={16} />}
                      {busy === "plan" ? t("c09.plan.busy") : t("c09.plan.cta")}
                    </button>
                  </div>
                </section>

                <section className="card" aria-labelledby="plan-title">
                  <div className="card-header">
                    <div>
                      <h2 className="card-title" id="plan-title">{t("c09.preview.title")}</h2>
                      <p className="card-sub">{t("c09.preview.sub")}</p>
                    </div>
                    {plan && <span className="plan-id" title={plan.planId}>{plan.planId}</span>}
                  </div>

                  {!plan ? (
                    <div className="empty">{t("c09.preview.empty")}</div>
                  ) : (
                    <>
                      <div className="stats">
                        <div className="stat">
                          <div className="stat-label">{t("c09.stats.total")}</div>
                          <div className="stat-value">{plan.stats.total}</div>
                        </div>
                        <div className="stat">
                          <div className="stat-label">{t("c09.stats.conflict")}</div>
                          <div className="stat-value">{plan.stats.conflict}</div>
                        </div>
                        <div className="stat">
                          <div className="stat-label">{t("c09.stats.warning")}</div>
                          <div className="stat-value">{plan.stats.warning}</div>
                        </div>
                        <div className="stat">
                          <div className="stat-label">{t("c09.stats.blocked")}</div>
                          <div className="stat-value">{plan.stats.blocked}</div>
                        </div>
                      </div>

                      <div className="table">
                        <div className="row row-head">
                          <span>{t("c09.table.file")}</span>
                          <span>{t("c09.table.reason")}</span>
                          <span>{t("c09.table.status")}</span>
                        </div>
                        {/* 方案 6.2：大列表只挂窗口内的行；去重的计划可能有上万条。 */}
                        <VirtualList
                          label={t("c09.table.file")}
                          items={plan.entries}
                          getKey={(entry) => entry.entryId}
                          renderRow={(entry) => (
                            <div className={`row${entry.blocked ? " blocked" : ""}`}>
                              <span className="path" title={entry.source}>{entry.source}</span>
                              <span className="reason" title={entry.reason}>{entry.reason}</span>
                              <span>{statusBadge(t, plan, entry.entryId)}</span>
                            </div>
                          )}
                        />
                      </div>

                      {plan.warnings.length > 0 && (
                        <div className="warnings">
                          {plan.warnings.map((warning, index) => (
                            <div className={`warning ${warning.riskLevel}`} key={`${warning.kind}-${index}`}>
                              <Info size={14} />
                              <span>
                                <strong>{warning.kind}</strong>
                                {warning.suggestion ? ` · ${warning.suggestion}` : ""}
                              </span>
                            </div>
                          ))}
                        </div>
                      )}
                    </>
                  )}
                </section>
              </div>

              <div className="stack">
                <section className="card" aria-labelledby="execute-title">
                  <div className="card-header">
                    <div>
                      <h2 className="card-title" id="execute-title">{t("c09.execute.title")}</h2>
                      <p className="card-sub">{t("c09.execute.sub")}</p>
                    </div>
                    {applyTask && stateBadge(t, applyTask.state)}
                  </div>

                  {(applyTask || undoTask) && (
                    <div className="task-stack">
                      {applyTask && (
                        <TaskPanel
                          role="apply"
                          task={applyTask}
                          onCancel={onCancel}
                          onPause={onPause}
                          onResume={onResume}
                        />
                      )}
                      {undoTask && (
                        <TaskPanel
                          role="undo"
                          task={undoTask}
                          onCancel={onCancel}
                          onPause={onPause}
                          onResume={onResume}
                        />
                      )}
                    </div>
                  )}

                  {planStale && (
                    <div className="banner banner-warn" role="status" data-testid="plan-stale">
                      <div>{t("c09.stale")}</div>
                    </div>
                  )}

                  <label className="check" htmlFor="confirm">
                    <input
                      id="confirm"
                      type="checkbox"
                      checked={confirmed}
                      onChange={(event) => setConfirmed(event.target.checked)}
                      disabled={!plan || Boolean(applyTask) || taskActive || busy !== null || planStale}
                    />
                    {t("c09.execute.confirm", { count: deletableCount })}
                  </label>

                  <div className="divider" />

                  <div className="button-row">
                    <button
                      className="button button-primary"
                      type="button"
                      onClick={() => {
                        if (protectionHits.length > 0) {
                          setDangerOpen(true);
                          return;
                        }
                        void onApply();
                      }}
                      disabled={!canApply}
                    >
                      {isActive(applyTask) ? <Loader2 size={16} className="spin" /> : <Play size={16} />}
                      {isActive(applyTask) ? t("c09.execute.busy") : t("c09.execute.apply")}
                    </button>
                    <button className="button button-secondary" type="button" onClick={onUndo} disabled={!canUndo}>
                      {isActive(undoTask) ? <Loader2 size={16} className="spin" /> : <Undo2 size={16} />}
                      {isActive(undoTask) ? t("c09.execute.undoBusy") : t("c09.execute.undo")}
                    </button>
                    {applyTask && ["partial-failed", "failed"].includes(applyTask.state) && (
                      <button className="button button-secondary" type="button" onClick={onRetry} disabled={!canRetry}>
                        <RefreshCw size={16} /> {t("common.retryFailed")}
                      </button>
                    )}
                  </div>

                  {dangerOpen && (
                    <DangerConfirm
                      confirmLabel={t("c09.danger.confirm")}
                      items={protectionHits.map((entry) =>
                        t("c09.danger.item", {
                          path: entry.target ?? entry.source,
                          reason: entry.blocked?.reason ?? t("c09.danger.protectedReason")
                        })
                      )}
                      title={t("c09.danger.title")}
                      body={t("c09.danger.body", { count: protectionHits.length })}
                      onCancel={() => setDangerOpen(false)}
                      onConfirm={() => {
                        setDangerOpen(false);
                        void onApply();
                      }}
                    />
                  )}

                  {hasBlocked && (
                    <div className="warning danger" style={{ marginTop: "var(--space-3)" }}>
                      <AlertTriangle size={14} />
                      <span>{t("c09.execute.blockedHint")}</span>
                    </div>
                  )}

                  <div className="hint" style={{ marginTop: "var(--space-3)" }}>
                    {t("c09.execute.windowHint")}
                  </div>
                </section>

                <section className="card" aria-labelledby="cleanup-title">
                  <div className="card-header">
                    <div>
                      <h2 className="card-title" id="cleanup-title">{t("c09.cleanup.title")}</h2>
                      <p className="card-sub">{t("c09.cleanup.sub")}</p>
                    </div>
                    <HardDrive size={18} />
                  </div>

                  <div className="field" style={{ marginBottom: "var(--space-3)" }}>
                    <label htmlFor="retention">{t("c09.cleanup.retention")}</label>
                    <input
                      id="retention"
                      className="input"
                      type="number"
                      min={1}
                      max={30}
                      value={retentionDays}
                      onChange={(event) => setRetentionDays(Number(event.target.value))}
                    />
                  </div>

                  <label className="check" htmlFor="purge">
                    <input
                      id="purge"
                      type="checkbox"
                      checked={purge}
                      onChange={(event) => setPurge(event.target.checked)}
                    />
                    {t("c09.cleanup.purge")}
                  </label>

                  <div className="toolbar">
                    <span className="hint">
                      {purge ? t("c09.cleanup.hintPurge") : t("c09.cleanup.hintDry")}
                    </span>
                    <button
                      className="button button-danger"
                      type="button"
                      onClick={onCleanup}
                      disabled={busy !== null || taskActive}
                    >
                      {busy === "cleanup" ? <Loader2 size={16} className="spin" /> : <Trash2 size={16} />}
                      {busy === "cleanup"
                        ? t("c09.cleanup.busy")
                        : purge
                          ? t("c09.cleanup.ctaPurge")
                          : t("c09.cleanup.ctaDry")}
                    </button>
                  </div>
                </section>

                <section className="card" aria-labelledby="result-title">
                  <div className="card-header">
                    <div>
                      <h2 className="card-title" id="result-title">{t("c09.result.title")}</h2>
                      <p className="card-sub">{t("c09.result.sub")}</p>
                    </div>
                    <RefreshCw size={18} />
                  </div>

                  {!applyTask?.report && !undoTask?.report && !cleanupOutcome && (
                    <div className="empty">{t("c09.result.empty")}</div>
                  )}

                  {applyTask?.report && (
                    <div className="table">
                      <div className="row row-head">
                        <span>{t("c09.result.entry")}</span>
                        <span>{t("c09.result.error")}</span>
                        <span>{t("c09.result.status")}</span>
                      </div>
                      <VirtualList
                        label={t("c09.result.entry")}
                        items={applyTask.report.entries}
                        getKey={(entry) => entry.entryId}
                        renderRow={(entry) => (
                          <div className="row">
                            <span className="path" title={entry.entryId}>{entry.entryId}</span>
                            <span className="reason" title={entry.error?.message}>
                              {entry.error ? `${entry.error.code} · ${entry.error.message}` : "—"}
                            </span>
                            <span>
                              <span
                                className={`badge ${
                                  entry.status === "ok"
                                    ? "badge-safe"
                                    : entry.status === "failed"
                                      ? "badge-danger"
                                      : "badge-warn"
                                }`}
                              >
                                {entry.status}
                              </span>
                            </span>
                          </div>
                        )}
                      />
                    </div>
                  )}

                  {undoTask?.report && (
                    <div className="banner banner-info" style={{ marginTop: "var(--space-3)" }}>
                      <CheckCircle2 size={16} />
                      <span>
                        {t("c09.result.journal", {
                          count: undoReverted,
                          tail: undoFailures
                            ? t("c09.result.journalTail", { count: undoFailures })
                            : ""
                        })}
                      </span>
                    </div>
                  )}

                  {cleanupOutcome && (
                    <div className="banner banner-info" style={{ marginTop: "var(--space-3)" }}>
                      <CheckCircle2 size={16} />
                      <span>
                        {t("c09.result.cutoff", {
                          cutoff: cleanupOutcome.cutoff,
                          expired: cleanupOutcome.expiredEntries,
                          records: cleanupOutcome.removedEntries,
                          files: cleanupOutcome.removedFiles
                        })}
                      </span>
                    </div>
                  )}
                </section>

                <section className="card" aria-labelledby="about-title">
                  <div className="card-header">
                    <div>
                      <h2 className="card-title" id="about-title">{t("c09.about.title")}</h2>
                      <p className="card-sub">{t("c09.about.sub")}</p>
                    </div>
                  </div>
                  <div className="hint">
                    {t("c09.about.meta", {
                      kernel: meta?.kernelVersion ?? "…",
                      contract: meta?.contractVersion ?? "…"
                    })}
                  </div>
                  <div className="hint" style={{ marginTop: "var(--space-2)" }}>
                    {t("c09.about.hint")}
                  </div>
                </section>
              </div>
            </div>
            </div>
            ) : activeView === "tasks" ? (
              <TaskHistory live={taskViews} onBack={() => setActiveView("c09")} />
            ) : activeView === "settings" ? (
              <SettingsPage onBack={() => setActiveView("c09")} />
            ) : (
              <FeaturePages view={activeView} onBack={() => setActiveView("c09")} />
            )}
          </div>
        </main>
      </div>

      {/* 方案 §7.7：取消只停止派发，已完成项默认保留；有写入时才问是否一并撤销。
          危险动作（撤销）不带默认焦点，初始焦点落「保留」。 */}
      {cancelReview && (
        <DangerConfirm
          cancelLabel={t("c09.cancelReview.keep")}
          confirmLabel={t("c09.cancelReview.confirm")}
          items={[]}
          title={t("c09.cancelReview.title", { count: cancelReview.completed })}
          body={t("c09.cancelReview.body", { count: cancelReview.completed })}
          onCancel={() => setCancelReview(null)}
          onConfirm={onCancelReviewConfirm}
        />
      )}
    </div>
  );
}

function App() {
  return (
    <I18nProvider>
      <AppShell />
    </I18nProvider>
  );
}

export default App;
