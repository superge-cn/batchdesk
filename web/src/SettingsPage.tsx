import { useCallback, useEffect, useState } from "react";
import { Info, RotateCcw } from "lucide-react";
import { getConfig, resetScope, setConfig, toIpcError } from "./lib/ipc";
import { DEFAULT_LANG, useI18n, type MessageKey } from "./i18n";
import { ErrorBanner } from "./ErrorBanner";
import type { IpcError, ScopeView, SettingValue, ConfigScope } from "./lib/ipc/types";

type FieldKind = "select" | "text" | "number" | "auto" | "boolean" | "lines";

interface OptionDef {
  value: string;
  labelKey: MessageKey;
}

interface FieldDef {
  key: string;
  labelKey: MessageKey;
  kind: FieldKind;
  options?: OptionDef[];
  hintKey?: MessageKey;
}

interface ScopeDef {
  id: ConfigScope;
  labelKey: MessageKey;
  hintKey: MessageKey;
  fields: FieldDef[];
}

/**
 * 设置页字段清单（设计 03 §4.1 的字段存在性与类型为权威）。
 * 这里只排 UI：可选值集合与 kernel 的枚举一一对应，新增字段必须同时改 kernel 与语言包。
 */
const SCOPES: ScopeDef[] = [
  {
    id: "general",
    labelKey: "settings.scope.general",
    hintKey: "settings.scope.general.hint",
    fields: [
      {
        key: "language",
        labelKey: "settings.field.language",
        kind: "select",
        hintKey: "settings.hint.language",
        options: [
          { value: "zh-CN", labelKey: "settings.language.zh" },
          { value: "en-US", labelKey: "settings.language.en" }
        ]
      },
      {
        key: "theme",
        labelKey: "settings.field.theme",
        kind: "select",
        hintKey: "settings.hint.theme",
        options: [
          { value: "system", labelKey: "settings.theme.system" },
          { value: "light", labelKey: "settings.theme.light" },
          { value: "dark", labelKey: "settings.theme.dark" }
        ]
      },
      {
        key: "defaultOutputDir",
        labelKey: "settings.field.defaultOutputDir",
        kind: "text",
        hintKey: "settings.hint.outputDir"
      },
      {
        key: "launchBehavior",
        labelKey: "settings.field.launchBehavior",
        kind: "select",
        options: [
          { value: "normal", labelKey: "settings.launch.normal" },
          { value: "minimized", labelKey: "settings.launch.minimized" }
        ]
      }
    ]
  },
  {
    id: "processing",
    labelKey: "settings.scope.processing",
    hintKey: "settings.scope.processing.hint",
    fields: [
      {
        key: "onConflict",
        labelKey: "settings.field.onConflict",
        kind: "select",
        options: [
          { value: "skip", labelKey: "settings.onConflict.skip" },
          { value: "sequence", labelKey: "settings.onConflict.sequence" },
          { value: "overwrite", labelKey: "settings.onConflict.overwrite" },
          { value: "ask", labelKey: "settings.onConflict.ask" }
        ]
      },
      {
        key: "encodingPolicy",
        labelKey: "settings.field.encodingPolicy",
        kind: "select",
        options: [
          { value: "auto", labelKey: "settings.encodingPolicy.auto" },
          { value: "strict-utf8", labelKey: "settings.encodingPolicy.strictUtf8" }
        ]
      },
      {
        key: "newlinePolicy",
        labelKey: "settings.field.newlinePolicy",
        kind: "select",
        options: [
          { value: "keep", labelKey: "settings.newlinePolicy.keep" },
          { value: "crlf", labelKey: "settings.newlinePolicy.crlf" },
          { value: "lf", labelKey: "settings.newlinePolicy.lf" }
        ]
      },
      {
        key: "deletionMode",
        labelKey: "settings.field.deletionMode",
        kind: "select",
        options: [
          { value: "move-to-quarantine", labelKey: "settings.deletionMode.moveToQuarantine" },
          { value: "permanent", labelKey: "settings.deletionMode.permanent" }
        ]
      },
      { key: "keepBackup", labelKey: "settings.field.keepBackup", kind: "boolean" }
    ]
  },
  {
    id: "performance",
    labelKey: "settings.scope.performance",
    hintKey: "settings.scope.performance.hint",
    fields: [
      { key: "concurrency", labelKey: "settings.field.concurrency", kind: "auto" },
      { key: "memoryBudget", labelKey: "settings.field.memoryBudget", kind: "auto" },
      { key: "largeFileThreshold", labelKey: "settings.field.largeFileThreshold", kind: "auto" }
    ]
  },
  {
    id: "security",
    labelKey: "settings.scope.security",
    hintKey: "settings.scope.security.hint",
    fields: [
      {
        key: "protectedDirs",
        labelKey: "settings.field.protectedDirs",
        kind: "lines",
        hintKey: "settings.hint.protectedDirs"
      },
      { key: "quarantineDir", labelKey: "settings.field.quarantineDir", kind: "text" },
      {
        key: "quarantineRetentionDays",
        labelKey: "settings.field.quarantineRetentionDays",
        kind: "number",
        hintKey: "settings.hint.retention"
      },
      {
        key: "placeholderPolicy",
        labelKey: "settings.field.placeholderPolicy",
        kind: "select",
        options: [
          { value: "skip", labelKey: "settings.placeholderPolicy.skip" },
          {
            value: "download-then-process",
            labelKey: "settings.placeholderPolicy.downloadThenProcess"
          }
        ]
      },
      {
        key: "diskSpaceMarginFloorBytes",
        labelKey: "settings.field.diskSpaceMarginFloorBytes",
        kind: "number"
      },
      { key: "diskSpaceMarginRatio", labelKey: "settings.field.diskSpaceMarginRatio", kind: "number" }
    ]
  },
  {
    id: "logging",
    labelKey: "settings.scope.logging",
    hintKey: "settings.scope.logging.hint",
    fields: [
      { key: "retentionDays", labelKey: "settings.field.retentionDays", kind: "number" },
      {
        key: "logFullPaths",
        labelKey: "settings.field.logFullPaths",
        kind: "boolean",
        hintKey: "settings.hint.logFullPaths"
      }
    ]
  }
];

