/**
 * 全局唯一的液态玻璃绘制画布。
 *
 * 合成顺序：背景图（模糊基底） → 液态玻璃容器（本组件） → DOM 内容 → 高亮反馈。
 * 每一帧：先用 BG_FRAG 铺满背景，再对每个已注册面板开启 Scissor 局部渲染玻璃。
 */

import { useEffect, useRef } from "react";
import { BG_FRAG, GLASS_FRAG, GLASS_VERT } from "./glassShader";
import { listGlass, subscribeGlass } from "./glassRegistry";

export type GlassQuality = "quality" | "balanced";

export interface GlassCanvasProps {
  imageUrl: string | null;
  fit: "cover" | "contain";
  /** 感知模糊半径（CSS px） */
  blurPx: number;
  /** 乳白着色强度 0..1 */
  tint: number;
  /** 亮度增益 */
  brightness: number;
  /** 玻璃基础不透明度 0..1 */
  glassOpacity: number;
  /** 背景不透明度 0..1，WebGL 档位下由着色器实现 */
  bgOpacity: number;
  /** 边缘色散强度（仅质量档生效） */
  dispersion: number;
  quality: GlassQuality;
  /** 首帧绘制完成（供截图比对与就绪提示使用） */
  onFirstFrame?: () => void;
  /** WebGL2 不可用或初始化失败时回调，由上层降级到 CSS 档 */
  onUnavailable: (reason: string) => void;
}

interface Program {
  program: WebGLProgram;
  uniforms: Map<string, WebGLUniformLocation | null>;
}

/** 取 uniform 位置；不存在时返回 null（WebGL 会静默忽略），避免到处断言 */
function u(program: Program, name: string): WebGLUniformLocation | null {
  return program.uniforms.get(name) ?? null;
}

interface CanvasApi {
  loadImage: (url: string | null) => void;
  invalidate: () => void;
}

const FALLBACK_RGB: [number, number, number] = [0.043, 0.184, 0.322];

function compileShader(
  gl: WebGL2RenderingContext,
  type: number,
  source: string,
  defines: string[],
): WebGLShader {
  const header = defines.length ? defines.join("\n") + "\n" : "";
  const shader = gl.createShader(type);
  if (!shader) throw new Error("createShader 失败");
  const withHeader = source.replace("#version 300 es", `#version 300 es\n${header}`);
  gl.shaderSource(shader, withHeader);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    // 某些驱动在编译失败时返回空日志，此时补充错误码与源码首行，避免出现「未知错误」
    const rawLog = gl.getShaderInfoLog(shader);
    const glError = gl.getError();
    const kind = type === gl.VERTEX_SHADER ? "顶点" : "片元";
    const firstLine = withHeader.split("\n")[0] ?? "";
    const detail =
      rawLog && rawLog.trim().length > 0
        ? rawLog.trim()
        : `驱动未返回日志（glError=0x${glError.toString(16)}，首行="${firstLine}"）`;
    // 保留源码，便于在控制台继续排查
    console.error(`[VideoFlow] ${kind}着色器编译失败`, detail, withHeader);
    gl.deleteShader(shader);
    throw new Error(`${kind}着色器编译失败: ${detail}`);
  }
  return shader;
}

function createProgram(
  gl: WebGL2RenderingContext,
  fragSource: string,
  defines: string[],
  uniformNames: string[],
): Program {
  const vs = compileShader(gl, gl.VERTEX_SHADER, GLASS_VERT, []);
  const fs = compileShader(gl, gl.FRAGMENT_SHADER, fragSource, defines);
  const program = gl.createProgram();
  if (!program) throw new Error("createProgram 失败");
  gl.attachShader(program, vs);
  gl.attachShader(program, fs);
  gl.bindAttribLocation(program, 0, "aPos");
  gl.linkProgram(program);
  gl.deleteShader(vs);
  gl.deleteShader(fs);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    const log = gl.getProgramInfoLog(program) ?? "未知错误";
    gl.deleteProgram(program);
    throw new Error(`着色器链接失败: ${log}`);
  }
  const uniforms = new Map<string, WebGLUniformLocation | null>();
  for (const name of uniformNames) uniforms.set(name, gl.getUniformLocation(program, name));
  return { program, uniforms };
}

