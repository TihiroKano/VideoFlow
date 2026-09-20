/**
 * 首次启动设备探测与渲染档位推荐（项目书 §6）。
 *
 * 只做本地能力探测，不上传任何硬件指纹；用户手动选择永久优先于自动推荐。
 */

export type GlassTier = "quality" | "balanced" | "performance";

export interface DeviceProfile {
  recommendation: GlassTier;
  reasons: string[];
  webgl2: boolean;
  floatRenderTarget: boolean;
  rendererName: string;
  cores: number;
  deviceMemoryGB: number | null;
  frameP95Ms: number | null;
  reducedMotion: boolean;
}

interface GlProbe {
  ok: boolean;
  rendererName: string;
  floatRenderTarget: boolean;
}

function probeWebGL2(): GlProbe {
  const canvas = document.createElement("canvas");
  canvas.width = 16;
  canvas.height = 16;
  let gl: WebGL2RenderingContext | null = null;
  try {
    gl = canvas.getContext("webgl2", { failIfMajorPerformanceCaveat: false });
  } catch {
    gl = null;
  }
  if (!gl) return { ok: false, rendererName: "不可用", floatRenderTarget: false };

  let rendererName = "未知";
  const dbg = gl.getExtension("WEBGL_debug_renderer_info");
  if (dbg) {
    const raw = gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL);
    if (typeof raw === "string") rendererName = raw;
  } else {
    const raw = gl.getParameter(gl.RENDERER);
    if (typeof raw === "string") rendererName = raw;
  }

  const floatRenderTarget = gl.getExtension("EXT_color_buffer_float") !== null;
  gl.getExtension("WEBGL_lose_context")?.loseContext();
  return { ok: true, rendererName, floatRenderTarget };
}

/** 120 帧 rAF 微基准，取 p95 帧时间（项目书 §6.1） */
function measureFrameTime(samples = 120): Promise<number | null> {
  return new Promise((resolve) => {
    const times: number[] = [];
    let last = performance.now();
    let count = 0;

    const tick = (now: number) => {
      times.push(now - last);
      last = now;
      count += 1;
      if (count >= samples) {
        // 丢弃前 20 帧预热
        const trimmed = times.slice(20).sort((a, b) => a - b);
        if (trimmed.length === 0) {
          resolve(null);
          return;
        }
        const idx = Math.min(trimmed.length - 1, Math.floor(trimmed.length * 0.95));
        resolve(trimmed[idx] ?? null);
        return;
      }
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
}

function looksDiscrete(rendererName: string): boolean {
  const s = rendererName.toLowerCase();
  return /(nvidia|geforce|rtx|gtx|radeon|rx\s?\d|arc\s?a\d|quadro|apple m\d)/.test(s);
}

export async function profileDevice(): Promise<DeviceProfile> {
  const reasons: string[] = [];
  const gl = probeWebGL2();
  const cores = navigator.hardwareConcurrency || 0;
  const nav = navigator as Navigator & { deviceMemory?: number };
  const deviceMemoryGB = typeof nav.deviceMemory === "number" ? nav.deviceMemory : null;
  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  const frameP95Ms = gl.ok ? await measureFrameTime() : null;

  let recommendation: GlassTier;
  if (!gl.ok) {
    recommendation = "performance";
    reasons.push("WebGL2 不可用");
  } else if (reducedMotion) {
    recommendation = "performance";
    reasons.push("系统开启了减少动效");
  } else if (deviceMemoryGB !== null && deviceMemoryGB < 4) {
    recommendation = "performance";
    reasons.push(`内存约 ${deviceMemoryGB} GB，低于 4 GB`);
  } else if (
    cores >= 8 &&
    (deviceMemoryGB === null || deviceMemoryGB >= 8) &&
    gl.floatRenderTarget &&
    looksDiscrete(gl.rendererName) &&
    (frameP95Ms === null || frameP95Ms <= 18)
  ) {
    recommendation = "quality";
    reasons.push(`GPU：${gl.rendererName}`);
    reasons.push(`${cores} 个逻辑核心`);
    if (frameP95Ms !== null) reasons.push(`帧时间 p95 ${frameP95Ms.toFixed(1)} ms`);
  } else {
    recommendation = "balanced";
    reasons.push("WebGL2 可用");
    if (deviceMemoryGB !== null) reasons.push(`内存约 ${deviceMemoryGB} GB`);
    else reasons.push("内存信息不可得");
    if (frameP95Ms !== null) reasons.push(`帧时间 p95 ${frameP95Ms.toFixed(1)} ms`);
    reasons.push("不建议开启实时多层折射");
  }

  if (recommendation !== "performance" && frameP95Ms !== null && frameP95Ms > 28) {
    recommendation = "performance";
    reasons.push("帧时间 p95 超过 28 ms，已下沉到性能档");
  }

  return {
    recommendation,
    reasons,
    webgl2: gl.ok,
    floatRenderTarget: gl.floatRenderTarget,
    rendererName: gl.rendererName,
    cores,
    deviceMemoryGB,
    frameP95Ms,
    reducedMotion,
  };
}