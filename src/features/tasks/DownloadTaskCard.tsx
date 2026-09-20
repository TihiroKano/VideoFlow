/**
 * 单个任务卡（项目书 §2.3 DownloadTaskCard）。
 * 进度、速度、剩余时间、原因和下一步始终可见。
 */

import { memo, useMemo, useState } from "react";
import {
  IconArrowUp,
  IconCheck,
  IconChevronDown,
  IconChevronRight,
  IconClose,
  IconFolder,
  IconPause,
  IconPlay,
  IconRefresh,
  IconTrash,
} from "@/components/icons";
import {
  deleteTaskFile,
  describeError,
  engineLabel,
  formatBytes,
  formatEta,
  formatSpeed,
  isTerminal,
  openFile,
  reparseTask,
  revealFile,
  statusLabel,
  statusLabelFor,
  taskAction,
} from "@/services/ipc";
import type { DownloadTask } from "@/services/types";
import { useTaskStore } from "@/stores/taskStore";
import { useUiStore } from "@/stores/uiStore";

interface ActionSpec {
  key: string;
  label: string;
  icon: React.ReactNode;
  tone?: "danger" | "active";
  run: () => void | Promise<void>;
}

function shareLabel(task: DownloadTask): string {
  const parts: string[] = [];
  if (task.qualityLabel) parts.push(task.qualityLabel);
  if (task.container) parts.push(task.container.toUpperCase());
  if (task.totalBytes) parts.push(formatBytes(task.totalBytes));
  return parts.join(" · ") || "规格待确认";
}