/** 无用户图片时生成一张深蓝渐变兜底纹理，避免出现透明黑底 */
function makeFallbackImage(): HTMLCanvasElement {
  const size = 512;
  const cv = document.createElement("canvas");
  cv.width = size;
  cv.height = size;
  const ctx = cv.getContext("2d");
  if (ctx) {
    const grad = ctx.createLinearGradient(0, 0, size, size);
    grad.addColorStop(0, "#0d6cb8");
    grad.addColorStop(0.5, "#0b5e9f");
    grad.addColorStop(1, "#062f57");
    ctx.fillStyle = grad;
    ctx.fillRect(0, 0, size, size);
  }
  return cv;
}

export function GlassCanvas(props: GlassCanvasProps): React.JSX.Element {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const apiRef = useRef<CanvasApi | null>(null);
  const propsRef = useRef(props);
  propsRef.current = props;

  // 初始化：上下文、着色器、纹理与渲染循环
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    let disposed = false;
    let raf = 0;

    let gl: WebGL2RenderingContext | null = null;
    try {
      gl = canvas.getContext("webgl2", {
        alpha: false,
        antialias: false,
        depth: false,
        stencil: false,
        premultipliedAlpha: false,
        preserveDrawingBuffer: false,
        powerPreference: "high-performance",
      });
    } catch {
      gl = null;
    }
    if (!gl) {
      props.onUnavailable("WebGL2 不可用");
      return;
    }
    const ctx: WebGL2RenderingContext = gl;

    // 上下文可能已被同元素上的前一次挂载弄丢（例如 StrictMode 的双次挂载），
    // 此时任何 compile/link 都会以 CONTEXT_LOST_WEBGL 失败。
    if (ctx.isContextLost()) {
      props.onUnavailable("WebGL2 上下文已丢失");
      return;
    }

    const onContextLost = (event: Event) => {
      event.preventDefault();
      propsRef.current.onUnavailable("WebGL2 上下文丢失");
    };
    canvas.addEventListener("webglcontextlost", onContextLost, false);

    const GLASS_UNIFORMS = [
      "uBg", "uResolution", "uRect", "uImgSize", "uFit", "uRadius", "uBevel",
      "uThickness", "uIor", "uRefract", "uBlurLod", "uTint", "uBrightness",
      "uOpacity", "uDispersion", "uLightDir", "uBgOpacity", "uBgBase",
    ];
    const BG_UNIFORMS = ["uBg", "uResolution", "uImgSize", "uFit", "uFallback", "uBgOpacity"];

    let bgProgram: Program;
    let glassProgram: Program;
    let glassProgramPlain: Program;
    try {
      bgProgram = createProgram(ctx, BG_FRAG, [], BG_UNIFORMS);
      // 质量档：开启色散与额外抽样
      glassProgram = createProgram(ctx, GLASS_FRAG, ["#define GLASS_QUALITY"], GLASS_UNIFORMS);
      glassProgramPlain = createProgram(ctx, GLASS_FRAG, [], GLASS_UNIFORMS);
    } catch (err) {
      const reason = err instanceof Error ? err.message : "着色器初始化失败";
      props.onUnavailable(reason);
      return;
    }

    // 全屏三角形（比四边形少一次光栅化边界）
    const vao = ctx.createVertexArray();
    ctx.bindVertexArray(vao);
    const vbo = ctx.createBuffer();
    ctx.bindBuffer(ctx.ARRAY_BUFFER, vbo);
    ctx.bufferData(ctx.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), ctx.STATIC_DRAW);
    ctx.enableVertexAttribArray(0);
    ctx.vertexAttribPointer(0, 2, ctx.FLOAT, false, 0, 0);

    const texture = ctx.createTexture();
    ctx.bindTexture(ctx.TEXTURE_2D, texture);
    ctx.texParameteri(ctx.TEXTURE_2D, ctx.TEXTURE_WRAP_S, ctx.CLAMP_TO_EDGE);
    ctx.texParameteri(ctx.TEXTURE_2D, ctx.TEXTURE_WRAP_T, ctx.CLAMP_TO_EDGE);
    ctx.texParameteri(ctx.TEXTURE_2D, ctx.TEXTURE_MIN_FILTER, ctx.LINEAR_MIPMAP_LINEAR);
    ctx.texParameteri(ctx.TEXTURE_2D, ctx.TEXTURE_MAG_FILTER, ctx.LINEAR);

    let imgW = 1;
    let imgH = 1;
    let textureReady = false;
    let needsRender = true;
    let cssW = 0;
    let cssH = 0;
    let dpr = 1;
    let firstFrameDone = false;

    const requestRender = () => {
      needsRender = true;
    };

    /** 上一帧每个面板的落点（CSS px），用于低频比对 */
    const drawnRects = new Map<HTMLElement, [number, number, number, number]>();
    /** 位置容差：亚像素抖动不值得重绘 */
    const RECT_EPSILON = 0.5;

    /**
     * 有没有面板在页面上挪了位置。
     *
     * 这是「DOM 结构没变但布局变了」的兜底：文本换行、CSS 类切换、
     * 图片加载完撑开高度……都不会产生 childList 变更，光靠 MutationObserver 会漏。
     * 比对放在低频（每 3 帧）执行，避免每帧都强制一次布局计算。
     */
    const layoutChanged = () => {
      const entries = listGlass();
      if (entries.length !== drawnRects.size) return true;
      for (const entry of entries) {
        const prev = drawnRects.get(entry.el);
        if (!prev) return true;
        const r = entry.el.getBoundingClientRect();
        if (
          Math.abs(r.left - prev[0]) > RECT_EPSILON ||
          Math.abs(r.top - prev[1]) > RECT_EPSILON ||
          Math.abs(r.width - prev[2]) > RECT_EPSILON ||
          Math.abs(r.height - prev[3]) > RECT_EPSILON
        ) {
          return true;
        }
      }
      return false;
    };

    const uploadImage = (source: TexImageSource, w: number, h: number) => {
      if (disposed) return;
      imgW = Math.max(1, w);
      imgH = Math.max(1, h);
      ctx.bindTexture(ctx.TEXTURE_2D, texture);
      ctx.pixelStorei(ctx.UNPACK_FLIP_Y_WEBGL, false);
      ctx.texImage2D(ctx.TEXTURE_2D, 0, ctx.RGBA, ctx.RGBA, ctx.UNSIGNED_BYTE, source);
      ctx.generateMipmap(ctx.TEXTURE_2D);
      textureReady = true;
      requestRender();
    };

    const fallback = makeFallbackImage();
    uploadImage(fallback, fallback.width, fallback.height);

    let imageToken = 0;
    const loadImage = (url: string | null) => {
      const token = ++imageToken;
      // 切换图片时先保留当前纹理，加载完成后再替换，避免闪出底色
      if (!url) {
        uploadImage(fallback, fallback.width, fallback.height);
        return;
      }
      const img = new Image();
      img.crossOrigin = "anonymous";
      img.onload = () => {
        if (disposed || token !== imageToken) return;
        uploadImage(img, img.naturalWidth, img.naturalHeight);
      };
      img.onerror = () => {
        if (disposed || token !== imageToken) return;
        uploadImage(fallback, fallback.width, fallback.height);
      };
      img.src = url;
    };
    loadImage(props.imageUrl);

    const resize = () => {
      const rect = canvas.getBoundingClientRect();
      const nextDpr = Math.min(window.devicePixelRatio || 1, 2);
      const w = Math.max(1, Math.round(rect.width));
      const h = Math.max(1, Math.round(rect.height));
      if (w === cssW && h === cssH && nextDpr === dpr) return;
      cssW = w;
      cssH = h;
      dpr = nextDpr;
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
      requestRender();
    };
    const ro = new ResizeObserver(resize);
    ro.observe(canvas);
    resize();

    // 面板自身尺寸/位置变化也要重绘。
    // 只监听 canvas 是不够的：内容变化（例如空状态与解析结果切换）会改变面板高度，
    // 但画布尺寸不变，此时若不重绘，画布上会残留上一帧的面板位置。
    const panelObserver = new ResizeObserver(() => requestRender());
    const observed = new Set<HTMLElement>();
    const syncObserved = () => {
      const current = new Set(listGlass().map((e) => e.el));
      for (const el of observed) {
        if (!current.has(el)) {
          panelObserver.unobserve(el);
          observed.delete(el);
        }
      }
      for (const el of current) {
        if (!observed.has(el)) {
          panelObserver.observe(el);
          observed.add(el);
        }
      }
    };

    const unsubscribe = subscribeGlass(() => {
      syncObserved();
      requestRender();
    });
    syncObserved();

    // DOM 结构变化（列表插入、模式切换、条件渲染）会让面板整体移动，但尺寸不变，
    // 于是上面那个 ResizeObserver 不会触发 —— 玻璃就会留在上一帧的位置上，
    // 表现为「下拉框错位」（转换页导入文件后尤其明显：面板内部是滚动容器，
    // 列表撑高也不改变面板自身的尺寸）。
    // 只监听 childList / subtree：进度条改样式（attributes）、进度文字改内容
    // （characterData）都不该触发重绘，否则下载中的任务会每 250ms 刷一次画布。
    const domObserver = new MutationObserver(() => requestRender());
    domObserver.observe(document.body, { childList: true, subtree: true });

    const onScroll = () => requestRender();
    window.addEventListener("scroll", onScroll, true);

    apiRef.current = { loadImage, invalidate: requestRender };

    const drawBackground = (p: GlassCanvasProps) => {
      ctx.useProgram(bgProgram.program);
      ctx.activeTexture(ctx.TEXTURE0);
      ctx.bindTexture(ctx.TEXTURE_2D, texture);
      ctx.uniform1i(u(bgProgram, "uBg"), 0);
      ctx.uniform2f(u(bgProgram, "uResolution"), cssW, cssH);
      ctx.uniform2f(u(bgProgram, "uImgSize"), imgW, imgH);
      ctx.uniform1f(u(bgProgram, "uFit"), p.fit === "contain" ? 1 : 0);
      ctx.uniform3f(
        u(bgProgram, "uFallback"),
        FALLBACK_RGB[0],
        FALLBACK_RGB[1],
        FALLBACK_RGB[2],
      );
      ctx.uniform1f(u(bgProgram, "uBgOpacity"), p.bgOpacity);
      ctx.drawArrays(ctx.TRIANGLES, 0, 3);
    };

    /**
     * 面板祖先里所有「会裁剪的容器」的交集（视口坐标）。
     *
     * 为什么必须有这一层：面板本身常常是 `overflow: auto` 的滚动容器，
     * 里面的控件滚出可视区后，DOM 会被容器裁掉，但**画布不会** ——
     * `getBoundingClientRect()` 给的是未裁剪的矩形。结果就是玻璃整块画到
     * 面板外面，压在相邻面板上（用户报的「穿模」），并在面板边缘留下多余的一条线。
     */
    const clipCache = new Map<Element, { left: number; top: number; right: number; bottom: number }>();
    const clipRectOf = (el: HTMLElement) => {
      let left = 0;
      let top = 0;
      let right = window.innerWidth;
      let bottom = window.innerHeight;
      for (let node = el.parentElement; node; node = node.parentElement) {
        let box = clipCache.get(node);
        if (!box) {
          const cs = getComputedStyle(node);
          const clips =
            cs.overflow !== "visible" || cs.overflowX !== "visible" || cs.overflowY !== "visible";
          const rect = node.getBoundingClientRect();
          box = clips
            ? { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom }
            : { left: 0, top: 0, right: Number.POSITIVE_INFINITY, bottom: Number.POSITIVE_INFINITY };
          clipCache.set(node, box);
        }
        left = Math.max(left, box.left);
        top = Math.max(top, box.top);
        right = Math.min(right, box.right);
        bottom = Math.min(bottom, box.bottom);
        if (right <= left || bottom <= top) break;
      }
      return { left, top, right, bottom };
    };

    const drawPanels = (p: GlassCanvasProps) => {
      const entries = listGlass();
      drawnRects.clear();
      clipCache.clear();
      if (entries.length === 0) return;
      const prog = p.quality === "quality" ? glassProgram : glassProgramPlain;
      const browserW = canvas.clientWidth;
      const browserH = canvas.clientHeight;
      if (browserW === 0 || browserH === 0) return;

      ctx.useProgram(prog.program);
      ctx.activeTexture(ctx.TEXTURE0);
      ctx.bindTexture(ctx.TEXTURE_2D, texture);
      ctx.uniform1i(u(prog, "uBg"), 0);
      ctx.uniform2f(u(prog, "uResolution"), cssW, cssH);
      ctx.uniform2f(u(prog, "uImgSize"), imgW, imgH);
      ctx.uniform1f(u(prog, "uFit"), p.fit === "contain" ? 1 : 0);
      ctx.uniform1f(u(prog, "uIor"), 1.46);
      ctx.uniform1f(u(prog, "uDispersion"), p.dispersion);
      ctx.uniform1f(u(prog, "uBgOpacity"), p.bgOpacity);
      ctx.uniform3f(
        u(prog, "uBgBase"),
        FALLBACK_RGB[0],
        FALLBACK_RGB[1],
        FALLBACK_RGB[2],
      );
      // 屏幕空间光源：左上方掠射柔光（局部坐标 y 向下）
      ctx.uniform3f(u(prog, "uLightDir"), -0.42, -0.5, 0.76);

      // 感知模糊半径对应的 mip 层级。
      // displayScale 把 CSS 像素换算成背景图像素，直接决定 mip 层级的物理含义。
      const displayScale = Math.min(cssW / imgW, cssH / imgH);
      const lod = Math.max(0, Math.min(8, Math.log2(Math.max(1, p.blurPx * displayScale)) - 1.4));

      for (const entry of entries) {
        const rect = entry.el.getBoundingClientRect();
        // 记下落点供 layoutChanged 比对：被跳过的面板也要记，
        // 否则它会每帧都被判成「变了」，白白重绘
        drawnRects.set(entry.el, [rect.left, rect.top, rect.width, rect.height]);
        if (rect.width < 2 || rect.height < 2) continue;

        // 与祖先裁剪区求交：scissor 只画可见的那部分像素，
        // 但 uRect 仍给面板的真实矩形 —— 只裁像素，不改形状、折射与倒角
        const clip = clipRectOf(entry.el);
        const left = Math.max(rect.left, clip.left);
        const top = Math.max(rect.top, clip.top);
        const right = Math.min(rect.right, clip.right);
        const bottom = Math.min(rect.bottom, clip.bottom);
        if (right - left < 2 || bottom - top < 2) continue;
        if (bottom < 0 || right < 0 || top > browserH || left > browserW) continue;

        const sx = Math.round(left * dpr);
        const sy = Math.round((browserH - bottom) * dpr);
        const sw = Math.max(1, Math.round((right - left) * dpr));
        const sh = Math.max(1, Math.round((bottom - top) * dpr));
        ctx.scissor(sx, sy, sw, sh);

        ctx.uniform4f(u(prog, "uRect"), rect.left, rect.top, rect.width, rect.height);
        ctx.uniform1f(u(prog, "uRadius"), entry.radius);
        ctx.uniform1f(u(prog, "uBevel"), entry.bevel);
        ctx.uniform1f(u(prog, "uThickness"), entry.thickness);
        ctx.uniform1f(u(prog, "uRefract"), 1.0);
        ctx.uniform1f(u(prog, "uBlurLod"), lod);
        ctx.uniform1f(u(prog, "uTint"), entry.tint * p.tint);
        ctx.uniform1f(u(prog, "uBrightness"), entry.brightness * p.brightness);
        ctx.uniform1f(u(prog, "uOpacity"), entry.opacity * p.glassOpacity);
        ctx.drawArrays(ctx.TRIANGLES, 0, 3);
      }
    };

    let sweepTick = 0;
    const frame = () => {
      raf = requestAnimationFrame(frame);
      if (disposed || !textureReady) return;
      // 空闲时每 3 帧查一次「有没有面板挪位置」，兜住所有没产生结构变更的布局变化
      if (!needsRender && ++sweepTick % 3 === 0 && layoutChanged()) needsRender = true;
      if (!needsRender) return;
      needsRender = false;

      const p = propsRef.current;
      ctx.viewport(0, 0, canvas.width, canvas.height);
      ctx.disable(ctx.SCISSOR_TEST);
      ctx.disable(ctx.BLEND);
      drawBackground(p);

      ctx.enable(ctx.SCISSOR_TEST);
      ctx.enable(ctx.BLEND);
      ctx.blendFuncSeparate(
        ctx.SRC_ALPHA,
        ctx.ONE_MINUS_SRC_ALPHA,
        ctx.ONE,
        ctx.ONE_MINUS_SRC_ALPHA,
      );
      drawPanels(p);
      ctx.disable(ctx.SCISSOR_TEST);

      if (!firstFrameDone) {
        firstFrameDone = true;
        propsRef.current.onFirstFrame?.();
      }
    };
    raf = requestAnimationFrame(frame);

    return () => {
      disposed = true;
      cancelAnimationFrame(raf);
      apiRef.current = null;
      ro.disconnect();
      panelObserver.disconnect();
      observed.clear();
      domObserver.disconnect();
      drawnRects.clear();
      clipCache.clear();
      unsubscribe();
      window.removeEventListener("scroll", onScroll, true);
      canvas.removeEventListener("webglcontextlost", onContextLost, false);
      ctx.deleteTexture(texture);
      ctx.deleteBuffer(vbo);
      ctx.deleteVertexArray(vao);
      ctx.deleteProgram(bgProgram.program);
      ctx.deleteProgram(glassProgram.program);
      ctx.deleteProgram(glassProgramPlain.program);
      // 注意：这里绝不调用 WEBGL_lose_context.loseContext()。
      // 该 canvas 元素会被下一次挂载复用，而 getContext 对同一元素只会返回同一个
      // 上下文对象；一旦主动丢失，重挂载后拿到的就是死上下文，所有 compile 都会以
      // CONTEXT_LOST_WEBGL 失败。资源已在上面逐个释放，无需丢失上下文。
    };
    // 只在挂载/卸载时初始化；运行时参数通过 propsRef 在帧内读取
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 图片变化 → 重新上传纹理
  useEffect(() => {
    apiRef.current?.loadImage(props.imageUrl);
  }, [props.imageUrl]);

  // 视觉参数变化 → 请求重绘
  useEffect(() => {
    apiRef.current?.invalidate();
  }, [
    props.fit,
    props.blurPx,
    props.tint,
    props.brightness,
    props.glassOpacity,
    props.bgOpacity,
    props.dispersion,
    props.quality,
  ]);

  return <canvas ref={canvasRef} className="vf-glass-canvas" aria-hidden="true" />;
}