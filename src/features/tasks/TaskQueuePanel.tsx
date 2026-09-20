/**
 * 下载队列面板（项目书 §2.2「活跃任务」）。
 * 顶部给出全局速度、剩余任务与全局暂停；逐任务展示进度与动作。
 */

import { useMemo } from "react";
import { GlassSurface } from "@/components/glass/GlassSurface";
import { IconChevronRight, IconDownloadTray, IconPause, IconPlay } from "@/components/icons";
import { DownloadTaskCard } from "./DownloadTaskCard";
import { formatSpeed, isTerminal, taskAction } from "@/services/ipc";
import type { DownloadTask } from "@/services/types";
import { useTaskStore } from "@/stores/taskStore";
import { useUiStore } from "@/stores/uiStore";

function GlobalPauseButton({ tasks }: { tasks: DownloadTask[] }): React.JSX.Element | null {
  const pushToast = useUiStore((s) => s.pushToast);
  const upsertTask = useTaskStore((s) => s.upsert);

  const running = tasks.filter((t) => t.status === "downloading" || t.status === "queued");
  const paused = tasks.filter((t) => t.status === "paused");

  if (running.length === 0 && paused.length === 0) return null;

  const allPausedOrIdle = running.length === 0;
  const label = allPausedOrIdle ? "全部继续" : "全部暂停";

  const onClick = async () => {
    const targets = allPausedOrIdle ? paused : running;
    const action = allPausedOrIdle ? "resume" : "pause";
    const results = await Promise.allSettled(targets.map((t) => taskAction(t.id, action)));
    for (const r of results) {
      if (r.status === "fulfilled") upsertTask(r.value);
    }
    const failed = results.filter((r) => r.status === "rejected").length;
    if (failed > 0) {
      pushToast({
        level: "warning",
        message: `${failed} 个任务未能${allPausedOrIdle ? "继续" : "暂停"}`,
        persistent: true,
      });
    }
  };

  return (
    <button type="button" className="vf-btn-ghost vf-queue__global" onClick={() => void onClick()}>
      {allPausedOrIdle ? <IconPlay size={16} /> : <IconPause size={16} />}
      <span>{label}</span>
    </button>
  );
}

export function TaskQueuePanel(): React.JSX.Element {
  const tasks = useTaskStore((s) => s.tasks);
  const queue = useTaskStore((s) => s.queue);
  const setPage = useUiStore((s) => s.setPage);
  const mode = useUiStore((s) => s.mode);

  // 面板跟随顶部模式切换：下载工作台下面是「下载任务」，转换工作台下面是「转换任务」。
  // 任务本身带 kind，按 kind 过滤，避免在转换模式下混进下载任务。
  const isConvert = mode === "convert";
  const kind = isConvert ? "convert" : "download";
  const mine = useMemo(() => tasks.filter((t) => t.kind === kind), [tasks, kind]);

  // store 已按「进行中 → 等待 → 已暂停 → 终态」排序，这里直接取前若干条。
  // 队列需要展示刚完成的任务（与预览图一致），因此不过滤终态；
  // 更早的历史记录在「下载记录」里查看。
  const visible = useMemo(() => mine.slice(0, 6), [mine]);
  const remaining = useMemo(() => mine.filter((t) => !isTerminal(t.status)).length, [mine]);

  return (
    <GlassSurface variant="panel" className="vf-queue" id="glass-queue" tint={0.44} opacity={0.84}>
      <header className="vf-queue__header">
        <div className="vf-queue__title">
          <IconDownloadTray size={19} />
          <h2>{isConvert ? "转换任务" : "下载任务"}</h2>
        </div>
        <div className="vf-queue__meta">
          {queue.totalSpeedBps > 0 ? (
            <span className="vf-queue__speed">{formatSpeed(queue.totalSpeedBps)}</span>
          ) : null}
          {remaining > 0 ? <span className="vf-queue__count">剩余 {remaining}</span> : null}
          <GlobalPauseButton tasks={mine} />
          <button
            type="button"
            className="vf-queue__all"
            onClick={() => setPage(isConvert ? "converts" : "downloads")}
          >
            <span>全部任务</span>
            <IconChevronRight size={16} />
          </button>
        </div>
      </header>

      <div className="vf-queue__body vf-scroll">
        {visible.length === 0 ? (
          <p className="vf-queue__empty">
            {isConvert
              ? "还没有转换任务。选择文件并设置输出格式后，任务会出现在这里。"
              : "还没有下载任务。粘贴链接并解析后，任务会出现在这里。"}
          </p>
        ) : (
          <ul className="vf-queue__list">
            {visible.map((task) => (
              <li key={task.id}>
                <DownloadTaskCard task={task} />
              </li>
            ))}
          </ul>
        )}
      </div>
    </GlassSurface>
  );
}