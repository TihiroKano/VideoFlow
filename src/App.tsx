/** 应用入口：装配设置、背景、设备探测与主进程事件。 */

import { useEffect, useRef } from "react";
import { AppShell } from "@/components/layout/AppShell";
import { profileDevice } from "@/services/deviceProfile";
import { getSnapshot, isTauri, isTerminal, subscribeEvents } from "@/services/ipc";
import type { DownloadTask } from "@/services/types";
import { useSettingsStore, syncSettingsToBackend } from "@/stores/settingsStore";
import { useTaskStore } from "@/stores/taskStore";
import { useUiStore } from "@/stores/uiStore";
import { useComposeStore } from "@/stores/composeStore";
import { useHistoryStore } from "@/stores/historyStore";
import { loadPreviewSeed, seedRequested } from "@/features/dev/devSeed";

/** 有任务在跑时的对账间隔：够快让进度条即使丢了事件也能自愈 */
const RECONCILE_ACTIVE_MS = 1000;
/** 全部任务都停了之后放慢，避免无谓的轮询 */
const RECONCILE_IDLE_MS = 5000;

/**
 * 任务列表的稳定签名。
 *
 * 包含 `downloadedBytes`：进度事件若中断，这一项会与后端不一致，对账随即把界面拉回
 * 正确值——「进度条卡死、点一下暂停才跳到当前值」正是由此兜住。
 */
function taskSignature(tasks: DownloadTask[]): string {
  return tasks
    .map((t) => `${t.id}:${t.status}:${t.version}:${t.downloadedBytes}`)
    .sort()
    .join("|");
}

