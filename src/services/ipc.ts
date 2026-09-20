/**
 * IPC 客户端：与 Rust 主进程通信的唯一入口。
 * 所有 mutation 都携带 idempotencyKey，重复调用不会产生副作用（项目书 §4.3）。
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppNotice,
  ConvertMode,
  DownloadTask,
  EngineKind,
  QueueStats,
  ResolveFailure,
  ResolvedMedia,
  StreamSelection,
  TaskKind,
  TaskProgress,
  TaskStatus,
} from "./types";

export interface ResolveOutcome {
  url: string;
  ok: boolean;
  media?: ResolvedMedia;
  failure?: ResolveFailure;
}

export interface SnapshotPayload {
  seq: number;
  tasks: DownloadTask[];
  queue: QueueStats;
}

/** 是否运行在 Tauri 壳内（纯浏览器预览时为 false） */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export class BackendUnavailableError extends Error {
  constructor() {
    super("未连接到 VideoFlow 后台服务，请在 Tauri 应用窗口中运行");
    this.name = "BackendUnavailableError";
  }
}

/**
 * 把主进程返回的结构化错误转成可读文本。
 *
 * 主进程的 AppError 序列化为普通对象（不是 Error 实例），
 * 直接取 err.message 会得到 undefined，所以这里统一归一化。
 */
export function describeError(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  if (err && typeof err === "object") {
    const e = err as { message?: unknown; code?: unknown; hint?: unknown; detail?: unknown };
    const parts: string[] = [];
    if (typeof e.message === "string" && e.message) parts.push(e.message);
    if (typeof e.hint === "string" && e.hint) parts.push(e.hint);
    if (typeof e.detail === "string" && e.detail) parts.push(e.detail);
    if (parts.length > 0) return parts.join("　");
    if (typeof e.code === "string" && e.code) return `错误代码：${e.code}`;
  }
  return "发生未知错误";
}

function newKey(): string {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) throw new BackendUnavailableError();
  return invoke<T>(command, args);
}

// ---- Commands ----

export function resolveUrls(urls: string[]): Promise<ResolveOutcome[]> {
  return call<ResolveOutcome[]>("resolve_urls", { urls });
}

export function createTask(input: {
  resolvedId: string;
  selection: StreamSelection;
  targetDir: string;
  title: string;
}): Promise<DownloadTask> {
  return call<DownloadTask>("create_task", {
    payload: { ...input, idempotencyKey: newKey() },
  });
}

export function taskAction(
  taskId: string,
  action: "pause" | "resume" | "cancel" | "retry" | "prioritize" | "unprioritize",
): Promise<DownloadTask> {
  return call<DownloadTask>("task_action", {
    payload: { taskId, action, idempotencyKey: newKey() },
  });
}

/**
 * 重新解析任务来源链接，并把新地址写回任务后重新排队。
 * 用于「需重新解析」状态（解析过期、下载 403/404）——项目书 §3.1 第 4 条。
 */
export function reparseTask(taskId: string): Promise<DownloadTask> {
  return call<DownloadTask>("reparse_task", { taskId });
}

export function removeTask(taskId: string): Promise<void> {
  return call<void>("remove_task", { taskId });
}

export function getSnapshot(afterSeq = 0): Promise<SnapshotPayload> {
  return call<SnapshotPayload>("get_snapshot", { afterSeq });
}

export function openFile(path: string): Promise<void> {
  return call<void>("open_file", { path });
}

export function revealFile(path: string): Promise<void> {
  return call<void>("reveal_file", { path });
}

export function deleteFile(path: string): Promise<void> {
  return call<void>("delete_file", { path });
}

/**
 * 删除任务对应的视频文件（移入系统回收站），并把任务标记为「已删除」。
 * 记录会保留在列表里，便于用户看到文件确实去掉了。
 */
export function deleteTaskFile(taskId: string): Promise<DownloadTask> {
  return call<DownloadTask>("delete_task_file", { taskId });
}

export function pickDirectory(defaultPath?: string): Promise<string | null> {
  return call<string | null>("pick_directory", { defaultPath: defaultPath ?? null });
}

