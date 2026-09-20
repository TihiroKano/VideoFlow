/**
 * 玻璃面板注册表
 *
 * 约束（项目书 §5.1）：全局只有一个 WebGL2 Canvas，禁止每个组件各建一个 context。
 * 组件通过 GlassSurface 声明「我需要玻璃」，由 GlassCanvas 统一以 Scissor 分区绘制。
 */

export interface GlassEntry {
  /** 稳定标识，用于调试与去重 */
  id: string;
  /** 关联的 DOM 宿主元素，位置由 getBoundingClientRect 实时读取 */
  el: HTMLElement;
  /** 圆角半径（CSS px） */
  radius: number;
  /** 倒角宽度（CSS px） */
  bevel: number;
  /** 视觉厚度，影响折射位移量与高度场 */
  thickness: number;
  /** 乳白着色强度 0..1 */
  tint: number;
  /** 基础不透明度 0..1 */
  opacity: number;
  /** 亮度增益 */
  brightness: number;
  /** 绘制顺序（越小越先绘制） */
  order: number;
}

const entries = new Map<HTMLElement, GlassEntry>();
const listeners = new Set<() => void>();
let orderSeq = 0;

function notify(): void {
  for (const fn of listeners) fn();
}

export function registerGlass(
  entry: Omit<GlassEntry, "order"> & { order?: number },
): () => void {
  const full: GlassEntry = { ...entry, order: entry.order ?? orderSeq++ };
  entries.set(entry.el, full);
  notify();

  return () => {
    entries.delete(entry.el);
    notify();
  };
}

/** 更新已注册面板的视觉参数（不改变位置） */
export function updateGlass(el: HTMLElement, patch: Partial<GlassEntry>): void {
  const cur = entries.get(el);
  if (!cur) return;
  entries.set(el, { ...cur, ...patch });
  notify();
}

/**
 * 按文档顺序返回面板：父元素先绘制，子元素（嵌套的控件）后绘制，
 * 保证嵌套玻璃的层叠关系与 DOM 一致。
 * React 的子组件 effect 先于父组件执行，因此不能依赖注册先后。
 */
export function listGlass(): GlassEntry[] {
  const all = [...entries.values()];
  all.sort((a, b) => {
    if (a.el === b.el) return 0;
    const rel = a.el.compareDocumentPosition(b.el);
    if (rel & Node.DOCUMENT_POSITION_FOLLOWING) return -1;
    if (rel & Node.DOCUMENT_POSITION_PRECEDING) return 1;
    return a.order - b.order;
  });
  return all;
}

export function subscribeGlass(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/**
 * 请求重绘玻璃画布。
 *
 * 布局变化（页面切换、模式切换、列表增删）不一定伴随画布尺寸或面板尺寸变化，
 * 此时需要显式请求重绘，否则画布上会残留上一帧的面板位置。
 */
export function notifyGlassRedraw(): void {
  notify();
}

/** 读取元素实际生效的圆角半径，保证 CSS 与着色器一致 */
export function readRadius(el: HTMLElement): number {
  const raw = getComputedStyle(el).borderTopLeftRadius;
  const parsed = Number.parseFloat(raw);
  return Number.isFinite(parsed) ? parsed : 0;
}