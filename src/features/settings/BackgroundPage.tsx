/**
 * 背景控制台（项目书 §2.2 设置页 / 参考图右侧面板的产品化落点）。
 * 只接受用户上传的 JPG / PNG / WebP 静态图片；不支持视频或动态壁纸。
 */

import { useState } from "react";
import { GlassSurface } from "@/components/glass/GlassSurface";
import { IconImage, IconRefresh, IconUpload } from "@/components/icons";
import { describeError, readImageAsDataUrl } from "@/services/ipc";
import { defaultBackground, useBackgroundStore, type BgFit } from "@/stores/backgroundStore";
import { useUiStore } from "@/stores/uiStore";

const MAX_UPLOAD_BYTES = 12 * 1024 * 1024;

const FIT_OPTIONS: { value: BgFit; label: string }[] = [
  { value: "cover", label: "填充（裁切）" },
  { value: "contain", label: "适应（留边）" },
];

interface SliderRowProps {
  id: string;
  label: string;
  min: number;
  max: number;
  step: number;
  value: number;
  display: string;
  onChange: (v: number) => void;
}

function SliderRow({ id, label, min, max, step, value, display, onChange }: SliderRowProps): React.JSX.Element {
  return (
    <div className="vf-slider">
      <div className="vf-slider__head">
        <label htmlFor={id} className="vf-field-label">
          {label}
        </label>
        <span className="vf-slider__value">{display}</span>
      </div>
      <input
        id={id}
        type="range"
        className="vf-slider__input"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
      />
    </div>
  );
}

export function BackgroundPage(): React.JSX.Element {
  const bg = useBackgroundStore();
  const pushToast = useUiStore((s) => s.pushToast);
  const [busy, setBusy] = useState(false);

  const chooseImage = async () => {
    setBusy(true);
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "图片", extensions: ["jpg", "jpeg", "png", "webp"] }],
      });
      if (typeof selected !== "string") return;

      const dataUrl = await readImageAsDataUrl(selected);
      const name = selected.split(/[\\/]/).pop() ?? "background";

      if (dataUrl.length > MAX_UPLOAD_BYTES * 1.4) {
        pushToast({
          level: "warning",
          message: "图片体积过大",
          detail: "请选择小于 12 MB 的图片，否则会明显影响启动速度。",
          persistent: true,
        });
        return;
      }

      bg.setImage(dataUrl, name);
      pushToast({ level: "success", message: `背景已更换为 ${name}`, persistent: false });
    } catch (err) {
      const message = describeError(err);
      pushToast({
        level: "warning",
        message: "无法读取图片",
        detail: message.includes("not allowed") || message.includes("__TAURI")
          ? "请在 VideoFlow 应用窗口中使用此功能。"
          : message || undefined,
        persistent: true,
      });
    } finally {
      setBusy(false);
    }
  };

  return (
    <GlassSurface variant="panel" className="vf-bgpage" id="glass-background" tint={0.5} opacity={0.88}>
      <header className="vf-bgpage__header">
        <h1 className="vf-bgpage__title">背景</h1>
        <p className="vf-bgpage__subtitle">
          仅支持 JPG、PNG、WebP 静态图片。图片只在本机使用，不会上传。
        </p>
      </header>

      <div className="vf-bgpage__preview">
        <div className="vf-bgpage__thumb">
          {bg.imageUrl ? (
            <img src={bg.imageUrl} alt="" className="vf-bgpage__thumb-img" />
          ) : (
            <div className="vf-bgpage__thumb-empty">
              <IconImage size={26} />
            </div>
          )}
        </div>
        <div className="vf-bgpage__meta">
          <p className="vf-bgpage__name vf-truncate" title={bg.imageName}>
            {bg.imageName}
          </p>
          <div className="vf-bgpage__buttons">
            <button
              type="button"
              className="vf-btn-primary vf-bgpage__choose"
              disabled={busy}
              onClick={() => void chooseImage()}
            >
              <IconUpload size={18} />
              <span>{busy ? "读取中…" : "选择图片"}</span>
            </button>
            <button
              type="button"
              className="vf-btn-ghost vf-bgpage__reset"
              onClick={() => {
                bg.restoreDefault();
                pushToast({ level: "success", message: "已恢复默认背景", persistent: false });
              }}
            >
              <IconRefresh size={17} />
              <span>恢复默认</span>
            </button>
          </div>
        </div>
      </div>

      <div className="vf-divider" />

      <div className="vf-bgpage__controls">
        <div className="vf-picker__field">
          <span className="vf-field-label">适配方式</span>
          <GlassSurface variant="control" className="vf-bgpage__segmented" tint={0.3} opacity={0.68}>
            {FIT_OPTIONS.map((opt) => (
              <button
                key={opt.value}
                type="button"
                className="vf-bgpage__seg-btn"
                aria-pressed={bg.fit === opt.value}
                onClick={() => bg.setFit(opt.value)}
              >
                {opt.label}
              </button>
            ))}
          </GlassSurface>
        </div>

        <SliderRow
          id="vf-bg-opacity"
          label="透明度"
          min={0.2}
          max={1}
          step={0.02}
          value={bg.opacity}
          display={`${Math.round(bg.opacity * 100)}%`}
          onChange={bg.setOpacity}
        />

        <SliderRow
          id="vf-bg-blur"
          label="模糊程度"
          min={0}
          max={60}
          step={1}
          value={bg.blur}
          display={`${bg.blur} px`}
          onChange={bg.setBlur}
        />

        <SliderRow
          id="vf-bg-brightness"
          label="亮度"
          min={0.5}
          max={1.6}
          step={0.02}
          value={bg.brightness}
          display={`${Math.round(bg.brightness * 100)}%`}
          onChange={bg.setBrightness}
        />
      </div>

      <p className="vf-bgpage__note">
        预览图为 <code>docs{String.fromCharCode(92)}背景图1.jpg</code> 的副本；恢复默认即可回到该图。
        {bg.imageUrl === defaultBackground ? "" : " 当前使用自定义图片。"}
      </p>
    </GlassSurface>
  );
}