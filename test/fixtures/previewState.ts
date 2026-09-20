/**
 * 预览图填充态夹具（仅用于开发期的视觉比对，不参与任何产品逻辑）。
 *
 * 用途：把应用置于 docs/预览图.png 所示的状态，做逐项并排比对。
 * 触发方式：开发模式下打开应用并访问 `?seed=preview`，或用「演示数据」开关。
 *
 * 本目录整体删除不会影响应用构建：src/features/dev/devSeed.ts 通过
 * import.meta.glob 动态发现夹具，找不到就静默跳过。
 */

import type { DownloadTask, ResolvedMedia } from "@/services/types";

const NOW = "2026-09-19T20:00:00.000Z";

/** 与预览图一致的解析结果 */
export const previewMedia: ResolvedMedia = {
  resolvedId: "seed-preview-media",
  title: "天气之子 - 预告片 | 你的名字。新作",
  description: "新海诚最新力作，关于天气与爱的故事。",
  sourceName: "Bilibili",
  sourceUrl: "https://www.bilibili.com/video/BV1seed",
  thumbnailUrl: undefined,
  durationSec: 156,
  streams: [
    {
      id: "seed-1080p",
      container: "mp4",
      kind: "muxed",
      qualityLabel: "1080P",
      width: 1920,
      height: 1080,
      fps: 30,
      audioLabel: "AAC 128kbps",
      estimatedBytes: 47_817_523,
      codec: "avc1.640028",
      needsMerge: false,
    },
    {
      id: "seed-720p",
      container: "mp4",
      kind: "muxed",
      qualityLabel: "720P",
      width: 1280,
      height: 720,
      fps: 30,
      audioLabel: "AAC 128kbps",
      estimatedBytes: 33_659_699,
      codec: "avc1.64001F",
      needsMerge: false,
    },
    {
      id: "seed-480p",
      container: "mp4",
      kind: "muxed",
      qualityLabel: "480P",
      width: 854,
      height: 480,
      fps: 30,
      audioLabel: "AAC 128kbps",
      estimatedBytes: 15_728_640,
      codec: "avc1.64001E",
      needsMerge: false,
    },
  ],
  subtitles: [
    { id: "sub-zh-CN", label: "简体中文", language: "zh-CN" },
    { id: "sub-ja", label: "日本語", language: "ja" },
  ],
  expiresAt: null,
  providerId: "yt-dlp",
  providerVersion: "fixture",
};

/** 与预览图一致的三条任务 */
export const previewTasks: DownloadTask[] = [
  {
    id: "seed-task-1",
    kind: "download",
    sourceUrl: "https://www.bilibili.com/video/BV1seed1",
    providerId: "yt-dlp",
    engine: "yt_dlp",
    title: "天气之子 - 预告片 | 你的名字。新作",
    status: "downloading",
    priority: 0,
    createdAt: NOW,
    updatedAt: NOW,
    version: 1,
    qualityLabel: "1080P",
    container: "mp4",
    audioLabel: "AAC 128kbps",
    targetPath: "D:\\VideoFlow\\Downloads\\天气之子 - 预告片.mp4",
    downloadedBytes: 32_521_000,
    totalBytes: 47_817_523,
    speedBps: 2_516_582,
    etaSec: 6,
    retryCount: 0,
    error: null,
    resume: null,
  },
  {
    id: "seed-task-2",
    kind: "download",
    sourceUrl: "https://www.bilibili.com/video/BV1seed2",
    providerId: "yt-dlp",
    engine: "yt_dlp",
    title: "RADWIMPS - なんでもないや",
    status: "queued",
    priority: 0,
    createdAt: NOW,
    updatedAt: NOW,
    version: 1,
    qualityLabel: "720P",
    container: "mp4",
    audioLabel: "AAC 128kbps",
    targetPath: "D:\\VideoFlow\\Downloads\\RADWIMPS - なんでもないや.mp4",
    downloadedBytes: 0,
    totalBytes: 33_659_699,
    speedBps: 0,
    etaSec: null,
    retryCount: 0,
    error: null,
    resume: null,
  },
  {
    id: "seed-task-3",
    kind: "download",
    sourceUrl: "https://www.bilibili.com/video/BV1seed3",
    providerId: "yt-dlp",
    engine: "yt_dlp",
    title: "你的名字。 - 片段",
    status: "completed",
    priority: 0,
    createdAt: NOW,
    updatedAt: NOW,
    version: 1,
    qualityLabel: "1080P",
    container: "mp4",
    audioLabel: "AAC 128kbps",
    targetPath: "D:\\VideoFlow\\Downloads\\你的名字。 - 片段.mp4",
    downloadedBytes: 82_103_910,
    totalBytes: 82_103_910,
    speedBps: 0,
    etaSec: null,
    retryCount: 0,
    error: null,
    resume: null,
  },
];

/** 队列统计：与预览图的「全局速度」一致 */
export const previewQueue = {
  activeCount: 1,
  queuedCount: 1,
  totalSpeedBps: 2_516_582,
};