export function readImageAsDataUrl(path: string): Promise<string> {
  return call<string>("read_image_as_data_url", { path });
}

export function ensureDownloadDir(dir: string): Promise<string> {
  return call<string>("ensure_download_dir", { dir });
}

export function setSetting(key: string, value: unknown): Promise<void> {
  return call<void>("set_setting", { key, value });
}

/** 探测 sidecar 是否就绪（yt-dlp / ffmpeg），用于设置页诊断展示 */
export interface SidecarStatus {
  ytDlp: { available: boolean; version?: string | null; path?: string | null };
  ffmpeg: { available: boolean; version?: string | null; path?: string | null };
}

export function sidecarStatus(): Promise<SidecarStatus> {
  return call<SidecarStatus>("sidecar_status");
}

/**
 * 登录态来源诊断结果。
 *
 * 抖音这类站点必须借浏览器登录态，而浏览器运行时会锁住 Cookie 数据库，
 * 应用侧绕不过去；`browserRunning` 就是提前把这件事告诉用户。
 */
export interface CookieSourceStatus {
  kind: "none" | "browser" | "file" | "account" | "unknown";
  browserLabel?: string | null;
  /** null 表示无法判定（非 Windows，或系统命令不可用） */
  browserRunning?: boolean | null;
  fileExists?: boolean | null;
  /** kind = account 时：已登录的站点名 */
  accountSites?: string[] | null;
  /** kind = account 时：账户 Cookie 文件是否存在 */
  accountFileExists?: boolean | null;
}

export function cookieSourceStatus(source: string): Promise<CookieSourceStatus> {
  return call<CookieSourceStatus>("cookie_source_status", { source });
}

// ---- 账户登录 ----

/** 一个可登录站点的状态 */
export interface AccountSiteStatus {
  key: string;
  label: string;
  loggedIn: boolean;
  /** Cookie 名不稳定、需要用户点「我已完成登录」的站点 */
  needsManualConfirm: boolean;
  /** 账号名（服务端校验通过才有） */
  userName?: string | null;
  /** 是否大会员。B 站的 4K 与 1080P 高码率需要大会员，登录本身不够 */
  isVip?: boolean | null;
}

export function accountStatus(): Promise<AccountSiteStatus[]> {
  return call<AccountSiteStatus[]>("account_status");
}

/**
 * 打开登录窗口，等用户完成后导出 Cookie。
 *
 * 会一直等到用户扫码完成（或点「我已完成登录」），因此 UI 需要展示进行中状态。
 */
export function accountLogin(siteKey: string): Promise<AccountSiteStatus[]> {
  return call<AccountSiteStatus[]>("account_login", { siteKey });
}

/** 用户在登录窗口点「我已完成登录」，让等待流程立刻收工 */
export function accountConfirmLogin(siteKey: string): Promise<void> {
  return call<void>("account_confirm_login", { siteKey });
}

export function accountLogout(siteKey: string): Promise<AccountSiteStatus[]> {
  return call<AccountSiteStatus[]>("account_logout", { siteKey });
}

/** 重新从内嵌浏览器导出 Cookie 并复校（登录久了 Cookie 会刷新） */
export function accountRefresh(): Promise<AccountSiteStatus[]> {
  return call<AccountSiteStatus[]>("account_refresh");
}

// ---- 格式转换 ----

/** 一个编码选项；音频与视频共用，下面两个档位字段按用途标注 */
export interface CodecOption {
  key: string;
  label: string;
  /** 音频专用：目标码率（kbps）；无损编码不返回 */
  bitrateKbps?: number;
  /** 音频专用：是否无损编码（FLAC / PCM） */
  lossless: boolean;
  /** 视频专用：本机是否有可用的硬件编码器 */
  hardware: boolean;
  /** 视频专用：选中的硬件路径短名，如 `QSV` / `NVENC` */
  hardwareLabel?: string;
  /** 视频专用：这条硬件路径是否为实验性（未在开发机验证过） */
  hardwareExperimental: boolean;
  /** 视频专用：可选的编码器路径（自动 / 各硬件候选 / 软件） */
  encoders: EncoderOption[];
}