function TaskActions({
  task,
  leading,
}: {
  task: DownloadTask;
  /** 左侧附加按钮（详情开关），与动作按钮同处一个网格单元 */
  leading?: React.ReactNode;
}): React.JSX.Element {
  const upsert = useTaskStore((s) => s.upsert);
  const remove = useTaskStore((s) => s.remove);
  const pushToast = useUiStore((s) => s.pushToast);
  const requestConfirm = useUiStore((s) => s.requestConfirm);

  const runAction = async (action: "pause" | "resume" | "retry") => {
    try {
      const updated = await taskAction(task.id, action);
      upsert(updated);
    } catch (err) {
      pushToast({
        level: "error",
        message: `${statusLabel(task.status)} 状态下操作失败`,
        detail: describeError(err),
        persistent: true,
      });
    }
  };

  /** 重新解析：换一份新的媒体地址后重新排队（项目书 §3.1 第 4 条） */
  const runReparse = async () => {
    try {
      upsert(await reparseTask(task.id));
      pushToast({ level: "info", message: "已重新解析，任务已重新排队", persistent: false });
    } catch (err) {
      pushToast({
        level: "error",
        message: "重新解析失败",
        detail: describeError(err),
        persistent: true,
      });
    }
  };

  /** 手动置顶 / 取消置顶（项目书 §3.2） */
  const runPriority = async () => {
    try {
      upsert(await taskAction(task.id, task.priority > 0 ? "unprioritize" : "prioritize"));
    } catch (err) {
      pushToast({
        level: "error",
        message: "调整队列优先级失败",
        detail: describeError(err),
        persistent: true,
      });
    }
  };

  const actions = useMemo<ActionSpec[]>(() => {
    const list: ActionSpec[] = [];

    if (task.status === "completed") {
      list.push({
        key: "open",
        label: "打开文件",
        icon: <IconCheck size={19} />,
        run: async () => {
          if (!task.targetPath) {
            pushToast({ level: "warning", message: "该任务没有可打开的文件路径", persistent: true });
            return;
          }
          await openFile(task.targetPath);
        },
      });
      list.push({
        key: "reveal",
        label: "在文件夹中显示",
        icon: <IconFolder size={19} />,
        run: async () => {
          if (!task.targetPath) {
            pushToast({ level: "warning", message: "该任务没有可定位的文件路径", persistent: true });
            return;
          }
          await revealFile(task.targetPath);
        },
      });
      // 直接删除磁盘上的视频文件：默认移入系统回收站，并显示确切路径供二次确认
      list.push({
        key: "delete",
        label: "删除文件",
        icon: <IconTrash size={19} />,
        tone: "danger",
        run: () => {
          const target = task.targetPath;
          if (!target) {
            pushToast({ level: "warning", message: "该任务没有可删除的文件路径", persistent: true });
            return;
          }
          requestConfirm({
            title: "删除这个视频文件？",
            message: `文件将被移入系统回收站：\n${target}`,
            confirmText: "删除文件",
            tone: "danger",
            requireExplicitClick: true,
            onConfirm: async () => {
              try {
                // 由主进程同时删文件与改状态：界面上的「已完成」随后会变成「已删除」
                upsert(await deleteTaskFile(task.id));
                pushToast({ level: "success", message: "文件已移入回收站", persistent: false });
              } catch (err) {
                pushToast({
                  level: "error",
                  message: "删除文件失败",
                  detail: describeError(err),
                  persistent: true,
                });
              }
            },
          });
        },
      });
      return list;
    }

    if (task.status === "deleted") {
      // 文件已经在回收站，这里只剩「移出列表」
      list.push({
        key: "forget",
        label: "移出列表",
        icon: <IconClose size={19} />,
        tone: "danger",
        run: () =>
          requestConfirm({
            title: "移出这条记录？",
            message: "只移除列表里的记录，回收站中的文件不受影响。",
            confirmText: "移出列表",
            tone: "default",
            onConfirm: () => remove(task.id),
          }),
      });
      return list;
    }

    if (task.status === "downloading" || task.status === "merging" || task.status === "verifying") {
      list.push({
        key: "pause",
        label: "暂停",
        icon: <IconPause size={19} />,
        run: () => runAction("pause"),
      });
    } else if (task.status === "paused") {
      list.push({
        key: "resume",
        label: "继续",
        icon: <IconPlay size={19} />,
        run: () => runAction("resume"),
      });
    } else if (task.status === "queued" || task.status === "retry_wait") {
      list.push({
        key: "start",
        label: "立即开始",
        icon: <IconPlay size={19} />,
        run: () => runAction("resume"),
      });
    } else if (task.status === "failed") {
      list.push({
        key: "retry",
        label: "重试",
        icon: <IconRefresh size={19} />,
        run: () => runAction("retry"),
      });
    } else if (task.status === "needs_reparse") {
      // 解析过期 / 403 / 404：只有重新解析才能拿到新的媒体地址
      list.push({
        key: "reparse",
        label: "重新解析",
        icon: <IconRefresh size={19} />,
        run: runReparse,
      });
    } else if (task.status === "cancelled") {
      list.push({
        key: "retry",
        label: "重新下载",
        icon: <IconRefresh size={19} />,
        run: () => runAction("retry"),
      });
    }

    // 手动置顶：只对还没结束的任务有意义，运行中的任务不会被抢占
    if (!isTerminal(task.status)) {
      const pinned = task.priority > 0;
      list.push({
        key: "priority",
        label: pinned ? "取消置顶" : "置顶",
        icon: <IconArrowUp size={19} />,
        tone: pinned ? "active" : undefined,
        run: runPriority,
      });
    }

    // 取消：只删除临时分片，不触碰已完成文件（已完成任务在本函数开头已返回）
    list.push({
        key: "cancel",
        label: task.status === "cancelled" ? "移除记录" : "取消下载",
        icon: <IconClose size={19} />,
        tone: "danger",
        run: () => {
          if (task.status === "cancelled") {
            requestConfirm({
              title: "移除这条记录？",
              message: "只移除列表中的记录，磁盘上已下载的文件不会被删除。",
              confirmText: "移除记录",
              tone: "default",
              onConfirm: () => remove(task.id),
            });
            return;
          }
          requestConfirm({
            title: "取消这个下载？",
            message: `将删除「${task.title}」的临时分片与断点数据。已经完成下载的文件不会被删除。`,
            confirmText: "取消下载",
            tone: "danger",
            onConfirm: async () => {
              try {
                await taskAction(task.id, "cancel");
              } catch (err) {
                pushToast({
                  level: "error",
                  message: "取消下载失败",
                  detail: describeError(err),
                  persistent: true,
                });
              }
            },
          });
        },
    });

    return list;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [task.status, task.id, task.title, task.targetPath, task.priority]);

  return (
    <div className="vf-task__actions">
      {leading}
      {actions.map((a) => (
        <button
          key={a.key}
          type="button"
          className="vf-btn-icon"
          data-tone={a.tone}
          title={a.label}
          aria-label={a.label}
          onClick={() => void a.run()}
        >
          {a.icon}
        </button>
      ))}
    </div>
  );
}

function TaskCardInner({ task }: { task: DownloadTask }): React.JSX.Element {
  const [detailOpen, setDetailOpen] = useState(false);

  // 进度只从 task 读：进度事件写的就是任务对象本身，避免两份数据走偏
  const downloaded = task.downloadedBytes;
  const total = task.totalBytes ?? null;
  const speed = task.speedBps;
  const eta = task.etaSec ?? null;

  const known = typeof total === "number" && total > 0;
  const ratio = known ? Math.min(1, downloaded / total) : 0;
  const percent = known ? Math.round(ratio * 100) : null;

  const isDone = task.status === "completed";
  const showBar = known || isDone || task.status === "deleted";
  const barRatio = isDone && !known ? 1 : ratio;

  const statusText = useMemo(() => {
    if (percent !== null && task.status === "downloading") return `${percent}%`;
    // 暂停时明确告诉用户「已经存下多少、还能接着下」（项目书 §3.4 操作表）。
    // 但转码不能断点续传（FFmpeg 单次跑完），说「可恢复」是骗人的——
    // 恢复时其实是从头再来，如实说明。
    if ((task.status === "paused" || task.status === "pausing") && downloaded > 0) {
      if (task.kind === "convert") {
        return `${statusLabelFor(task.status, task.kind)} · 继续将从头开始`;
      }
      return `${statusLabel(task.status)} · 可恢复 ${formatBytes(downloaded)}`;
    }
    return statusLabelFor(task.status, task.kind);
  }, [percent, task.status, task.kind, downloaded]);

  /** 下载 / 网络详情：把主进程真正知道的值摊开，不做猜测 */
  const detailToggle = (
    <button
      type="button"
      className="vf-btn-icon"
      data-tone={detailOpen ? "active" : undefined}
      title={detailOpen ? "收起下载详情" : "下载详情"}
      aria-label={detailOpen ? "收起下载详情" : "展开下载详情"}
      aria-expanded={detailOpen}
      onClick={() => setDetailOpen((v) => !v)}
    >
      {detailOpen ? <IconChevronDown size={19} /> : <IconChevronRight size={19} />}
    </button>
  );

  return (
    <article className="vf-task" data-status={task.status}>
      <div className="vf-task__thumb">
        {task.thumbnailUrl ? (
          <img
            src={task.thumbnailUrl}
            alt=""
            loading="lazy"
            /* 站点图床（如 Bilibili 的 hdslb.com）有防盗链：带本应用的 Referer 会 403，
               必须显式声明不发 Referer 才取得到 */
            referrerPolicy="no-referrer"
            className="vf-task__thumb-img"
            onError={(e) => {
              // 取不到封面就退回色块，不要留一个破图图标
              e.currentTarget.style.display = "none";
            }}
          />
        ) : (
          <div className="vf-task__thumb-fallback" aria-hidden="true" />
        )}
      </div>

      <div className="vf-task__text">
        <h3 className="vf-task__title vf-truncate" title={task.title}>
          {task.title}
        </h3>
        <p className="vf-task__share">{shareLabel(task)}</p>
      </div>

      <div className="vf-task__progress">
        <div className="vf-task__progress-head">
          <span className="vf-task__status">{statusText}</span>
          {task.priority > 0 ? <span className="vf-task__pin">已置顶</span> : null}
        </div>
        <div
          className="vf-task__bar"
          role="progressbar"
          aria-label={`${task.title} 下载进度`}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={percent ?? undefined}
          aria-valuetext={known ? undefined : statusLabel(task.status)}
        >
          <div
            className="vf-task__bar-fill"
            style={{ width: `${(showBar ? barRatio : 0) * 100}%` }}
            data-indeterminate={known ? "false" : "true"}
          />
        </div>
      </div>

      <div className="vf-task__speed">
        {task.status === "deleted" ? null : (
          <>
            <span className="vf-task__speed-value">{formatSpeed(speed)}</span>
            <span className="vf-task__eta">{formatEta(eta)}</span>
          </>
        )}
      </div>

      <TaskActions task={task} leading={detailToggle} />

      {detailOpen ? (
        <dl className="vf-task__detail">
          <div className="vf-task__detail-row">
            <dt>执行引擎</dt>
            <dd>{engineLabel(task.engine)}</dd>
          </div>
          <div className="vf-task__detail-row">
            <dt>已接收 / 总量</dt>
            <dd className="vf-task__detail-num">
              {formatBytes(downloaded)}
              {known ? ` / ${formatBytes(total)}` : " / 待确认"}
              {percent !== null ? `（${percent}%）` : ""}
            </dd>
          </div>
          <div className="vf-task__detail-row">
            <dt>实时速度</dt>
            <dd className="vf-task__detail-num">{formatSpeed(speed) || "—"}</dd>
          </div>
          <div className="vf-task__detail-row">
            <dt>剩余时间</dt>
            <dd className="vf-task__detail-num">{formatEta(eta) || "—"}</dd>
          </div>
          <div className="vf-task__detail-row">
            <dt>状态 / 重试</dt>
            <dd>
              {statusLabelFor(task.status, task.kind)}
              {task.retryCount > 0 ? ` · 已重试 ${task.retryCount} 次` : ""}
            </dd>
          </div>
          <div className="vf-task__detail-row">
            <dt>队列优先级</dt>
            <dd>{task.priority > 0 ? "已置顶" : "普通"}</dd>
          </div>
          <div className="vf-task__detail-row">
            <dt>保存位置</dt>
            <dd className="vf-truncate" title={task.targetPath ?? undefined}>
              {task.targetPath ?? "—"}
            </dd>
          </div>
          <div className="vf-task__detail-row">
            <dt>来源链接</dt>
            <dd className="vf-truncate" title={task.sourceUrl}>
              {task.sourceUrl}
            </dd>
          </div>
          {task.error ? (
            <div className="vf-task__detail-row">
              <dt>错误</dt>
              <dd>
                [{task.error.code}] {task.error.message}
                {task.error.detail ? ` · ${task.error.detail}` : ""}
              </dd>
            </div>
          ) : null}
        </dl>
      ) : null}

      {task.error ? (
        <p className="vf-task__error" role="status">
          {task.error.message}
          {task.error.retryable ? "（可重试）" : ""}
        </p>
      ) : null}
    </article>
  );
}

export const DownloadTaskCard = memo(TaskCardInner);