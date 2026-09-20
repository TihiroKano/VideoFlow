/**
 * 声明式玻璃容器。
 *
 * WebGL 档位（quality / balanced）：宿主保持透明，由全局 GlassCanvas 以 Scissor 绘制玻璃；
 * CSS 档位（performance）：宿主自带毛玻璃材质，绝不初始化 WebGL。
 *
 * 无论哪一档，布局、圆角、文字对比与交互语义完全一致（项目书 §5.2）。
 */

import { useEffect, useLayoutEffect, useRef } from "react";
import { readRadius, registerGlass, updateGlass } from "./glassRegistry";
import { useSettingsStore } from "@/stores/settingsStore";
import { supportsBackdropFilter } from "@/services/featureSupport";

export type GlassVariant = "panel" | "card" | "control";

export interface GlassSurfaceProps {
  variant?: GlassVariant;
  /** 倒角宽度（CSS px）：越大边缘越圆润 */
  bevel?: number;
  /** 视觉厚度：影响折射位移量 */
  thickness?: number;
  /** 乳白着色强度 0~1 */
  tint?: number;
  /** 基础不透明度 0~1 */
  opacity?: number;
  /** 亮度增益 */
  brightness?: number;
  /** 是否启用指针跟随高光 */
  interactive?: boolean;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
  id?: string;
  role?: string;
  "aria-label"?: string;
}

const VARIANT_DEFAULTS: Record<GlassVariant, { bevel: number; thickness: number; tint: number; opacity: number }> = {
  panel: { bevel: 34, thickness: 26, tint: 0.34, opacity: 0.88 },
  card: { bevel: 24, thickness: 18, tint: 0.28, opacity: 0.86 },
  control: { bevel: 14, thickness: 10, tint: 0.22, opacity: 0.78 },
};

export function GlassSurface({
  variant = "panel",
  bevel,
  thickness,
  tint,
  opacity,
  brightness = 1,
  interactive = false,
  className,
  style,
  children,
  id,
  role,
  "aria-label": ariaLabel,
}: GlassSurfaceProps): React.JSX.Element {
  const ref = useRef<HTMLDivElement | null>(null);
  const tier = useSettingsStore((s) => s.effectiveTier);
  const glassIntensity = useSettingsStore((s) => s.glassIntensity);
  const mode = tier === "performance" ? "css" : "webgl";

  const defaults = VARIANT_DEFAULTS[variant];
  const resolved = {
    bevel: bevel ?? defaults.bevel,
    thickness: thickness ?? defaults.thickness,
    tint: (tint ?? defaults.tint) * glassIntensity,
    opacity: opacity ?? defaults.opacity,
  };

  // 注册到全局画布
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (mode !== "webgl") return;

    const unregister = registerGlass({
      id: id ?? `glass-${Math.random().toString(36).slice(2, 9)}`,
      el,
      radius: readRadius(el),
      bevel: resolved.bevel,
      thickness: resolved.thickness,
      tint: resolved.tint,
      opacity: resolved.opacity,
      brightness,
    });
    return unregister;
  }, [mode, id, resolved.bevel, resolved.thickness, resolved.tint, resolved.opacity, brightness]);

  // 圆角在窗口尺寸变化后可能被 clamp 改变，重新读取
  useEffect(() => {
    const el = ref.current;
    if (!el || mode !== "webgl") return;
    const ro = new ResizeObserver(() => {
      updateGlass(el, { radius: readRadius(el) });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [mode]);

  // 指针位置写入 CSS 变量，供高光层使用
  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!interactive) return;
    const el = ref.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    el.style.setProperty("--vf-pointer-x", `${e.clientX - rect.left}px`);
    el.style.setProperty("--vf-pointer-y", `${e.clientY - rect.top}px`);
  };

  return (
    <div
      ref={ref}
      id={id}
      role={role}
      aria-label={ariaLabel}
      className={className ? `vf-glass-host ${className}` : "vf-glass-host"}
      data-glass-mode={mode}
      data-glass-variant={variant}
      data-glass-interactive={interactive ? "true" : "false"}
      data-glass-blur={mode === "css" && !supportsBackdropFilter() ? "solid" : "frost"}
      onPointerMove={onPointerMove}
      style={{ borderRadius: `var(--vf-radius-${variant})`, ...style }}
    >
      {children}
    </div>
  );
}