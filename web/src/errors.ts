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
 * 收口的码位＝设计 06 §2 码表的**全部**码位（2026-11-04 从 21 个补到 53 个）。
 * 门禁 scripts/check_locales.py 会把本表与 06 §2 的码表逐码对齐：
 * 码表里新增一个码位而这里没接，CI 直接红。未收录的码位仍走「未知码位」分支显示后端原文。
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
  E3008: { summary: "error.E3008.summary", action: "error.E3008.action" },
  E3009: { summary: "error.E3009.summary", action: "error.E3009.action" },
  E3010: { summary: "error.E3010.summary", action: "error.E3010.action" },
  E5001: { summary: "error.E5001.summary", action: "error.E5001.action" },
  E5009: { summary: "error.E5009.summary", action: "error.E5009.action" },
  E9001: { summary: "error.E9001.summary", action: "error.E9001.action" },
  E1006: { summary: "error.E1006.summary", action: "error.E1006.action" },
  E2003: { summary: "error.E2003.summary", action: "error.E2003.action" },
  E2007: { summary: "error.E2007.summary", action: "error.E2007.action" },
  E2008: { summary: "error.E2008.summary", action: "error.E2008.action" },
  E3005: { summary: "error.E3005.summary", action: "error.E3005.action" },
  E3006: { summary: "error.E3006.summary", action: "error.E3006.action" },
  E4001: { summary: "error.E4001.summary", action: "error.E4001.action" },
  E4002: { summary: "error.E4002.summary", action: "error.E4002.action" },
  E4003: { summary: "error.E4003.summary", action: "error.E4003.action" },
  E4004: { summary: "error.E4004.summary", action: "error.E4004.action" },
  E4005: { summary: "error.E4005.summary", action: "error.E4005.action" },
  E5002: { summary: "error.E5002.summary", action: "error.E5002.action" },
  E5003: { summary: "error.E5003.summary", action: "error.E5003.action" },
  E5004: { summary: "error.E5004.summary", action: "error.E5004.action" },
  E5005: { summary: "error.E5005.summary", action: "error.E5005.action" },
  E5006: { summary: "error.E5006.summary", action: "error.E5006.action" },
  E5007: { summary: "error.E5007.summary", action: "error.E5007.action" },
  E5008: { summary: "error.E5008.summary", action: "error.E5008.action" },
  E5010: { summary: "error.E5010.summary", action: "error.E5010.action" },
  E5011: { summary: "error.E5011.summary", action: "error.E5011.action" },
  E5012: { summary: "error.E5012.summary", action: "error.E5012.action" },
  E6001: { summary: "error.E6001.summary", action: "error.E6001.action" },
  E6002: { summary: "error.E6002.summary", action: "error.E6002.action" },
  E6003: { summary: "error.E6003.summary", action: "error.E6003.action" },
  E6004: { summary: "error.E6004.summary", action: "error.E6004.action" },
  E6005: { summary: "error.E6005.summary", action: "error.E6005.action" },
  E6006: { summary: "error.E6006.summary", action: "error.E6006.action" },
  E6007: { summary: "error.E6007.summary", action: "error.E6007.action" },
  E6008: { summary: "error.E6008.summary", action: "error.E6008.action" },
  E6009: { summary: "error.E6009.summary", action: "error.E6009.action" },
  E6010: { summary: "error.E6010.summary", action: "error.E6010.action" },
  E6011: { summary: "error.E6011.summary", action: "error.E6011.action" },
} as const satisfies Record<
  string,
  { summary: MessageKey; action: MessageKey }
>;

export type ErrorCode = keyof typeof ERROR_TEXT_KEYS;

/** 资源里已有本地化文案的码位（供门禁脚本与测试共用）。 */
export const ERROR_CODES: readonly ErrorCode[] = Object.keys(
  ERROR_TEXT_KEYS,
) as ErrorCode[];

export function isKnownErrorCode(code: string): code is ErrorCode {
  return Object.prototype.hasOwnProperty.call(ERROR_TEXT_KEYS, code);
}

type Translator = (
  key: MessageKey,
  params?: Record<string, string | number>,
) => string;

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
export function describeError(
  t: Translator,
  error: Pick<IpcError, "code" | "message">,
): ErrorText {
  if (!isKnownErrorCode(error.code)) {
    return { title: error.message, localized: false };
  }
  const keys = ERROR_TEXT_KEYS[error.code];
  return { title: t(keys.summary), action: t(keys.action), localized: true };
}
