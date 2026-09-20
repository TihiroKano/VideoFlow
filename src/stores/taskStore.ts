/**
 * 任务队列状态。
 *
 * 后端是状态真相源：前端只做展示与乐观中间态，收到 task.updated 后收敛（项目书 §4.2）。
 */

import { create } from "zustand";
import type { DownloadTask, QueueStats, TaskProgress } from "@/services/types";
import { isTerminal } from "@/services/ipc";

/** 进度事件最大 4 次/秒/任务，超出部分丢弃，避免高频重渲染 */
const PROGRESS_INTERVAL_MS = 250;

interface TaskState {
  tasks: DownloadTask[];
  queue: QueueStats;
  lastProgressAt: Record<string, number>;
  replaceAll: (tasks: DownloadTask[]) => void;
  upsert: (task: DownloadTask) => void;
  remove: (taskId: string) => void;
  applyProgress: (p: TaskProgress) => void;
  setQueue: (q: QueueStats) => void;
}

const EMPTY_QUEUE: QueueStats = { activeCount: 0, queuedCount: 0, totalSpeedBps: 0 };

export const useTaskStore = create<TaskState>((set, get) => ({
  tasks: [],
  queue: EMPTY_QUEUE,
  lastProgressAt: {},

  replaceAll: (tasks) => set({ tasks: [...tasks].sort(sortTasks) }),

  upsert: (task) => {
    const tasks = get().tasks.slice();
    const idx = tasks.findIndex((t) => t.id === task.id);
    if (idx >= 0) tasks[idx] = task;
    else tasks.push(task);
    tasks.sort(sortTasks);
    set({ tasks });
  },

  remove: (taskId) => {
    set({ tasks: get().tasks.filter((t) => t.id !== taskId) });
  },

  /**
   * 进度只写回任务对象本身——任务卡展示进度的**唯一**数据源就是 `task`。
   *
   * 这里曾经另存一份 `progress` 记录，而卡片优先读它。两份数据一旦不同步
   * （例如进度事件中断），卡片就会一直显示那份陈旧记录，忽略任务对象里更新的
   * 字节数，表现为「进度条卡死，点一下暂停才跳到当前值」——因为命令返回的
   * 任务对象才是新的。
   */
  applyProgress: (p) => {
    const now = Date.now();
    const last = get().lastProgressAt[p.taskId] ?? 0;
    if (now - last < PROGRESS_INTERVAL_MS) return;
    const lastProgressAt = { ...get().lastProgressAt, [p.taskId]: now };
    const tasks = get().tasks.map((t) =>
      t.id === p.taskId
        ? {
            ...t,
            downloadedBytes: p.downloadedBytes,
            totalBytes: p.totalBytes ?? t.totalBytes,
            speedBps: p.speedBps,
            etaSec: p.etaSec ?? null,
          }
        : t,
    );
    set({ tasks, lastProgressAt });
  },

  setQueue: (q) => set({ queue: q }),
}));

/** 排序：用户置顶 > 进行中 > 等待 > 终态；同组内按创建时间倒序 */
function sortTasks(a: DownloadTask, b: DownloadTask): number {
  if (a.priority !== b.priority) return b.priority - a.priority;
  const ra = rank(a);
  const rb = rank(b);
  if (ra !== rb) return ra - rb;
  return b.createdAt.localeCompare(a.createdAt);
}

function rank(t: DownloadTask): number {
  if (isTerminal(t.status)) return 3;
  if (t.status === "downloading" || t.status === "merging" || t.status === "verifying") return 0;
  if (t.status === "queued" || t.status === "retry_wait" || t.status === "pausing") return 1;
  if (t.status === "paused" || t.status === "needs_reparse") return 2;
  return 2;
}

export function activeTasks(tasks: DownloadTask[]): DownloadTask[] {
  return tasks.filter((t) => !isTerminal(t.status));
}

export function completedTasks(tasks: DownloadTask[]): DownloadTask[] {
  return tasks.filter((t) => t.status === "completed");
}