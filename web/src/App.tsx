import { useEffect, useMemo, useState } from "react";
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
import {
  applyPlan,
  cancelTask,
  cleanupRetention,
  getIpcMeta,
  getStorageInfo,
  getTask,
  isTauri,
  listenTaskDone,
  listenTaskProgress,
  listenTaskState,
  pauseTask,
  planOperation,
  resumeTask,
  toIpcError,
  undoPlan
} from "./lib/ipc";
import type {
  ApplyReport,
  CleanupOutcome,
  IpcError,
  IpcMeta,
  KeepPolicy,
  PlanDocument,
  ProgressPhase,
  StorageInfo,
  TaskState
} from "./lib/ipc/types";

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

function isActive(task: TaskView | null): boolean {
  return Boolean(task && !TERMINAL_STATES.includes(task.state));
}

function phaseLabel(phase: ProgressPhase): string {
  switch (phase) {
    case "queued":
      return "排队中";
    case "scanning":
      return "扫描中";
    case "planning":
      return "生成计划";
    case "executing":
      return "执行中";
    case "journaling":
      return "回滚 / 写 Journal";
    case "finalizing":
      return "收尾";
  }
}

function statusBadge(plan: PlanDocument, entryId: string) {
  const entry = plan.entries.find((item) => item.entryId === entryId);
  if (entry?.blocked) {
    return <span className="badge badge-danger">受阻 {entry.blocked.code}</span>;
  }
  return <span className="badge">待清理</span>;
}

function stateBadge(state: TaskState) {
  if (state === "completed") return <span className="badge badge-safe">已完成</span>;
  if (state === "partial-failed") return <span className="badge badge-warn">部分失败</span>;
  if (state === "cancelled") return <span className="badge badge-warn">已取消</span>;
  if (state === "failed") return <span className="badge badge-danger">失败</span>;
  if (state === "paused") return <span className="badge badge-warn">已暂停</span>;
  if (state === "cancelling") return <span className="badge badge-warn">取消中</span>;
  if (state === "running") return <span className="badge">执行中</span>;
  return <span className="badge">排队中</span>;
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
  const percent =
    task.total > 0
      ? Math.min(100, Math.round((task.done / task.total) * 100))
      : task.state === "completed"
        ? 100
        : 0;
  return (
    <div className="task-panel">
      <div className="task-head">
        <span className="task-role">{role === "apply" ? "执行任务" : "撤销任务"}</span>
        {stateBadge(task.state)}
        <span className="task-id" title={task.taskId}>{task.taskId}</span>
      </div>
      <div
        className="progress"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={task.total}
        aria-valuenow={task.done}
        aria-label={`${role} 进度`}
      >
        <div
          className={`progress-fill${task.state === "failed" ? " danger" : ""}`}
          style={{ width: `${percent}%` }}
        />
      </div>
      <div className="hint">
        {phaseLabel(task.phase)} · {task.done}/{task.total}
        {task.message ? ` · ${task.message}` : ""}
      </div>
      {active && (
        <div className="button-row task-actions">
          {task.state === "paused" ? (
            <button className="button button-secondary" type="button" onClick={() => onResume(task.taskId)}>
              <Play size={14} /> 继续
            </button>
          ) : (
            <button
              className="button button-secondary"
              type="button"
              onClick={() => onPause(task.taskId)}
              disabled={task.state !== "running" && task.state !== "queued"}
            >
              <Pause size={14} /> 暂停
            </button>
          )}
          <button className="button button-danger" type="button" onClick={() => onCancel(task.taskId)}>
            <X size={14} /> 取消任务
          </button>
        </div>
      )}
    </div>
  );
}

