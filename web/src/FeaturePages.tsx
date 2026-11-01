import { useEffect, useState } from "react";
import { Loader2, Undo2 } from "lucide-react";
import {
  applyPlan,
  getTask,
  isTauri,
  listenTaskDone,
  listenTaskProgress,
  listenTaskState,
  planOperation,
  renamePreview,
  toIpcError,
  undoDryRun,
  undoPlan
} from "./lib/ipc";
import type {
  IpcError,
  PlanDocument,
  PreviewStatus,
  ProgressPhase,
  RenamePreview,
  TaskState,
  UndoDryRun
} from "./lib/ipc/types";

type View = "f05" | "f11" | "f12";
type Busy = "plan" | "apply" | "undo" | null;

interface TaskView {
  taskId: string;
  state: TaskState;
  phase: ProgressPhase;
  done: number;
  total: number;
  message?: string;
}

function planOperationFor(
  view: View,
  form: Record<string, string>
): Promise<PlanDocument> {
  if (view === "f05") {
    return planOperation({
      kind: "f05",
      params: {
        file: form.file,
        outputDir: form.outputDir,
        ...(form.targetBytes ? { targetBytes: Number(form.targetBytes) } : {}),
        ...(form.rows ? { rows: Number(form.rows) } : {}),
        ...(form.delimiter ? { delimiter: form.delimiter } : {})
      },
      sourcePaths: []
    } as never);
  }
  if (view === "f11") {
    return planOperation({
      kind: "f11",
      params: { list: form.list, root: form.root },
      sourcePaths: []
    } as never);
  }
  return planOperation({
    kind: "f12",
    params: {
      dir: form.dir,
      ...(form.prefix ? { prefix: form.prefix } : {}),
      ...(form.suffix ? { suffix: form.suffix } : {}),
      ...(form.find ? { find: form.find } : {}),
      replace: form.replace,
      scope: form.scope
    },
    sourcePaths: []
  } as never);
}

function statusBadge(status: string) {
  if (status === "normal") return <span className="badge badge-safe">正常</span>;
  if (status === "conflict") return <span className="badge badge-danger">冲突</span>;
  if (status === "invalid") return <span className="badge badge-danger">非法</span>;
  if (status === "too-long") return <span className="badge badge-warn">超长</span>;
  if (status === "protected") return <span className="badge badge-danger">保护</span>;
  return <span className="badge">{status}</span>;
}

