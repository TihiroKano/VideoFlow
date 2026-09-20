/**
 * 领域模型与 IPC 契约类型（对应 docs/VideoFlow-项目书.md §4）。
 * 与 Rust 侧 src-tauri/src/core/model.rs 保持一致。
 */

export type TaskStatus =
  | "draft"
  | "resolving"
  | "queued"
  | "downloading"
  | "pausing"
  | "paused"
  | "retry_wait"
  | "verifying"
  | "merging"
  | "completed"
  | "failed"
  | "cancelled"
  | "needs_reparse"
  /** 文件已被删除（移入系统回收站），记录保留 */
  | "deleted";

export type TaskKind = "download" | "convert";

/** 下载引擎 */
export type EngineKind = "native_http" | "yt_dlp" | "hls" | "convert";

export interface MediaStream {
  /** 稳定标识，用于选择与续传校验 */
  id: string;
  /** 容器格式，如 mp4 / webm / m4a */
  container: string;
  /** 媒体类型 */
  kind: "video" | "audio" | "muxed";
  /** 展示用清晰度标签，如 1080P */
  qualityLabel?: string;
  width?: number;
  height?: number;
  fps?: number;
  /** 音频编码描述，如 AAC 128kbps */
  audioLabel?: string;
  /** 估算字节数；无法确认时为 null，UI 显示「待确认」 */
  estimatedBytes?: number | null;
  /** 编码标识，如 avc1.640028 */
  codec?: string;
  /** Provider 提供的内容哈希（SHA-256 十六进制小写）；无则只做长度校验 */
  sha256?: string;
  /** 该流是否需要音视频合并 */
  needsMerge: boolean;
}

export interface SubtitleTrack {
  id: string;
  label: string;
  language?: string;
}

export interface ResolvedMedia {
  /** 解析结果标识，带 TTL，过期后需重新解析 */
  resolvedId: string;
  title: string;
  description?: string;
  sourceName: string;
  sourceUrl: string;
  thumbnailUrl?: string;
  durationSec?: number;
  streams: MediaStream[];
  subtitles: SubtitleTrack[];
  /** ISO 时间；为空表示不过期 */
  expiresAt?: string | null;
  /** 解析器标识与版本，用于问题追踪 */
  providerId: string;
  providerVersion: string;
}

export interface StreamSelection {
  streamId: string;
  /** 需要合并时使用的音频流 */
  audioStreamId?: string;
  subtitleId?: string;
}

export interface TaskError {
  code: string;
  message: string;
  retryable: boolean;
  detail?: string;
}

export interface ChunkState {
  index: number;
  start: number;
  end: number;
  done: boolean;
}

export interface ResumeCheckpoint {
  etag?: string | null;
  lastModified?: string | null;
  chunks: ChunkState[];
  tempPath?: string | null;
  /** 服务器是否支持 Range */
  rangeSupported: boolean;
}

/** 转换任务的模式 */
export type ConvertMode = "video" | "audio_extract";

/** 转换任务的方案（`kind === "convert"` 时有值） */
export interface ConvertPlan {
  /** 源文件绝对路径 */
  sourcePath: string;
  mode: ConvertMode;
  /** 目标容器 key，见 `ConvertCapabilities` */
  container: string;
  videoCodec?: string;
  audioCodec?: string;
  /** 缩放到的高度；缺省表示保持原始分辨率 */
  scaleHeight?: number;
}

export interface DownloadTask {
  id: string;
  kind: TaskKind;
  sourceUrl: string;
  providerId?: string;
  title: string;
  status: TaskStatus;
  priority: number;
  createdAt: string;
  updatedAt: string;
  /** 序列号，用于前端丢弃过期命令的乐观状态 */
  version: number;
  thumbnailUrl?: string;
  qualityLabel?: string;
  container?: string;
  audioLabel?: string;
  targetPath?: string | null;
  downloadedBytes: number;
  totalBytes?: number | null;
  speedBps: number;
  etaSec?: number | null;
  retryCount: number;
  error?: TaskError | null;
  resume?: ResumeCheckpoint | null;
  /** 该任务实际使用的下载引擎 */
  engine: EngineKind;
  /** 转换任务的方案 */
  convert?: ConvertPlan | null;
}

export interface TaskProgress {
  taskId: string;
  downloadedBytes: number;
  totalBytes?: number | null;
  speedBps: number;
  etaSec?: number | null;
}

export interface QueueStats {
  activeCount: number;
  queuedCount: number;
  totalSpeedBps: number;
}

export interface AppNotice {
  seq: number;
  level: "info" | "success" | "warning" | "error";
  code: string;
  message: string;
  taskId?: string;
}

export interface ResolveFailure {
  code: string;
  message: string;
  retryable: boolean;
  /** 用户可执行的下一步提示 */
  hint?: string;
}