import { useCallback, useEffect, useState, type ReactNode } from "react";
import { Play, RefreshCw, RotateCcw, Undo2, X } from "lucide-react";
import {
  cancelTask,
  listTasks,
  recoverTask,
  retryTask,
  toIpcError,
  undoPlan
} from "./lib/ipc";
import { VirtualList } from "./VirtualList";
import { isActiveState, phaseLabel, stateBadge, type Translate } from "./labels";
import { useI18n } from "./i18n";
import type { IpcError, TaskState, TaskSummary } from "./lib/ipc/types";

/** 本会话内由 task:// 事件推动的实时态；列表里的静态摘要用它覆盖。 */
export interface LiveTaskState {
  state: TaskState;
  phase: TaskSummary["phase"];
  done: number;
  total: number;
  message?: string;
}

const PAGE_LIMIT = 100;

function shortId(taskId: string) {
  // 常见任务 ID 是 `task:apply:<planId>` 或直接 ULID，长度可达 40+：
  // 超长时只显示尾部（尾部含随机段，足够区分），短 ID 原样显示便于核对。
  return taskId.length > 20 ? `…${taskId.slice(-10)}` : taskId;
}

function progressText(t: Translate, task: TaskSummary) {
  const percent = task.total > 0 ? Math.min(100, Math.round((task.done / task.total) * 100)) : 0;
  return `${phaseLabel(t, task.phase)} · ${task.done}/${task.total}${task.total > 0 ? ` · ${percent}%` : ""}`;
}

function timeText(task: TaskSummary) {
  const stamp = task.finishedAt ?? task.startedAt;
  if (!stamp) return "—";
  // 壳层给的是 RFC3339；这里只展示到秒，避免宽表里出现一长串小数。
  return stamp.replace("T", " ").replace(/(\.\d+)?(Z|[+-]\d{2}:\d{2})?$/, "");
}

/**
 * 任务与撤销（P2）。
 *
 * 为什么撤销放在这里：撤销按 plan 走共享 Journal（方案 7.8），而任务摘要现在带 `planId`，
 * 于是"刚才那次执行"可以直接从这里回滚，不必回到生成计划的页面。功能自建撤销是禁止的，
 * 这里调用的仍是 `undoPlan` 同一条通道。
 *
 * 任务列表本身也可能上百条，照样走窗口化（方案 6.2）。
 */
