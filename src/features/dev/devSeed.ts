/**
 * 开发期视觉比对开关。
 *
 * 触发方式（仅开发构建生效）：
 *   VITE_VF_SEED=preview npm run tauri dev   # Tauri 窗口没有 URL query，用环境变量
 *   http://localhost:1420/?seed=preview      # 浏览器中直接加 query
 *
 * 夹具位于 test/fixtures/，由 devFixtures.ts 通过 import.meta.glob 动态发现：
 * 该目录被整体删除后应用不受影响；生产构建也不会把它打进产物。
 */

import type { DownloadTask, ResolvedMedia } from "@/services/types";

export interface DevSeed {
  media: ResolvedMedia;
  tasks: DownloadTask[];
  queue: { activeCount: number; queuedCount: number; totalSpeedBps: number };
}

export const isDevBuild = import.meta.env.DEV;

export function seedRequested(): boolean {
  if (!isDevBuild) return false;
  if (import.meta.env.VITE_VF_SEED === "preview") return true;
  if (typeof window === "undefined") return false;
  return new URLSearchParams(window.location.search).get("seed") === "preview";
}

/** 加载预览态夹具；生产构建下该分支被消除，夹具不会进入产物 */
export async function loadPreviewSeed(): Promise<DevSeed | null> {
  if (!isDevBuild) return null;
  const { loadFixture } = await import("./devFixtures");
  return loadFixture();
}