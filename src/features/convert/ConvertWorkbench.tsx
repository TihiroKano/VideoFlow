/**
 * 格式转换工作台 —— 真实转码（走主进程的 FFmpeg 引擎）。
 *
 * 两条模式：
 * - **视频转换**：换容器 / 重编码视频与音频 / 缩放分辨率；
 * - **音频转换**：丢掉视频轨只留音轨，或在音频格式之间互转（mp3 / m4a / flac / wav）。
 *
 * 两个关键约束：
 * - **容器与编码不是自由组合**（WebM 只吃 VP9/AV1 + Opus，MOV 只吃 H.264/H.265，
 *   而 AVI 配 H.265 会产出能写入却解码报错的坏文件）。所以选项一律由主进程下发的
 *   能力表驱动，容器变化时立刻收敛编码选择，从源头杜绝产出坏文件。
 * - **GPU 加速只是「允许」**：开关打开时由主进程按「这台机器探测到没探测到硬件编码器」
 *   决定走 h264_qsv / hevc_qsv 还是回落软件编码，界面只负责如实说明会走哪条路。
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { GlassSurface } from "@/components/glass/GlassSurface";
import {
  IconChevronDown,
  IconClose,
  IconConvert,
  IconFolder,
  IconUpload,
} from "@/components/icons";
import {
  BackendUnavailableError,
  convertCapabilities,
  createConvertTask,
  describeError,
  formatBytes,
  pickDirectory,
  probeMediaInfo,
  type CodecOption,
  type ConvertCapabilities,
  type EncoderOption,
  type MediaInfo,
} from "@/services/ipc";
import { audioQualityHints } from "./audioQuality";
import type { ConvertMode } from "@/services/types";
import { useSettingsStore } from "@/stores/settingsStore";
import { useTaskStore } from "@/stores/taskStore";
import { useUiStore } from "@/stores/uiStore";

interface LocalFile {
  path: string;
  name: string;
  info: MediaInfo | null;
}

const MAX_FILES = 10;

/**
 * 把当前选择收敛到允许的取值。
 *
 * 用户先选编码、后换容器时，原编码可能不被新容器接受（例如 H.264 → WebM）。
 * 这时换回该容器支持的第一项，而不是留一个提交必然失败的组合。
 */
export function converge(current: string, allowed: string[]): string {
  const first = allowed[0];
  if (first === undefined) return current;
  return allowed.includes(current) ? current : first;
}

/** 转码耗时较长，提交前把「从现在起会发生什么」讲清楚 */
function describePlan(
  mode: ConvertMode,
  containerLabel: string,
  videoLabel: string,
  audioLabel: string,
  resolutionLabel: string,
): string {
  if (mode === "audio_extract") {
    // 音频目标本身就是格式，再挂一个编码名只会重复（「MP3（MP3）」）
    return `仅保留音轨 → ${containerLabel}`;
  }
  const parts = [videoLabel, audioLabel];
  if (resolutionLabel) parts.push(resolutionLabel);
  return `${containerLabel} · ${parts.join(" · ")}`;
}

