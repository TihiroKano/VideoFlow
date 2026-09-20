/** 首页：模式切换驱动的工作台 + 常驻下载队列。 */

import { DownloadWorkbench } from "./DownloadWorkbench";
import { ConvertWorkbench } from "@/features/convert/ConvertWorkbench";
import { TaskQueuePanel } from "@/features/tasks/TaskQueuePanel";
import { useUiStore } from "@/stores/uiStore";

export function HomePage(): React.JSX.Element {
  const mode = useUiStore((s) => s.mode);

  return (
    <div className="vf-page-stack">
      {mode === "download" ? <DownloadWorkbench /> : <ConvertWorkbench />}
      <TaskQueuePanel />
    </div>
  );
}