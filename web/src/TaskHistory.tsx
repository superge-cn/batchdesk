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
import { TableHead, columnVarsStyle, useUiTableState } from "./TableHead";
import { sortRows } from "./lib/uiState";
import type { MessageKey } from "./i18n";
import { ErrorBanner } from "./ErrorBanner";
import { isActiveState, phaseLabel, stateBadge, taskKindLabel, type Translate } from "./labels";
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
// 列宽＋列头排序（设计 03 §11.2）：排序键按 `TaskSummary` 的字段名取
// （AGENTS 引用回核：`taskId`／`done`／`total`／`state` 均取自类型定义，不猜）。
// `actions` 列不参与排序——它是操作列，没有可比较的值。
const TASKS_TABLE = "table.tasks";
const TASKS_COLUMNS = ["task", "progress", "state", "actions"];
const TASKS_LABELS: Record<string, MessageKey> = {
  task: "tasks.table.task",
  progress: "tasks.table.progress",
  state: "tasks.table.state",
  actions: "tasks.table.actions"
};

/**
 * 排序比较键（设计 03 §11.2 的视图态排序）：取 `TaskSummary` 的字段值，不取界面文案
 * （文案随语言变，键不随语言变）。进度按 `done/total` 的比值排——条目数受计划规模影响，
 * 比值才反映"跑到哪了"。
 */
function taskSortValue(task: TaskSummary, column: string): string | number | undefined {
  if (column === "task") return task.taskId;
  if (column === "progress") return task.total === 0 ? 0 : task.done / task.total;
  if (column === "state") return task.state;
  return undefined;
}

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
  // 列宽把手（设计 03 §11.2 的 tables）：表 id 固定为 table.tasks。
  const uiTables = useUiTableState();

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

  /**
   * 实时态覆盖静态快照：列表打开期间任务还在跑时，进度与状态必须是活的。
   * 用户排过序就按排序显示；没排过保持"最近结束在前"的缺省（视图态，不改派发序）。
   */
  const merged = items.map((task) =>
    live[task.taskId] ? { ...task, ...live[task.taskId] } : task
  );
  const userSort = uiTables.sortOf(TASKS_TABLE);
  const rows = userSort
    ? sortRows(merged, userSort, taskSortValue)
    : merged.sort((left, right) => {
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

    // 回滚任务的重试没有意义：壳层只接受 apply 任务重试（否则必然 E1002）。判别用 job，
    // 因为 kind 装的是被处理计划的功能码，壳层从不把它写成 "undo"。
    if ((task.state === "failed" || task.state === "partial-failed") && task.job !== "undo") {
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
    // 回滚为终态，不可二次回滚（设计 01 §7、feature.execute.done 的措辞同源）。
    if (planId && task.job !== "undo" && !isActiveState(task.state)) {
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
      {error && <ErrorBanner error={error} onClose={() => setError(null)} />}
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
          <div
            className="table"
            style={columnVarsStyle(TASKS_COLUMNS, uiTables.widths(TASKS_TABLE))}
          >
            <TableHead
              tableId={TASKS_TABLE}
              tables={uiTables}
              columns={TASKS_COLUMNS.map((id) => ({
                id,
                label: t(TASKS_LABELS[id]),
                // actions 是操作列，没有可比较的值；其余三列按 TaskSummary 字段排。
                sortable: id !== "actions"
              }))}
            />
            <VirtualList
              label={t("tasks.detail")}
              items={rows}
              getKey={(task) => task.taskId}
              renderRow={(task) => (
                <div className="row row-tasks">
                  <span className="path" title={task.taskId}>
                    {taskKindLabel(t, task.kind)} · {shortId(task.taskId)}
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
