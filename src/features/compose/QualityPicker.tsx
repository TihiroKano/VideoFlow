/**
 * 规格选择（项目书 §2.3 QualityPicker）。
 * 只有可用选项才可选；展示容器、分辨率、帧率、音频与估算大小。
 */

import { useMemo } from "react";
import { GlassSelect } from "@/components/controls/GlassSelect";
import { formatBytes } from "@/services/ipc";
import type { MediaStream, ResolvedMedia, StreamSelection } from "@/services/types";

export interface QualityPickerProps {
  media: ResolvedMedia;
  selection: StreamSelection | null;
  onChange: (selection: StreamSelection) => void;
  disabled?: boolean;
}

/** 选择默认流：分辨率最高的视频流 + 其配套音频 */
export function pickDefaultSelection(media: ResolvedMedia): StreamSelection | null {
  const videos = media.streams.filter((s) => s.kind === "video");
  const pool = videos.length > 0 ? videos : media.streams;
  if (pool.length === 0) return null;

  const best = pool.reduce((acc, cur) => {
    const accPixels = (acc.width ?? 0) * (acc.height ?? 0);
    const curPixels = (cur.width ?? 0) * (cur.height ?? 0);
    return curPixels > accPixels ? cur : acc;
  });

  const audio = media.streams.find((s) => s.kind === "audio");
  return {
    streamId: best.id,
    audioStreamId: best.needsMerge ? audio?.id : undefined,
  };
}

function qualityLabel(stream: MediaStream): string {
  if (stream.qualityLabel) return stream.qualityLabel;
  if (stream.height) return `${stream.height}P`;
  if (stream.kind === "audio") return stream.audioLabel ?? "音频";
  return "默认";
}

/**
 * 编码的显示名。
 *
 * 必须让用户看见这个：抖音 4K 档只有 H.265，而 Windows 自带播放器解不了，
 * 下载下来双击打不开，很容易被当成「下载坏了」。标出来至少能自证原因。
 */
function codecLabel(codec: string | undefined): string | null {
  if (!codec) return null;
  const c = codec.toLowerCase();
  if (c.startsWith("avc") || c.startsWith("h264")) return "H.264";
  if (c.startsWith("hev") || c.startsWith("hvc") || c.startsWith("h265")) return "H.265";
  if (c.startsWith("av01")) return "AV1";
  if (c.startsWith("vp9") || c.startsWith("vp09")) return "VP9";
  return codec.toUpperCase();
}

export function QualityPicker({
  media,
  selection,
  onChange,
  disabled = false,
}: QualityPickerProps): React.JSX.Element {
  const videoStreams = useMemo(() => media.streams.filter((s) => s.kind !== "audio"), [media.streams]);
  const audioStreams = useMemo(() => media.streams.filter((s) => s.kind === "audio"), [media.streams]);

  const selectedStream = videoStreams.find((s) => s.id === selection?.streamId) ?? videoStreams[0];

  // 最高分辨率标记为推荐
  const recommendedId = useMemo(() => {
    if (videoStreams.length === 0) return undefined;
    return videoStreams.reduce((acc, cur) => {
      const accPixels = (acc.width ?? 0) * (acc.height ?? 0);
      const curPixels = (cur.width ?? 0) * (cur.height ?? 0);
      return curPixels > accPixels ? cur : acc;
    }).id;
  }, [videoStreams]);

  const containers = useMemo(() => {
    const set = new Set<string>();
    for (const s of videoStreams) set.add(s.container.toUpperCase());
    return [...set];
  }, [videoStreams]);

  const update = (patch: Partial<StreamSelection>) => {
    const base: StreamSelection = selection ?? { streamId: selectedStream?.id ?? "" };
    onChange({ ...base, ...patch });
  };

  const qualityDisplay = selectedStream
    ? `${qualityLabel(selectedStream)}${codecLabel(selectedStream.codec) ? ` · ${codecLabel(selectedStream.codec)}` : ""}${selectedStream.id === recommendedId && videoStreams.length > 1 ? "（推荐）" : ""}`
    : "无可用清晰度";

  const qualityOptions = videoStreams.map((s) => ({
    key: s.id,
    label: `${qualityLabel(s)}${s.height && s.fps ? ` · ${s.height}P ${s.fps}fps` : ""}${s.id === recommendedId && videoStreams.length > 1 ? "（推荐）" : ""}${codecLabel(s.codec) ? ` · ${codecLabel(s.codec)}` : ""} · ${formatBytes(s.estimatedBytes)}`,
  }));

  const audioOptions =
    audioStreams.length === 0
      ? [{ key: "", label: "已包含在视频流中" }]
      : audioStreams.map((s) => ({ key: s.id, label: s.audioLabel ?? s.container.toUpperCase() }));

  return (
    <div className="vf-picker">
      <div className="vf-picker__field">
        <label className="vf-field-label" htmlFor="vf-quality-select">
          清晰度
        </label>
        <GlassSelect
          id="vf-quality-select"
          value={selectedStream?.id ?? ""}
          display={qualityDisplay}
          options={qualityOptions}
          disabled={disabled || videoStreams.length === 0}
          onChange={(streamId) => update({ streamId })}
        />
      </div>

      <div className="vf-picker__field">
        <label className="vf-field-label" htmlFor="vf-format-select">
          格式
        </label>
        <GlassSelect
          id="vf-format-select"
          value={selectedStream ? selectedStream.container.toLowerCase() : ""}
          display={selectedStream ? selectedStream.container.toUpperCase() : "待确认"}
          options={containers.map((c) => ({ key: c.toLowerCase(), label: c }))}
          disabled={disabled || containers.length <= 1}
          onChange={() => {
            /* 同一流内不做容器转换；如需转码走格式转换模式 */
          }}
        />
      </div>

      <div className="vf-picker__field">
        <label className="vf-field-label" htmlFor="vf-audio-select">
          音频
        </label>
        <GlassSelect
          id="vf-audio-select"
          value={selection?.audioStreamId ?? ""}
          display={
            audioStreams.length > 0
              ? ((audioStreams.find((s) => s.id === selection?.audioStreamId) ?? audioStreams[0])
                  ?.audioLabel ?? "默认音频")
              : (selectedStream?.audioLabel ?? "无需单独音轨")
          }
          options={audioOptions}
          disabled={disabled || audioStreams.length === 0}
          onChange={(audioStreamId) => update({ audioStreamId: audioStreamId || undefined })}
        />
      </div>
    </div>
  );
}