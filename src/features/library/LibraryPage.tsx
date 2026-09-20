/**
 * 文件库页面：下载记录 / 转换记录（项目书 §2.2）。
 * 只管理 VideoFlow 已知文件；删除必须显示确切路径并二次确认，默认移入系统回收站。
 */

import { useMemo, useState } from "react";
import { GlassSurface } from "@/components/glass/GlassSurface";
import {
  IconCheck,
  IconExternal,
  IconFolder,
  IconRefresh,
  IconSearch,
  IconTrash,
  IconWarning,
} from "@/components/icons";
import {
  deleteFile,
  describeError,
  fileGone,
  formatBytes,
  isTerminal,
  openFile,
  revealFile,
  statusLabel,
} from "@/services/ipc";
import type { DownloadTask } from "@/services/types";
import { useTaskStore } from "@/stores/taskStore";
import { useUiStore } from "@/stores/uiStore";

export interface LibraryPageProps {
  kind: "download" | "convert";
}

function formatDate(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "--";
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export function LibraryPage({ kind }: LibraryPageProps): React.JSX.Element {
  const tasks = useTaskStore((s) => s.tasks);
  const remove = useTaskStore((s) => s.remove);
  const pushToast = useUiStore((s) => s.pushToast);
  const requestConfirm = useUiStore((s) => s.requestConfirm);
  const [query, setQuery] = useState("");

  const rows = useMemo(() => {
    const filtered = tasks.filter((t) => t.kind === kind && isTerminal(t.status));
    const q = query.trim().toLowerCase();
    const searched = q ? filtered.filter((t) => t.title.toLowerCase().includes(q)) : filtered;
    return [...searched].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  }, [tasks, kind, query]);

  const title = kind === "download" ? "下载记录" : "转换记录";

  const guardPath = (task: DownloadTask): string | null => {
    if (!task.targetPath) {
      pushToast({
        level: "warning",
        message: "这条记录没有关联的本地文件",
        detail: "可能文件在外部被移动或删除。",
        persistent: true,
      });
      return null;
    }
    return task.targetPath;
  };

  return (
    <GlassSurface variant="panel" className="vf-library" id="glass-library" tint={0.5} opacity={0.88}>
      <header className="vf-library__header">
        <h1 className="vf-library__title">{title}</h1>
        <div className="vf-library__search">
          <IconSearch size={17} />
          <input
            type="search"
            className="vf-library__search-input"
            placeholder="搜索名称"
            value={query}
            aria-label={`搜索${title}`}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
      </header>

      {rows.length === 0 ? (
        <p className="vf-library__empty">
          {query ? "没有匹配的记录" : `还没有${title}。完成的任务会出现在这里。`}
        </p>
      ) : (
        <div className="vf-library__body vf-scroll">
          <table className="vf-library__table">
            <thead>
              <tr>
                <th scope="col">名称</th>
                <th scope="col">规格</th>
                <th scope="col">大小</th>
                <th scope="col">完成时间</th>
                <th scope="col">状态</th>
                <th scope="col" className="vf-library__th-actions">
                  操作
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((task) => {
                // 已删除的任务：文件在回收站里，打开/定位必然失败，直接禁用
                const gone = fileGone(task.status);
                return (
                <tr key={task.id}>
                  <td className="vf-library__name" title={task.title}>
                    <span className="vf-truncate">{task.title}</span>
                    {gone ? (
                      <span className="vf-library__path vf-library__path--missing">
                        <IconWarning size={13} /> 文件已移入回收站
                      </span>
                    ) : task.targetPath ? (
                      <span className="vf-library__path vf-truncate" title={task.targetPath}>
                        {task.targetPath}
                      </span>
                    ) : (
                      <span className="vf-library__path vf-library__path--missing">
                        <IconWarning size={13} /> 文件已在外部移动
                      </span>
                    )}
                  </td>
                  <td>
                    {[task.qualityLabel, task.container?.toUpperCase()].filter(Boolean).join(" · ") || "—"}
                  </td>
                  <td>{formatBytes(task.totalBytes ?? task.downloadedBytes)}</td>
                  <td>{formatDate(task.updatedAt)}</td>
                  <td>
                    <span className="vf-library__status" data-status={task.status}>
                      {statusLabel(task.status)}
                    </span>
                  </td>
                  <td>
                    <div className="vf-library__actions">
                      <button
                        type="button"
                        className="vf-btn-icon"
                        title="打开文件"
                        aria-label="打开文件"
                        disabled={!task.targetPath || gone}
                        onClick={() => {
                          const p = guardPath(task);
                          if (p) void openFile(p);
                        }}
                      >
                        <IconCheck size={17} />
                      </button>
                      <button
                        type="button"
                        className="vf-btn-icon"
                        title="定位目录"
                        aria-label="定位目录"
                        disabled={!task.targetPath || gone}
                        onClick={() => {
                          const p = guardPath(task);
                          if (p) void revealFile(p);
                        }}
                      >
                        <IconFolder size={17} />
                      </button>
                      <button
                        type="button"
                        className="vf-btn-icon"
                        title={gone ? "移出记录" : "删除记录"}
                        aria-label={gone ? "移出记录" : "删除记录"}
                        onClick={() => {
                          requestConfirm({
                            title: gone ? "移出这条记录？" : "删除这条记录？",
                            message: gone
                              ? "文件已经在系统回收站里，这里只移除列表中的记录。"
                              : `记录将从列表中移除。${
                                  task.targetPath
                                    ? `磁盘文件「${task.targetPath}」会被移入系统回收站。`
                                    : "该记录没有关联的本地文件。"
                                }`,
                            confirmText: gone ? "移出记录" : "删除记录",
                            tone: "danger",
                            requireExplicitClick: true,
                            onConfirm: async () => {
                              try {
                                // 已删除的任务不再重复删文件（免得真把回收站里的东西再删一次）
                                if (!gone && task.targetPath) await deleteFile(task.targetPath);
                                remove(task.id);
                                pushToast({
                                  level: "success",
                                  message: gone ? "记录已移出" : "记录已删除",
                                  persistent: false,
                                });
                              } catch (err) {
                                pushToast({
                                  level: "error",
                                  message: "删除失败",
                                  detail: describeError(err),
                                  persistent: true,
                                });
                              }
                            },
                          });
                        }}
                      >
                        <IconTrash size={17} />
                      </button>
                    </div>
                  </td>
                </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      <footer className="vf-library__footer">
        <span>共 {rows.length} 条记录</span>
        <span className="vf-library__hint">
          <IconExternal size={14} /> 找不到文件时会标记「已在外部移动」，记录不会被自动清除
        </span>
        <span className="vf-library__hint">
          <IconRefresh size={14} /> 删除默认移入系统回收站
        </span>
      </footer>
    </GlassSurface>
  );
}