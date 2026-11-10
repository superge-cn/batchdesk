import { useCallback, useEffect, useMemo, useState } from "react";
import { Download, Info, Loader2, Play, Sparkles, Trash2, Upload } from "lucide-react";
import {
  applyPlan,
  deleteRecipe,
  describeRecipe,
  exportRecipe,
  getUiState,
  importRecipe,
  listRecipes,
  loadRecipe,
  setUiState,
  toIpcError
} from "./lib/ipc";
import { ErrorBanner } from "./ErrorBanner";
import { useI18n } from "./i18n";
import { actionLabel } from "./labels";
import { NAV_LABEL_BY_ID } from "./navCatalog";
import { TemplateRunner } from "./TemplateRunner";
import { VirtualList } from "./VirtualList";
import type {
  IpcError,
  PlanDocument,
  RecipeApplySpec,
  RecipeDocument,
  RecipeOrigin,
  RecipeSlotSpec
} from "./lib/ipc/types";

/**
 * 处理模板（C37，方案 §15、设计 12 §5）。
 *
 * **这一版接通「一键套用」**（OQ-40①＋OQ-41②，ADR-0077/0080）：列表、查看步骤、
 * 删除、导入、导出之外，可套用的模板现在能填路径槽位后生成**预览计划**——
 * 只出计划不执行（方案 §15.2.2），核对后由「执行此计划」显式走 applyPlan。
 *
 * *Phase B 多步模板*（方案 §15.2.3）走 [TemplateRunner](./TemplateRunner)：按步
 * plan/apply，首步先预览确认，后续步骤按运行模式（一键/逐步骤，存 UiState）
 * 自动或逐步确认；风险闸门命中即停下降级。
 *
 * 套用面的判定来自壳层 `describeRecipe`（与 `loadRecipe` 同源，两套口径不可能分叉）：
 * - 未绑定/全部步骤都不可套用 → 不给按钮，绑定行说明原因；
 * - 变换正文不在计划参数里的功能（归档规则/清单/过滤/排序/列操作/来源列/列选择）
 *   → 不给按钮并展示原因（OQ-43 登记后续裁决）；
 * - 采样口径随模板存档（v5），没存的由壳层映射表按缺省补。
 *
 * 硬约束不变：模板不含源路径与输出目录（不变量 13），套用时现选位置；
 * 任何应用路径都必须重新预览（方案 §15.2.2）。
 */
