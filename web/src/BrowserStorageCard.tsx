import { useCallback, useEffect, useState } from "react";
import { Download, RefreshCw, Trash2 } from "lucide-react";
import { defaultProductSaver } from "./lib/web/applyChannel";
import { formatBytes } from "./lib/format";
import {
  clearPersisted,
  listPersisted,
  opfsSupported,
  readPersisted,
  type PersistedProduct
} from "./lib/web/opfsStore";
import { useI18n } from "./i18n";

/**
 * 设置页的「浏览器存储」卡片：执行产物在浏览器里留的那一份。
 *
 * 为什么要一个入口：A3-13 只做了「执行后自动写」与「面板上的清除」，
 * `listPersisted`／`readPersisted` 没有界面——关掉页面后用户找不回产物，
 * 「跨会话留存」等于打了折。这里把读回、单件下载、整体清理补齐。
 *
 * 持久化边界（AGENTS.md 不变量 14）：OPFS 里只放**产物字节**，不进 Config/UiState。
 * 零强制力（设计 05）：这里的任何判定都不构成授权或配额依据；失败静默降级。
 */
export function BrowserStorageCard() {
  const { t } = useI18n();
  const supported = opfsSupported();
  const [items, setItems] = useState<PersistedProduct[] | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    if (!supported) {
      setItems([]);
      return;
    }
    setBusy(true);
    try {
      setItems(await listPersisted());
    } finally {
      setBusy(false);
    }
  }, [supported]);

  useEffect(() => {
    void load();
  }, [load]);

  const onDownload = async (item: PersistedProduct) => {
    const blob = await readPersisted(item.planId, item.name);
    if (blob === null) {
      // 列出来之后被别处清掉了：刷新一下，别让用户点一个永远失败的按钮。
      await load();
      return;
    }
    defaultProductSaver(item.name, blob);
  };

  const onClear = async () => {
    setBusy(true);
    try {
      if (await clearPersisted()) setItems([]);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="card" aria-labelledby="browser-storage">
      <div className="card-header">
        <div>
          <h2 className="card-title" id="browser-storage">
            {t("settings.browserStore.title")}
          </h2>
          <p className="card-sub">{t("settings.browserStore.hint")}</p>
        </div>
        <button
          className="button button-secondary"
          type="button"
          onClick={() => void load()}
          disabled={busy || !supported}
        >
          <RefreshCw size={14} /> {t("settings.browserStore.refresh")}
        </button>
      </div>

      {!supported ? (
        <p className="hint" data-testid="browser-store-unsupported">
          {t("settings.browserStore.unsupported")}
        </p>
      ) : items === null ? (
        <p className="hint">{t("settings.browserStore.loading")}</p>
      ) : items.length === 0 ? (
        <p className="hint" data-testid="browser-store-empty">
          {t("settings.browserStore.empty")}
        </p>
      ) : (
        <>
          <ul className="dialog-list" data-testid="browser-store-list">
            {items.map((item) => (
              <li className="dialog-list" key={`${item.planId}/${item.name}`}>
                <span>{item.name}</span>{" "}
                <span className="hint">{formatBytes(item.size)}</span>{" "}
                <button
                  className="button button-secondary"
                  type="button"
                  data-testid="browser-store-download"
                  onClick={() => void onDownload(item)}
                >
                  <Download size={14} /> {t("feature.sandbox.download")}
                </button>
              </li>
            ))}
          </ul>
          <button
            className="button button-secondary"
            type="button"
            data-testid="browser-store-clear"
            onClick={() => void onClear()}
            disabled={busy}
          >
            <Trash2 size={14} /> {t("feature.sandbox.clearPersisted")}
          </button>
        </>
      )}
    </section>
  );
}