/** 一条可选的编码器路径 */
export interface EncoderOption {
  /** `auto` / `software` / 硬件编码器名（如 `h264_nvenc`） */
  key: string;
  /** 界面短名：自动 / 软件（CPU）/ QSV / NVENC */
  label: string;
  /** 本机是否可用；不可用的选项置灰、选不中 */
  available: boolean;
  experimental: boolean;
}

/** 一个容器选项，并声明它接受哪些编码 */
export interface ContainerOption {
  key: string;
  label: string;
  /** 该容器允许的视频编码 key */
  video: string[];
  /** 该容器允许的音频编码 key */
  audio: string[];
}

export interface ResolutionOption {
  height: number | null;
  label: string;
}

/**
 * 转码能力表（主进程是唯一真相源）。
 *
 * 容器与编码**不是自由组合**：WebM 只接受 VP9/AV1 + Opus，MOV 只接受 H.264/H.265，
 * 而 AVI 配 H.265 会产出一个能写入但解码报错的坏文件。界面必须按这张表做联动过滤，
 * 否则用户会选到必然失败或产出坏文件的组合。
 */
export interface ConvertCapabilities {
  containers: ContainerOption[];
  audioTargets: ContainerOption[];
  videoCodecs: CodecOption[];
  audioCodecs: CodecOption[];
  resolutions: ResolutionOption[];
  /** 本机是否存在可用的硬件编码器（主进程真编一帧探测出来的） */
  hardwareAvailable: boolean;
}

export function convertCapabilities(): Promise<ConvertCapabilities> {
  return call<ConvertCapabilities>("convert_capabilities");
}

/** 源文件的基本信息，供转换页回显 */
export interface MediaInfo {
  durationSec?: number | null;
  size: number;
  width?: number | null;
  height?: number | null;
  videoCodec?: string | null;
  audioCodec?: string | null;
  /** 音频码率（bps）；flac 这类读不到时为 undefined */
  audioBitrate?: number | null;
  hasVideo: boolean;
}

export function probeMediaInfo(path: string): Promise<MediaInfo> {
  return call<MediaInfo>("probe_media_info", { path });
}

export interface CreateConvertInput {
  sourcePath: string;
  mode: ConvertMode;
  container: string;
  videoCodec?: string;
  audioCodec?: string;
  scaleHeight?: number;
  /** 是否允许显卡编码（H.264 / H.265，探测不到硬件时后端自动回落软件） */
  useHardware?: boolean;
  /** 点名走哪条硬件路径（如 `h264_nvenc`）；不传 = 自动按优先级挑 */
  hardwareEncoder?: string;
  targetDir: string;
}

export function createConvertTask(input: CreateConvertInput): Promise<DownloadTask> {
  return call<DownloadTask>("create_convert_task", { payload: input });
}

// ---- 链接历史 ----

export interface HistoryEntry {
  url: string;
  title: string;
  sourceName: string;
  /** 最近一次使用时间（RFC 3339） */
  at: string;
}

export function historyList(): Promise<HistoryEntry[]> {
  return call<HistoryEntry[]>("history_list");
}

/** 记一条解析成功的链接，返回更新后的完整列表 */
export function historyAdd(input: {
  url: string;
  title: string;
  sourceName: string;
}): Promise<HistoryEntry[]> {
  return call<HistoryEntry[]>("history_add", { payload: input });
}

export function historyRemove(url: string): Promise<HistoryEntry[]> {
  return call<HistoryEntry[]>("history_remove", { url });
}

export function historyClear(): Promise<void> {
  return call<void>("history_clear");
}

// ---- Events ----

export interface EventHandlers {
  onTaskUpdated?: (task: DownloadTask) => void;
  onTaskProgress?: (progress: TaskProgress) => void;
  onTaskRemoved?: (taskId: string) => void;
  onQueueUpdated?: (queue: QueueStats) => void;
  onNotice?: (notice: AppNotice) => void;
}

