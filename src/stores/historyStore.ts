/**
 * 链接历史：记录解析成功过的链接，最多 100 条（上限由主进程强制）。
 *
 * 做主进程的镜像而不是本地存储：数据落在 `D:\VideoFlow\history.json`，
 * 与 settings.json / tasks.json 同盘，不占系统盘，也不随 WebView2 profile 丢失。
 * 后台未就绪（纯浏览器预览）时保持空列表，界面自行降级。
 */

import { create } from "zustand";
import {
  historyAdd,
  historyClear as clearRemote,
  historyList,
  historyRemove as removeRemote,
  type HistoryEntry,
} from "@/services/ipc";

interface HistoryState {
  entries: HistoryEntry[];
  loaded: boolean;
  /** 从主进程拉一次全量；界面启动时调用 */
  refresh: () => Promise<void>;
  /** 记一条（解析成功时调用） */
  record: (input: { url: string; title: string; sourceName: string }) => Promise<void>;
  remove: (url: string) => Promise<void>;
  clear: () => Promise<void>;
}

/** 下拉里最多展示这么多条，避免panel 被撑满 */
export const VISIBLE_LIMIT = 10;

export const useHistoryStore = create<HistoryState>((set) => ({
  entries: [],
  loaded: false,

  refresh: async () => {
    try {
      const entries = await historyList();
      set({ entries, loaded: true });
    } catch {
      // 纯浏览器预览或后台未就绪：保持空列表
      set({ loaded: true });
    }
  },

  record: async (input) => {
    try {
      set({ entries: await historyAdd(input) });
    } catch {
      // 记录失败不影响解析本身，静默忽略
    }
  },

  remove: async (url) => {
    try {
      set({ entries: await removeRemote(url) });
    } catch {
      // 同上
    }
  },

  clear: async () => {
    try {
      await clearRemote();
      set({ entries: [] });
    } catch {
      // 同上
    }
  },
}));

/** 相对时间：历史下拉里「3 分钟前」比完整时间戳更好扫读 */
export function relativeTime(iso: string, now = Date.now()): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const diff = Math.max(0, now - t);
  const min = Math.floor(diff / 60000);
  if (min < 1) return "刚刚";
  if (min < 60) return `${min} 分钟前`;
  const hour = Math.floor(min / 60);
  if (hour < 24) return `${hour} 小时前`;
  const day = Math.floor(hour / 24);
  if (day < 30) return `${day} 天前`;
  const month = Math.floor(day / 30);
  if (month < 12) return `${month} 个月前`;
  return `${Math.floor(month / 12)} 年前`;
}