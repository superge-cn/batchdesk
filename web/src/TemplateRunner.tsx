import { useRef, useState } from "react";
import { Loader2, Play, RotateCcw } from "lucide-react";
import { applyPlan, getTask, listenTaskState, loadRecipe, toIpcError, undoPlan } from "./lib/ipc";
import type {
  PlanDocument,
  RecipeApplySpec,
  RecipeDocument,
  RecipeSlotSpec,
  StepApplySpec
} from "./lib/ipc/types";
import { describeError } from "./errors";
import { useI18n, type MessageKey } from "./i18n";
import { actionLabel } from "./labels";
import { VirtualList } from "./VirtualList";

/**
 * 多步模板执行器（C37 Phase B，方案 §15.2.3）。
 *
 * 步骤线性排列、不建依赖图；**不预生成所有步骤计划**——第 2 步起的输入
 * （`source: previous`＝上一步实际产物）在第 1 步执行前不存在，所以每步
 * 单独 `loadRecipe(stepIndex)` 出计划再执行。
 *
 * 运行模式（本机 UI 偏好，存 UiState，设计 03 §5.2）：
 * - 逐步骤：每步都先 plan、再由用户确认执行；
 * - 一键：首步始终先预览确认；后续每步先 plan，**风险闸门**通过后自动 apply。
 *
 * 风险闸门：计划有冲突/警告/阻断，或该步是高风险步骤（删除等 §7.11 口径），
 * 或无法自动取到输入 → 不自动 apply，停下并降级为逐步骤确认。
 * 含高风险步骤的模板一键不可用（由 `describeRecipe` 的 `oneClickAllowed` 给出）。
 *
 * 某步失败停止后续；已完成步骤可按各自 Journal 撤销（`undoPlan(planId)`）；
 * 不自动跨步回滚；某步无产物记「跳过」并继续，不当作成功。
 */

type StepStatus = "pending" | "planned" | "running" | "done" | "failed" | "skipped" | "blocked";

interface RunStep {
  index: number;
  status: StepStatus;
  plan?: PlanDocument;
  taskId?: string;
}

interface RunState {
  steps: RunStep[];
  /** 当前步骤下标；等于步数＝全部结束。 */
  current: number;
  slotValues: Record<string, string>;
  mode: "one-click" | "stepwise";
  planning: boolean;
  applying: boolean;
  /** 一键自动推进中：锁定模式与输入，避免半途改口径。 */
  auto: boolean;
  notice: string | null;
}

const STATUS_KEY: Record<StepStatus, MessageKey> = {
  pending: "templates.run.status.pending",
  planned: "templates.run.status.planned",
  running: "templates.run.status.running",
  done: "templates.run.status.done",
  failed: "templates.run.status.failed",
  skipped: "templates.run.status.skipped",
  blocked: "templates.run.status.blocked"
};

/** 风险闸门命中原因（§7.11 口径由壳层 `highRisk` 与计划统计共同给出）。 */
const GATE_KEY: Record<"conflict" | "warning" | "blocked" | "high-risk", MessageKey> = {
  conflict: "templates.run.gate.conflict",
  warning: "templates.run.gate.warning",
  blocked: "templates.run.gate.blocked",
  "high-risk": "templates.run.gate.high-risk"
};

const TERMINAL_STATES = new Set([
  "completed",
  "partial-failed",
  "failed",
  "cancelled",
  "interrupted"
]);

/** 任务终态等待：先查一次（可能已经跑完），再订阅状态事件；两端都兜住竞态。 */
async function waitTaskTerminal(taskId: string): Promise<string> {
  try {
    const detail = await getTask(taskId);
    if (TERMINAL_STATES.has(detail.summary.state)) return detail.summary.state;
  } catch {
    // 任务表还没见到它（极端竞态）：交给事件等待。
  }
  return new Promise<string>((resolve) => {
    let unlisten: (() => void) | null = null;
    let settled = false;
    const finish = (state: string) => {
      if (settled) return;
      settled = true;
      unlisten?.();
      resolve(state);
    };
    void listenTaskState((event) => {
      if (event.taskId === taskId && TERMINAL_STATES.has(event.state)) finish(event.state);
    }).then((fn) => {
      if (settled) fn();
      else unlisten = fn;
    });
  });
}

