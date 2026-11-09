import { useEffect, useRef, useState } from "react";
import { Loader2, Send, Sparkles, Wrench, X } from "lucide-react";
import {
  aiAsk,
  aiCancel,
  getAiStatus,
  listenAiBundle,
  listenAiDelta,
  listenAiDone,
  listenAiError,
  listenAiTool,
  toIpcError
} from "./lib/ipc";
import type {
  AiContext,
  AiStatus,
  AiToolCall,
  IpcError,
  PlanBundle,
  PlanDocument
} from "./lib/ipc/types";
import { ErrorBanner } from "./ErrorBanner";
import { useI18n } from "./i18n";

export interface AiPanelProps {
  open: boolean;
  onClose: () => void;
  /** 计划束的唯一出口：交给既有预览面板；AI 面板不提供执行按钮。 */
  onOpenPlan: (plan: PlanDocument) => void;
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
export function AiPanel({ open, onClose, onOpenPlan, context }: AiPanelProps) {
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
    logRef.current?.scrollTo?.({ top: logRef.current.scrollHeight });
  }, [messages]);

  if (!open) return null;

  const unavailable = status !== null && !status.configured;
  const canSend = !busy && !unavailable && draft.trim().length > 0;

  const sendNow = (text: string, withNames: boolean) => {
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
              {message.bundle && message.bundle.steps[0] && (
                <div className="ai-bundle" data-testid="ai-bundle">
                  <div className="ai-bundle-title">{t("ai.bundle.title")}</div>
                  <div className="ai-bundle-meta">
                    {t("ai.bundle.meta", {
                      steps: message.bundle.steps.length,
                      entries: message.bundle.steps[0].plan.entries.length
                    })}
                  </div>
                  <button
                    className="button button-secondary"
                    type="button"
                    data-testid="ai-open-preview"
                    onClick={() => onOpenPlan(message.bundle!.steps[0].plan)}
                  >
                    {t("ai.bundle.openPreview")}
                  </button>
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
