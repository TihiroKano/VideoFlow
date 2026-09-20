/**
 * 解析结果卡（项目书 §2.3 MediaPreviewCard）。
 * 状态：loading / success / unsupported / auth-required / error
 * 未知或不可验证的数据标为「待确认」，不虚构。
 */

import { GlassSurface } from "@/components/glass/GlassSurface";
import {
  IconClock,
  IconDimensions,
  IconFileSize,
  IconPlay,
  IconSource,
  IconWarning,
} from "@/components/icons";
import { formatBytes, formatDuration } from "@/services/ipc";
import type { ResolvedMedia } from "@/services/types";
import type { PreviewState } from "@/stores/composeStore";

export interface MediaPreviewCardProps {
  state: PreviewState;
  media?: ResolvedMedia | null;
  errorMessage?: string | null;
}

function MetaChip({
  icon,
  children,
}: {
  icon: React.ReactNode;
  children: React.ReactNode;
}): React.JSX.Element {
  return (
    <span className="vf-meta-chip">
      <span className="vf-meta-chip__icon">{icon}</span>
      <span className="vf-meta-chip__text">{children}</span>
    </span>
  );
}

export function MediaPreviewCard({
  state,
  media,
  errorMessage,
}: MediaPreviewCardProps): React.JSX.Element | null {
  if (state === "idle") return null;

  if (state === "loading") {
    return (
      <div className="vf-preview vf-preview--loading" aria-busy="true" aria-live="polite">
        <div className="vf-preview__thumb vf-skeleton" />
        <div className="vf-preview__info">
          <div className="vf-skeleton vf-skeleton--line" style={{ width: "62%" }} />
          <div className="vf-skeleton vf-skeleton--line" style={{ width: "40%" }} />
          <div className="vf-skeleton vf-skeleton--line" style={{ width: "52%", height: "14px" }} />
          <p className="vf-preview__loading-text">正在读取可下载信息…</p>
        </div>
      </div>
    );
  }

  if (state === "error") {
    return (
      <div className="vf-preview vf-preview--error" role="alert">
        <div className="vf-preview__thumb vf-preview__thumb--error">
          <IconWarning size={26} />
        </div>
        <div className="vf-preview__info">
          <h3 className="vf-preview__title">无法解析该链接</h3>
          <p className="vf-preview__desc">{errorMessage ?? "请检查链接后重试"}</p>
        </div>
      </div>
    );
  }

  if (!media) return null;

  const primary = media.streams.find((s) => s.kind !== "audio") ?? media.streams[0];
  const sizeBytes = primary?.estimatedBytes ?? null;

  return (
    <div className="vf-preview">
      <div className="vf-preview__thumb">
        {media.thumbnailUrl ? (
          <img
            src={media.thumbnailUrl}
            alt=""
            loading="lazy"
            /* 站点图床有防盗链：带本应用的 Referer 会 403，必须显式声明不发 Referer */
            referrerPolicy="no-referrer"
            className="vf-preview__thumb-img"
            onError={(e) => {
              // 封面失败时退回来源色块，不留白
              e.currentTarget.style.display = "none";
            }}
          />
        ) : (
          <div className="vf-preview__thumb-fallback" aria-hidden="true" />
        )}
        <span className="vf-preview__play" aria-hidden="true">
          <IconPlay size={22} />
        </span>
        {media.durationSec ? (
          <span className="vf-preview__duration">{formatDuration(media.durationSec)}</span>
        ) : null}
      </div>

      <div className="vf-preview__info">
        <h3 className="vf-preview__title vf-clamp-2" title={media.title}>
          {media.title}
        </h3>
        {media.description ? (
          <p className="vf-preview__desc vf-clamp-2">{media.description}</p>
        ) : null}

        <div className="vf-preview__meta">
          <MetaChip icon={<IconClock size={17} />}>{formatDuration(media.durationSec)}</MetaChip>
          <MetaChip icon={<IconDimensions size={17} />}>
            {primary?.width && primary.height ? `${primary.width} × ${primary.height}` : "分辨率待确认"}
          </MetaChip>
          <MetaChip icon={<IconFileSize size={17} />}>{formatBytes(sizeBytes)}</MetaChip>
        </div>

        <div className="vf-preview__meta">
          <MetaChip icon={<IconSource size={17} />}>{media.sourceName}</MetaChip>
        </div>
      </div>
    </div>
  );
}

/** 解析结果卡外层玻璃：与预览图一致，卡片比工作台更高海拔 */
export function PreviewSurface({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <GlassSurface variant="card" className="vf-preview-surface" tint={0.42} opacity={0.86}>
      {children}
    </GlassSurface>
  );
}