/** 订阅主进程事件，返回取消订阅函数 */
export async function subscribeEvents(handlers: EventHandlers): Promise<() => void> {
  if (!isTauri()) return () => undefined;

  const unlisteners: UnlistenFn[] = [];
  const register = async <T>(event: string, fn: (payload: T) => void) => {
    const un = await listen<T>(event, (e) => fn(e.payload));
    unlisteners.push(un);
  };

  await register<{ seq: number; task: DownloadTask }>("task_updated", (p) => {
    handlers.onTaskUpdated?.(p.task);
  });
  await register<{ seq: number; taskId: string }>("task_removed", (p) => {
    handlers.onTaskRemoved?.(p.taskId);
  });
  await register<TaskProgress & { seq: number }>("task_progress", (p) => {
    handlers.onTaskProgress?.(p);
  });
  await register<QueueStats & { seq: number }>("queue_updated", (p) => {
    handlers.onQueueUpdated?.(p);
  });
  await register<AppNotice>("app_notice", (p) => {
    handlers.onNotice?.(p);
  });

  return () => {
    for (const un of unlisteners) un();
  };
}

// ---- 展示辅助 ----

const STATUS_LABEL: Record<TaskStatus, string> = {
  draft: "草稿",
  resolving: "解析中",
  queued: "等待中",
  downloading: "下载中",
  pausing: "正在暂停",
  paused: "已暂停",
  retry_wait: "等待重试",
  verifying: "校验中",
  merging: "正在合并",
  completed: "已完成",
  failed: "失败",
  cancelled: "已取消",
  needs_reparse: "需重新解析",
  deleted: "已删除",
};

export function statusLabel(status: TaskStatus): string {
  return STATUS_LABEL[status];
}

/**
 * 按任务类型取状态文案。
 *
 * 转换任务复用了下载的状态机，但「下载中 / 正在合并」用在转码上会误导用户，
 * 所以这里按 kind 给出准确的措辞。
 */
export function statusLabelFor(status: TaskStatus, kind: TaskKind): string {
  if (kind === "convert") {
    if (status === "downloading") return "转换中";
    if (status === "merging") return "封装中";
    if (status === "verifying") return "校验中";
  }
  return STATUS_LABEL[status];
}

/** 任务所用引擎的展示名 */
export function engineLabel(engine: EngineKind): string {
  switch (engine) {
    case "yt_dlp":
      return "yt-dlp · 站点授权流";
    case "hls":
      return "FFmpeg · HLS 拉流";
    case "convert":
      return "FFmpeg · 本地转码";
    default:
      return "原生 HTTP · Range 分段";
  }
}

const ACTIVE_STATUSES: ReadonlySet<TaskStatus> = new Set([
  "downloading",
  "verifying",
  "merging",
  "pausing",
  "queued",
  "retry_wait",
]);

export function isActive(status: TaskStatus): boolean {
  return ACTIVE_STATUSES.has(status);
}

export function isTerminal(status: TaskStatus): boolean {
  return status === "completed" || status === "failed" || status === "cancelled" || status === "deleted";
}

/** 文件是否已经不在磁盘上（删除后），用来禁用打开/定位 */
export function fileGone(status: TaskStatus): boolean {
  return status === "deleted";
}

/** 字节数 → 可读文本 */
export function formatBytes(bytes: number | null | undefined, digits = 1): string {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes) || bytes < 0) return "待确认";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(digits)} ${units[unit]}`;
}

export function formatSpeed(bps: number | null | undefined): string {
  if (!bps || bps <= 0) return "";
  return `${formatBytes(bps, 1)}/s`;
}

export function formatDuration(sec: number | null | undefined): string {
  if (sec === null || sec === undefined || !Number.isFinite(sec) || sec <= 0) return "--:--";
  const total = Math.round(sec);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}

export function formatEta(sec: number | null | undefined): string {
  if (sec === null || sec === undefined || !Number.isFinite(sec) || sec <= 0) return "";
  if (sec < 60) return `剩余 ${Math.round(sec)} 秒`;
  if (sec < 3600) return `剩余 ${Math.round(sec / 60)} 分钟`;
  return `剩余 ${(sec / 3600).toFixed(1)} 小时`;
}