export function TemplatesPage({ onBack }: { onBack: () => void }) {
  const { t } = useI18n();
  const [items, setItems] = useState<RecipeDocument[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<IpcError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  /** 每个模板的套用面（describeRecipe）；取不到的（刚被删等）不渲染套用按钮。 */
  const [specs, setSpecs] = useState<Record<string, RecipeApplySpec | null>>({});
  /**
   * 模板运行偏好（设计 03 §11.2）：本机 UI 偏好，存 UiState，不写进 RecipeDocument、
   * 不进导出（§15.3）。缺省逐步骤——安全默认（§7.4），一键需显式开启。
   */
  const [templatePrefs, setTemplatePrefs] = useState<
    Record<string, { runMode?: "stepwise" | "one-click" }>
  >({});
  /** 多步执行器：目标模板＋它的逐段评估。 */
  const [runTarget, setRunTarget] = useState<{ item: RecipeDocument; spec: RecipeApplySpec } | null>(
    null
  );
  /** 删除是两段式：先选中要删的，再确认——避免误触把模板删掉。 */
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);
  const [importText, setImportText] = useState("");
  const [exported, setExported] = useState<{ name: string; payload: string } | null>(null);
  const [copied, setCopied] = useState(false);
  /** 套用对话框：目标模板＋它的槽位评估。 */
  const [applyTarget, setApplyTarget] = useState<{ item: RecipeDocument; spec: RecipeApplySpec } | null>(null);
  /** 槽位输入值（键＝槽位键；单值一行文本，多值一行一个路径）。 */
  const [slotValues, setSlotValues] = useState<Record<string, string>>({});
  const [applying, setApplying] = useState(false);
  /** 套用产出的预览计划（未执行；执行按钮显式走 applyPlan）。 */
  const [applied, setApplied] = useState<{ name: string; plan: PlanDocument } | null>(null);
  const [executing, setExecuting] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const page = await listRecipes(0, 200);
      setItems(page.items);
      setTotal(page.total);
      // 套用面与列表同源取回：判定是纯内存静态评估，逐条一次调用很便宜；
      // 单条失败不拖垮整个列表（该模板只是没有套用按钮）。
      const entries = await Promise.all(
        page.items.map(async (item) => {
          try {
            return [item.recipeId, await describeRecipe(item.recipeId)] as const;
          } catch {
            return [item.recipeId, null] as const;
          }
        })
      );
      setSpecs(Object.fromEntries(entries));
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // 运行偏好只读一次（本机、可重置，设计 03 §11）；写失败不阻塞套用。
  useEffect(() => {
    getUiState()
      .then((state) => setTemplatePrefs(state.templatePrefs ?? {}))
      .catch(() => undefined);
  }, []);

  /**
   * 运行模式缺省（方案 §15.2.3，OQ-45 裁决）：本机新建（`origin: "local"`）默认一键，
   * 导入／来源未知默认逐步骤；本机偏好（UiState `templatePrefs`）显式覆盖来源缺省。
   * 含高风险步骤的模板仍由 `describeRecipe.oneClickAllowed` 禁掉一键。
   */
  const runModeFor = (
    recipeId: string,
    origin?: RecipeOrigin
  ): "one-click" | "stepwise" => {
    const pref = templatePrefs[recipeId]?.runMode;
    if (pref === "one-click" || pref === "stepwise") return pref;
    return origin === "local" ? "one-click" : "stepwise";
  };

  const persistRunMode = (recipeId: string, mode: "one-click" | "stepwise") => {
    const next = { ...templatePrefs, [recipeId]: { runMode: mode } };
    setTemplatePrefs(next);
    setUiState({ templatePrefs: next }).catch(() => undefined);
  };

  const stepSummary = useMemo(
    () => (item: RecipeDocument) =>
      Array.from(new Set(item.steps.map((step) => step.kind))).map((kind) => actionLabel(t, kind)),
    [t]
  );

  /**
   * 该模板能否套用（OQ-36 裁决①落地后的判据）：每一步都要有功能码绑定。
   *
   * 刻意不说"可以套用"——套用通道还卡在 OQ-40；这里只如实回答"绑没绑上"，
   * 因为绑定决定的是**将来**能不能路由，用户现在就该知道自己存的东西缺什么。
   */
  const binding = useMemo(
    () => (item: RecipeDocument) => {
      const features = item.steps.map((step) => step.feature?.trim()).filter(Boolean) as string[];
      if (features.length === 0) {
        return { bound: false as const, label: "" };
      }
      const label = features
        .map((code) => {
          const labelKey = NAV_LABEL_BY_ID[code as keyof typeof NAV_LABEL_BY_ID];
          return labelKey ? `${t(labelKey as never)} (${code})` : code;
        })
        .join(" → ");
      return { bound: true as const, label };
    },
    [t]
  );

  const onExport = async (item: RecipeDocument) => {
    setError(null);
    setNotice(null);
    setCopied(false);
    try {
      const result = await exportRecipe(item.recipeId);
      setExported({ name: item.name, payload: result.payload });
    } catch (value) {
      setError(toIpcError(value));
    }
  };

  const onCopy = async () => {
    if (!exported) return;
    try {
      await navigator.clipboard?.writeText(exported.payload);
      setCopied(true);
    } catch {
      // 剪贴板被拒时保留文本框，用户自己选中复制——不谎报成功。
      setCopied(false);
    }
  };

  const onDelete = async (recipeId: string) => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await deleteRecipe(recipeId);
      setPendingDelete(null);
      setNotice(t("templates.notice.deleted"));
      await refresh();
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setBusy(false);
    }
  };

  const onImport = async () => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const imported = await importRecipe(importText);
      setImportText("");
      setNotice(t("templates.notice.imported", { name: imported.name }));
      await refresh();
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setBusy(false);
    }
  };

  /**
   * 能否套用：单步看 `applicable`；多步看**有没有任何一步**可套用——
   * 不可套用的那几步由执行器如实停住（§15.2.3「某步失败停止后续」），
   * 不因为其中一步不可用就否掉整份模板。
   */
  const canApply = (spec: RecipeApplySpec | null | undefined): boolean =>
    !!spec && (spec.applicable || spec.steps.some((step) => step.applicable));

  const openApply = (item: RecipeDocument, spec: RecipeApplySpec) => {
    setError(null);
    setNotice(null);
    // 多步模板走执行器：按步 plan/apply，不预生成所有步骤（§15.2.3）。
    if (item.steps.length > 1) {
      setRunTarget({ item, spec });
      return;
    }
    setSlotValues({});
    setApplyTarget({ item, spec });
  };

  /** 多值槽一行一个路径；单值槽去首尾空白。空行丢弃，空结果返回 undefined 让壳层报缺槽。 */
  const slotParam = (spec: RecipeSlotSpec, value: string | undefined): string | string[] | undefined => {
    const text = (value ?? "").trim();
    if (!spec.many) {
      return text.length > 0 ? text : undefined;
    }
    const items = text
      .split(/[\n,\uFF0C]+/)
      .map((line) => line.trim())
      .filter(Boolean);
    return items.length > 0 ? items : undefined;
  };

  const onSubmitApply = async () => {
    if (!applyTarget) return;
    const { item, spec } = applyTarget;
    const pathParams: Record<string, unknown> = {};
    for (const slot of [...spec.required, ...spec.outputs]) {
      const value = slotParam(slot, slotValues[slot.key]);
      if (value !== undefined) {
        pathParams[slot.key] = value;
      }
    }
    setApplying(true);
    setError(null);
    setNotice(null);
    try {
      const plan = await loadRecipe(item.recipeId, pathParams);
      setApplied({ name: item.name, plan });
      setApplyTarget(null);
      setNotice(t("templates.notice.applied"));
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setApplying(false);
    }
  };

  const onExecute = async () => {
    if (!applied) return;
    setExecuting(true);
    setError(null);
    setNotice(null);
    try {
      const handle = await applyPlan(applied.plan.planId, {
        onConflict: "skip",
        overwriteConfirmed: false
      });
      setNotice(t("templates.notice.executed", { taskId: handle.taskId }));
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setExecuting(false);
    }
  };

  return (
    <div className="content">
      {error && <ErrorBanner error={error} onClose={() => setError(null)} />}
      {notice && (
        <div className="banner banner-info" role="status">
          <div>{notice}</div>
        </div>
      )}

      <section className="card" aria-labelledby="templates-title">
        <div className="card-header">
          <div>
            <h2 className="card-title" id="templates-title">
              {t("templates.list.title")}
            </h2>
            <p className="card-sub">{t("templates.list.subtitle", { total })}</p>
          </div>
          <div className="button-row">
            <button
              className="button button-secondary"
              type="button"
              onClick={() => void refresh()}
              disabled={loading}
            >
              <Loader2 size={14} className={loading ? "spin" : undefined} /> {t("common.refresh")}
            </button>
            <button className="button button-ghost" type="button" onClick={onBack}>
              {t("common.backC09")}
            </button>
          </div>
        </div>

        <div className="banner banner-info" role="status">
          <Info size={14} />
          <div>
            <strong>{t("templates.applyNotice.title")}</strong> {t("templates.applyNotice.body")}
          </div>
        </div>

        {items.length === 0 ? (
          <div className="empty">{loading ? t("templates.loading") : t("templates.empty")}</div>
        ) : (
          <div className="stack">
            {items.map((item) => (
              <article className="card" key={item.recipeId} aria-label={item.name}>
                <div className="card-header">
                  <div>
                    <h3 className="card-title">{item.name}</h3>
                    {item.description && <p className="card-sub">{item.description}</p>}
                    <p className="hint">
                      {t("templates.meta", {
                        steps: item.steps.length,
                        kinds: stepSummary(item).join(" / "),
                        updated: item.updatedAt ?? item.createdAt
                      })}
                    </p>
                    <p className="hint" data-testid="template-binding">
                      {binding(item).bound
                        ? t("templates.bound", { features: binding(item).label })
                        : t("templates.unbound")}
                    </p>
                    {/* 已绑定但预声明不可套用（正文缺失/旧压缩模板）：如实给原因，不给假按钮。 */}
                    {binding(item).bound &&
                    specs[item.recipeId] &&
                    !canApply(specs[item.recipeId]) ? (
                      <p className="hint" data-testid="template-refused">
                        {t("templates.refusedReason", {
                          reason: specs[item.recipeId]!.reason ?? ""
                        })}
                      </p>
                    ) : null}
                    {item.match?.extensions?.length ? (
                      <p className="hint">
                        {t("templates.match", { extensions: item.match.extensions.join(", ") })}
                      </p>
                    ) : null}
                  </div>
                  <div className="button-row">
                    {canApply(specs[item.recipeId]) ? (
                      <button
                        className="button button-primary"
                        type="button"
                        data-testid="template-apply"
                        onClick={() => openApply(item, specs[item.recipeId]!)}
                      >
                        <Sparkles size={14} /> {t("templates.apply")}
                      </button>
                    ) : null}
                    <button
                      className="button button-secondary"
                      type="button"
                      onClick={() => void onExport(item)}
                    >
                      <Download size={14} /> {t("templates.export")}
                    </button>
                    {pendingDelete === item.recipeId ? (
                      <>
                        <button
                          className="button button-danger"
                          type="button"
                          disabled={busy}
                          onClick={() => void onDelete(item.recipeId)}
                        >
                          <Trash2 size={14} /> {t("templates.delete.confirm")}
                        </button>
                        <button
                          className="button button-ghost"
                          type="button"
                          onClick={() => setPendingDelete(null)}
                        >
                          {t("common.cancel")}
                        </button>
                      </>
                    ) : (
                      <button
                        className="button button-danger"
                        type="button"
                        onClick={() => setPendingDelete(item.recipeId)}
                      >
                        <Trash2 size={14} /> {t("templates.delete")}
                      </button>
                    )}
                  </div>
                </div>

                <ol className="result-list">
                  {item.steps.map((step, index) => (
                    <li className="result-row" key={step.stepId}>
                      <span className="badge">{t("templates.step", { index: index + 1 })}</span>
                      <span className="reason">{actionLabel(t, step.kind)}</span>
                      <span className="hint">
                        {t("templates.stepParams", {
                          count: Object.keys(step.paramsSnapshot).length
                        })}
                        {step.sampling
                          ? ` · ${t("templates.stepSampling", {
                              sampling: Object.entries(step.sampling)
                                .filter(([, value]) => value !== undefined)
                                .map(([key, value]) => `${key}=${value}`)
                                .join(", ")
                            })}`
                          : ""}
                        {step.content
                          ? ` · ${t("templates.stepContent", {
                              count: Object.keys(step.content).length
                            })}`
                          : ""}
                        {step.source === "previous" ? ` · ${t("templates.stepPrevious")}` : ""}
                      </span>
                    </li>
                  ))}
                </ol>
              </article>
            ))}
          </div>
        )}
      </section>

      {applyTarget && (
        <section className="card" aria-labelledby="template-apply-title">
          <div className="card-header">
            <div>
              <h2 className="card-title" id="template-apply-title">
                {t("templates.apply.title", { name: applyTarget.item.name })}
              </h2>
              <p className="card-sub">
                {t("templates.apply.feature", { feature: applyTarget.spec.feature ?? "" })}
              </p>
              <p className="hint">{t("templates.apply.hint")}</p>
            </div>
            <div className="button-row">
              <button
                className="button button-ghost"
                type="button"
                onClick={() => setApplyTarget(null)}
              >
                {t("templates.apply.cancel")}
              </button>
            </div>
          </div>
          {[...applyTarget.spec.required, ...applyTarget.spec.outputs].map((slot, index) => (
            <label className="field" key={slot.key}>
              <span>
                {slot.key}
                {slot.many ? ` · ${t("templates.apply.manyLabel")}` : ""}
                {index >= applyTarget.spec.required.length
                  ? ` · ${t("templates.apply.outputLabel")}`
                  : ""}
              </span>
              {slot.many ? (
                <textarea
                  className="textarea mono"
                  rows={3}
                  spellCheck={false}
                  aria-label={slot.key}
                  data-testid={`apply-slot-${slot.key}`}
                  placeholder={t("templates.apply.manyHint")}
                  value={slotValues[slot.key] ?? ""}
                  onChange={(event) =>
                    setSlotValues((current) => ({
                      ...current,
                      [slot.key]: event.target.value
                    }))
                  }
                />
              ) : (
                <input
                  className="input"
                  type="text"
                  spellCheck={false}
                  aria-label={slot.key}
                  data-testid={`apply-slot-${slot.key}`}
                  value={slotValues[slot.key] ?? ""}
                  onChange={(event) =>
                    setSlotValues((current) => ({
                      ...current,
                      [slot.key]: event.target.value
                    }))
                  }
                />
              )}
            </label>
          ))}
          <div className="button-row">
            <button
              className="button button-primary"
              type="button"
              data-testid="apply-submit"
              disabled={applying}
              onClick={() => void onSubmitApply()}
            >
              <Loader2 size={14} className={applying ? "spin" : undefined} />
              {t("templates.apply.submit")}
            </button>
          </div>
        </section>
      )}

      {applied && (
        <section className="card" aria-labelledby="template-preview-title">
          <div className="card-header">
            <div>
              <h2 className="card-title" id="template-preview-title">
                {t("templates.preview.title", { name: applied.name })}
              </h2>
              <p className="card-sub">{t("templates.preview.hint")}</p>
              <p className="hint" data-testid="apply-preview-stats">
                {t("templates.preview.stats", {
                  total: applied.plan.stats.total,
                  conflict: applied.plan.stats.conflict,
                  warning: applied.plan.stats.warning,
                  blocked: applied.plan.stats.blocked
                })}
              </p>
            </div>
            <div className="button-row">
              <button
                className="button button-primary"
                type="button"
                data-testid="apply-execute"
                disabled={executing || applied.plan.entries.length === 0}
                onClick={() => void onExecute()}
              >
                <Play size={14} /> {t("templates.preview.execute")}
              </button>
            </div>
          </div>
          {applied.plan.warnings.length > 0 && (
            <ul className="result-list" data-testid="apply-preview-warnings">
              {applied.plan.warnings.map((warning, index) => (
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
            label={t("templates.preview.entries", { total: applied.plan.entries.length })}
            items={applied.plan.entries}
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
        </section>
      )}

      {runTarget && (
        <TemplateRunner
          item={runTarget.item}
          spec={runTarget.spec}
          initialMode={runModeFor(runTarget.item.recipeId, runTarget.item.origin)}
          onModeChange={(mode) => persistRunMode(runTarget.item.recipeId, mode)}
          onClose={() => setRunTarget(null)}
        />
      )}

      {exported && (
        <section className="card" aria-labelledby="template-export-title">
          <div className="card-header">
            <div>
              <h2 className="card-title" id="template-export-title">
                {t("templates.export.title", { name: exported.name })}
              </h2>
              <p className="card-sub">{t("templates.export.hint")}</p>
            </div>
            <div className="button-row">
              <button className="button button-secondary" type="button" onClick={() => void onCopy()}>
                {copied ? t("templates.export.copied") : t("templates.export.copy")}
              </button>
              <button
                className="button button-ghost"
                type="button"
                onClick={() => setExported(null)}
              >
                {t("common.close")}
              </button>
            </div>
          </div>
          <textarea
            className="textarea mono"
            rows={10}
            readOnly
            aria-label={t("templates.export.title", { name: exported.name })}
            value={exported.payload}
          />
        </section>
      )}

      <section className="card" aria-labelledby="template-import-title">
        <div className="card-header">
          <div>
            <h2 className="card-title" id="template-import-title">
              {t("templates.import.title")}
            </h2>
            <p className="card-sub">{t("templates.import.hint")}</p>
          </div>
        </div>
        <label className="field">
          <span>{t("templates.import.label")}</span>
          <textarea
            className="textarea mono"
            rows={6}
            spellCheck={false}
            placeholder={t("templates.import.placeholder")}
            value={importText}
            onChange={(event) => setImportText(event.target.value)}
          />
        </label>
        <div className="button-row">
          <button
            className="button button-primary"
            type="button"
            disabled={busy || importText.trim().length === 0}
            onClick={() => void onImport()}
          >
            <Upload size={14} /> {t("templates.import.submit")}
          </button>
        </div>
      </section>
    </div>
  );
}
