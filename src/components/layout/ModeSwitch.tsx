/** 顶部中央模式分段控件：视频下载（默认） / 格式转换。 */

import { IconConvert, IconDownloadTray } from "@/components/icons";
import { useUiStore } from "@/stores/uiStore";

export function ModeSwitch(): React.JSX.Element {
  const mode = useUiStore((s) => s.mode);
  const setMode = useUiStore((s) => s.setMode);

  return (
    <div className="vf-segmented" role="tablist" aria-label="工作模式">
      <button
        type="button"
        role="tab"
        className="vf-segmented__item"
        aria-selected={mode === "download"}
        onClick={() => setMode("download")}
      >
        <IconDownloadTray size={19} />
        <span>视频下载</span>
      </button>
      <button
        type="button"
        role="tab"
        className="vf-segmented__item"
        aria-selected={mode === "convert"}
        onClick={() => setMode("convert")}
      >
        <IconConvert size={19} />
        <span>格式转换</span>
      </button>
    </div>
  );
}