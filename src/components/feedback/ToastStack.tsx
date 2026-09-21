/**
 * 提示条：非错误类提示 6 秒后渐隐淡出；失败、有数据损失或权限问题必须持久显示且可复盘（项目书 §2.3）。
 */

import { useEffect, useState } from "react";
import { IconCheck, IconClose, IconInfo, IconWarning } from "@/components/icons";
import { useUiStore, type Toast } from "@/stores/uiStore";

const AUTO_DISMISS_MS = 6000;
const LEAVE_MS = 320;

function toneIcon(level: Toast["level"]): React.ReactNode {
  if (level === "success") return <IconCheck size={18} />;
  if (level === "warning" || level === "error") return <IconWarning size={18} />;
  return <IconInfo size={18} />;
}

function ToastRow({ toast }: { toast: Toast }): React.JSX.Element {
  const dismiss = useUiStore((s) => s.dismissToast);
  const [leaving, setLeaving] = useState(false);

  useEffect(() => {
    if (toast.persistent) return;
    const timer = window.setTimeout(() => setLeaving(true), AUTO_DISMISS_MS);
    return () => window.clearTimeout(timer);
  }, [toast.id, toast.persistent]);

  useEffect(() => {
    if (!leaving) return;
    // 减弱动效时不做过渡，直接移除
    if (document.documentElement.dataset.vfReducedMotion === "true") {
      dismiss(toast.id);
      return;
    }
    const timer = window.setTimeout(() => dismiss(toast.id), LEAVE_MS);
    return () => window.clearTimeout(timer);
  }, [leaving, toast.id, dismiss]);

  return (
    <div
      className={leaving ? "vf-toast vf-toast--leaving" : "vf-toast"}
      data-level={toast.level}
      role="status"
      aria-live="polite"
    >
      <span className="vf-toast__icon">{toneIcon(toast.level)}</span>
      <div className="vf-toast__body">
        <p className="vf-toast__message">{toast.message}</p>
        {toast.detail ? <p className="vf-toast__detail">{toast.detail}</p> : null}
      </div>
      <button
        type="button"
        className="vf-toast__close"
        aria-label="关闭提示"
        onClick={() => dismiss(toast.id)}
      >
        <IconClose size={15} />
      </button>
    </div>
  );
}

export function ToastStack(): React.JSX.Element | null {
  const toasts = useUiStore((s) => s.toasts);
  if (toasts.length === 0) return null;
  return (
    <div className="vf-toast-stack">
      {toasts.map((t) => (
        <ToastRow key={t.id} toast={t} />
      ))}
    </div>
  );
}