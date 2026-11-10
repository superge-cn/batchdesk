import { useEffect, useRef, useState } from "react";
import { Loader2, Layers, Send, Sparkles, Wrench, X } from "lucide-react";
import {
  aiAsk,
  aiCancel,
  getAiStatus,
  getTask,
  isTauri,
  listenAiBundle,
  listenAiDelta,
  listenAiDone,
  listenAiError,
  listenAiTool,
  listenTaskDone,
  listenTaskState,
  setAiApiKey,
  toIpcError
} from "./lib/ipc";
import type {
  AiContext,
  AiStatus,
  AiToolCall,
  IpcError,
  PlanBundle,
  PlanDocument,
  TaskState
} from "./lib/ipc/types";
import { ErrorBanner } from "./ErrorBanner";
import { useI18n } from "./i18n";

export interface AiPanelProps {
  open: boolean;
  onClose: () => void;
  /** 计划束的唯一出口：交给既有预览面板；AI 面板不提供执行按钮。 */
  onOpenPlan: (plan: PlanDocument) => void;
  /** 配方套用是本地 UI 流程：跳到既有模板库选配方/填路径，配方名与路径不出端。 */
  onOpenTemplates: () => void;
  context: AiContext;
}

interface AiToolView {
  call: AiToolCall;
  allowed: boolean;
}

interface AiMessage {
  messageId: string;
  role: "user" | "assistant";
  text: string;
  tools: AiToolView[];
  bundle?: PlanBundle;
  error?: IpcError;
  done?: boolean;
}

const emptyAssistant = (messageId: string): AiMessage => ({
  messageId,
  role: "assistant",
  text: "",
  tools: []
});

/**
 * F15 Phase 1 前端面板（设计 14 §2/§3/§7/§9）。
 *
 * 安全底线：本组件**不调用** `applyPlan`，唯一的执行入口是「去预览」——把
 * `PlanBundle` 的计划交给既有预览面板，执行仍由用户确认。事件按 messageId 归档，
 * 容忍事件早于 `aiAsk` 返回、迟到与重复；所有错误走 E6xxx 本地化文案。
 */