function displayValue(setting: SettingValue | undefined): unknown {
  if (!setting) return undefined;
  return setting.value ?? setting.defaultValue;
}

function asText(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  if (Array.isArray(value)) return value.map((item) => String(item)).join("\n");
  return String(value);
}

/**
 * 设置（设计 03 §6.7 六分组里的五个可写组；授权组只读，走 getLicenseState）。
 *
 * 三条边界：
 * 1. 只写 ConfigDocument（设计 03 §7 的 setConfig/resetScope），不碰 UiState；
 * 2. 逐项显示来源——用户看到的是"生效值"而不是"文件里写了什么"；
 * 3. 语言与主题经 i18n 层即时生效，其余字段保存后由使用方下次读取时生效。
 */
export function SettingsPage({
  onBack,
  onResetUiState
}: {
  onBack: () => void;
  /** 重置本机界面状态（设计 03 §11.3）。不传时该区块的按钮保持 disabled。 */
  onResetUiState?: () => Promise<void>;
}) {
  const { t, lang, theme, setLang, setTheme } = useI18n();
  const [views, setViews] = useState<Partial<Record<ConfigScope, ScopeView>>>({});
  const [error, setError] = useState<IpcError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState<ConfigScope | null>(null);
  const [readOnly, setReadOnly] = useState(false);
  const [resettingUiState, setResettingUiState] = useState(false);

  const load = useCallback(async () => {
    try {
      const loaded = await Promise.all(SCOPES.map((scope) => getConfig(scope.id)));
      const next: Partial<Record<ConfigScope, ScopeView>> = {};
      loaded.forEach((view) => {
        next[view.scope] = view;
      });
      setViews(next);
      setError(null);
    } catch (value) {
      setError(toIpcError(value));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const applyResult = (view: ScopeView) => {
    setViews((previous) => ({ ...previous, [view.scope]: view }));
    setNotice(t("settings.saved"));
    setError(null);
  };

  const save = async (scope: ConfigScope, key: string, value: unknown) => {
    setBusy(scope);
    setNotice(null);
    try {
      const view = await setConfig(scope, { [key]: value });
      applyResult(view);
    } catch (value) {
      const failure = toIpcError(value);
      setReadOnly(failure.code === "E1007");
      setError({ ...failure, message: t("settings.saveError", { message: failure.message }) });
      // 保存失败时回读一次，避免界面显示一个并未落盘的"已保存"值。
      void load();
    } finally {
      setBusy(null);
    }
  };

  const reset = async (scope: ConfigScope) => {
    setBusy(scope);
    setNotice(null);
    try {
      const view = await resetScope(scope);
      applyResult(view);
      if (scope === "general") {
        setLang(DEFAULT_LANG);
        setTheme("light");
      }
    } catch (value) {
      const failure = toIpcError(value);
      setReadOnly(failure.code === "E1007");
      setError({ ...failure, message: t("settings.saveError", { message: failure.message }) });
    } finally {
      setBusy(null);
    }
  };

  const change = (scope: ConfigScope, field: FieldDef, raw: string, checked?: boolean) => {
    if (scope === "general" && field.key === "language") {
      setLang(raw as typeof lang);
      setNotice(t("settings.saved"));
      return;
    }
    if (scope === "general" && field.key === "theme") {
      setTheme(raw as typeof theme);
      setNotice(t("settings.saved"));
      return;
    }
    if (field.kind === "boolean") return void save(scope, field.key, Boolean(checked));
    if (field.kind === "lines") {
      const lines = raw
        .split(/\r?\n/)
        .map((line) => line.trim())
        .filter(Boolean);
      return void save(scope, field.key, lines);
    }
    if (field.kind === "number") {
      if (raw.trim() === "") return void save(scope, field.key, null);
      const parsed = Number(raw);
      if (!Number.isFinite(parsed)) return;
      return void save(scope, field.key, parsed);
    }
    if (field.kind === "auto") {
      const trimmed = raw.trim();
      if (trimmed === "" || trimmed === "auto") return void save(scope, field.key, "auto");
      const parsed = Number(trimmed);
      if (!Number.isFinite(parsed)) return;
      return void save(scope, field.key, parsed);
    }
    return void save(scope, field.key, raw.trim() === "" ? null : raw);
  };

  /** 重置界面状态：只清 UiState，不动 ConfigDocument（设计 03 §11.3 的边界）。 */
  async function resetUiStateMemory() {
    if (!onResetUiState) return;
    setResettingUiState(true);
    setNotice(null);
    setError(null);
    try {
      await onResetUiState();
      setNotice(t("settings.uiState.done"));
    } catch (value) {
      setError(toIpcError(value));
    } finally {
      setResettingUiState(false);
    }
  }

  const control = (scope: ScopeDef, field: FieldDef) => {
    const setting = views[scope.id]?.values[field.key];
    const value = displayValue(setting);
    const id = `${scope.id}-${field.key}`;
    const disabled = busy === scope.id;

    if (field.kind === "select") {
      const current =
        field.key === "language" ? lang : field.key === "theme" ? theme : asText(value);
      return (
        <select
          id={id}
          className="select"
          value={current}
          disabled={disabled}
          onChange={(event) => change(scope.id, field, event.target.value)}
        >
          {(field.options ?? []).map((option) => (
            <option key={option.value} value={option.value}>
              {t(option.labelKey)}
            </option>
          ))}
        </select>
      );
    }
    if (field.kind === "boolean") {
      return (
        <label className="check" htmlFor={id}>
          <input
            id={id}
            type="checkbox"
            checked={value === true}
            disabled={disabled}
            onChange={(event) => change(scope.id, field, "", event.target.checked)}
          />
          {t(field.labelKey)}
        </label>
      );
    }
    if (field.kind === "lines") {
      return (
        <textarea
          id={id}
          className="textarea"
          rows={4}
          value={asText(value)}
          disabled={disabled}
          spellCheck={false}
          onChange={(event) => change(scope.id, field, event.target.value)}
        />
      );
    }
    return (
      <input
        id={id}
        className="input"
        value={asText(value)}
        disabled={disabled}
        placeholder={field.kind === "auto" ? "auto" : ""}
        inputMode={field.kind === "number" || field.kind === "auto" ? "numeric" : undefined}
        spellCheck={false}
        onChange={(event) => change(scope.id, field, event.target.value)}
      />
    );
  };

  return (
    <div className="content">
      {error && <ErrorBanner error={error} onClose={() => setError(null)} />}
      {readOnly && (
        <div className="banner banner-warn" role="status">
          <div>{t("settings.readonly")}</div>
        </div>
      )}
      {notice && (
        <div className="banner banner-info" role="status">
          <div>{notice}</div>
        </div>
      )}

      <div className="content-grid">
        <div className="stack">
          {SCOPES.map((scope) => (
            <section className="card" aria-labelledby={`scope-${scope.id}`} key={scope.id}>
              <div className="card-header">
                <div>
                  <h2 className="card-title" id={`scope-${scope.id}`}>
                    {t(scope.labelKey)}
                  </h2>
                  <p className="card-sub">{t(scope.hintKey)}</p>
                </div>
                <button
                  className="button button-secondary"
                  type="button"
                  onClick={() => void reset(scope.id)}
                  disabled={busy === scope.id}
                >
                  <RotateCcw size={14} /> {t("settings.reset")}
                </button>
              </div>

              <div className="field-grid">
                {scope.fields.map((field) => {
                  const setting = views[scope.id]?.values[field.key];
                  const customized = setting?.effectiveSource === "user";
                  return (
                    <div
                      className={`field${field.kind === "lines" || field.kind === "boolean" ? " wide" : ""}`}
                      key={`${scope.id}-${field.key}`}
                    >
                      {field.kind !== "boolean" && (
                        <label htmlFor={`${scope.id}-${field.key}`}>{t(field.labelKey)}</label>
                      )}
                      {control(scope, field)}
                      <div className="hint">
                        {customized ? t("settings.source.user") : t("settings.source.default")}
                        {field.hintKey ? ` · ${t(field.hintKey)}` : ""}
                      </div>
                    </div>
                  );
                })}
              </div>
            </section>
          ))}

          <section className="card" aria-labelledby="scope-ui-state">
            <div className="card-header">
              <div>
                <h2 className="card-title" id="scope-ui-state">
                  {t("settings.uiState.title")}
                </h2>
                <p className="card-sub">{t("settings.uiState.hint")}</p>
              </div>
              <button
                className="button button-secondary"
                type="button"
                onClick={() => void resetUiStateMemory()}
                disabled={resettingUiState || !onResetUiState}
              >
                <RotateCcw size={14} /> {t("settings.uiState.action")}
              </button>
            </div>
          </section>

          <div className="hint">
            <Info size={14} /> {t("settings.webNote")}
          </div>

          <div className="button-row">
            <button className="button button-ghost" type="button" onClick={onBack}>
              {t("common.backC09")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