/** 多值槽一行一个路径；单值槽去首尾空白。空行丢弃，空结果返回 undefined 让壳层报缺槽。 */
function slotParam(slot: RecipeSlotSpec, value: string | undefined): string | string[] | undefined {
  const text = (value ?? "").trim();
  if (!slot.many) return text.length > 0 ? text : undefined;
  const items = text
    .split(/[\n,\uFF0C]+/)
    .map((line) => line.trim())
    .filter(Boolean);
  return items.length > 0 ? items : undefined;
}

type Translate = (key: MessageKey, params?: Record<string, string | number>) => string;

/**
 * 界面只渲染 `describeError` 的本地化文案（设计 06 §3：`IpcError.message`／`detail`
 * 是后端技术串，只进诊断日志）。未收录的码位由 errors.ts 兜底。
 */
function errorTitle(t: Translate, value: unknown): string {
  return describeError(t, toIpcError(value)).title;
}

export function TemplateRunner({
  item,
  spec,
  initialMode,
  onModeChange,
  onClose
}: {
  item: RecipeDocument;
  spec: RecipeApplySpec;
  initialMode: "one-click" | "stepwise";
  onModeChange: (mode: "one-click" | "stepwise") => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const [run, setRun] = useState<RunState>(() => ({
    steps: item.steps.map((_, index) => ({ index, status: "pending" as StepStatus })),
    current: 0,
    slotValues: {},
    // 含高风险步骤的模板一键不可用（§15.2.3）：即便本机偏好存的是一键也降级。
    mode: spec.oneClickAllowed ? initialMode : "stepwise",
    planning: false,
    applying: false,
    auto: false,
    notice: null
  }));
  // 异步循环（等任务终态）里读到的 state 必须是最新的：ref 与 setState 同步提交。
  const ref = useRef(run);
  const commit = (next: RunState) => {
    ref.current = next;
    setRun(next);
  };

  const collectParams = (
    stepSpec: StepApplySpec,
    values: Record<string, string>
  ): Record<string, unknown> => {
    const params: Record<string, unknown> = {};
    for (const slot of [...stepSpec.required, ...stepSpec.outputs]) {
      const value = slotParam(slot, values[slot.key]);
      if (value !== undefined) params[slot.key] = value;
    }
    return params;
  };

  /**
   * 自动执行前必填槽与输出槽都要齐：缺一就无法生成计划，只能停下降级
   * （不猜输出位置——输出目录按 §15.2.4「应用时重选」，不自动编造）。
   */
  const missingSlots = (stepSpec: StepApplySpec, params: Record<string, unknown>): boolean => {
    const empty = (value: unknown): boolean => {
      if (value === undefined || value === null) return true;
      if (typeof value === "string") return value.trim().length === 0;
      if (Array.isArray(value)) return value.length === 0;
      return false;
    };
    if (stepSpec.required.some((slot) => empty(params[slot.key]))) return true;
    return stepSpec.outputs.length > 0 && !stepSpec.outputs.some((slot) => !empty(params[slot.key]));
  };

  /** 第 `index` 步的输入取自第 `index-1` 步的实际产物：用那一步计划的 target 预填必填槽。 */
  const initialSlotValues = (index: number, stepRuns: RunStep[]): Record<string, string> => {
    const templateStep = item.steps[index];
    const stepSpec = spec.steps[index];
    if (!templateStep || !stepSpec || templateStep.source !== "previous" || index === 0) {
      return {};
    }
    const targets = (stepRuns[index - 1]?.plan?.entries ?? [])
      .map((entry) => entry.target)
      .filter((target): target is string => Boolean(target));
    if (targets.length === 0) return {};
    const values: Record<string, string> = {};
    for (const slot of stepSpec.required) {
      values[slot.key] = slot.many ? targets.join("\n") : targets[0];
    }
    return values;
  };

  const gateKey = (plan: PlanDocument, stepSpec: StepApplySpec): MessageKey | null => {
    if (stepSpec.highRisk) return GATE_KEY["high-risk"];
    if (plan.stats.blocked > 0) return GATE_KEY.blocked;
    if (plan.stats.conflict > 0) return GATE_KEY.conflict;
    if (plan.stats.warning > 0) return GATE_KEY.warning;
    return null;
  };

  const planCurrent = async () => {
    const state = ref.current;
    if (state.planning || state.current >= item.steps.length) return;
    const stepSpec = spec.steps[state.current];
    if (!stepSpec?.applicable) return;
    commit({ ...state, planning: true, notice: null });
    try {
      const plan = await loadRecipe(
        item.recipeId,
        collectParams(stepSpec, state.slotValues),
        state.current
      );
      const base = ref.current;
      const next = base.steps.map((step) => ({ ...step }));
      if (plan.entries.length === 0) {
        // 无产物记「跳过」并继续，不当作成功（§15.2.3）。
        next[base.current] = { ...next[base.current], status: "skipped", plan };
        const following = base.current + 1;
        const advanced: RunState = {
          ...base,
          steps: next,
          planning: false,
          current: following,
          slotValues: initialSlotValues(following, next),
          notice:
            following >= item.steps.length
              ? t("templates.run.finished")
              : t("templates.run.skipped")
        };
        commit(advanced);
        if (base.mode === "one-click" && following < item.steps.length) {
          void continueOneClick(advanced, following);
        }
        return;
      }
      next[base.current] = { ...next[base.current], status: "planned", plan };
      commit({ ...base, steps: next, planning: false });
    } catch (value) {
      const base = ref.current;
      const next = base.steps.map((step) => ({ ...step }));
      next[base.current] = { ...next[base.current], status: "failed" };
      commit({
        ...base,
        steps: next,
        planning: false,
        notice: t("templates.run.stepFailed", { index: base.current + 1 })
      });
    }
  };

  const executeCurrent = async () => {
    const state = ref.current;
    if (state.applying) return;
    const stepRun = state.steps[state.current];
    if (!stepRun?.plan) return;
    commit({ ...state, applying: true, notice: null });
    try {
      const handle = await applyPlan(stepRun.plan.planId, {
        onConflict: "skip",
        overwriteConfirmed: false
      });
      const base = ref.current;
      const running = base.steps.map((step) => ({ ...step }));
      running[base.current] = { ...running[base.current], status: "running", taskId: handle.taskId };
      commit({ ...base, steps: running });
      // 等这一步真正跑完再决定下一步：某步失败停止后续（不自动跨步回滚）。
      const terminal = await waitTaskTerminal(handle.taskId);
      const settled = ref.current;
      const next = settled.steps.map((step) => ({ ...step }));
      if (terminal !== "completed") {
        next[settled.current] = { ...next[settled.current], status: "failed" };
        commit({
          ...settled,
          steps: next,
          applying: false,
          notice: t("templates.run.stepFailed", { index: settled.current + 1 })
        });
        return;
      }
      next[settled.current] = { ...next[settled.current], status: "done", taskId: handle.taskId };
      const following = settled.current + 1;
      const advanced: RunState = {
        ...settled,
        steps: next,
        applying: false,
        current: following,
        slotValues: initialSlotValues(following, next),
        notice:
          following >= item.steps.length ? t("templates.run.finished") : settled.notice
      };
      commit(advanced);
      if (settled.mode === "one-click" && following < item.steps.length) {
        void continueOneClick(advanced, following);
      }
    } catch (value) {
      const base = ref.current;
      const next = base.steps.map((step) => ({ ...step }));
      next[base.current] = { ...next[base.current], status: "failed" };
      commit({
        ...base,
        steps: next,
        applying: false,
        notice: t("templates.run.stepFailed", { index: base.current + 1 })
      });
    }
  };

  /** 一键模式的后续步骤：先 plan，风险闸门通过后自动 apply；否则停下降级。 */
  const continueOneClick = async (base: RunState, startIndex: number) => {
    const stepRuns = base.steps.map((step) => ({ ...step }));
    commit({ ...base, auto: true });
    let index = startIndex;
    while (index < item.steps.length) {
      const current = ref.current;
      if (!current) return;
      const stepSpec = spec.steps[index];
      if (!stepSpec?.applicable) {
        stepRuns[index] = { ...stepRuns[index], status: "blocked" };
        commit({
          ...current,
          steps: [...stepRuns],
          current: index,
          auto: false,
          notice: t("templates.run.blocked", { index: index + 1 })
        });
        return;
      }
      const values = initialSlotValues(index, stepRuns);
      const params = collectParams(stepSpec, values);
      if (missingSlots(stepSpec, params)) {
        // 取不到输入（`original` 步骤或上一步无产物）：降级为逐步骤，让用户现选。
        commit({
          ...current,
          steps: [...stepRuns],
          current: index,
          auto: false,
          slotValues: values,
          notice: t("templates.run.degraded", { index: index + 1 })
        });
        return;
      }
      stepRuns[index] = { ...stepRuns[index], status: "running" };
      commit({ ...current, steps: [...stepRuns], current: index });
      let plan: PlanDocument;
      try {
        plan = await loadRecipe(item.recipeId, params, index);
      } catch (value) {
        stepRuns[index] = { ...stepRuns[index], status: "failed" };
        commit({
          ...(ref.current ?? current),
          steps: [...stepRuns],
          current: index,
          auto: false,
          notice: t("templates.run.stepFailed", { index: index + 1 })
        });
        return;
      }
      if (plan.entries.length === 0) {
        stepRuns[index] = { ...stepRuns[index], status: "skipped", plan };
        index += 1;
        continue;
      }
      const gate = gateKey(plan, stepSpec);
      if (gate) {
        stepRuns[index] = { ...stepRuns[index], status: "planned", plan };
        commit({
          ...(ref.current ?? current),
          steps: [...stepRuns],
          current: index,
          auto: false,
          notice: t("templates.run.gate", { index: index + 1, reason: t(gate) })
        });
        return;
      }
      try {
        const handle = await applyPlan(plan.planId, {
          onConflict: "skip",
          overwriteConfirmed: false
        });
        const terminal = await waitTaskTerminal(handle.taskId);
        if (terminal !== "completed") {
          stepRuns[index] = {
            ...stepRuns[index],
            status: "failed",
            plan,
            taskId: handle.taskId
          };
          commit({
            ...(ref.current ?? current),
            steps: [...stepRuns],
            current: index,
            auto: false,
            notice: t("templates.run.stepFailed", { index: index + 1 })
          });
          return;
        }
        stepRuns[index] = { ...stepRuns[index], status: "done", plan, taskId: handle.taskId };
      } catch (value) {
        stepRuns[index] = { ...stepRuns[index], status: "failed", plan };
        commit({
          ...(ref.current ?? current),
          steps: [...stepRuns],
          current: index,
          auto: false,
          notice: t("templates.run.stepFailed", { index: index + 1 })
        });
        return;
      }
      index += 1;
    }
    commit({
      ...(ref.current ?? base),
      steps: [...stepRuns],
      current: item.steps.length,
      auto: false,
      notice: t("templates.run.finished")
    });
  };

  const undoStep = async (index: number) => {
    const state = ref.current;
    const stepRun = state.steps[index];
    if (!stepRun?.plan) return;
    try {
      const handle = await undoPlan(stepRun.plan.planId);
      commit({
        ...(ref.current ?? state),
        notice: t("templates.run.undoStarted", { taskId: handle.taskId })
      });
    } catch (value) {
      commit({ ...(ref.current ?? state), notice: errorTitle(t, value) });
    }
  };

  const changeMode = (next: "one-click" | "stepwise") => {
    const state = ref.current;
    if (state.auto || state.applying) return;
    commit({ ...state, mode: next });
    onModeChange(next);
  };

  const finished = run.current >= item.steps.length;
  const currentSpec = spec.steps[run.current];
  const currentStep = run.steps[run.current];
  const modeLabel =
    run.mode === "one-click"
      ? t("templates.run.mode.oneClick")
      : t("templates.run.mode.stepwise");

  return (
    <section className="card" aria-labelledby="template-run-title" data-testid="run-panel">
      <div className="card-header">
        <div>
          <h2 className="card-title" id="template-run-title">
            {t("templates.run.title", { name: item.name })}
          </h2>
          <p className="card-sub">
            {t("templates.run.subtitle", { steps: item.steps.length, mode: modeLabel })}
          </p>
          {run.notice && (
            <p className="hint" role="status" data-testid="run-notice">
              {run.notice}
            </p>
          )}
        </div>
        <div className="button-row">
          <button className="button button-ghost" type="button" onClick={onClose}>
            {t("common.close")}
          </button>
        </div>
      </div>

      <div className="button-row" role="group" aria-label={t("templates.run.modeLabel")}>
        <button
          className="button button-secondary"
          type="button"
          data-testid="run-mode-one-click"
          aria-pressed={run.mode === "one-click"}
          disabled={!spec.oneClickAllowed || run.auto || run.applying}
          onClick={() => changeMode("one-click")}
        >
          {t("templates.run.mode.oneClick")}
        </button>
        <button
          className="button button-secondary"
          type="button"
          data-testid="run-mode-stepwise"
          aria-pressed={run.mode === "stepwise"}
          disabled={run.auto || run.applying}
          onClick={() => changeMode("stepwise")}
        >
          {t("templates.run.mode.stepwise")}
        </button>
      </div>
      {!spec.oneClickAllowed && (
        <p className="hint" data-testid="run-one-click-disabled">
          {t("templates.run.oneClickDisabled")}
        </p>
      )}
      <p className="hint">{t("templates.run.modeHint")}</p>

      <ol className="result-list">
        {item.steps.map((templateStep, index) => {
          const stepSpec = spec.steps[index];
          const runStep = run.steps[index];
          return (
            <li className="result-row" key={templateStep.stepId} data-testid={`run-step-${index}`}>
              <span className="badge">{t("templates.step", { index: index + 1 })}</span>
              <span className="reason">{actionLabel(t, templateStep.kind)}</span>
              <span className="hint">
                {stepSpec?.feature ?? ""}
                {templateStep.source === "previous" ? ` · ${t("templates.stepPrevious")}` : ""}
                {stepSpec?.highRisk ? ` · ${t("templates.run.highRisk")}` : ""}
              </span>
              <span className="badge" data-testid={`run-step-status-${index}`}>
                {t(STATUS_KEY[runStep.status])}
              </span>
              {runStep.status === "done" ? (
                <button
                  className="button button-ghost"
                  type="button"
                  data-testid={`run-undo-${index}`}
                  onClick={() => void undoStep(index)}
                >
                  <RotateCcw size={14} /> {t("templates.run.undo")}
                </button>
              ) : null}
            </li>
          );
        })}
      </ol>

      {run.auto && (
        <p className="hint" data-testid="run-auto">
          <Loader2 size={14} className="spin" /> {t("templates.run.autoRunning")}
        </p>
      )}

      {!finished && !run.auto && currentSpec ? (
        <div className="stack" data-testid="run-current">
          <h3 className="card-title">
            {t("templates.run.currentStep", { index: run.current + 1 })}
            {currentSpec.feature ? ` · ${currentSpec.feature}` : ""}
          </h3>
          {!currentSpec.applicable ? (
            <p className="hint" data-testid="run-step-blocked">
              {currentSpec.reason ?? t("templates.run.blocked", { index: run.current + 1 })}
            </p>
          ) : (
            <>
              {[...currentSpec.required, ...currentSpec.outputs].map((slot, slotIndex) => (
                <label className="field" key={slot.key}>
                  <span>
                    {slot.key}
                    {slot.many ? ` · ${t("templates.apply.manyLabel")}` : ""}
                    {slotIndex >= currentSpec.required.length
                      ? ` · ${t("templates.apply.outputLabel")}`
                      : ""}
                  </span>
                  {slot.many ? (
                    <textarea
                      className="textarea mono"
                      rows={3}
                      spellCheck={false}
                      aria-label={slot.key}
                      data-testid={`run-slot-${slot.key}`}
                      placeholder={t("templates.apply.manyHint")}
                      value={run.slotValues[slot.key] ?? ""}
                      onChange={(event) =>
                        commit({
                          ...ref.current,
                          slotValues: {
                            ...ref.current.slotValues,
                            [slot.key]: event.target.value
                          }
                        })
                      }
                    />
                  ) : (
                    <input
                      className="input"
                      type="text"
                      spellCheck={false}
                      aria-label={slot.key}
                      data-testid={`run-slot-${slot.key}`}
                      value={run.slotValues[slot.key] ?? ""}
                      onChange={(event) =>
                        commit({
                          ...ref.current,
                          slotValues: {
                            ...ref.current.slotValues,
                            [slot.key]: event.target.value
                          }
                        })
                      }
                    />
                  )}
                </label>
              ))}
              <div className="button-row">
                <button
                  className="button button-primary"
                  type="button"
                  data-testid="run-plan"
                  disabled={run.planning || run.applying}
                  onClick={() => void planCurrent()}
                >
                  <Loader2 size={14} className={run.planning ? "spin" : undefined} />
                  {t("templates.apply.submit")}
                </button>
              </div>
            </>
          )}
        </div>
      ) : null}

      {currentStep?.plan && currentStep.status === "planned" && !run.auto ? (
        <div className="stack" data-testid="run-preview">
          <h3 className="card-title">
            {t("templates.run.previewTitle", { index: run.current + 1 })}
          </h3>
          <p className="hint" data-testid="run-preview-stats">
            {t("templates.preview.stats", {
              total: currentStep.plan.stats.total,
              conflict: currentStep.plan.stats.conflict,
              warning: currentStep.plan.stats.warning,
              blocked: currentStep.plan.stats.blocked
            })}
          </p>
          {currentStep.plan.warnings.length > 0 && (
            <ul className="result-list" data-testid="run-preview-warnings">
              {currentStep.plan.warnings.map((warning, index) => (
                <li className="result-row" key={`${warning.kind}-${index}`}>
                  <span className="badge">{t("templates.preview.warning")}</span>
                  <span className="reason">{warning.kind}</span>
                  <span className="hint">{warning.suggestion ?? ""}</span>
                </li>
              ))}
            </ul>
          )}
          {/* 方案 6.2：计划条目可能上千，只挂窗口内的行。 */}
          <VirtualList
            label={t("templates.preview.entries", { total: currentStep.plan.entries.length })}
            items={currentStep.plan.entries}
            getKey={(entry) => entry.entryId}
            renderRow={(entry) => (
              <div className="row">
                <span className="path" title={entry.source}>
                  {entry.source}
                </span>
                <span className="path" title={entry.target ?? entry.reason}>
                  {entry.target ?? entry.reason}
                  {entry.blocked ? ` · ${entry.blocked.code}` : ""}
                </span>
              </div>
            )}
          />
          <div className="button-row">
            <button
              className="button button-primary"
              type="button"
              data-testid="run-execute"
              disabled={run.applying}
              onClick={() => void executeCurrent()}
            >
              <Play size={14} /> {t("templates.preview.execute")}
            </button>
          </div>
        </div>
      ) : null}
    </section>
  );
}
