/**
 * 预警类别 → 界面标题 key 的唯一映射。类别清单以设计 06 §2A 为对照源，
 * `scripts/check_locales.py` 第 ⑪ 项做表 ↔ 本映射 ↔ 两语言包逐类对齐：
 * 漏接或另造 key 都直接红。
 *
 * 只映射**类别标题**；`PlanWarning.suggestion` 是内核产出的明细文本，
 * 是否继续原样显示属设计 06 §5 待决（与 OQ-38 同族），这里不替它措辞。
 */
import type { MessageKey } from "./i18n";

export const WARNING_KIND_KEYS: Record<string, MessageKey> = {
  "binary-skipped": "warning.binarySkipped.title",
  "column-drift": "warning.columnDrift.title",
  "count-exceeds-rows": "warning.countExceedsRows.title",
  "csv-row-width-mismatch": "warning.csvRowWidthMismatch.title",
  "csv-text-mode": "warning.csvTextMode.title",
  "delimiter-not-detected": "warning.delimiterNotDetected.title",
  "duplicate-column-name": "warning.duplicateColumnName.title",
  "duplicate-column-names": "warning.duplicateColumnNames.title",
  "duplicate-rows": "warning.duplicateRows.title",
  "duplicates-removed": "warning.duplicatesRemoved.title",
  "empty-directory": "warning.emptyDirectory.title",
  "empty-file": "warning.emptyFile.title",
  "empty-list": "warning.emptyList.title",
  "empty-replacement": "warning.emptyReplacement.title",
  "encrypted-archive": "warning.encryptedArchive.title",
  "existing-target": "warning.existingTarget.title",
  "existing-target-skipped": "warning.existingTargetSkipped.title",
  "external-sort": "warning.externalSort.title",
  "gbk-filename-fallback": "warning.gbkFilenameFallback.title",
  "hardlink-skipped": "warning.hardlinkSkipped.title",
  "header-normalized": "warning.headerNormalized.title",
  "header-only": "warning.headerOnly.title",
  "high-hit-rule": "warning.highHitRule.title",
  "incomplete-hash": "warning.incompleteHash.title",
  "invalid-rows": "warning.invalidRows.title",
  "invalid-target-names": "warning.invalidTargetNames.title",
  "locked": "warning.locked.title",
  "long-path-caution": "warning.longPathCaution.title",
  "long-path-disabled": "warning.longPathDisabled.title",
  "manifest-missing": "warning.manifestMissing.title",
  "mixed-delimiters": "warning.mixedDelimiters.title",
  "mixed-line-endings": "warning.mixedLineEndings.title",
  "mixed-source-encodings": "warning.mixedSourceEncodings.title",
  "needs-manual-review": "warning.needsManualReview.title",
  "no-change": "warning.noChange.title",
  "no-duplicates": "warning.noDuplicates.title",
  "no-header-positional": "warning.noHeaderPositional.title",
  "no-line-endings": "warning.noLineEndings.title",
  "no-matching-rows": "warning.noMatchingRows.title",
  "no-newline-hard-cut": "warning.noNewlineHardCut.title",
  "no-safe-entries": "warning.noSafeEntries.title",
  "non-numeric-cells": "warning.nonNumericCells.title",
  "overlong-line-single-part": "warning.overlongLineSinglePart.title",
  "parent-is-file": "warning.parentIsFile.title",
  "parts-exceed-lines": "warning.partsExceedLines.title",
  "password-unused": "warning.passwordUnused.title",
  "placeholders-skipped": "warning.placeholdersSkipped.title",
  "protected-target-skipped": "warning.protectedTargetSkipped.title",
  "range-exceeds-rows": "warning.rangeExceedsRows.title",
  "readonly": "warning.readonly.title",
  "rule-hits": "warning.ruleHits.title",
  "scan-skipped": "warning.scanSkipped.title",
  "schema-extra-columns": "warning.schemaExtraColumns.title",
  "schema-missing-columns": "warning.schemaMissingColumns.title",
  "split-parts-estimate": "warning.splitPartsEstimate.title",
  "target-exists-skipped": "warning.targetExistsSkipped.title",
  "two-phase-rename": "warning.twoPhaseRename.title",
  "unclosed-quote": "warning.unclosedQuote.title",
  "unmatched-kept": "warning.unmatchedKept.title",
  "unreadable-or-symlink-skipped": "warning.unreadableOrSymlinkSkipped.title",
  "unsafe-entry-skipped": "warning.unsafeEntrySkipped.title",
  "walk-missing": "warning.walkMissing.title",
  "zero-byte-ignored": "warning.zeroByteIgnored.title",
};

/**
 * 类别标题。映射查不到说明界面没跟上契约类别（设计 06 §2A 追加了类别而这里没补）——
 * 这种情况把 slug 摆在「未登记的预警类别」的措辞里，让它看上去像个缺陷，
 * 而不是伪装成一条正常文案。
 */
export function warningKindTitle(
  t: (key: MessageKey, params?: Record<string, string | number>) => string,
  kind: string,
): string {
  const key = WARNING_KIND_KEYS[kind];
  return key ? t(key) : t("common.warningUnregistered", { kind });
}
