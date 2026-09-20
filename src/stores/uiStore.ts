/** 页面导航、模式切换、提示与确认对话框。 */

import { create } from "zustand";

export type PageId = "home" | "downloads" | "converts" | "background" | "settings";
export type WorkMode = "download" | "convert";

export interface Toast {
  id: string;
  level: "info" | "success" | "warning" | "error";
  message: string;
  /** 失败与数据损失类提示必须持久显示并可复盘 */
  persistent: boolean;
  detail?: string;
  taskId?: string;
  createdAt: number;
}

export interface ConfirmRequest {
  title: string;
  message: string;
  confirmText: string;
  tone: "default" | "danger";
  /** 删除文件等动作要求点击按钮确认，Esc 不生效 */
  requireExplicitClick?: boolean;
  onConfirm: () => void | Promise<void>;
}

interface UiState {
  page: PageId;
  mode: WorkMode;
  settingsOpen: boolean;
  toasts: Toast[];
  confirm: ConfirmRequest | null;
  setPage: (page: PageId) => void;
  setMode: (mode: WorkMode) => void;
  setSettingsOpen: (open: boolean) => void;
  pushToast: (toast: Omit<Toast, "id" | "createdAt"> & { id?: string }) => string;
  dismissToast: (id: string) => void;
  clearToasts: () => void;
  requestConfirm: (req: ConfirmRequest) => void;
  closeConfirm: () => void;
}

let toastSeq = 0;

export const useUiStore = create<UiState>((set, get) => ({
  page: "home",
  mode: "download",
  settingsOpen: false,
  toasts: [],
  confirm: null,

  setPage: (page) => set({ page }),
  setMode: (mode) => set({ mode }),
  setSettingsOpen: (open) => set({ settingsOpen: open }),

  pushToast: (toast) => {
    const id = toast.id ?? `t${++toastSeq}`;
    const entry: Toast = { ...toast, id, createdAt: Date.now() };
    set({ toasts: [...get().toasts, entry] });
    return id;
  },

  dismissToast: (id) => set({ toasts: get().toasts.filter((t) => t.id !== id) }),
  clearToasts: () => set({ toasts: [] }),

  requestConfirm: (req) => set({ confirm: req }),
  closeConfirm: () => set({ confirm: null }),
}));