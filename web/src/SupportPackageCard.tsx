import { Download, Eye } from "lucide-react";
import { useState } from "react";
import { describeError } from "./errors";
import { useI18n } from "./i18n";
import { buildSupportPackage, toIpcError, writeSupportPackage } from "./lib/ipc";
import type { DiagnosticLogRecord, SupportPackage } from "./lib/ipc/types";
import { VirtualList } from "./VirtualList";

/**
 * 支持包卡片（设计 10 §5；方案 §9.7／§12.5）。
 *
 * 两条边界：
 * 1. **先预览后导出**——`writeSupportPackage` 写回的就是刚预览过的那份，不重新组装
 *    （避免"预览的内容"与"落盘的内容"不一致）；
 * 2. 内容不含文件内容、许可证载荷与完整路径（设计 10 §3）；界面也不渲染后端错误原文，
 *    只走 `describeError`（ADR-0061／设计 06 §3）。
 */
export function SupportPackageCard() {
  const { t } = useI18n();
  const [reproSteps, setReproSteps] = useState("");
  const [includeLog, setIncludeLog] = useState(true);
  const [includeStats, setIncludeStats] = useState(true);
  const [preview, setPreview] = useState<SupportPackage | null>(null);
  const [exportedPath, setExportedPath] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<"idle" | "building" | "writing">("idle");

  const build = async () => {
    setBusy("building");
    setError(null);
    setExportedPath(null);
    try {
      setPreview(
        await buildSupportPackage({
          reproSteps,
          includeDiagnosticLog: includeLog,
          includeErrorCodeStats: includeStats
        })
      );
    } catch (value) {
      setPreview(null);
      setError(describeError(t, toIpcError(value)).title);
    } finally {
      setBusy("idle");
    }
  };

  const exportPackage = async () => {
    if (!preview) return;
    setBusy("writing");
    setError(null);
    try {
      setExportedPath(await writeSupportPackage(preview));
    } catch (value) {
      setError(describeError(t, toIpcError(value)).title);
    } finally {
      setBusy("idle");
    }
  };

  return (
    <section className="card" aria-labelledby="scope-support">
      <div className="card-header">
        <div>
          <h2 className="card-title" id="scope-support">
            {t("settings.support.title")}
          </h2>
          <p className="card-sub">{t("settings.support.hint")}</p>
        </div>
      </div>

      <div className="field-grid">
        <div className="field wide">
          <label htmlFor="support-repro">{t("settings.support.reproSteps")}</label>
          <textarea
            id="support-repro"
            className="textarea"
            rows={3}
            value={reproSteps}
            spellCheck={false}
            placeholder={t("settings.support.reproSteps.placeholder")}
            onChange={(event) => setReproSteps(event.target.value)}
          />
          <div className="hint">{t("settings.support.reproSteps.hint")}</div>
        </div>
        <div className="field wide">
          <label className="check" htmlFor="support-include-log">
            <input
              id="support-include-log"
              type="checkbox"
              checked={includeLog}
              onChange={(event) => setIncludeLog(event.target.checked)}
            />
            {t("settings.support.includeLog")}
          </label>
          <label className="check" htmlFor="support-include-stats">
            <input
              id="support-include-stats"
              type="checkbox"
              checked={includeStats}
              onChange={(event) => setIncludeStats(event.target.checked)}
            />
            {t("settings.support.includeStats")}
          </label>
        </div>
      </div>

      <div className="button-row">
        <button
          className="button button-secondary"
          type="button"
          onClick={() => void build()}
          disabled={busy !== "idle"}
        >
          <Eye size={14} /> {t("settings.support.preview")}
        </button>
        <button
          className="button"
          type="button"
          onClick={() => void exportPackage()}
          disabled={!preview || busy !== "idle"}
        >
          <Download size={14} /> {t("settings.support.export")}
        </button>
      </div>

      {error && (
        <div className="banner banner-danger" role="alert">
          {error}
        </div>
      )}
      {exportedPath && (
        <div className="banner banner-info" role="status">
          {t("settings.support.exported", { path: exportedPath })}
        </div>
      )}

      {preview && <SupportPreview package={preview} />}
    </section>
  );
}

function SupportPreview({ package: pkg }: { package: SupportPackage }) {
  const { t } = useI18n();
  return (
    <div className="stack">
      <div className="hint mono">
        {t("settings.support.environment", {
          version: pkg.version,
          build: pkg.build,
          os: pkg.os,
          arch: pkg.arch,
          generatedAt: pkg.generatedAt
        })}
      </div>

      <div>
        <div className="hint">{t("settings.support.reproTitle")}</div>
        {pkg.reproSteps.trim() ? (
          <div className="mono">{pkg.reproSteps}</div>
        ) : (
          <div className="muted">{t("settings.support.reproEmpty")}</div>
        )}
      </div>

      {pkg.errorCodeStats.length > 0 && (
        <div>
          <div className="hint">{t("settings.support.statsTitle")}</div>
          <div className="mono">
            {pkg.errorCodeStats.map((stat) => (
              <div key={stat.code}>
                {stat.code} × {stat.count}
              </div>
            ))}
          </div>
        </div>
      )}

      <div>
        <div className="hint">
          {t("settings.support.logTitle", { count: pkg.diagnosticLog.length })}
        </div>
        {pkg.diagnosticLog.length === 0 ? (
          <div className="empty">{t("settings.support.logEmpty")}</div>
        ) : (
          // 方案 6.2：日志行数由壳层上限决定（工程默认 200），只能窗口化渲染。
          <VirtualList
            label={t("settings.support.logTitle", { count: pkg.diagnosticLog.length })}
            items={pkg.diagnosticLog}
            getKey={(_record, index) => String(index)}
            renderRow={(record) => <LogRow record={record} />}
          />
        )}
      </div>
    </div>
  );
}

/** 单行等宽日志：时间／级别／组件／码／消息。长消息省略号截断，原文进 title。 */
function LogRow({ record }: { record: DiagnosticLogRecord }) {
  return (
    <div className="log-row">
      <span>{record.ts}</span>
      <span>{record.level}</span>
      <span>{record.target}</span>
      {record.code && <span>{record.code}</span>}
      <span title={record.message}>{record.message}</span>
    </div>
  );
}