export function AiPanel({ open, onClose, onOpenPlan, onOpenTemplates, context }: AiPanelProps) {
  const { t } = useI18n();
  const [status, setStatus] = useState<AiStatus | null>(null);
  const [statusError, setStatusError] = useState<IpcError | null>(null);
  const [messages, setMessages] = useState<AiMessage[]>([]);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [consentOpen, setConsentOpen] = useState(false);
  const [pendingText, setPendingText] = useState("");
  const [includeNames, setIncludeNames] = useState(false);
  const [consentGiven, setConsentGiven] = useState(false);
  const [apiKey, setApiKey] = useState("");
  const [savingKey, setSavingKey] = useState(false);
  const [keyNotice, setKeyNotice] = useState<string | null>(null);
  const [keyError, setKeyError] = useState<IpcError | null>(null);
  /** 计划束各步骤的执行终态：planId → task state（只跟踪 apply 任务）。 */
  const [planStates, setPlanStates] = useState<Record<string, TaskState>>({});
  const taskPlanIds = useRef(new Map<string, string>());
  const taskFetching = useRef(new Set<string>());
  const logRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    getAiStatus()
      .then((next) => {
        setStatus(next);
        setStatusError(null);
      })
      .catch((value) => setStatusError(toIpcError(value)));
  }, [open]);

  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    const track = (subscription: Promise<() => void>) => {
      subscription
        .then((unlisten) => unlisteners.push(unlisten))
        .catch(() => undefined);
    };
    const patch = (messageId: string, update: Partial<AiMessage>) =>
      setMessages((previous) => {
        const index = previous.findIndex((item) => item.messageId === messageId);
        if (index < 0) {
          return [...previous, { ...emptyAssistant(messageId), ...update }];
        }
        const next = [...previous];
        next[index] = { ...next[index], ...update };
        return next;
      });
    track(
      listenAiDelta((event) =>
        setMessages((previous) => {
          const index = previous.findIndex((item) => item.messageId === event.messageId);
          if (index < 0) {
            return [
              ...previous,
              { ...emptyAssistant(event.messageId), text: event.text }
            ];
          }
          const next = [...previous];
          next[index] = {
            ...next[index],
            text: next[index].text + event.text
          };
          return next;
        })
      )
    );
    track(
      listenAiTool((event) =>
        setMessages((previous) => {
          const index = previous.findIndex((item) => item.messageId === event.messageId);
          const tool = { call: event.call, allowed: event.allowed };
          if (index < 0) {
            return [
              ...previous,
              { ...emptyAssistant(event.messageId), tools: [tool] }
            ];
          }
          const next = [...previous];
          next[index] = { ...next[index], tools: [...next[index].tools, tool] };
          return next;
        })
      )
    );
    track(
      listenAiBundle((event) => patch(event.messageId, { bundle: event.bundle }))
    );
    track(
      listenAiDone((event) => {
        setMessages((previous) => {
          const index = previous.findIndex((item) => item.messageId === event.messageId);
          const current =
            index >= 0 ? previous[index] : emptyAssistant(event.messageId);
          const suffix = event.message ?? "";
          // 收尾文案只在尚未随流式输出出现过时追加：Provider 可能把同一段文本
          // 既走 delta 又放进 done（src-tauri 的 mock provider 就是这样）。
          const text =
            suffix.length > 0 && !current.text.includes(suffix)
              ? current.text + suffix
              : current.text || t("ai.done");
          const updated: AiMessage = {
            ...current,
            text,
            bundle: event.bundle ?? current.bundle,
            done: true
          };
          if (index < 0) return [...previous, updated];
          const next = [...previous];
          next[index] = updated;
          return next;
        });
        setBusy(false);
        setActiveId(null);
      })
    );
    track(
      listenAiError((event) => {
        patch(event.messageId, { error: event.error, done: true });
        setBusy(false);
        setActiveId(null);
      })
    );
    return () => unlisteners.forEach((unlisten) => unlisten());
    // 订阅只建一次：t() 的变化不需要重建事件通道。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    let alive = true;
    const record = (taskId: string, state?: TaskState) => {
      const known = taskPlanIds.current.get(taskId);
      if (known) {
        if (state) {
          setPlanStates((previous) => ({ ...previous, [known]: state }));
          return;
        }
        // done 事件不带终态：再读一次任务，避免最后一条 state 早于 done 丢失。
        void getTask(taskId)
          .then((detail) => {
            if (!alive || detail.summary.job !== "apply" || !detail.summary.planId) return;
            setPlanStates((previous) => ({
              ...previous,
              [detail.summary.planId as string]: detail.summary.state
            }));
          })
          .catch(() => undefined);
        return;
      }
      if (taskFetching.current.has(taskId)) return;
      taskFetching.current.add(taskId);
      void getTask(taskId)
        .then((detail) => {
          taskFetching.current.delete(taskId);
          if (!alive || detail.summary.job !== "apply") return;
          const planId = detail.summary.planId;
          if (!planId) return;
          taskPlanIds.current.set(taskId, planId);
          setPlanStates((previous) => ({
            ...previous,
            [planId]: detail.summary.state
          }));
        })
        .catch(() => {
          taskFetching.current.delete(taskId);
        });
    };
    const stateSubscription = listenTaskState((event) => record(event.taskId, event.state));
    const doneSubscription = listenTaskDone((event) => record(event.taskId));
    return () => {
      alive = false;
      void stateSubscription.then((unlisten) => unlisten());
      void doneSubscription.then((unlisten) => unlisten());
    };
  }, []);

  useEffect(() => {
    logRef.current?.scrollTo?.({ top: logRef.current.scrollHeight });
  }, [messages]);

  if (!open) return null;

  const unavailable = status !== null && !status.configured;
  const canSend = !busy && !unavailable && draft.trim().length > 0;

  const sendNow = (text: string, withNames: boolean) => {
    const history = messages
      .filter((message) => message.role === "user")
      .map((message) => message.text)
      .filter((item) => item.length > 0);
    setDraft("");
    const userMessage: AiMessage = {
      messageId: `user-${Date.now()}`,
      role: "user",
      text,
      tools: [],
      done: true
    };
    setMessages((previous) => [...previous, userMessage]);
    setBusy(true);
    setConsentOpen(false);
    void aiAsk({
      text,
      history,
      context: {
        ...context,
        includeFileNames: withNames,
        ...(withNames ? { fileNames: context.fileNames } : { fileNames: undefined })
      },
      consentToken: "ui-consent"
    })
      .then((outcome) => {
        setActiveId(outcome.messageId);
        setMessages((previous) => {
          if (previous.some((item) => item.messageId === outcome.messageId)) {
            return previous;
          }
          return [...previous, emptyAssistant(outcome.messageId)];
        });
      })
      .catch((value) => {
        setStatusError(toIpcError(value));
        setBusy(false);
        setActiveId(null);
      });
  };

  const send = () => {
    const text = draft.trim();
    if (!text || !canSend) return;
    if (status?.consentRequired && !consentGiven) {
      // 首次发送先过同意弹窗；输入保留到真正发出为止（取消后可直接改）。
      setPendingText(text);
      setIncludeNames(false);
      setConsentOpen(true);
      return;
    }
    sendNow(text, includeNames);
  };

  const confirmConsent = () => {
    setConsentGiven(true);
    sendNow(pendingText, includeNames);
  };

  const cancelActive = () => {
    if (!activeId) return;
    void aiCancel(activeId).catch(() => undefined);
  };

  const dismissError = (messageId: string) =>
    setMessages((previous) =>
      previous.map((item) =>
        item.messageId === messageId ? { ...item, error: undefined } : item
      )
    );

  const saveApiKey = () => {
    const value = apiKey.trim();
    if (!value || savingKey) return;
    setSavingKey(true);
    setKeyError(null);
    setKeyNotice(null);
    void setAiApiKey(value)
      .then(() => {
        setApiKey("");
        setKeyNotice(t("ai.config.saved"));
        return getAiStatus().then((next) => setStatus(next));
      })
      .catch((error) => setKeyError(toIpcError(error)))
      .finally(() => setSavingKey(false));
  };

  return (
    <aside
      className="ai-panel"
      data-testid="ai-panel"
      role="complementary"
      aria-label={t("ai.title")}
    >
      <div className="ai-panel-header">
        <div className="ai-panel-title">
          <Sparkles size={16} />
          <span>{t("ai.title")}</span>
        </div>
        <button
          className="button button-ghost"
          type="button"
          onClick={onClose}
          aria-label={t("ai.close")}
          data-testid="ai-close"
        >
          <X size={14} />
        </button>
      </div>

      <div className="ai-status" data-testid="ai-status">
        {statusError ? (
          <span>{t("ai.status.error")}</span>
        ) : status === null ? (
          <span>{t("ai.status.loading")}</span>
        ) : status.configured ? (
          <span>{t("ai.status.ready", { provider: status.provider ?? "", model: status.model ?? "" })}</span>
        ) : (
          <span>{t("ai.status.unavailable")}</span>
        )}
      </div>

      {statusError && (
        <div data-testid="ai-error">
          <ErrorBanner error={statusError} onClose={() => setStatusError(null)} />
        </div>
      )}

      {status && !status.configured && isTauri() && (
        <div className="ai-config" data-testid="ai-config">
          <div className="ai-config-title">{t("ai.config.title")}</div>
          <p className="muted">{t("ai.config.hint")}</p>
          <input
            className="input"
            type="password"
            autoComplete="off"
            value={apiKey}
            placeholder={t("ai.config.placeholder")}
            aria-label={t("ai.config.title")}
            onChange={(event) => setApiKey(event.target.value)}
            data-testid="ai-config-key"
          />
          <div className="button-row">
            <button
              className="button button-secondary"
              type="button"
              data-testid="ai-config-save"
              disabled={savingKey || apiKey.trim().length === 0}
              onClick={saveApiKey}
            >
              {savingKey ? t("ai.config.saving") : t("ai.config.save")}
            </button>
          </div>
          {keyNotice && (
            <div className="muted" role="status">
              {keyNotice}
            </div>
          )}
          {keyError && <ErrorBanner error={keyError} onClose={() => setKeyError(null)} />}
        </div>
      )}

      <div className="ai-shortcuts">
        <button
          className="button button-secondary"
          type="button"
          data-testid="ai-open-templates"
          onClick={onOpenTemplates}
        >
          <Layers size={14} /> {t("ai.templates.open")}
        </button>
        <div className="muted">{t("ai.templates.hint")}</div>
      </div>

      <div className="ai-log" role="log" aria-live="polite" data-testid="ai-log" ref={logRef}>
        {messages.length === 0 ? (
          <div className="empty">{t("ai.empty")}</div>
        ) : (
          messages.map((message) => (
            <div
              key={message.messageId}
              className={`ai-message ${message.role}${message.done ? " done" : ""}`}
            >
              <div className="ai-message-role">
                {message.role === "user" ? t("ai.role.user") : t("ai.role.assistant")}
              </div>
              {message.text && <div className="ai-message-text">{message.text}</div>}
              {message.tools.map((tool, index) => (
                <div
                  className={`ai-tool-card${tool.allowed ? "" : " danger"}`}
                  data-testid="ai-tool-card"
                  key={`${tool.call.callId}-${index}`}
                >
                  <Wrench size={13} />
                  <span>{tool.allowed ? t("ai.tool.allowed") : t("ai.tool.forbidden")}</span>
                  <code>{tool.call.name}</code>
                </div>
              ))}
              {message.bundle && message.bundle.steps.length > 0 && (
                <div className="ai-bundle" data-testid="ai-bundle">
                  <div className="ai-bundle-title">{t("ai.bundle.title")}</div>
                  <div className="ai-bundle-meta">
                    {t("ai.bundle.meta", {
                      steps: message.bundle.steps.length,
                      entries: message.bundle.steps.reduce(
                        (total, step) => total + step.plan.entries.length,
                        0
                      )
                    })}
                  </div>
                  {message.bundle.steps.length > 1 && (
                    <div className="ai-bundle-progress" data-testid="ai-bundle-progress">
                      {t("ai.bundle.progress", {
                        done: message.bundle.steps.filter(
                          (step) => planStates[step.plan.planId] === "completed"
                        ).length,
                        total: message.bundle.steps.length
                      })}
                    </div>
                  )}
                  {message.bundle.steps.some((step) => {
                    const state = planStates[step.plan.planId];
                    return (
                      state === "failed" || state === "partial-failed" || state === "cancelled"
                    );
                  }) && (
                    <div
                      className="ai-bundle-conflict"
                      data-testid="ai-bundle-stopped"
                      role="alert"
                    >
                      {t("ai.bundle.stopped")}
                    </div>
                  )}
                  {message.bundle.conflicts.map((conflict, index) => (
                    <div
                      className="ai-bundle-conflict"
                      data-testid="ai-bundle-conflict"
                      role="alert"
                      key={`${conflict.kind}-${index}`}
                    >
                      {t("ai.bundle.conflict", { steps: conflict.stepIds.length })}
                      {conflict.paths && conflict.paths.length > 0 && (
                        <div className="muted">{conflict.paths.join(", ")}</div>
                      )}
                    </div>
                  ))}
                  <ol className="ai-bundle-steps">
                    {message.bundle.steps.map((step, index) => {
                      const state = planStates[step.plan.planId];
                      const dependencyStates = step.dependsOn.map((dependencyId) => {
                        const dependency = message.bundle!.steps.find(
                          (item) => item.stepId === dependencyId
                        );
                        return dependency ? planStates[dependency.plan.planId] : undefined;
                      });
                      const failedDependency = dependencyStates.some(
                        (value) =>
                          value === "failed" ||
                          value === "partial-failed" ||
                          value === "cancelled"
                      );
                      const waitingDependency = dependencyStates.some(
                        (value) => value !== "completed"
                      );
                      const disabled =
                        step.status === "skipped" || failedDependency || waitingDependency;
                      const label =
                        step.status === "skipped"
                          ? t("ai.bundle.blocked")
                          : failedDependency
                            ? t("ai.bundle.stoppedStep")
                            : waitingDependency
                              ? t("ai.bundle.waiting")
                              : t("ai.bundle.openPreview");
                      const stateLabel =
                        state === "completed"
                          ? t("ai.bundle.state.completed")
                          : state === "failed" || state === "partial-failed"
                            ? t("ai.bundle.state.failed")
                            : state === "cancelled"
                              ? t("ai.bundle.state.cancelled")
                              : state
                                ? t("ai.bundle.state.running")
                                : null;
                      return (
                        <li className="ai-bundle-step" key={step.stepId}>
                          <div className="ai-bundle-step-head">
                            <span>
                              {t("ai.bundle.step", { index: index + 1, kind: step.plan.kind })}
                            </span>
                            <span className="muted">
                              {t("ai.bundle.entries", { count: step.plan.entries.length })}
                            </span>
                            {stateLabel && (
                              <span
                                className="ai-bundle-step-state"
                                data-testid={`ai-step-state-${step.stepId}`}
                              >
                                {stateLabel}
                              </span>
                            )}
                          </div>
                          {step.dependsOn.length > 0 && (
                            <div className="muted">
                              {t("ai.bundle.dependsOn", { steps: step.dependsOn.join(", ") })}
                            </div>
                          )}
                          <button
                            className="button button-secondary"
                            type="button"
                            data-testid={
                              index === 0 ? "ai-open-preview" : `ai-open-preview-${step.stepId}`
                            }
                            disabled={disabled}
                            onClick={() => onOpenPlan(step.plan)}
                          >
                            {label}
                          </button>
                        </li>
                      );
                    })}
                  </ol>
                </div>
              )}
              {message.error && (
                <div data-testid="ai-error">
                  <ErrorBanner error={message.error} onClose={() => dismissError(message.messageId)} />
                </div>
              )}
            </div>
          ))
        )}
        {busy && (
          <div className="ai-thinking" role="status">
            <Loader2 size={13} className="spin" /> {t("ai.thinking")}
          </div>
        )}
      </div>

      <div className="ai-compose">
        <input
          className="input"
          data-testid="ai-input"
          value={draft}
          placeholder={unavailable ? t("ai.placeholder.unavailable") : t("ai.placeholder")}
          aria-label={t("ai.input.aria")}
          disabled={unavailable}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.shiftKey) {
              event.preventDefault();
              send();
            }
          }}
        />
        <div className="ai-actions">
          {busy && activeId ? (
            <button
              className="button button-secondary"
              type="button"
              data-testid="ai-cancel"
              onClick={cancelActive}
            >
              {t("ai.cancel")}
            </button>
          ) : null}
          <button
            className="button button-primary"
            type="button"
            data-testid="ai-send"
            onClick={send}
            disabled={!canSend}
          >
            <Send size={14} /> {t("ai.send")}
          </button>
        </div>
      </div>

      {consentOpen && (
        <div
          className="ai-consent"
          data-testid="ai-consent"
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="ai-consent-title"
          onKeyDown={(event) => {
            if (event.key === "Escape") setConsentOpen(false);
          }}
        >
          <div className="ai-consent-card">
            <h2 id="ai-consent-title">{t("ai.consent.title")}</h2>
            <p>
              {t("ai.consent.body", {
                count: context.fileCount,
                kinds: context.kinds.length > 0 ? context.kinds.join(", ") : t("ai.consent.none"),
                locale: context.locale
              })}
            </p>
            <p className="muted">{t("ai.consent.notSent")}</p>
            <label className="check">
              <input
                type="checkbox"
                checked={includeNames}
                disabled={!context.fileNames || context.fileNames.length === 0}
                onChange={(event) => setIncludeNames(event.target.checked)}
                data-testid="ai-consent-names"
              />
              {t("ai.consent.includeNames")}
            </label>
            {includeNames && context.fileNames && context.fileNames.length > 0 && (
              <ul className="ai-consent-names">
                {context.fileNames.map((name) => (
                  <li key={name}>{name}</li>
                ))}
              </ul>
            )}
            <div className="button-row">
              <button
                className="button button-secondary"
                type="button"
                autoFocus
                data-testid="ai-consent-cancel"
                onClick={() => setConsentOpen(false)}
              >
                {t("ai.consent.cancel")}
              </button>
              <button
                className="button button-primary"
                type="button"
                data-testid="ai-consent-confirm"
                onClick={confirmConsent}
              >
                {t("ai.consent.confirm")}
              </button>
            </div>
          </div>
        </div>
      )}
    </aside>
  );
}
