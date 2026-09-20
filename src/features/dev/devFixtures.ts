/**
 * 开发期视觉比对夹具的加载器。
 *
 * 这个模块只在开发构建中被动态引入（见 devSeed.ts），
 * 生产构建里 `import.meta.env.DEV` 为常量 false，整段分支会被打包器消除，
 * 因此 test/fixtures 下的夹具不会进入发布产物。
 */

import type { DownloadTask, ResolvedMedia } from "@/services/types";

export interface DevSeed {
  media: ResolvedMedia;
  tasks: DownloadTask[];
  queue: { activeCount: number; queuedCount: number; totalSpeedBps: number };
}

const modules = import.meta.glob("/test/fixtures/*.ts");

/** 夹具是否存在（test 目录被删除后为 false） */
export function hasSeedFixture(): boolean {
  return Boolean(modules["/test/fixtures/previewState.ts"]);
}

/** 加载预览态夹具；不可用时返回 null，绝不伪造成功 */
export async function loadFixture(): Promise<DevSeed | null> {
  const loader = modules["/test/fixtures/previewState.ts"];
  if (!loader) return null;
  try {
    const mod = (await loader()) as Record<string, unknown>;
    // 兼容两种导出形态：分别导出 media/tasks/queue，或统一导出 preview* 前缀
    const media = (mod.media ?? mod.previewMedia) as ResolvedMedia | undefined;
    const tasks = (mod.tasks ?? mod.previewTasks) as DownloadTask[] | undefined;
    const queue = (mod.queue ?? mod.previewQueue) as DevSeed["queue"] | undefined;
    if (!media || !tasks || !queue) return null;
    return { media, tasks, queue };
  } catch {
    return null;
  }
}