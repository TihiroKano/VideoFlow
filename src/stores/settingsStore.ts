/** 应用设置：渲染档位、下载、网络出口、外观、辅助功能。本地持久化，不上传。 */

import { create } from "zustand";
import type { DeviceProfile, GlassTier } from "@/services/deviceProfile";
import {
  networkDiagnostics,
  setSetting,
  type NetworkDiagnostics,
  type SidecarStatus,
} from "@/services/ipc";
import {
  composeProxyUrl,
  type ProxyMode,
  type ProxyType,
} from "@/features/settings/proxyConfig";

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
  /** 网络出口：direct（直连）/ system（跟随系统代理）/ custom（自定义代理） */
  proxyMode: ProxyMode;
  /** 自定义代理类型（值即 scheme）：http | https | socks5 | socks5h */
  proxyType: ProxyType;
  /** 自定义代理的 host:port */
  proxyHost: string;
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
  // 默认跟随系统代理：这与 reqwest / yt-dlp 原本的行为一致，不能默认直连
  proxyMode: "system",
  proxyType: "http",
  proxyHost: "",
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
  /**
   * 网络诊断结果（本机 DNS / 直连 / 系统代理 / 自定义代理）。
   *
   * 启动时跑一次并缓存，用户在设置页点「重新检测」再跑；最坏 5 秒出头，
   * 所以不放进首屏关键路径。
   */
  network: NetworkDiagnostics | null;
  networkProbing: boolean;
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
  setProxyMode: (mode: ProxyMode) => void;
  setProxyType: (type: ProxyType) => void;
  setProxyHost: (host: string) => void;
  /** 重新跑网络诊断；返回结果供调用方决定要不要提示 */
  refreshNetwork: () => Promise<NetworkDiagnostics | null>;
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
    proxyMode: state.proxyMode,
    proxyType: state.proxyType,
    proxyHost: state.proxyHost,
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

/**
 * 推送自定义代理地址。
 *
 * 地址非法时后端会拒绝（SETTING_INVALID），这里把空串推过去表示「暂时没有可用地址」——
 * 用户在输入框里打字的过程中不该被报错打断，保存的仍是他最后填对的地址。
 */
async function pushProxyUrl(state: { proxyType: ProxyType; proxyHost: string }): Promise<void> {
  const url = composeProxyUrl({ type: state.proxyType, host: state.proxyHost });
  if (!url) return;
  await pushToBackend("proxyUrl", url);
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
    // 网络出口：模式与自定义地址要一起推，否则中途会出现「custom 但地址还是旧的」
    pushToBackend("proxyMode", s.proxyMode),
    pushToBackend("proxyUrl", composeProxyUrl({ type: s.proxyType, host: s.proxyHost })),
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
  network: null,
  networkProbing: false,
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

  // ---- 网络出口 ----

  setProxyMode: (mode) => {
    set({ proxyMode: mode });
    persist(get());
    void pushToBackend("proxyMode", mode);
    // 出口变了，诊断结果立刻作废：让设置页显示「检测中」而不是旧结论
    set({ network: null });
  },

  setProxyType: (type) => {
    set({ proxyType: type });
    persist(get());
    void pushProxyUrl(get());
    set({ network: null });
  },

  setProxyHost: (host) => {
    set({ proxyHost: host });
    persist(get());
    void pushProxyUrl(get());
    set({ network: null });
  },

  refreshNetwork: async () => {
    set({ networkProbing: true });
    try {
      const result = await networkDiagnostics();
      set({ network: result });
      return result;
    } catch {
      // 纯浏览器预览或后台未就绪：保持未知状态，界面显示「未检测」
      set({ network: null });
      return null;
    } finally {
      set({ networkProbing: false });
    }
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