function App() {
  const [theme, setTheme] = useState<"light" | "dark">("light");
  const [root, setRoot] = useState("");
  const [keep, setKeep] = useState<KeepPolicy>("earliest");
  const [quickWindow, setQuickWindow] = useState("");
  const [protectedText, setProtectedText] = useState("");
  const [plan, setPlan] = useState<PlanDocument | null>(null);
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

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  useEffect(() => {
    getStorageInfo().then(setStorage).catch((value) => setError(toIpcError(value)));
    getIpcMeta().then(setMeta).catch(() => undefined);
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
  const deletableCount = plan?.entries.filter((entry) => !entry.blocked).length ?? 0;
  const canApply =
    Boolean(plan) &&
    deletableCount > 0 &&
    !applyTask &&
    !undoTask &&
    !taskActive &&
    busy === null &&
    confirmed;
  const applyHasApplied = Boolean(
    applyTask?.report?.entries.some(
      (entry) => entry.status === "ok" || entry.status === "overwritten"
    )
  );
  const canUndo =
    Boolean(applyTask) &&
    !undoTask &&
    !taskActive &&
    busy === null &&
    ["completed", "partial-failed", "failed"].includes(applyTask?.state ?? "") &&
    applyHasApplied;
  const undoReverted =
    undoTask?.report?.entries.filter((entry) => entry.status === "ok").length ?? 0;
  const undoFailures =
    undoTask?.report?.entries.filter((entry) => entry.status === "failed").length ?? 0;

  async function onPlan() {
    if (!root.trim()) {
      setError({ code: "E1001", message: "请填写要扫描的目录", retryable: false });
      return;
    }
    const quick = quickWindow.trim();
    if (quick && (!Number.isInteger(Number(quick)) || Number(quick) <= 0)) {
      setError({ code: "E1002", message: "快速哈希窗口必须是正整数（留空表示跳过）", retryable: false });
      return;
    }
    setBusy("plan");
    setError(null);
    setNotice(null);
    setTaskViews({});
    setApplyTaskId(null);
    setUndoTaskId(null);
    setCleanupOutcome(null);
    setConfirmed(false);
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
      setNotice(`计划已生成：${next.entries.length} 条待清理，${next.stats.blocked} 条受阻。请核对预览后执行。`);
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
      setNotice("任务已加入队列；执行进度与结果通过 task:// 事件推送。");
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
      setNotice("撤销任务已加入队列；回滚进度通过 task:// 事件推送。");
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

  async function onCleanup() {
    setBusy("cleanup");
    setError(null);
    setNotice(null);
    try {
      const outcome = await cleanupRetention(retentionDays, purge);
      setCleanupOutcome(outcome);
      setNotice(
        outcome.dryRun
          ? "清理演练完成，未删除任何文件。勾选永久删除后才会执行。"
          : `清理完成：移除 ${outcome.removedEntries} 条 Journal 记录、${outcome.removedFiles} 个隔离文件。`
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
              <div className="brand-sub">本地批量文件工作台</div>
            </div>
          </div>

          <div>
            <div className="nav-label">工作区</div>
            <nav className="nav" aria-label="功能导航">
              <button className="nav-item active" type="button">
                <span>
                  <FolderSearch size={16} /> 重复文件查找
                </span>
                <span className="soon">C09</span>
              </button>
              <button className="nav-item" type="button" disabled>
                <span>
                  <Files size={16} /> 文件整理
                </span>
                <span className="soon">后续</span>
              </button>
              <button className="nav-item" type="button" disabled>
                <span>
                  <History size={16} /> 任务与撤销
                </span>
                <span className="soon">后续</span>
              </button>
              <button className="nav-item" type="button" disabled>
                <span>
                  <Settings size={16} /> 设置
                </span>
                <span className="soon">后续</span>
              </button>
            </nav>
          </div>

          <div className="side-spacer" />

          <div className="storage">
            <div>
              <ShieldCheck size={14} /> 隔离目录与 Journal
            </div>
            <div className="storage-path" title={storage?.quarantineDir}>
              {storage?.quarantineDir ?? "读取中…"}
            </div>
            <div className="storage-path" title={storage?.journalFile}>
              {storage?.journalFile ?? ""}
            </div>
            <div>保留期默认 7 天 · 可配 1–30 天</div>
          </div>
        </aside>

        <main className="main">
          <header className="topbar">
            <div className="title-group">
              <div className="eyebrow">C09 · 数据清理</div>
              <h1 className="title">重复文件查找</h1>
              <p className="subtitle">先预览、再执行、可回滚、有日志。</p>
            </div>
            <div className="top-actions">
              {!isTauri() && <span className="mock-banner">浏览器预览 · mock IPC</span>}
              <span className="badge">P0★ · G1</span>
              <button
                className="theme-toggle"
                type="button"
                onClick={() => setTheme(theme === "light" ? "dark" : "light")}
                aria-label="切换主题"
              >
                {theme === "light" ? <Moon size={14} /> : <Sun size={14} />}
                {theme === "light" ? "深色" : "浅色"}
              </button>
            </div>
          </header>

          <div className="content">
            {error && (
              <div className="banner banner-danger" role="alert">
                <div>
                  <strong>{error.code}</strong> · {error.message}
                  {error.detail && <div className="muted">{error.detail}</div>}
                </div>
                <button className="button button-ghost" type="button" onClick={() => setError(null)}>
                  关闭
                </button>
              </div>
            )}
            {notice && (
              <div className="banner banner-info" role="status">
                <div>{notice}</div>
              </div>
            )}

            <div className="content-grid">
              <div className="stack">
                <section className="card" aria-labelledby="params-title">
                  <div className="card-header">
                    <div>
                      <h2 className="card-title" id="params-title">扫描与清理参数</h2>
                      <p className="card-sub">快速窗口留空时使用“大小 → 全量”两阶段；填写后启用三阶段。</p>
                    </div>
                    <span className="badge">只读扫描</span>
                  </div>

                  <div className="field-grid">
                    <div className="field wide">
                      <label htmlFor="root">扫描目录</label>
                      <input
                        id="root"
                        className="input"
                        value={root}
                        onChange={(event) => setRoot(event.target.value)}
                        placeholder="例如 D:\资料归档 或 /home/user/Documents"
                        spellCheck={false}
                      />
                    </div>
                    <div className="field">
                      <label htmlFor="keep">保留策略</label>
                      <select
                        id="keep"
                        className="select"
                        value={keep}
                        onChange={(event) => setKeep(event.target.value as KeepPolicy)}
                      >
                        <option value="earliest">最早（默认）</option>
                        <option value="shortest-path">最短路径</option>
                        <option value="newest">最新</option>
                      </select>
                    </div>
                    <div className="field">
                      <label htmlFor="quick">快速哈希窗口（字节）</label>
                      <input
                        id="quick"
                        className="input"
                        inputMode="numeric"
                        value={quickWindow}
                        onChange={(event) => setQuickWindow(event.target.value)}
                        placeholder="留空 = 跳过"
                      />
                    </div>
                    <div className="field wide">
                      <label htmlFor="protected">保护目录（每行一个，可留空）</label>
                      <textarea
                        id="protected"
                        className="textarea"
                        value={protectedText}
                        onChange={(event) => setProtectedText(event.target.value)}
                        placeholder="例如 D:\系统资料&#10;/home/user/important"
                        spellCheck={false}
                      />
                      <div className="hint">S02 保护目录在计划期标记受阻，执行期再次复核；命中后不会移动文件。</div>
                    </div>
                  </div>

                  <div className="toolbar">
                    <span className="hint">仅扫描，不写入；计划生成后可逐条核对。</span>
                    <button
                      className="button button-primary"
                      type="button"
                      onClick={onPlan}
                      disabled={busy !== null || taskActive}
                    >
                      {busy === "plan" ? <Loader2 size={16} className="spin" /> : <FolderSearch size={16} />}
                      {busy === "plan" ? "正在扫描与分组…" : "生成计划预览"}
                    </button>
                  </div>
                </section>

                <section className="card" aria-labelledby="plan-title">
                  <div className="card-header">
                    <div>
                      <h2 className="card-title" id="plan-title">计划预览</h2>
                      <p className="card-sub">每一行都是将要移入隔离目录的文件；受阻项不会执行。</p>
                    </div>
                    {plan && <span className="plan-id" title={plan.planId}>{plan.planId}</span>}
                  </div>

                  {!plan ? (
                    <div className="empty">填写扫描目录并生成计划后，这里会显示重复组与清理条目。</div>
                  ) : (
                    <>
                      <div className="stats">
                        <div className="stat">
                          <div className="stat-label">总条目</div>
                          <div className="stat-value">{plan.stats.total}</div>
                        </div>
                        <div className="stat">
                          <div className="stat-label">冲突</div>
                          <div className="stat-value">{plan.stats.conflict}</div>
                        </div>
                        <div className="stat">
                          <div className="stat-label">警告</div>
                          <div className="stat-value">{plan.stats.warning}</div>
                        </div>
                        <div className="stat">
                          <div className="stat-label">受阻</div>
                          <div className="stat-value">{plan.stats.blocked}</div>
                        </div>
                      </div>

                      <div className="table">
                        <div className="table-scroll">
                          <div className="row row-head">
                            <span>待清理文件</span>
                            <span>保留依据 / 原因</span>
                            <span>状态</span>
                          </div>
                          {plan.entries.map((entry) => (
                            <div className={`row${entry.blocked ? " blocked" : ""}`} key={entry.entryId}>
                              <span className="path" title={entry.source}>{entry.source}</span>
                              <span className="reason" title={entry.reason}>{entry.reason}</span>
                              <span>{statusBadge(plan, entry.entryId)}</span>
                            </div>
                          ))}
                        </div>
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
                      <h2 className="card-title" id="execute-title">执行</h2>
                      <p className="card-sub">删除动作只移入隔离目录；先核对预览，再确认执行。</p>
                    </div>
                    {applyTask && stateBadge(applyTask.state)}
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

                  <label className="check" htmlFor="confirm">
                    <input
                      id="confirm"
                      type="checkbox"
                      checked={confirmed}
                      onChange={(event) => setConfirmed(event.target.checked)}
                      disabled={!plan || Boolean(applyTask) || taskActive || busy !== null}
                    />
                    我已核对预览，确认将 {deletableCount} 条文件移入隔离目录
                  </label>

                  <div className="divider" />

                  <div className="button-row">
                    <button className="button button-primary" type="button" onClick={onApply} disabled={!canApply}>
                      {isActive(applyTask) ? <Loader2 size={16} className="spin" /> : <Play size={16} />}
                      {isActive(applyTask) ? "执行中…" : "移入隔离目录"}
                    </button>
                    <button className="button button-secondary" type="button" onClick={onUndo} disabled={!canUndo}>
                      {isActive(undoTask) ? <Loader2 size={16} className="spin" /> : <Undo2 size={16} />}
                      {isActive(undoTask) ? "撤销中…" : "撤销本次计划"}
                    </button>
                  </div>

                  {hasBlocked && (
                    <div className="warning danger" style={{ marginTop: "var(--space-3)" }}>
                      <AlertTriangle size={14} />
                      <span>存在受阻条目；执行时会跳过，不会触碰保护目录。</span>
                    </div>
                  )}

                  <div className="hint" style={{ marginTop: "var(--space-3)" }}>
                    撤销窗口默认 7 天；超过窗口后需先执行保留期清理。
                  </div>
                </section>

                <section className="card" aria-labelledby="cleanup-title">
                  <div className="card-header">
                    <div>
                      <h2 className="card-title" id="cleanup-title">保留期清理</h2>
                      <p className="card-sub">默认演练；勾选永久删除后才会移除过期隔离文件与 Journal。</p>
                    </div>
                    <HardDrive size={18} />
                  </div>

                  <div className="field" style={{ marginBottom: "var(--space-3)" }}>
                    <label htmlFor="retention">保留天数（1–30）</label>
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
                    永久删除过期隔离文件（不可撤销）
                  </label>

                  <div className="toolbar">
                    <span className="hint">{purge ? "本次将永久删除。" : "演练模式，不删除文件。"}</span>
                    <button
                      className="button button-danger"
                      type="button"
                      onClick={onCleanup}
                      disabled={busy !== null || taskActive}
                    >
                      {busy === "cleanup" ? <Loader2 size={16} className="spin" /> : <Trash2 size={16} />}
                      {busy === "cleanup" ? "正在处理保留期…" : purge ? "永久清理" : "演练清理"}
                    </button>
                  </div>
                </section>

                <section className="card" aria-labelledby="result-title">
                  <div className="card-header">
                    <div>
                      <h2 className="card-title" id="result-title">执行结果</h2>
                      <p className="card-sub">结果只来自引擎报告；没有静默处理。</p>
                    </div>
                    <RefreshCw size={18} />
                  </div>

                  {!applyTask?.report && !undoTask?.report && !cleanupOutcome && (
                    <div className="empty">执行后显示每条结果。</div>
                  )}

                  {applyTask?.report && (
                    <div className="result-list">
                      {applyTask.report.entries.map((entry) => (
                        <div className="result-row" key={entry.entryId}>
                          <span className="path">{entry.entryId}</span>
                          <span className={`badge ${entry.status === "ok" ? "badge-safe" : entry.status === "failed" ? "badge-danger" : "badge-warn"}`}>
                            {entry.status}
                          </span>
                          {entry.error && <span className="path">{entry.error.code} · {entry.error.message}</span>}
                        </div>
                      ))}
                    </div>
                  )}

                  {undoTask?.report && (
                    <div className="banner banner-info" style={{ marginTop: "var(--space-3)" }}>
                      <CheckCircle2 size={16} />
                      <span>
                        Journal 已恢复 {undoReverted} 项
                        {undoFailures ? `，${undoFailures} 项失败` : ""}。
                      </span>
                    </div>
                  )}

                  {cleanupOutcome && (
                    <div className="banner banner-info" style={{ marginTop: "var(--space-3)" }}>
                      <CheckCircle2 size={16} />
                      <span>
                        截止 {cleanupOutcome.cutoff}：过期 {cleanupOutcome.expiredEntries} 条，
                        移除 {cleanupOutcome.removedEntries} 条记录 / {cleanupOutcome.removedFiles} 个文件。
                      </span>
                    </div>
                  )}
                </section>

                <section className="card" aria-labelledby="about-title">
                  <div className="card-header">
                    <div>
                      <h2 className="card-title" id="about-title">当前形态</h2>
                      <p className="card-sub">桌面壳：本地文件系统；Web/壳零授权强制力。</p>
                    </div>
                  </div>
                  <div className="hint">
                    内核 {meta?.kernelVersion ?? "…"} · 契约 v{meta?.contractVersion ?? "…"}
                  </div>
                  <div className="hint" style={{ marginTop: "var(--space-2)" }}>
                    执行与撤销已走桌面任务队列；进度、暂停、取消与终态通过 task:// 事件推送。
                  </div>
                </section>
              </div>
            </div>
          </div>
        </main>
      </div>
    </div>
  );
}

export default App;
