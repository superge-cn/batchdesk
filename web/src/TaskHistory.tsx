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
import { isActiveState, phaseLabel, stateBadge } from "./labels";
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

function progressText(task: TaskSummary) {
  const percent = task.total > 0 ? Math.min(100, Math.round((task.done / task.total) * 100)) : 0;
  return `${phaseLabel(task.phase)} · ${task.done}/${task.total}${task.total > 0 ? ` · ${percent}%` : ""}`;
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
            void run(task, () => recoverTask(task.taskId, "resume"), "已提交续跑；进度见上方任务面板。")
          }
        >
          <Play size={14} /> 续跑
        </button>,
        <button
          key="rollback"
          className="button button-secondary"
          type="button"
          disabled={busy}
          onClick={() =>
            void run(task, () => recoverTask(task.taskId, "rollback"), "已提交回滚。")
          }
        >
          <Undo2 size={14} /> 回滚
        </button>,
        <button
          key="discard"
          className="button button-ghost"
          type="button"
          disabled={busy}
          onClick={() => void run(task, () => recoverTask(task.taskId, "discard"), "已放弃恢复。")}
        >
          放弃
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
          aria-label={`取消任务 ${task.taskId}`}
          onClick={() => void run(task, () => cancelTask(task.taskId), "取消已提交；未落盘的条目不再执行。")}
        >
          <X size={14} /> 取消
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
          aria-label={`重试失败项 ${task.taskId}`}
          onClick={() => void run(task, () => retryTask(task.taskId), "已提交重试；只重跑失败条目。")}
        >
          <RotateCcw size={14} /> 重试
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
          aria-label={`撤销计划 ${planId}`}
          onClick={() =>
            void run(task, () => undoPlan(planId), "已提交撤销；回滚为终态。")
          }
        >
          <Undo2 size={14} /> 撤销
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
            关闭
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
            <h2 className="card-title" id="tasks-title">任务与撤销</h2>
            <p className="card-sub">
              共 {total} 条任务 · 进行中 {activeCount} 条 · 撤销走共享 Journal，回滚为终态。
            </p>
          </div>
          <div className="button-row">
            <button
              className="button button-secondary"
              type="button"
              onClick={() => void refresh()}
              disabled={loading}
            >
              <RefreshCw size={14} className={loading ? "spin" : undefined} /> 刷新
            </button>
            <button className="button button-ghost" type="button" onClick={onBack}>
              返回 C09
            </button>
          </div>
        </div>

        {rows.length === 0 ? (
          <div className="empty">
            还没有任务记录。去「数据清理」或任一功能页生成预览并执行，任务会出现在这里。
          </div>
        ) : (
          <div className="table">
            <div className="row row-tasks row-head">
              <span>任务</span>
              <span>进度</span>
              <span>状态</span>
              <span>操作</span>
            </div>
            <VirtualList
              label="任务列表"
              items={rows}
              getKey={(task) => task.taskId}
              renderRow={(task) => (
                <div className="row row-tasks">
                  <span className="path" title={task.taskId}>
                    {task.kind} · {shortId(task.taskId)}
                  </span>
                  <span className="reason" title={task.message}>
                    {progressText(task)}
                    {task.message ? ` · ${task.message}` : ""}
                  </span>
                  <span>{stateBadge(task.state)}</span>
                  <span className="button-row task-row-actions">
                    {timeText(task) !== "—" && (
                      <span className="hint" title="开始 / 完成时间">{timeText(task)}</span>
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
