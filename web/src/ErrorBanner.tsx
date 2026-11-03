import { describeError } from "./errors";
import { useI18n } from "./i18n";
import type { IpcError } from "./lib/ipc/types";

/**
 * 统一的错误横幅：码位 ＋ 本地化摘要 ＋ 建议动作（设计 06 §3、设计 09 §5）。
 *
 * 刻意**不**渲染 `IpcError.message`／`detail` 原文：06 §3 规定技术细节只进诊断日志，
 * 而这两处常带绝对路径与内部标识。码位本身仍显示，便于反馈时引用。
 */
export function ErrorBanner({ error, onClose }: { error: IpcError; onClose: () => void }) {
  const { t } = useI18n();
  const { title, action } = describeError(t, error);

  return (
    <div className="banner banner-danger" role="alert">
      <div>
        <strong>{error.code}</strong> · {title}
        {action && <div className="muted">{action}</div>}
      </div>
      <button className="button button-ghost" type="button" onClick={onClose}>
        {t("common.close")}
      </button>
    </div>
  );
}
