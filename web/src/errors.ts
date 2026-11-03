import type { MessageKey } from "./i18n";
import type { IpcError } from "./lib/ipc/types";

/**
 * 错误码 → 用户文案 key 的**唯一映射**（设计 06 §3、设计 09 §5）。
 *
 * 规则（06 §3）：
 * - `summary` 一句人话；`action` 建议动作；两者都进界面。
 * - `detail` 只进诊断日志，**不进界面**——所以这里没有 detail 的 key，
 *   界面上也不渲染 `IpcError.message`／`detail` 原文（它们可能带路径与内部细节）。
 *
 * 收口的码位＝当前界面真会遇到的码（壳层 11 个＋计划/执行条目里的内核码 10 个）。
 * 06 §2 表里的其余码位（E1006、E2001 之外的参数类、E4xxx、E5xxx 其余、E6xxx）
 * 尚未接入本表——它们若冒到界面，会走「未知码位」分支显示后端原文而不是空白。
 */
export const ERROR_TEXT_KEYS = {
  E1001: { summary: "error.E1001.summary", action: "error.E1001.action" },
  E1002: { summary: "error.E1002.summary", action: "error.E1002.action" },
  E1003: { summary: "error.E1003.summary", action: "error.E1003.action" },
  E1004: { summary: "error.E1004.summary", action: "error.E1004.action" },
  E1005: { summary: "error.E1005.summary", action: "error.E1005.action" },
  E1007: { summary: "error.E1007.summary", action: "error.E1007.action" },
  E2001: { summary: "error.E2001.summary", action: "error.E2001.action" },
  E2002: { summary: "error.E2002.summary", action: "error.E2002.action" },
  E2004: { summary: "error.E2004.summary", action: "error.E2004.action" },
  E2005: { summary: "error.E2005.summary", action: "error.E2005.action" },
  E2006: { summary: "error.E2006.summary", action: "error.E2006.action" },
  E2009: { summary: "error.E2009.summary", action: "error.E2009.action" },
  E2010: { summary: "error.E2010.summary", action: "error.E2010.action" },
  E3001: { summary: "error.E3001.summary", action: "error.E3001.action" },
  E3002: { summary: "error.E3002.summary", action: "error.E3002.action" },
  E3003: { summary: "error.E3003.summary", action: "error.E3003.action" },
  E3004: { summary: "error.E3004.summary", action: "error.E3004.action" },
  E3007: { summary: "error.E3007.summary", action: "error.E3007.action" },
  E5001: { summary: "error.E5001.summary", action: "error.E5001.action" },
  E5009: { summary: "error.E5009.summary", action: "error.E5009.action" },
  E9001: { summary: "error.E9001.summary", action: "error.E9001.action" }
} as const satisfies Record<string, { summary: MessageKey; action: MessageKey }>;

export type ErrorCode = keyof typeof ERROR_TEXT_KEYS;

/** 资源里已有本地化文案的码位（供门禁脚本与测试共用）。 */
export const ERROR_CODES: readonly ErrorCode[] = Object.keys(ERROR_TEXT_KEYS) as ErrorCode[];

export function isKnownErrorCode(code: string): code is ErrorCode {
  return Object.prototype.hasOwnProperty.call(ERROR_TEXT_KEYS, code);
}

type Translator = (key: MessageKey, params?: Record<string, string | number>) => string;

export interface ErrorText {
  /** 界面上的主行：一句人话。 */
  title: string;
  /** 界面上的次行：建议动作；未收录的码位没有这一行。 */
  action?: string;
  /** 该码位是否有本地化文案（false 时 title 是后端原文的兜底）。 */
  localized: boolean;
}

/**
 * 把 `IpcError` 翻成界面文案。
 *
 * - 收录的码位：`error.<code>.summary`／`.action`，**不渲染** `message`／`detail`
 *   （06 §3：技术细节只进日志）。
 * - 未收录的码位：退回 `message` 原文并标 `localized: false`——宁可显示一句英文，
 *   也不要空白。界面只在 DEV 下把这种情况打日志，生产不弹技术文本。
 */
export function describeError(t: Translator, error: Pick<IpcError, "code" | "message">): ErrorText {
  if (!isKnownErrorCode(error.code)) {
    return { title: error.message, localized: false };
  }
  const keys = ERROR_TEXT_KEYS[error.code];
  return { title: t(keys.summary), action: t(keys.action), localized: true };
}
