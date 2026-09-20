/** 应用设置：渲染档位、下载、外观、辅助功能。本地持久化，不上传。 */

import { create } from "zustand";
import type { DeviceProfile, GlassTier } from "@/services/deviceProfile";
import { setSetting, type SidecarStatus } from "@/services/ipc";

export type TierChoice = "auto" | GlassTier;

interface PersistedSettings {
  tierChoice: TierChoice;
  downloadDir: string;
  maxConcurrent: number;
  chunkConcurrency: number;
  /** 同一来源（同一站点）同时运行的任务数上限，避免触发服务端限流 */
  perHostConcurrency: number;
  /** 登录态来源："" | "browser:edge" | "browser:chrome" | "browser:firefox" | "file:<路径>" */
  cookiesSource: string;
  reducedMotion: boolean;
  highContrast: boolean;
  glassIntensity: number;
}

const STORAGE_KEY = "videoflow.settings.v1";

const DEFAULT_DOWNLOAD_DIR = "D:\\VideoFlow\\Downloads";

const defaults: PersistedSettings = {
  tierChoice: "auto",
  downloadDir: DEFAULT_DOWNLOAD_DIR,
  maxConcurrent: 3,
  chunkConcurrency: 4,
  perHostConcurrency: 2,
  cookiesSource: "",
  reducedMotion: false,
  highContrast: false,
  glassIntensity: 1,
};

function load(): PersistedSettings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return defaults;
    const parsed = JSON.parse(raw) as Partial<PersistedSettings>;
    return { ...defaults, ...parsed };
  } catch {
    return defaults;
  }
}

interface SettingsState extends PersistedSettings {
  profile: DeviceProfile | null;
  profiling: boolean;
  /**
   * sidecar（yt-dlp / ffmpeg）探测结果。
   *
   * 探测要把两个可执行文件真的拉起来取版本，本机实测 2–3 秒，因此一次会话只做一次，
   * 之后进设置页直接复用——工具就在磁盘上，会话期间不会变。
   */
  sidecar: SidecarStatus | null;
  /** 实际生效的渲染档位（自动档会随探测结果收敛） */
  effectiveTier: GlassTier;
  effectiveReason: string;
  setTierChoice: (choice: TierChoice) => void;
  setProfile: (profile: DeviceProfile) => void;
  setProfiling: (v: boolean) => void;
  setSidecar: (status: SidecarStatus) => void;
  setDownloadDir: (dir: string) => void;
  setMaxConcurrent: (n: number) => void;
  setChunkConcurrency: (n: number) => void;
  setPerHostConcurrency: (n: number) => void;
  setCookiesSource: (source: string) => void;
  setReducedMotion: (v: boolean) => void;
  setHighContrast: (v: boolean) => void;
  setGlassIntensity: (n: number) => void;
  reset: () => void;
}

function persist(state: SettingsState): void {
  const payload: PersistedSettings = {
    tierChoice: state.tierChoice,
    downloadDir: state.downloadDir,
    maxConcurrent: state.maxConcurrent,
    chunkConcurrency: state.chunkConcurrency,
    perHostConcurrency: state.perHostConcurrency,
    cookiesSource: state.cookiesSource,
    reducedMotion: state.reducedMotion,
    highContrast: state.highContrast,
    glassIntensity: state.glassIntensity,
  };
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(payload));
  } catch {
    // 存储不可用时忽略：设置只在本次会话生效
  }
}

/** 把下载相关设置推给主进程；后台未就绪时静默忽略 */
async function pushToBackend(key: string, value: unknown): Promise<void> {
  try {
    await setSetting(key, value);
  } catch {
    // 纯浏览器预览或后台未就绪：前端设置仍然生效于本次会话
  }
}

/** 应用启动时把本地设置同步给主进程，保证主进程与界面一致 */
export async function syncSettingsToBackend(): Promise<void> {
  const s = useSettingsStore.getState();
  await Promise.all([
    pushToBackend("downloadDir", s.downloadDir),
    pushToBackend("maxConcurrent", s.maxConcurrent),
    pushToBackend("chunkConcurrency", s.chunkConcurrency),
    pushToBackend("perHostConcurrency", s.perHostConcurrency),
    pushToBackend("cookiesSource", s.cookiesSource),
    // 设备档位决定后端是否自动下调并发（项目书 §3.2）
    pushToBackend("deviceTier", s.effectiveTier),
  ]);
}

/** 自动档按推荐收敛；用户手选永久优先（项目书 §6.2） */
function resolveTier(state: {
  tierChoice: TierChoice;
  profile: DeviceProfile | null;
}): { tier: GlassTier; reason: string } {
  const { tierChoice, profile } = state;
  if (!profile) {
    return { tier: "balanced", reason: "尚未完成设备探测，先使用平衡档" };
  }
  if (tierChoice !== "auto") {
    return { tier: tierChoice, reason: "由用户手动指定" };
  }
  return { tier: profile.recommendation, reason: "根据本机能力自动推荐" };
}

const initial = load();
const initialResolved = resolveTier({ tierChoice: initial.tierChoice, profile: null });

export const useSettingsStore = create<SettingsState>((set, get) => ({
  ...initial,
  profile: null,
  profiling: false,
  sidecar: null,
  effectiveTier: initialResolved.tier,
  effectiveReason: initialResolved.reason,

  setTierChoice: (choice) => {
    set({ tierChoice: choice });
    const { tier, reason } = resolveTier({ tierChoice: choice, profile: get().profile });
    set({ effectiveTier: tier, effectiveReason: reason });
    persist(get());
    // 档位变了要让主进程重新算下载并发上限（项目书 §3.2）
    void pushToBackend("deviceTier", tier);
  },

  setProfile: (profile) => {
    set({ profile });
    const { tier, reason } = resolveTier({ tierChoice: get().tierChoice, profile });
    set({ effectiveTier: tier, effectiveReason: reason });
    void pushToBackend("deviceTier", tier);
  },

  setProfiling: (v) => set({ profiling: v }),

  setSidecar: (status) => set({ sidecar: status }),

  setDownloadDir: (dir) => {
    set({ downloadDir: dir });
    persist(get());
    void pushToBackend("downloadDir", dir);
  },

  setMaxConcurrent: (n) => {
    const value = Math.min(6, Math.max(1, n));
    set({ maxConcurrent: value });
    persist(get());
    void pushToBackend("maxConcurrent", value);
  },

  setChunkConcurrency: (n) => {
    const value = Math.min(8, Math.max(1, n));
    set({ chunkConcurrency: value });
    persist(get());
    void pushToBackend("chunkConcurrency", value);
  },

  setPerHostConcurrency: (n) => {
    const value = Math.min(6, Math.max(1, n));
    set({ perHostConcurrency: value });
    persist(get());
    void pushToBackend("perHostConcurrency", value);
  },

  setCookiesSource: (source) => {
    set({ cookiesSource: source });
    persist(get());
    void pushToBackend("cookiesSource", source);
  },

  setReducedMotion: (v) => {
    set({ reducedMotion: v });
    persist(get());
  },

  setHighContrast: (v) => {
    set({ highContrast: v });
    persist(get());
  },

  setGlassIntensity: (n) => {
    set({ glassIntensity: Math.min(1.6, Math.max(0.4, n)) });
    persist(get());
  },

  reset: () => {
    set({ ...defaults });
    const { tier, reason } = resolveTier({ tierChoice: defaults.tierChoice, profile: get().profile });
    set({ effectiveTier: tier, effectiveReason: reason });
    persist(get());
  },
}));

export { DEFAULT_DOWNLOAD_DIR };