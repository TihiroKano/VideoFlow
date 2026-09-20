/**
 * 规格选择（项目书 §2.3 QualityPicker）。
 * 只有可用选项才可选；展示容器、分辨率、帧率、音频与估算大小。
 */

import { useMemo } from "react";
import { GlassSurface } from "@/components/glass/GlassSurface";
import { IconChevronDown } from "@/components/icons";
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

  return (
    <div className="vf-picker">
      <div className="vf-picker__field">
        <label className="vf-field-label" htmlFor="vf-quality-select">
          清晰度
        </label>
        <GlassSurface variant="control" className="vf-select" tint={0.32} opacity={0.7}>
          <span className="vf-select__value vf-truncate">
            {selectedStream ? qualityLabel(selectedStream) : "无可用清晰度"}
            {selectedStream && codecLabel(selectedStream.codec)
              ? ` · ${codecLabel(selectedStream.codec)}`
              : ""}
            {selectedStream?.id === recommendedId && videoStreams.length > 1 ? "（推荐）" : ""}
          </span>
          <span className="vf-select__chevron">
            <IconChevronDown size={18} />
          </span>
          <select
            id="vf-quality-select"
            className="vf-select__native"
            value={selectedStream?.id ?? ""}
            disabled={disabled || videoStreams.length === 0}
            aria-label="清晰度"
            onChange={(e) => update({ streamId: e.target.value })}
          >
            {videoStreams.map((s) => (
              <option key={s.id} value={s.id}>
                {qualityLabel(s)}
                {s.height && s.fps ? ` · ${s.height}P ${s.fps}fps` : ""}
                {s.id === recommendedId && videoStreams.length > 1 ? "（推荐）" : ""}
                {codecLabel(s.codec) ? ` · ${codecLabel(s.codec)}` : ""}
                {` · ${formatBytes(s.estimatedBytes)}`}
              </option>
            ))}
          </select>
        </GlassSurface>
      </div>

      <div className="vf-picker__field">
        <label className="vf-field-label" htmlFor="vf-format-select">
          格式
        </label>
        <GlassSurface variant="control" className="vf-select" tint={0.32} opacity={0.7}>
          <span className="vf-select__value vf-truncate">
            {selectedStream ? selectedStream.container.toUpperCase() : "待确认"}
          </span>
          <span className="vf-select__chevron">
            <IconChevronDown size={18} />
          </span>
          <select
            id="vf-format-select"
            className="vf-select__native"
            value={selectedStream?.container ?? ""}
            disabled={disabled || containers.length <= 1}
            aria-label="封装格式"
            onChange={() => {
              /* 同一流内不做容器转换；如需转码走格式转换模式 */
            }}
          >
            {containers.map((c) => (
              <option key={c} value={c.toLowerCase()}>
                {c}
              </option>
            ))}
          </select>
        </GlassSurface>
      </div>

      <div className="vf-picker__field">
        <label className="vf-field-label" htmlFor="vf-audio-select">
          音频
        </label>
        <GlassSurface variant="control" className="vf-select" tint={0.32} opacity={0.7}>
          <span className="vf-select__value vf-truncate">
            {audioStreams.length > 0
              ? (audioStreams.find((s) => s.id === selection?.audioStreamId) ?? audioStreams[0])
                  ?.audioLabel ?? "默认音频"
              : (selectedStream?.audioLabel ?? "无需单独音轨")}
          </span>
          <span className="vf-select__chevron">
            <IconChevronDown size={18} />
          </span>
          <select
            id="vf-audio-select"
            className="vf-select__native"
            value={selection?.audioStreamId ?? ""}
            disabled={disabled || audioStreams.length === 0}
            aria-label="音频"
            onChange={(e) => update({ audioStreamId: e.target.value || undefined })}
          >
            {audioStreams.length === 0 ? (
              <option value="">已包含在视频流中</option>
            ) : (
              audioStreams.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.audioLabel ?? s.container.toUpperCase()}
                </option>
              ))
            )}
          </select>
        </GlassSurface>
      </div>
    </div>
  );
}