export function ConvertWorkbench(): React.JSX.Element {
  const [files, setFiles] = useState<LocalFile[]>([]);
  const [caps, setCaps] = useState<ConvertCapabilities | null>(null);
  const [mode, setMode] = useState<ConvertMode>("video");
  const [container, setContainer] = useState("mp4");
  const [videoCodec, setVideoCodec] = useState("h264");
  const [audioCodec, setAudioCodec] = useState("aac");
  const [scaleHeight, setScaleHeight] = useState<number | null>(null);
  const [useHardware, setUseHardware] = useState(true);
  /** 编码器路径：`auto` / `software` / 硬件编码器名（如 `h264_nvenc`） */
  const [videoEncoder, setVideoEncoder] = useState("auto");
  const [outputDir, setOutputDir] = useState<string>("");
  const [submitting, setSubmitting] = useState(false);

  const downloadDir = useSettingsStore((s) => s.downloadDir);
  const upsertTask = useTaskStore((s) => s.upsert);
  const pushToast = useUiStore((s) => s.pushToast);

  const effectiveOutputDir = outputDir || downloadDir;

  // 能力表：容器与编码的合法组合由主进程决定，前端只做展示与过滤
  useEffect(() => {
    convertCapabilities()
      .then((c) => {
        setCaps(c);
        // 有硬件编码器就默认打开（用户要的是速度），没有就把开关落下去，
        // 免得界面显示「已开启」而实际每次都回落软件编码
        setUseHardware(c.hardwareAvailable);
        // 首次拿到能力表时收敛一次默认值，避免默认值恰好不被默认容器接受
        const first = c.containers[0];
        if (first) {
          setContainer(first.key);
          setVideoCodec((cur) => converge(cur, first.video));
          setAudioCodec((cur) => converge(cur, first.audio));
        }
      })
      .catch(() => {
        // 纯浏览器预览：能力表拿不到，界面按空列表展示，提交时会给出明确提示
      });
  }, []);

  /** 当前模式下的目标列表：视频转换用容器，音频转换用音频目标 */
  const targets = useMemo(() => {
    if (!caps) return [];
    return mode === "video" ? caps.containers : caps.audioTargets;
  }, [caps, mode]);

  const currentTarget = useMemo(
    () => targets.find((t) => t.key === container) ?? targets[0] ?? null,
    [targets, container],
  );

  // 容器变化时收敛编码与目标：只渲染该容器真正接受的选项
  useEffect(() => {
    if (!currentTarget) return;
    if (currentTarget.key !== container) setContainer(currentTarget.key);
    setVideoCodec((cur) => converge(cur, currentTarget.video));
    setAudioCodec((cur) => converge(cur, currentTarget.audio));
  }, [currentTarget, container]);

  /** 切换模式时把目标也切到该模式的第一项（容器列表完全不同） */
  const switchMode = (next: ConvertMode) => {
    setMode(next);
    const list = next === "video" ? caps?.containers : caps?.audioTargets;
    const first = list?.[0];
    if (first) {
      setContainer(first.key);
      setVideoCodec((v) => converge(v, first.video));
      setAudioCodec((a) => converge(a, first.audio));
    }
    if (next === "audio_extract") setScaleHeight(null);
  };

  const allowedVideo = useMemo(() => {
    if (!caps || !currentTarget) return [];
    return caps.videoCodecs.filter((c) => currentTarget.video.includes(c.key));
  }, [caps, currentTarget]);

  const allowedAudio = useMemo(() => {
    if (!caps || !currentTarget) return [];
    return caps.audioCodecs.filter((c) => currentTarget.audio.includes(c.key));
  }, [caps, currentTarget]);

  /** 容器变化会把编码收敛成别的（例如 WebM 只吃 VP9），点名过的路径要跟着退回自动 */
  const currentCodecEntry = allowedVideo.find((c) => c.key === videoCodec);
  useEffect(() => {
    if (videoEncoder === "auto" || videoEncoder === "software") return;
    if (currentCodecEntry && !currentCodecEntry.encoders.some((e) => e.key === videoEncoder)) {
      setVideoEncoder("auto");
    }
  }, [currentCodecEntry, videoEncoder]);

  const pickFiles = async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        multiple: true,
        directory: false,
        filters: [
          {
            name: "媒体文件",
            extensions: [
              "mp4", "mkv", "mov", "webm", "avi", "flv", "ts", "m4v", "mp3", "m4a", "aac",
              "flac", "wav", "ogg", "opus",
            ],
          },
        ],
      });
      if (!selected) return;
      const list = (Array.isArray(selected) ? selected : [selected]).filter(
        (p): p is string => typeof p === "string",
      );
      if (list.length >= MAX_FILES) {
        pushToast({
          level: "warning",
          message: `一次最多处理 ${MAX_FILES} 个文件`,
          detail: `已选取 ${list.length} 个，只保留前 ${MAX_FILES} 个。`,
          persistent: false,
        });
      }

      // 先落列表让界面立刻响应，再异步补上探测信息
      const picked: LocalFile[] = list.slice(0, MAX_FILES).map((p) => ({
        path: p,
        name: p.split(/[\\/]/).pop() ?? p,
        info: null,
      }));
      setFiles(picked);

      const withInfo = await Promise.all(
        picked.map(async (f) => {
          try {
            return { ...f, info: await probeMediaInfo(f.path) };
          } catch {
            // 探测失败不阻断：文件仍可尝试转换，后端还会再校验一次
            return f;
          }
        }),
      );
      setFiles(withInfo);
    } catch (err) {
      pushToast({
        level: "warning",
        message: "无法打开文件选择器",
        detail:
          err instanceof Error && err.message.includes("__TAURI")
            ? "请在 VideoFlow 应用窗口中使用此功能。"
            : undefined,
        persistent: true,
      });
    }
  };

  const pickOutput = async () => {
    try {
      const dir = await pickDirectory(effectiveOutputDir);
      if (dir) setOutputDir(dir);
    } catch {
      pushToast({ level: "warning", message: "无法打开目录选择器", persistent: true });
    }
  };

  /** 音频转换需要有音轨；无音轨的文件直接拦下，别让用户白等一次转码 */
  const missingAudio = useCallback(
    (f: LocalFile) => f.info !== null && !f.info.audioCodec,
    [],
  );

  const submit = async () => {
    if (files.length === 0 || !currentTarget) return;

    const noAudio = files.filter(missingAudio);
    if (mode === "audio_extract" && noAudio.length > 0) {
      pushToast({
        level: "warning",
        message: "有文件没有音轨，无法转换为音频",
        detail: noAudio.map((f) => f.name).join("、"),
        persistent: true,
      });
      return;
    }

    setSubmitting(true);
    let ok = 0;
    const failures: string[] = [];

    // 逐个建任务：一个失败不影响其余文件
    for (const f of files) {
      try {
        const task = await createConvertTask({
          sourcePath: f.path,
          mode,
          container: currentTarget.key,
          videoCodec: mode === "video" ? videoCodec : undefined,
          audioCodec,
          scaleHeight: mode === "video" ? (scaleHeight ?? undefined) : undefined,
          // 音频转换没有视频编码，硬件开关对它没有意义；
          // 点名了硬件路径时不受总开关影响（选了就按选的走）
          useHardware: mode === "video" ? useHardware || explicitHardware : undefined,
          hardwareEncoder: explicitHardware ? videoEncoder : undefined,
          targetDir: effectiveOutputDir,
        });
        upsertTask(task);
        ok += 1;
      } catch (err) {
        if (err instanceof BackendUnavailableError) {
          pushToast({
            level: "warning",
            message: "未连接到 VideoFlow 后台服务",
            detail: "请在 VideoFlow 应用窗口中使用。",
            persistent: true,
          });
          setSubmitting(false);
          return;
        }
        failures.push(`${f.name}：${describeError(err)}`);
      }
    }

    setSubmitting(false);

    if (ok > 0) {
      pushToast({
        level: "success",
        message: ok === 1 ? "已加入转换队列" : `已加入转换队列（${ok} 个）`,
        detail: `输出到 ${effectiveOutputDir}`,
        persistent: false,
      });
      setFiles([]);
    }
    if (failures.length > 0) {
      pushToast({
        level: "error",
        message: `${failures.length} 个文件无法加入队列`,
        detail: failures.join("；"),
        persistent: true,
      });
    }
  };

  const canSubmit = files.length > 0 && currentTarget !== null && !submitting;

  /** 下拉框外壳：项目里所有选择器都是这套结构 */
  const selectField = (
    id: string,
    label: string,
    value: string,
    display: string,
    options: { key: string; label: string; disabled?: boolean }[],
    onChange: (v: string) => void,
    disabled = false,
  ) => (
    <div className="vf-picker__field">
      <label className="vf-field-label" htmlFor={id}>
        {label}
      </label>
      <GlassSurface variant="control" className="vf-select" tint={0.32} opacity={0.7}>
        <span className="vf-select__value vf-truncate">{display}</span>
        <span className="vf-select__chevron">
          <IconChevronDown size={18} />
        </span>
        <select
          id={id}
          className="vf-select__native"
          value={value}
          disabled={disabled || options.length === 0}
          onChange={(e) => onChange(e.target.value)}
        >
          {options.map((o) => (
            // 不可用的路径（本机没这个硬件编码器）在这里置灰、选不中
            <option key={o.key} value={o.key} disabled={o.disabled}>
              {o.label}
            </option>
          ))}
        </select>
      </GlassSurface>
    </div>
  );

  const hardwareReady = caps?.hardwareAvailable ?? false;
  /** 用户是否点名了某条硬件路径（非自动、非软件） */
  const explicitHardware = videoEncoder !== "auto" && videoEncoder !== "software";

  /**
   * 下拉里的路径名。
   *
   * 视频编码下拉是「编码 × 路径」拍平的：自动 / QSV / NVENC / 软件。
   * 用不了的路径（本机没这个硬件编码器）会被置灰，选不中；
   * 用户点名的路径若恰好不可用，后端也只会回落，不会让任务失败。
   */
  const encoderLabel = (c: CodecOption, e: EncoderOption): string => {
    if (e.key === "software") return `${c.label}（软件 · CPU）`;
    if (e.key === "auto") {
      const path =
        hardwareReady && (useHardware || explicitHardware) && c.hardware && c.hardwareLabel
          ? [c.hardwareLabel, c.hardwareExperimental ? "实验性" : ""].filter(Boolean).join("·")
          : "软件";
      return `${c.label}（自动 · ${path}）`;
    }
    const suffix = [e.experimental ? "实验性" : "", e.available ? "" : "本机不可用"]
      .filter(Boolean)
      .join(" · ");
    return `${c.label}（${e.label}${suffix ? ` · ${suffix}` : ""}）`;
  };

  const videoOptions = allowedVideo.flatMap((c) =>
    c.encoders.map((e) => ({
      key: `${c.key}|${e.key}`,
      label: encoderLabel(c, e),
      // 不可用的路径只置灰，不隐藏：用户得看得见「这台机器有没有 NVENC」
      disabled: !e.available,
    })),
  );
  const audioOptions = allowedAudio.map((c) => ({ key: c.key, label: c.label }));

  const videoLabel =
    videoOptions.find((o) => o.key === `${videoCodec}|${videoEncoder}`)?.label ?? "—";
  const audioLabel = audioOptions.find((o) => o.key === audioCodec)?.label ?? "—";
  const resolutionLabel =
    mode === "video" && scaleHeight
      ? (caps?.resolutions.find((r) => r.height === scaleHeight)?.label ?? "")
      : "";
  const planText = currentTarget
    ? describePlan(mode, currentTarget.label, videoLabel, audioLabel, resolutionLabel)
    : "—";

  /** 本机实际可用的硬件路径名（QSV / NVENC），用于把提示写具体 */
  const gpuPaths = [
    ...new Set(
      (caps?.videoCodecs ?? [])
        .filter((c) => c.hardware && c.hardwareLabel)
        .map((c) => c.hardwareLabel as string),
    ),
  ];

  const currentCodec = allowedVideo.find((c) => c.key === videoCodec);
  const explicitEncoderLabel =
    videoEncoder !== "auto" && videoEncoder !== "software"
      ? currentCodec?.encoders.find((e) => e.key === videoEncoder)?.label
      : undefined;

  const gpuHint =
    caps === null
      ? "正在检测本机的硬件编码器…"
      : explicitHardware
        ? `本次固定走 ${explicitEncoderLabel ?? videoEncoder}；真编不动时会自动改用软件编码重跑，不会失败`
        : !hardwareReady
          ? "未检测到可用的硬件编码器，将使用软件编码（CPU）"
          : useHardware
            ? `H.264 / H.265 走 ${gpuPaths.join(" / ")} 硬件编码（实测快 3~5 倍）；AV1 / VP9 没有硬件路径，编不动时自动回落软件编码`
            : "已关闭，全部使用软件编码（更慢，但兼容性最好）";

  /**
   * 音质提示：规则见 features/convert/audioQuality.ts。
   *
   * 只在音频转换模式显示：视频转换时用户盯着的是画面，
   * 音频码率这类说明只会变成噪音（视频那边本来也会重编音轨）。
   */
  const qualityHints = useMemo(() => {
    if (mode !== "audio_extract") return [];
    const target = allowedAudio.find((c) => c.key === audioCodec);
    if (!target) return [];
    const sources = files
      .filter((f) => f.info?.audioCodec)
      .map((f) => ({
        name: f.name,
        codec: f.info?.audioCodec,
        bitrate: f.info?.audioBitrate,
      }));
    if (sources.length === 0) return [];
    return audioQualityHints(sources, target);
  }, [mode, files, allowedAudio, audioCodec]);

  return (
    <GlassSurface variant="panel" className="vf-workbench vf-convert" id="glass-convert" tint={0.5} opacity={0.88}>
      <div className="vf-convert__modes" role="tablist" aria-label="转换模式">
        <button
          type="button"
          role="tab"
          aria-selected={mode === "video"}
          className="vf-convert__mode"
          data-active={mode === "video" ? "true" : undefined}
          onClick={() => switchMode("video")}
        >
          视频转换
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={mode === "audio_extract"}
          className="vf-convert__mode"
          data-active={mode === "audio_extract" ? "true" : undefined}
          onClick={() => switchMode("audio_extract")}
        >
          音频转换
        </button>
      </div>

      <div className="vf-convert__dropzone">
        <IconUpload size={30} />
        <p className="vf-convert__dropzone-title">选择本地媒体文件</p>
        <p className="vf-convert__dropzone-hint">
          只处理你已拥有的本地文件，最多 {MAX_FILES} 个；不涉及任何在线资源
        </p>
        <button type="button" className="vf-btn-ghost vf-convert__pick" onClick={() => void pickFiles()}>
          浏览文件
        </button>
      </div>

      {files.length > 0 ? (
        <ul className="vf-convert__files">
          {files.map((f) => (
            <li key={f.path} className="vf-convert__file">
              <span className="vf-truncate" title={f.path}>
                {f.name}
              </span>
              <span className="vf-convert__filemeta">
                {f.info
                  ? [
                      f.info.hasVideo && f.info.height ? `${f.info.height}P` : "仅音频",
                      f.info.videoCodec?.toUpperCase(),
                      f.info.audioCodec?.toUpperCase(),
                      formatBytes(f.info.size),
                    ]
                      .filter(Boolean)
                      .join(" · ")
                  : "读取中…"}
              </span>
              <button
                type="button"
                className="vf-convert__fileremove"
                aria-label={`移除 ${f.name}`}
                onClick={() => setFiles((prev) => prev.filter((x) => x.path !== f.path))}
              >
                <IconClose size={14} />
              </button>
            </li>
          ))}
        </ul>
      ) : null}

      <div className="vf-divider" />

      <div className="vf-convert__grid">
        {selectField(
          "vf-convert-format",
          mode === "video" ? "目标容器" : "转换为",
          currentTarget?.key ?? "",
          currentTarget?.label ?? "载入中…",
          targets,
          setContainer,
        )}

        {mode === "video"
          ? selectField(
              "vf-convert-vcodec",
              "视频编码",
              `${videoCodec}|${videoEncoder}`,
              videoLabel,
              videoOptions,
              (v) => {
                const [codec = "", encoder = "auto"] = v.split("|");
                setVideoCodec(codec);
                setVideoEncoder(encoder);
                // 选了硬件路径就把总开关带上；选「软件」就把它关掉，
                // 免得出现「下拉写着 NVENC、开关却是关的」这种自相矛盾的状态
                if (encoder === "software") setUseHardware(false);
                else if (encoder !== "auto") setUseHardware(true);
              },
            )
          : null}

        {selectField(
          "vf-convert-acodec",
          "音频编码",
          audioCodec,
          audioLabel,
          audioOptions,
          setAudioCodec,
          mode === "audio_extract" && audioOptions.length <= 1,
        )}

        {mode === "video"
          ? selectField(
              "vf-convert-res",
              "分辨率",
              String(scaleHeight ?? ""),
              caps?.resolutions.find((r) => r.height === scaleHeight)?.label ?? "保持原始分辨率",
              (caps?.resolutions ?? []).map((r) => ({
                key: String(r.height ?? ""),
                label: r.label,
              })),
              (v) => setScaleHeight(v === "" ? null : Number(v)),
            )
          : null}
      </div>

      {mode === "video" ? (
        <div className="vf-convert__gpu">
          <div className="vf-convert__gpu-text">
            <span className="vf-convert__gpu-title">GPU 硬件加速</span>
            <span className="vf-convert__gpu-hint">{gpuHint}</span>
          </div>
          <label className="vf-switch">
            <input
              type="checkbox"
              checked={(useHardware || explicitHardware) && hardwareReady}
              disabled={!hardwareReady}
              aria-label="GPU 硬件加速"
              onChange={(e) => {
                setUseHardware(e.target.checked);
                // 关掉总开关时把「点名的硬件路径」退回自动，
                // 否则下拉还写着 NVENC、实际却走了软件，自相矛盾
                if (!e.target.checked && explicitHardware) setVideoEncoder("auto");
              }}
            />
            <span className="vf-switch__track" aria-hidden="true" />
          </label>
        </div>
      ) : null}

      {qualityHints.length > 0 ? (
        <ul className="vf-convert__hints">
          {qualityHints.map((h) => (
            <li key={h.text} data-level={h.level}>
              {h.text}
            </li>
          ))}
        </ul>
      ) : null}

      <p className="vf-convert__note">
        {mode === "video"
          ? "容器决定能用的编码：WebM 只支持 VP9 / AV1 + Opus，MOV 只支持 H.264 / H.265，AVI 不支持 H.265。下拉里已经按这些限制过滤，选不出会失败或产出坏文件的组合。"
          : "音频转换既能从视频里抽出音轨，也能在音频格式之间互转（例如 FLAC → MP3）。转到有损格式属于正常压缩；反过来把有损音频转成无损，音质并不会变好。"}
      </p>

      <div className="vf-convert__output">
        <button type="button" className="vf-btn-ghost vf-convert__outpick" onClick={() => void pickOutput()}>
          <IconFolder size={17} />
          <span>输出目录</span>
        </button>
        <span className="vf-convert__outpath vf-truncate" title={effectiveOutputDir}>
          {effectiveOutputDir}
        </span>
      </div>

      <div className="vf-workbench__actions">
        <span className="vf-convert__plan vf-truncate" title={planText}>
          {planText}
        </span>
        <button
          type="button"
          className="vf-btn-primary vf-convert__start"
          disabled={!canSubmit}
          onClick={() => void submit()}
        >
          <IconConvert size={20} />
          <span>{submitting ? "正在加入…" : "开始转换"}</span>
        </button>
      </div>
    </GlassSurface>
  );
}