export function FeaturePages({ view, onBack }: { view: View; onBack: () => void }) {
  const title =
    view === "f05" ? "CSV 分割（F05）" : view === "f11" ? "清单建档（F11）" : "批量改名（F12）";
  const [form, setForm] = useState<Record<string, string>>({});
  const [plan, setPlan] = useState<PlanDocument | null>(null);
  const [preview, setPreview] = useState<RenamePreview | null>(null);
  const [dryRun, setDryRun] = useState<UndoDryRun | null>(null);
  const [task, setTask] = useState<TaskView | null>(null);
  const [error, setError] = useState<IpcError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState<Busy>(null);
  const [canUndo, setCanUndo] = useState(false);

  const setField = (name: string, value: string) =>
    setForm((previous) => ({ ...previous, [name]: value }));

  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    const track = (subscription: Promise<() => void>) => {
      subscription
        .then((unlisten) => unlisteners.push(unlisten))
        .catch(() => undefined);
    };
    const patch = (taskId: string, update: Partial<TaskView>) =>
      setTask((current) =>
        current && current.taskId === taskId ? { ...current, ...update } : current
      );
    track(
      listenTaskProgress((event) => {
        const entryProgress =
          event.phase === "queued" || event.phase === "executing"
            ? { done: event.done, total: event.total }
            : {};
        patch(event.taskId, { phase: event.phase, ...entryProgress, message: event.message });
      })
    );
    track(listenTaskState((event) => patch(event.taskId, { state: event.state })));
    track(
      listenTaskDone((event) => {
        getTask(event.taskId)
          .then((detail) =>
            patch(event.taskId, {
              state: detail.summary.state,
              phase: detail.summary.phase,
              done: detail.summary.done,
              total: detail.summary.total,
              message: detail.summary.message
            })
          )
          .catch(() => undefined);
      })
    );
    return () => unlisteners.forEach((unlisten) => unlisten());
  }, []);

  const onPlan = async () => {
    setBusy("plan");
    setError(null);
    setNotice(null);
    setPlan(null);
    setPreview(null);
    setDryRun(null);
    setCanUndo(false);
    try {
      const next = await planOperationFor(view, form);
      setPlan(next);
      if (view === "f12") {
        try {
          setPreview(await renamePreview(next.planId));
        } catch (value) {
          setPreview(null);
        }
      }
      setNotice(`计划已生成：${next.entries.length} 条。核对预览后执行。`);
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setBusy(null);
    }
  };

  const onApply = async () => {
    if (!plan) return;
    setBusy("apply");
    setError(null);
    setNotice(null);
    try {
      const handle = await applyPlan(plan.planId, {
        onConflict: "skip",
        overwriteConfirmed: false
      });
      setTask({
        taskId: handle.taskId,
        state: "queued",
        phase: "queued",
        done: 0,
        total: plan.entries.length
      });
      setNotice("任务已加入队列；进度通过 task:// 事件推送。");
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setBusy(null);
    }
  };

  const onUndo = async () => {
    if (!plan) return;
    setBusy("undo");
    setError(null);
    setNotice(null);
    try {
      const handle = await undoPlan(plan.planId);
      setTask({
        taskId: handle.taskId,
        state: "queued",
        phase: "queued",
        done: 0,
        total: 0
      });
      setNotice("撤销任务已加入队列；回滚进度通过 task:// 事件推送。");
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setBusy(null);
    }
  };

  const onUndoDryRun = async () => {
    if (!plan) return;
    setError(null);
    try {
      setDryRun(await undoDryRun(plan.planId));
    } catch (value) {
      setError(toIpcError(value));
    }
  };

  const active = task !== null && !["completed", "partial-failed", "failed", "cancelled"].includes(task.state);
  const terminalOk = task !== null && ["completed", "partial-failed"].includes(task.state);

  const field = (
    label: string,
    name: string,
    placeholder: string,
    wide = false
  ) => (
    <div className={`field${wide ? " wide" : ""}`}>
      <label htmlFor={`${view}-${name}`}>{label}</label>
      <input
        id={`${view}-${name}`}
        className="input"
        value={form[name] ?? ""}
        onChange={(event) => setField(name, event.target.value)}
        placeholder={placeholder}
        spellCheck={false}
      />
    </div>
  );

  return (
    <div className="content">
      {error && (
        <div className="banner banner-danger" role="alert">
          <div>
            <strong>{error.code}</strong> · {error.message}
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
          <section className="card" aria-labelledby="feature-params">
            <div className="card-header">
              <div>
                <h2 className="card-title" id="feature-params">{title}</h2>
                <p className="card-sub">先预览、再执行、可撤销（C18 干跑：plan 命令即零落盘预览）。</p>
              </div>
              <button className="button button-ghost" type="button" onClick={onBack}>
                返回 C09
              </button>
            </div>

            <div className="field-grid">
              {view === "f05" && (
                <>
                  {field("源 CSV 文件", "file", "例如 D:\\名单.csv 或 /home/user/名单.csv", true)}
                  {field("分片目标字节（二选一）", "targetBytes", "留空则用行数")}
                  {field("分片行数（二选一）", "rows", "留空则用字节")}
                  {field("输出目录", "outputDir", "分片输出目录", true)}
                  {field("分隔符（可留空嗅探）", "delimiter", ",")}
                </>
              )}
              {view === "f11" && (
                <>
                  {field("目标根目录", "root", "清单路径相对该目录", true)}
                </>
              )}
              {view === "f12" && (
                <>
                  {field("改名目录", "dir", "仅目录一层（方案 §5.13）", true)}
                  {field("前缀", "prefix", "追加到词根前")}
                  {field("后缀", "suffix", "追加到词根后")}
                  {field("查找文本", "find", "字面查找")}
                  {field("替换为", "replace", "替换文本")}
                  {field("作用范围 full|stem|ext", "scope", "full")}
                </>
              )}
              {view === "f11" && (
                <div className="field wide">
                  <label htmlFor={`${view}-list`}>清单（每行一个相对路径）</label>
                  <textarea
                    id={`${view}-list`}
                    className="textarea"
                    value={form.list ?? ""}
                    onChange={(event) => setField("list", event.target.value)}
                    placeholder={"folder-a\nfolder-b/sub\n"}
                    rows={6}
                    spellCheck={false}
                  />
                  <div className="hint">路径相对目标根目录；禁止 `..` 逃逸（E1003）。</div>
                </div>
              )}
            </div>

            <div className="toolbar">
              <span className="hint">plan 命令即干跑预览（C18）：零落盘、重复预览幂等。</span>
              <button
                className="button button-primary"
                type="button"
                onClick={onPlan}
                disabled={busy !== null || active}
              >
                {busy === "plan" ? <Loader2 size={16} className="spin" /> : null}
                {busy === "plan" ? "正在生成计划…" : "生成计划预览"}
              </button>
            </div>
          </section>

          {plan && (
            <section className="card" aria-labelledby="feature-plan">
              <div className="card-header">
                <div>
                  <h2 className="card-title" id="feature-plan">计划预览</h2>
                  <p className="card-sub">
                    {plan.entries.length} 条 ·受阻 {plan.stats.blocked} ·警告 {plan.stats.warning}
                  </p>
                </div>
                <span className="plan-id" title={plan.planId}>{plan.planId}</span>
              </div>
              <div className="table">
                <div className="table-scroll">
                  <div className="row row-head">
                    <span>源</span>
                    <span>目标 / 说明</span>
                  </div>
                  {plan.entries.map((entry) => (
                    <div className="row" key={entry.entryId}>
                      <span className="path" title={entry.source}>{entry.source}</span>
                      <span className="path" title={entry.target ?? entry.reason}>
                        {entry.target ?? entry.reason}
                        {entry.blocked ? ` · ${entry.blocked.code}` : ""}
                      </span>
                    </div>
                  ))}
                </div>
              </div>
            </section>
          )}
        </div>

        <div className="stack">
          <section className="card" aria-labelledby="feature-execute">
            <div className="card-header">
              <div>
                <h2 className="card-title" id="feature-execute">执行与撤销</h2>
                <p className="card-sub">C06：撤销前可回滚预演；回滚为终态。</p>
              </div>
            </div>
            <div className="button-row">
              <button
                className="button button-primary"
                type="button"
                onClick={onApply}
                disabled={!plan || busy !== null || active || plan.entries.length === 0}
              >
                {busy === "apply" ? <Loader2 size={16} className="spin" /> : null}
                {active ? "执行中…" : "执行"}
              </button>
              <button
                className="button button-secondary"
                type="button"
                onClick={onUndoDryRun}
                disabled={!plan}
              >
                回滚预演
              </button>
              <button
                className="button button-secondary"
                type="button"
                onClick={onUndo}
                disabled={!plan || busy !== null || active}
              >
                {busy === "undo" ? <Loader2 size={16} className="spin" /> : <Undo2 size={16} />}
                撤销
              </button>
            </div>
            {task && (
              <div className="task-panel">
                <div className="task-head">
                  <span className="task-id" title={task.taskId}>{task.taskId}</span>
                  <span className="badge">{task.state}</span>
                </div>
                <div className="progress" role="progressbar" aria-label="进度">
                  <div
                    className="progress-fill"
                    style={{
                      width: `${task.total > 0 ? Math.min(100, Math.round((task.done / task.total) * 100)) : 0}%`
                    }}
                  />
                </div>
                <div className="hint">
                  {task.phase} · {task.done}/{task.total}
                  {task.message ? ` · ${task.message}` : ""}
                </div>
              </div>
            )}
            {dryRun && (
              <div className="banner banner-info" role="status">
                <div>
                  <strong>回滚预演</strong> · 冲突 {dryRun.conflictCount} · 缺失 {dryRun.missingCount}
                  {dryRun.entries.map((entry) => (
                    <div className="muted" key={entry.entryId}>
                      {entry.entryId} · {entry.status}
                      {entry.detail ? ` · ${entry.detail}` : ""}
                    </div>
                  ))}
                </div>
              </div>
            )}
            {terminalOk && (
              <div className="banner banner-info" role="status">
                <div>执行完成；如需恢复可点击「撤销」（回滚为终态，不可二次回滚）。</div>
              </div>
            )}
          </section>

          {view === "f12" && preview && (
            <section className="card" aria-labelledby="c05-preview">
              <div className="card-header">
                <div>
                  <h2 className="card-title" id="c05-preview">C05 预览与冲突检测</h2>
                  <p className="card-sub">
                    将处理 {preview.counts.toProcess} · 跳过 {preview.counts.skipped} · 冲突{" "}
                    {preview.counts.conflicts} · 异常 {preview.counts.blocked}
                  </p>
                </div>
              </div>
              <div className="table">
                <div className="table-scroll">
                  <div className="row row-head">
                    <span>状态</span>
                    <span>旧名 → 新名</span>
                  </div>
                  {preview.rows.map((row) => (
                    <div className="row" key={row.entryId}>
                      <span>{statusBadge(row.status as PreviewStatus)}{row.twoPhase ? " (两阶段)" : ""}</span>
                      <span className="path">
                        {row.oldPath} → {row.newPath}
                        {row.detail ? ` · ${row.detail}` : ""}
                      </span>
                    </div>
                  ))}
                </div>
              </div>
              {preview.highlights.length > 0 && (
                <div className="warnings">
                  {preview.highlights.map((highlight, index) => (
                    <div className="warning caution" key={index}>
                      <span>{highlight}</span>
                    </div>
                  ))}
                </div>
              )}
            </section>
          )}

          <section className="card">
            <div className="card-header">
              <div>
                <h2 className="card-title">形态</h2>
                <p className="card-sub">
                  {isTauri() ? "桌面壳：本地文件系统" : "浏览器预览 · mock IPC（无真实计划）"}
                </p>
              </div>
            </div>
          </section>
        </div>
      </div>
    </div>
  );
}
