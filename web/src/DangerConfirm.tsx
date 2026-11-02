import { useEffect, useRef, type ReactNode } from "react";
import { AlertTriangle } from "lucide-react";

/**
 * 危险级二次确认对话框（方案 §7.11：危险动作必须二次确认且确认按钮不带默认焦点；
 * U v1.3 §3.5：初始焦点落「取消」、危险按钮 destructive、不整块红底）。
 * 调用方负责判定与拦截——本组件只做确认，不放宽任何执行条件。
 */
export interface DangerConfirmProps {
  title: string;
  body: ReactNode;
  /** 高危条目清单（通常是命中保护目录的路径）。 */
  items: string[];
  confirmLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
}

export function DangerConfirm({ title, body, items, confirmLabel, onConfirm, onCancel }: DangerConfirmProps) {
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    // 「回车即确认」的误操作路径在视觉层即被消除：焦点始终落取消，不给危险按钮。
    cancelRef.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  return (
    <div className="dialog-backdrop" data-testid="danger-confirm">
      <div
        aria-labelledby="danger-confirm-title"
        aria-modal="true"
        className="dialog-card"
        role="alertdialog"
      >
        <h3 className="dialog-title" id="danger-confirm-title">
          <AlertTriangle aria-hidden="true" size={16} /> {title}
        </h3>
        <p className="dialog-body">{body}</p>
        {items.length > 0 && (
          <ul data-testid="danger-confirm-items">
            {items.map((item) => (
              <li className="dialog-list" key={item}>
                {item}
              </li>
            ))}
          </ul>
        )}
        <div className="dialog-actions">
          <button
            className="button button-secondary"
            data-testid="danger-cancel"
            ref={cancelRef}
            type="button"
            onClick={onCancel}
          >
            取消
          </button>
          <button
            className="button button-danger"
            data-testid="danger-confirm-button"
            type="button"
            onClick={onConfirm}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
