/**
 * 预检项类别 → 界面文案 key 的唯一映射。类别清单以设计 01 的 `PreflightItem.kind`
  * 枚举为对照源（追加式枚举，冻结后只增不改），`scripts/check_locales.py` 做
 * 表 ↔ 本映射 ↔ 两语言包逐类对齐：漏接或另造 key 都直接红。
 *
 * 只映射**类别标题**；`PreflightItem.message` 是内核产出的技术明细（常含路径与
 * 字节数），按 OQ-38 裁决的口径**不进界面**——界面只显示「级别＋类别＋错误码（＋路径）」，
 * 错误码的本地化摘要/行动指引走既有设计 06 §2 码位文案。未登记的类别回退原样
 * （同错误码的未知码位回退口径；门禁会先一步拦住漏接）。
 */
import type { MessageKey } from "./i18n";

export const PREFLIGHT_KIND_KEYS: Record<string, MessageKey> = {
  "disk-space": "c09.preflight.kind.diskSpace",
  "path": "c09.preflight.kind.path",
  "protected": "c09.preflight.kind.protected",
  "readonly": "c09.preflight.kind.readonly",
  "locked": "c09.preflight.kind.locked",
  "placeholder": "c09.preflight.kind.placeholder",
  "long-path": "c09.preflight.kind.longPath",
  "unc": "c09.preflight.kind.unc"
};

export function preflightKindTitle(
  t: (key: MessageKey, params?: Record<string, string | number>) => string,
  kind: string
): string {
  const key = PREFLIGHT_KIND_KEYS[kind];
  return key ? t(key) : kind;
}