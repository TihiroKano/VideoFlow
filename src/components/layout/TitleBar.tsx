/** 自绘标题栏：无边框窗口的最小化 / 最大化 / 关闭。 */

import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { IconClose, IconMaximize, IconMinimize, IconRestore } from "@/components/icons";
import { isTauri } from "@/services/ipc";

export function TitleBar(): React.JSX.Element {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    if (!isTauri()) return;
    const win = getCurrentWindow();
    let unlisten: (() => void) | undefined;

    win.isMaximized().then(setMaximized).catch(() => undefined);
    win
      .onResized(() => {
        win.isMaximized().then(setMaximized).catch(() => undefined);
      })
      .then((un) => {
        unlisten = un;
      })
      .catch(() => undefined);

    return () => unlisten?.();
  }, []);

  const guard = (fn: () => Promise<unknown>) => () => {
    if (!isTauri()) return;
    fn().catch(() => undefined);
  };

  return (
    <div className="vf-titlebar vf-no-drag">
      <button
        type="button"
        className="vf-titlebar__btn"
        aria-label="最小化"
        title="最小化"
        onClick={guard(() => getCurrentWindow().minimize())}
      >
        <IconMinimize size={18} />
      </button>
      <button
        type="button"
        className="vf-titlebar__btn"
        aria-label={maximized ? "还原" : "最大化"}
        title={maximized ? "还原" : "最大化"}
        onClick={guard(() => getCurrentWindow().toggleMaximize())}
      >
        {maximized ? <IconRestore size={17} /> : <IconMaximize size={16} />}
      </button>
      <button
        type="button"
        className="vf-titlebar__btn"
        data-tone="close"
        aria-label="关闭"
        title="关闭"
        onClick={guard(() => getCurrentWindow().close())}
      >
        <IconClose size={18} />
      </button>
    </div>
  );
}