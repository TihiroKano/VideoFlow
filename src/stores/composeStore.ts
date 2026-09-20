/**
 * 解析工作台状态。
 *
 * 放在 store 而不是组件内，原因有两个：
 * 1. 切换页面（首页 ↔ 设置）不丢失解析结果；
 * 2. 开发期可以整体注入夹具，便于与预览图逐项比对。
 */

import { create } from "zustand";
import type { ResolvedMedia, StreamSelection } from "@/services/types";

export type PreviewState = "idle" | "loading" | "success" | "error";

interface ComposeState {
  url: string;
  previewState: PreviewState;
  media: ResolvedMedia | null;
  selection: StreamSelection | null;
  error: string | null;
  setUrl: (url: string) => void;
  beginParse: () => void;
  succeed: (media: ResolvedMedia, selection: StreamSelection | null) => void;
  fail: (message: string) => void;
  setSelection: (selection: StreamSelection) => void;
  reset: () => void;
}

export const useComposeStore = create<ComposeState>((set) => ({
  url: "",
  previewState: "idle",
  media: null,
  selection: null,
  error: null,

  setUrl: (url) => set({ url }),

  beginParse: () =>
    set({ previewState: "loading", media: null, selection: null, error: null }),

  succeed: (media, selection) =>
    set({ previewState: "success", media, selection, error: null }),

  fail: (message) => set({ previewState: "error", media: null, selection: null, error: message }),

  setSelection: (selection) => set({ selection }),

  reset: () => set({ previewState: "idle", media: null, selection: null, error: null }),
}));