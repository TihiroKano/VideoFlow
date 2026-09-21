/**
 * 液态玻璃下拉框。
 *
 * 原生 select 的弹层由 Chromium 自绘，带不上玻璃材质（见 glass.css 旧补丁注释），
 * 因此展开态改为自绘弹层：portal 到 body 定位置顶，面板走 GlassSurface，
 * 与闭合态外壳同材质，WebGL / CSS 两档都覆盖。
 */

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { GlassSurface } from "@/components/glass/GlassSurface";
import { IconChevronDown } from "@/components/icons";

export interface GlassSelectOption {
  key: string;
  label: string;
  disabled?: boolean;
}

export interface GlassSelectProps {
  id?: string;
  value: string;
  /** 闭合态展示文案；缺省用选中项的 label */
  display?: string;
  options: GlassSelectOption[];
  onChange: (value: string) => void;
  disabled?: boolean;
  ariaLabel?: string;
  /** 设置页行内用的紧凑尺寸 */
  compact?: boolean;
}

const POPUP_GAP = 6;
const POPUP_MAX_HEIGHT = 320;

interface PopupPos {
  top: number;
  left: number;
  width: number;
  height: number;
}
export function GlassSelect({
  id,
  value,
  display,
  options,
  onChange,
  disabled = false,
  ariaLabel,
  compact = false,
}: GlassSelectProps): React.JSX.Element {
  const [open, setOpen] = useState(false);
  const [highlight, setHighlight] = useState(0);
  const [pos, setPos] = useState<PopupPos | null>(null);
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const popupRef = useRef<HTMLDivElement | null>(null);
  const contentWidthRef = useRef<number | null>(null);

  const selected = options.find((o) => o.key === value);
  const displayText = display ?? selected?.label ?? "";

  const enabledIndexFrom = (start: number, dir: 1 | -1): number => {
    for (let i = start; i >= 0 && i < options.length; i += dir) {
      if (!options[i]?.disabled) return i;
    }
    return -1;
  };

  const openList = () => {
    const at = options.findIndex((o) => o.key === value);
    setHighlight(at >= 0 && !options[at]?.disabled ? at : enabledIndexFrom(0, 1));
    setOpen(true);
  };

  const close = (refocus: boolean) => {
    setOpen(false);
    setPos(null);
    if (refocus) buttonRef.current?.focus();
  };

  const choose = (index: number) => {
    const option = options[index];
    if (!option || option.disabled) return;
    onChange(option.key);
    close(true);
  };

  const place = useCallback(() => {
    const button = buttonRef.current;
    const popup = popupRef.current;
    if (!button || !popup) return;
    // 首帧宽度为 auto，量出内容自然宽度；弹层至少与触发器同宽
    if (contentWidthRef.current === null) contentWidthRef.current = popup.scrollWidth;
    const rect = button.getBoundingClientRect();
    const height = Math.min(popup.scrollHeight, POPUP_MAX_HEIGHT);
    const width = Math.min(
      Math.max(rect.width, contentWidthRef.current, 140),
      window.innerWidth - 16,
    );
    const below = rect.bottom + POPUP_GAP + height <= window.innerHeight - 8;
    const above = rect.top - POPUP_GAP - height >= 8;
    const top = below || !above ? rect.bottom + POPUP_GAP : rect.top - POPUP_GAP - height;
    setPos({ top, left: rect.left, width, height });
  }, []);

  useLayoutEffect(() => {
    if (!open) return;
    contentWidthRef.current = null;
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open, place]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (e: PointerEvent) => {
      const target = e.target as Node;
      if (buttonRef.current?.contains(target) || popupRef.current?.contains(target)) return;
      close(false);
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [open]);

  useEffect(() => {
    if (!open || !popupRef.current) return;
    popupRef.current
      .querySelector('[data-highlighted="true"]')
      ?.scrollIntoView({ block: "nearest" });
  }, [open, highlight]);

  const moveHighlight = (dir: 1 | -1) => {
    setHighlight((cur) => {
      const next = enabledIndexFrom(cur + dir, dir);
      return next === -1 ? cur : next;
    });
  };

  const onButtonKeyDown = (e: React.KeyboardEvent<HTMLButtonElement>) => {
    if (disabled) return;
    if (!open) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        openList();
      }
      return;
    }
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        moveHighlight(1);
        break;
      case "ArrowUp":
        e.preventDefault();
        moveHighlight(-1);
        break;
      case "Home":
        e.preventDefault();
        setHighlight(enabledIndexFrom(0, 1));
        break;
      case "End":
        e.preventDefault();
        setHighlight(enabledIndexFrom(options.length - 1, -1));
        break;
      case "Enter":
      case " ":
        e.preventDefault();
        choose(highlight);
        break;
      case "Escape":
        e.preventDefault();
        close(true);
        break;
      case "Tab":
        close(false);
        break;
    }
  };

  const listboxId = id ? `${id}-listbox` : undefined;

  return (
    <>
      <GlassSurface
        variant="control"
        className={`vf-select${compact ? " vf-select--compact" : ""}`}
        tint={0.32}
        opacity={0.7}
      >
        <span className="vf-select__value vf-truncate">{displayText}</span>
        <span className={`vf-select__chevron${open ? " vf-select__chevron--open" : ""}`}>
          <IconChevronDown size={compact ? 15 : 18} />
        </span>
        <button
          ref={buttonRef}
          type="button"
          id={id}
          className="vf-select__trigger"
          disabled={disabled || options.length === 0}
          aria-label={ariaLabel}
          aria-haspopup="listbox"
          aria-expanded={open}
          aria-controls={open ? listboxId : undefined}
          onClick={() => (open ? close(true) : openList())}
          onKeyDown={onButtonKeyDown}
        />
      </GlassSurface>
      {open
        ? createPortal(
            <GlassSurface
              variant="control"
              className="vf-select__popup"
              tint={0.3}
              opacity={0.82}
              style={{
                top: pos?.top ?? 0,
                left: pos?.left ?? 0,
                width: pos?.width ?? undefined,
                visibility: pos ? "visible" : "hidden",
              }}
            >
              <div
                ref={popupRef}
                id={listboxId}
                className="vf-select__list"
                role="listbox"
                aria-label={ariaLabel ?? displayText}
                style={{ maxHeight: POPUP_MAX_HEIGHT }}
              >
                {options.map((o, i) => (
                  <div
                    key={o.key}
                    role="option"
                    aria-selected={o.key === value}
                    aria-disabled={o.disabled || undefined}
                    data-highlighted={i === highlight || undefined}
                    className="vf-select__option vf-truncate"
                    onPointerEnter={() => !o.disabled && setHighlight(i)}
                    onClick={() => choose(i)}
                  >
                    {o.label}
                  </div>
                ))}
              </div>
            </GlassSurface>,
            document.body,
          )
        : null}
    </>
  );
}
