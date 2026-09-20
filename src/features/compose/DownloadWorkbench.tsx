/**
 * 视频下载工作台：URL 胶囊 → 解析结果卡 → 规格选择 → 开始下载。
 *
 * UI 只负责展示与收集意图，解析与下载全部由主进程执行（项目书 §1.2）。
 */

import { useState } from "react";
import { GlassSurface } from "@/components/glass/GlassSurface";
import { IconDownloadTray } from "@/components/icons";
import { MediaPreviewCard, PreviewSurface } from "./MediaPreviewCard";
import { QualityPicker, pickDefaultSelection } from "./QualityPicker";
import { UrlComposer } from "./UrlComposer";
import { BackendUnavailableError, createTask, describeError, resolveUrls } from "@/services/ipc";
import { useComposeStore } from "@/stores/composeStore";
import { useHistoryStore } from "@/stores/historyStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { useTaskStore } from "@/stores/taskStore";
import { useUiStore } from "@/stores/uiStore";

export function DownloadWorkbench(): React.JSX.Element {
  const compose = useComposeStore();
  const [parsing, setParsing] = useState(false);
  const [starting, setStarting] = useState(false);

  const downloadDir = useSettingsStore((s) => s.downloadDir);
  const upsertTask = useTaskStore((s) => s.upsert);
  const pushToast = useUiStore((s) => s.pushToast);

  const handleParse = async (normalized: string) => {
    setParsing(true);
    compose.beginParse();

    try {
      const outcomes = await resolveUrls([normalized]);
      const first = outcomes[0];
      if (!first) {
        compose.fail("解析没有返回结果，请稍后重试");
        return;
      }
      if (first.ok && first.media) {
        compose.succeed(first.media, pickDefaultSelection(first.media));
        // 解析成功才记历史：失败或粘错的链接不该污染这份列表
        void useHistoryStore.getState().record({
          url: normalized,
          title: first.media.title,
          sourceName: first.media.sourceName,
        });
        return;
      }
      const failure = first.failure;
      compose.fail(failure?.message ?? "该链接暂不支持");
      if (failure?.hint) {
        pushToast({
          level: failure.retryable ? "warning" : "warning",
          message: failure.message,
          detail: failure.hint,
          persistent: true,
        });
      }
    } catch (err) {
      if (err instanceof BackendUnavailableError) {
        compose.fail("未连接到后台服务");
        pushToast({
          level: "warning",
          message: "未连接到 VideoFlow 后台服务",
          detail: "当前是纯浏览器预览。请启动 VideoFlow 应用窗口。",
          persistent: true,
        });
      } else {
        compose.fail(describeError(err));
      }
    } finally {
      setParsing(false);
    }
  };

  const handleStart = async () => {
    const media = compose.media;
    const selection = compose.selection;
    if (!media || !selection) return;

    setStarting(true);
    try {
      const task = await createTask({
        resolvedId: media.resolvedId,
        selection,
        targetDir: downloadDir,
        title: media.title,
      });
      upsertTask(task);
      pushToast({
        level: "success",
        message: `已加入下载队列：${task.title}`,
        detail: `保存到 ${downloadDir}`,
        persistent: false,
      });
      compose.setUrl("");
      compose.reset();
    } catch (err) {
      if (err instanceof BackendUnavailableError) {
        pushToast({
          level: "warning",
          message: "未连接到 VideoFlow 后台服务",
          detail: "请在 VideoFlow 应用窗口中使用。",
          persistent: true,
        });
      } else {
        pushToast({
          level: "error",
          message: "创建下载任务失败",
          detail: describeError(err),
          persistent: true,
        });
      }
    } finally {
      setStarting(false);
    }
  };

  const canStart = Boolean(compose.media && compose.selection) && compose.previewState === "success" && !starting;

  return (
    <GlassSurface variant="panel" className="vf-workbench" id="glass-workbench" tint={0.5} opacity={0.88}>
      <UrlComposer
        value={compose.url}
        onChange={compose.setUrl}
        onSubmit={handleParse}
        onCancel={() => {
          setParsing(false);
          compose.reset();
        }}
        parsing={parsing}
        externalError={compose.previewState === "error" ? compose.error : null}
      />

      {compose.previewState !== "idle" ? (
        <>
          <div className="vf-workbench__divider" />
          <PreviewSurface>
            <MediaPreviewCard
              state={compose.previewState}
              media={compose.media}
              errorMessage={compose.error}
            />
          </PreviewSurface>
        </>
      ) : null}

      {compose.previewState === "success" && compose.media ? (
        <QualityPicker
          media={compose.media}
          selection={compose.selection}
          onChange={compose.setSelection}
          disabled={starting}
        />
      ) : null}

      <div className="vf-workbench__actions">
        <button
          type="button"
          className="vf-btn-primary vf-workbench__start"
          disabled={!canStart}
          onClick={handleStart}
        >
          <IconDownloadTray size={20} />
          <span>开始下载</span>
        </button>
      </div>
    </GlassSurface>
  );
}