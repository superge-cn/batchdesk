import { useCallback, useEffect, useMemo, useState } from "react";
import { Download, Info, Loader2, Trash2, Upload } from "lucide-react";
import { deleteRecipe, exportRecipe, importRecipe, listRecipes, toIpcError } from "./lib/ipc";
import { ErrorBanner } from "./ErrorBanner";
import { useI18n } from "./i18n";
import { actionLabel } from "./labels";
import { NAV_LABEL_BY_ID } from "./navCatalog";
import type { IpcError, RecipeDocument } from "./lib/ipc/types";

/**
 * 处理模板（C37，方案 §15、设计 12 §5）。
 *
 * **这一版做到"绑定已记录、套用还没通道"**：列表、查看步骤、删除、导入、导出、
 * 「存为处理模板」都在；OQ-36 已裁（取①），所以模板现在**记着功能码**——
 * 每个步骤卡上会写明它绑到哪个功能，一对多且没绑定的直接说"未绑定"。
 *
 * 剩下的不是功能码问题，而是**输入通道**：方案 §15.2.5 说套用＝回填参数后调
 * `planOperation`，而设计 03/01 的 `loadRecipe` 只收 `recipeId`——模板按不变量 13
 * 不存路径，只给 id 就产不出计划。这条冲突登记在
 * [OQ-40](../../docs/OPEN-QUESTIONS.md) 等人类裁决（AGENTS「何时停下来问人」：命令面变更走人），
 * 裁决前这里不放一个点了没反应的按钮。
 *
 * 另两条硬约束写在交互里：
 * - 模板**不含**源路径与输出目录（不变量 13），所以"套用"时本来就要重选位置；
 * - 任何应用路径都必须重新预览（方案 §15.2.2），本页不提供"直接执行"。
 */
export function TemplatesPage({ onBack }: { onBack: () => void }) {
  const { t } = useI18n();
  const [items, setItems] = useState<RecipeDocument[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<IpcError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  /** 删除是两段式：先选中要删的，再确认——避免误触把模板删掉。 */
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);
  const [importText, setImportText] = useState("");
  const [exported, setExported] = useState<{ name: string; payload: string } | null>(null);
  const [copied, setCopied] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const page = await listRecipes(0, 200);
      setItems(page.items);
      setTotal(page.total);
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

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

        <div className="banner banner-warn" role="status">
          <Info size={14} />
          <div>
            <strong>{t("templates.blocked.title")}</strong> {t("templates.blocked.body")}
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
                    {item.match?.extensions?.length ? (
                      <p className="hint">
                        {t("templates.match", { extensions: item.match.extensions.join(", ") })}
                      </p>
                    ) : null}
                  </div>
                  <div className="button-row">
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
