/** 背景控制台状态：用户自由上传的静态图片背景（项目书 §2.2，不支持视频壁纸）。 */

import { create } from "zustand";
import defaultBackground from "@/assets/background-default.jpg";

/** 项目书只要求 cover / contain 两种适配（§2.2、§11） */
export type BgFit = "cover" | "contain";

interface Persisted {
  imageUrl: string | null;
  imageName: string;
  fit: BgFit;
  opacity: number;
  blur: number;
  brightness: number;
}

const STORAGE_KEY = "videoflow.background.v1";

const defaults: Persisted = {
  imageUrl: defaultBackground,
  imageName: "背景图1.jpg",
  fit: "cover",
  opacity: 1,
  blur: 26,
  brightness: 1,
};

function load(): Persisted {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return defaults;
    const parsed = JSON.parse(raw) as Partial<Persisted>;
    // 只允许 data: 与 http(s): 的图片，避免把任意字符串塞进 CSS
    const url = typeof parsed.imageUrl === "string" ? parsed.imageUrl : null;
    const safeUrl = url && /^(data:image\/|https?:|blob:)/.test(url) ? url : defaults.imageUrl;
    return { ...defaults, ...parsed, imageUrl: safeUrl };
  } catch {
    return defaults;
  }
}

interface BackgroundState extends Persisted {
  setImage: (url: string | null, name: string) => void;
  setFit: (fit: BgFit) => void;
  setOpacity: (v: number) => void;
  setBlur: (v: number) => void;
  setBrightness: (v: number) => void;
  setAll: (patch: Partial<Persisted>) => void;
  restoreDefault: () => void;
}

function persist(state: BackgroundState): void {
  const payload: Persisted = {
    imageUrl: state.imageUrl,
    imageName: state.imageName,
    fit: state.fit,
    opacity: state.opacity,
    blur: state.blur,
    brightness: state.brightness,
  };
  try {
    // 用户上传的图片以 data URL 形式保存，超出配额时降级为不持久化
    localStorage.setItem(STORAGE_KEY, JSON.stringify(payload));
  } catch {
    try {
      localStorage.setItem(
        STORAGE_KEY,
        JSON.stringify({ ...payload, imageUrl: state.imageUrl?.startsWith("data:") ? null : state.imageUrl }),
      );
    } catch {
      // 忽略：本次会话仍可正常使用
    }
  }
}

export const useBackgroundStore = create<BackgroundState>((set, get) => ({
  ...load(),

  setImage: (url, name) => {
    set({ imageUrl: url, imageName: name });
    persist(get());
  },
  setFit: (fit) => {
    set({ fit });
    persist(get());
  },
  setOpacity: (v) => {
    set({ opacity: Math.min(1, Math.max(0.2, v)) });
    persist(get());
  },
  setBlur: (v) => {
    set({ blur: Math.min(60, Math.max(0, v)) });
    persist(get());
  },
  setBrightness: (v) => {
    set({ brightness: Math.min(1.6, Math.max(0.5, v)) });
    persist(get());
  },
  setAll: (patch) => {
    set(patch);
    persist(get());
  },
  restoreDefault: () => {
    set({ ...defaults });
    persist(get());
  },
}));

export { defaultBackground };