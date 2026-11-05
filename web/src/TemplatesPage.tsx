import { useCallback, useEffect, useMemo, useState } from "react";
import { Download, Info, Loader2, Trash2, Upload } from "lucide-react";
import { deleteRecipe, exportRecipe, importRecipe, listRecipes, toIpcError } from "./lib/ipc";
import { ErrorBanner } from "./ErrorBanner";
import { useI18n } from "./i18n";
import { actionLabel } from "./labels";
import type { IpcError, RecipeDocument } from "./lib/ipc/types";

/**
 * 处理模板（C37，方案 §15、设计 12 §5）。
 *
 * **这一版只做"存取那一半"**：列表、查看步骤、删除、导入、导出，以及功能页那边的
 * 「存为处理模板」。缺的是"把模板变成计划"——那需要 `loadRecipe`，而它卡在
 * [OQ-36](../../docs/OPEN-QUESTIONS.md)：`TemplateStep.kind` 是动作（`split`／`rename`／
 * `csv-rows`），壳层按功能码（`f01`／`f03`）路由，动作→功能码是一对多。
 * 裁决前不实现（AGENTS「何时停下来问人」），页面把这件事写成一条横幅而不是给个
 * 假按钮：点了没反应比没有按钮更糟。
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
