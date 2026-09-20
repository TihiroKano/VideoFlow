/**
 * 危险动作确认框：取消下载、删除文件、清空记录（项目书 §2.3）。
 * 删除文件类动作要求点击按钮确认，Esc 不生效。
 */

import { useEffect, useRef } from "react";
import { useUiStore } from "@/stores/uiStore";

export function ConfirmDialog(): React.JSX.Element | null {
  const confirm = useUiStore((s) => s.confirm);
  const close = useUiStore((s) => s.closeConfirm);
  const confirmRef = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    if (!confirm) return;
    // 焦点立即转移，动画不延迟可操作性
    confirmRef.current?.focus();

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !confirm.requireExplicitClick) {
        e.preventDefault();
        close();
      }
      if (e.key === "Tab") {
        // 焦点锁定在对话框内
        const root = document.getElementById("vf-confirm-dialog");
        if (!root) return;
        const focusables = root.querySelectorAll<HTMLElement>("button");
        if (focusables.length === 0) return;
        const first = focusables[0];
        const last = focusables[focusables.length - 1];
        if (!first || !last) return;
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [confirm, close]);

  if (!confirm) return null;

  const onConfirm = () => {
    const action = confirm.onConfirm;
    close();
    void action();
  };

  return (
    <div className="vf-dialog-backdrop">
      <div
        id="vf-confirm-dialog"
        className="vf-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="vf-confirm-title"
        aria-describedby="vf-confirm-message"
      >
        <h2 id="vf-confirm-title" className="vf-dialog__title">
          {confirm.title}
        </h2>
        <p id="vf-confirm-message" className="vf-dialog__message">
          {confirm.message}
        </p>
        <div className="vf-dialog__actions">
          <button type="button" className="vf-btn-ghost vf-dialog__btn" onClick={close}>
            取消
          </button>
          <button
            ref={confirmRef}
            type="button"
            className={confirm.tone === "danger" ? "vf-btn-danger vf-dialog__btn" : "vf-btn-primary vf-dialog__btn"}
            onClick={onConfirm}
          >
            {confirm.confirmText}
          </button>
        </div>
      </div>
    </div>
  );
}