/** 应用骨架：背景层 → 玻璃画布 → 内容层 → 高亮反馈层（项目书 §5.1）。 */

import { useCallback, useEffect, useState } from "react";
import { GlassCanvas } from "@/components/glass/GlassCanvas";
import { notifyGlassRedraw } from "@/components/glass/glassRegistry";
import { ModeSwitch } from "./ModeSwitch";
import { Sidebar } from "./Sidebar";
import { TitleBar } from "./TitleBar";
import { ToastStack } from "@/components/feedback/ToastStack";
import { ConfirmDialog } from "@/components/feedback/ConfirmDialog";
import { useBackgroundStore } from "@/stores/backgroundStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { useUiStore } from "@/stores/uiStore";
import { HomePage } from "@/features/compose/HomePage";
import { LibraryPage } from "@/features/library/LibraryPage";
import { BackgroundPage } from "@/features/settings/BackgroundPage";
import { SettingsPage } from "@/features/settings/SettingsPage";

export function AppShell(): React.JSX.Element {
  const background = useBackgroundStore();
  const tier = useSettingsStore((s) => s.effectiveTier);
  const reducedMotion = useSettingsStore((s) => s.reducedMotion);
  const highContrast = useSettingsStore((s) => s.highContrast);
  const page = useUiStore((s) => s.page);

  // WebGL 不可用时自动下沉到 CSS 档，功能与布局不受影响
  const [webglFailed, setWebglFailed] = useState(false);
  const useWebgl = tier !== "performance" && !webglFailed;

  const onUnavailable = useCallback(() => setWebglFailed(true), []);

  useEffect(() => {
    const root = document.documentElement;
    root.dataset.vfReducedMotion = reducedMotion ? "true" : "false";
    root.dataset.vfContrast = highContrast ? "high" : "normal";
  }, [reducedMotion, highContrast]);

  // 页面切换会改变中央工作台的内容与面板高度，画布尺寸不变时需显式重绘
  useEffect(() => {
    notifyGlassRedraw();
  }, [page]);

  // 滚动条自动隐藏：平时完全透明，滚动时显形、停下约 1 秒后隐去。
  // 只在这一处监听（scroll 用捕获阶段拿得到），所有滚动容器统一生效。
  useEffect(() => {
    const timers = new WeakMap<EventTarget, number>();
    const onScroll = (event: Event) => {
      const el = event.target;
      if (!(el instanceof HTMLElement)) return;
      el.classList.add("vf-scrolling");
      const prev = timers.get(el);
      if (prev !== undefined) window.clearTimeout(prev);
      timers.set(
        el,
        window.setTimeout(() => {
          el.classList.remove("vf-scrolling");
          timers.delete(el);
        }, 900),
      );
    };
    window.addEventListener("scroll", onScroll, { capture: true, passive: true });
    return () => window.removeEventListener("scroll", onScroll, true);
  }, []);

  return (
    <div className="vf-window" data-bg-render={useWebgl ? "webgl" : "css"}>
      {/* 第一层：静态图片模糊基底 */}
      <div
        className="vf-bg-layer"
        style={{
          backgroundImage: background.imageUrl ? `url("${background.imageUrl}")` : undefined,
          backgroundSize: background.fit === "contain" ? "contain" : "cover",
          opacity: background.opacity,
          filter: `brightness(${background.brightness})`,
        }}
      />

      {/* 第二层：液态玻璃容器（全局唯一 canvas） */}
      {useWebgl ? (
        <GlassCanvas
          imageUrl={background.imageUrl}
          fit={background.fit === "contain" ? "contain" : "cover"}
          blurPx={background.blur}
          tint={1}
          brightness={background.brightness}
          glassOpacity={0.9}
          bgOpacity={background.opacity}
          dispersion={1.1}
          quality={tier === "quality" ? "quality" : "balanced"}
          onUnavailable={onUnavailable}
        />
      ) : null}

      {/* 第三层：DOM 内容 */}
      <div className="vf-content">
        <TitleBar />
        <div className="vf-shell">
          <Sidebar />
          {/*
            顶部条同时承担窗口拖动：data-tauri-drag-region 只对事件目标本身生效，
            因此点到模式切换按钮时不会触发拖动，按钮照常响应；
            这比额外铺一层透明遮罩更安全（遮罩会挡住下方控件）。
          */}
          <div className="vf-topbar" data-tauri-drag-region>
            {page === "home" ? <ModeSwitch /> : null}
          </div>
          <main className="vf-shell__main">
            {page === "home" ? <HomePage /> : null}
            {page === "downloads" ? <LibraryPage kind="download" /> : null}
            {page === "converts" ? <LibraryPage kind="convert" /> : null}
            {page === "background" ? <BackgroundPage /> : null}
            {page === "settings" ? <SettingsPage /> : null}
          </main>
        </div>
      </div>

      {/* 第四层：反馈与浮层 */}
      <ToastStack />
      <ConfirmDialog />
    </div>
  );
}