export function TaskHistory({
  live,
  onBack
}: {
  live: Record<string, LiveTaskState>;
  onBack: () => void;
}) {
  const [items, setItems] = useState<TaskSummary[]>([]);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState<IpcError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busyTask, setBusyTask] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const { t } = useI18n();

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const page = await listTasks(0, PAGE_LIMIT);
      setItems(page.items);
      setTotal(page.total);
      setError(null);
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  /** 实时态覆盖静态快照：列表打开期间任务还在跑时，进度与状态必须是活的。 */
  const rows = items
    .map((task) => (live[task.taskId] ? { ...task, ...live[task.taskId] } : task))
    .sort((left, right) => {
      const leftStamp = left.finishedAt ?? left.startedAt ?? "";
      const rightStamp = right.finishedAt ?? right.startedAt ?? "";
      if (leftStamp === rightStamp) return right.taskId.localeCompare(left.taskId);
      return rightStamp.localeCompare(leftStamp);
    });

  const run = async (task: TaskSummary, action: () => Promise<unknown>, done: string) => {
    setBusyTask(task.taskId);
    setNotice(null);
    try {
      await action();
      setNotice(done);
      await refresh();
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setBusyTask(null);
    }
  };

  const actionsFor = (task: TaskSummary) => {
    const busy = busyTask === task.taskId;
    const buttons: ReactNode[] = [];

    if (task.state === "interrupted") {
      buttons.push(
        <button
          key="resume"
          className="button button-secondary"
          type="button"
          disabled={busy}
          onClick={() =>
            void run(task, () => recoverTask(task.taskId, "resume"), t("tasks.notice.resumed"))
          }
        >
          <Play size={14} /> {t("common.resumeRun")}
        </button>,
        <button
          key="rollback"
          className="button button-secondary"
          type="button"
          disabled={busy}
          onClick={() =>
            void run(task, () => recoverTask(task.taskId, "rollback"), t("tasks.notice.rollback"))
          }
        >
          <Undo2 size={14} /> {t("common.rollback")}
        </button>,
        <button
          key="discard"
          className="button button-ghost"
          type="button"
          disabled={busy}
          onClick={() =>
            void run(task, () => recoverTask(task.taskId, "discard"), t("tasks.notice.discard"))
          }
        >
          {t("common.discard")}
        </button>
      );
      return buttons;
    }

    if (isActiveState(task.state)) {
      buttons.push(
        <button
          key="cancel"
          className="button button-danger"
          type="button"
          disabled={busy}
          aria-label={t("tasks.aria.cancel", { id: task.taskId })}
          onClick={() =>
            void run(task, () => cancelTask(task.taskId), t("tasks.notice.cancel"))
          }
        >
          <X size={14} /> {t("common.cancelAction")}
        </button>
      );
    }

    if ((task.state === "failed" || task.state === "partial-failed") && task.kind !== "undo") {
      buttons.push(
        <button
          key="retry"
          className="button button-secondary"
          type="button"
          disabled={busy}
          aria-label={t("tasks.aria.retry", { id: task.taskId })}
          onClick={() => void run(task, () => retryTask(task.taskId), t("tasks.notice.retry"))}
        >
          <RotateCcw size={14} /> {t("common.retry")}
        </button>
      );
    }

    const planId = task.planId;
    // undo 任务本身就消耗掉了那次撤销：它没有可再撤销的条目，不给入口（回滚为终态）。
    if (planId && task.kind !== "undo" && !isActiveState(task.state)) {
      buttons.push(
        <button
          key="undo"
          className="button button-secondary"
          type="button"
          disabled={busy}
          aria-label={t("tasks.aria.undo", { plan: planId })}
          onClick={() =>
            void run(task, () => undoPlan(planId), t("tasks.notice.undo"))
          }
        >
          <Undo2 size={14} /> {t("common.undo")}
        </button>
      );
    }

    return buttons;
  };

  const activeCount = rows.filter((task) => isActiveState(task.state)).length;

  return (
    <div className="content">
      {error && (
        <div className="banner banner-danger" role="alert">
          <div>
            <strong>{error.code}</strong> · {error.message}
            {error.detail && <div className="muted">{error.detail}</div>}
          </div>
          <button className="button button-ghost" type="button" onClick={() => setError(null)}>
            {t("common.close")}
          </button>
        </div>
      )}
      {notice && (
        <div className="banner banner-info" role="status">
          <div>{notice}</div>
        </div>
      )}

      <section className="card" aria-labelledby="tasks-title">
        <div className="card-header">
          <div>
            <h2 className="card-title" id="tasks-title">{t("tasks.detail")}</h2>
            <p className="card-sub">
              {t("tasks.summary", { total, active: activeCount })}
            </p>
          </div>
          <div className="button-row">
            <button
              className="button button-secondary"
              type="button"
              onClick={() => void refresh()}
              disabled={loading}
            >
              <RefreshCw size={14} className={loading ? "spin" : undefined} /> {t("common.refresh")}
            </button>
            <button className="button button-ghost" type="button" onClick={onBack}>
              {t("common.backC09")}
            </button>
          </div>
        </div>

        {rows.length === 0 ? (
          <div className="empty">
            {t("tasks.empty")}
          </div>
        ) : (
          <div className="table">
            <div className="row row-tasks row-head">
              <span>{t("tasks.table.task")}</span>
              <span>{t("tasks.table.progress")}</span>
              <span>{t("tasks.table.state")}</span>
              <span>{t("tasks.table.actions")}</span>
            </div>
            <VirtualList
              label={t("tasks.detail")}
              items={rows}
              getKey={(task) => task.taskId}
              renderRow={(task) => (
                <div className="row row-tasks">
                  <span className="path" title={task.taskId}>
                    {task.kind} · {shortId(task.taskId)}
                  </span>
                  <span className="reason" title={task.message}>
                    {progressText(t, task)}
                    {task.message ? ` · ${task.message}` : ""}
                  </span>
                  <span>{stateBadge(t, task.state)}</span>
                  <span className="button-row task-row-actions">
                    {timeText(task) !== "—" && (
                      <span className="hint" title={t("tasks.time")}>{timeText(task)}</span>
                    )}
                    {actionsFor(task)}
                  </span>
                </div>
              )}
            />
          </div>
        )}
      </section>
    </div>
  );
}