export function App(): React.JSX.Element {
  const setProfile = useSettingsStore((s) => s.setProfile);
  const setProfiling = useSettingsStore((s) => s.setProfiling);
  const tierChoice = useSettingsStore((s) => s.tierChoice);

  const replaceAll = useTaskStore((s) => s.replaceAll);
  const upsert = useTaskStore((s) => s.upsert);
  const removeTask = useTaskStore((s) => s.remove);
  const applyProgress = useTaskStore((s) => s.applyProgress);
  const setQueue = useTaskStore((s) => s.setQueue);

  const pushToast = useUiStore((s) => s.pushToast);
  const noticedRef = useRef(false);

  // 开发期夹具：?seed=preview 时进入预览图所示的填充态
  usePreviewSeed();

  // 事件之外的对账兜底：任何一次状态事件丢失都能在 2 秒内自愈
  useTaskReconcile();

  // 首次启动：<1 秒无感能力探测，不上传任何硬件信息（项目书 §6.1）
  useEffect(() => {
    let cancelled = false;
    setProfiling(true);
    profileDevice()
      .then((profile) => {
        if (cancelled) return;
        setProfile(profile);
        if (!noticedRef.current && tierChoice === "auto") {
          noticedRef.current = true;
          const label =
            profile.recommendation === "quality"
              ? "质量"
              : profile.recommendation === "balanced"
                ? "平衡"
                : "性能";
          pushToast({
            level: "info",
            message: `已为此设备推荐「${label}」效果，可随时在设置中更改`,
            detail: profile.reasons.slice(0, 3).join(" · "),
            persistent: false,
          });
        }
      })
      .catch(() => undefined)
      .finally(() => {
        if (!cancelled) setProfiling(false);
      });
    return () => {
      cancelled = true;
    };
    // 只在启动时探测一次
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 连接主进程事件总线；纯浏览器预览时静默跳过
  useEffect(() => {
    let dispose: (() => void) | undefined;
    let cancelled = false;

    (async () => {
      // 先把本地设置同步给主进程，再拉取任务快照
      await syncSettingsToBackend();

      // 网络诊断：启动时跑一次（最坏 5 秒出头，异步不阻塞界面），结果缓存进设置页。
      // 只在「当前出口配置有问题」时提醒——直连用户不该每次启动都被唠叨，
      // 真正解析失败时后端还会给出完整指引。
      void useSettingsStore
        .getState()
        .refreshNetwork()
        .then((diagnostics) => {
          if (!diagnostics || diagnostics.proxyOk) return;
          pushToast({
            level: "warning",
            message: diagnostics.verdict,
            detail: "可在「设置 → 网络与代理」里重新检测或切换出口",
            persistent: false,
          });
        })
        .catch(() => undefined);

      // 链接历史：一次会话拉一次即可，之后由 store 自己维护
      void useHistoryStore.getState().refresh();

      const seeded = seedRequested();

      try {
        const snapshot = await getSnapshot(0);
        if (cancelled) return;
        // 使用夹具做视觉比对时不覆盖，保证截图状态稳定
        if (!seeded) {
          replaceAll(snapshot.tasks);
          setQueue(snapshot.queue);
        }
      } catch {
        // 后台未就绪（例如纯浏览器预览），保持空状态
      }

      try {
        dispose = await subscribeEvents({
          onTaskUpdated: upsert,
          onTaskRemoved: removeTask,
          onTaskProgress: applyProgress,
          onQueueUpdated: setQueue,
          onNotice: (notice) => {
            pushToast({
              level: notice.level,
              message: notice.message,
              taskId: notice.taskId,
              persistent: notice.level === "error" || notice.level === "warning",
            });
          },
        });
      } catch {
        // 事件订阅失败不影响界面可用
      }
    })();

    return () => {
      cancelled = true;
      dispose?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return <AppShell />;
}

/**
 * 低频对账：保证界面最终一定收敛到主进程状态。
 *
 * 事件是主通路，但只要任何一次状态变更的事件丢失，界面就会永久停在旧状态——
 * 用户看到的就是「进度卡死，直到手动点一次暂停 / 继续才更新」，因为命令的返回值
 * 恰好是最新状态。这里周期性比对 任务id:状态:版本 的签名，发现不一致就整体收敛。
 * 项目书 §4.2 明确「后端是状态真相源，前端只做乐观展示」，这层对账正是该原则的兜底。
 */
function useTaskReconcile(): void {
  const replaceAll = useTaskStore((s) => s.replaceAll);
  const setQueue = useTaskStore((s) => s.setQueue);

  useEffect(() => {
    if (!isTauri()) return;
    // 预览夹具态下不能对账：夹具是纯前端的假任务，主进程里并没有它们，
    // 一比对就会把夹具整个清空（界面变成空队列）。
    if (seedRequested()) return;
    let stopped = false;
    let timer = 0;

    const tick = async () => {
      if (stopped) return;
      const before = taskSignature(useTaskStore.getState().tasks);
      try {
        const snapshot = await getSnapshot(0);
        if (stopped) return;
        // 签名一致说明事件没丢，什么都不做，避免无谓地重建列表
        if (taskSignature(snapshot.tasks) !== before) {
          replaceAll(snapshot.tasks);
          setQueue(snapshot.queue);
        }
      } catch {
        // 后台未就绪（纯浏览器预览），忽略
      }
      if (stopped) return;
      const active = useTaskStore.getState().tasks.some((t) => !isTerminal(t.status));
      timer = window.setTimeout(() => void tick(), active ? RECONCILE_ACTIVE_MS : RECONCILE_IDLE_MS);
    };

    timer = window.setTimeout(() => void tick(), RECONCILE_ACTIVE_MS);

    return () => {
      stopped = true;
      window.clearTimeout(timer);
    };
  }, [replaceAll, setQueue]);
}

/** 开发期：?seed=preview 时把应用置于预览图所示的填充态，便于逐项比对 */
export function usePreviewSeed(): void {
  useEffect(() => {
    if (!seedRequested()) return;
    let cancelled = false;
    void (async () => {
      const seed = await loadPreviewSeed();
      if (!seed || cancelled) return;
      useTaskStore.getState().replaceAll(seed.tasks);
      useTaskStore.getState().setQueue(seed.queue);
      useComposeStore
        .getState()
        .succeed(seed.media, { streamId: seed.media.streams[0]?.id ?? "" });
      useComposeStore.getState().setUrl(seed.media.sourceUrl);
    })();
    return () => {
      cancelled = true;
    };
  }, []);
}