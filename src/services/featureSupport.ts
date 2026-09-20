/** 运行时能力探测：决定 CSS 档位下玻璃材质的可用形态。 */

let backdropFilterCache: boolean | null = null;

/** WebView2 支持 backdrop-filter；不支持时退化为半透明实色 + 描边 */
export function supportsBackdropFilter(): boolean {
  if (backdropFilterCache !== null) return backdropFilterCache;
  if (typeof CSS === "undefined" || typeof CSS.supports !== "function") {
    backdropFilterCache = false;
    return false;
  }
  backdropFilterCache =
    CSS.supports("backdrop-filter", "blur(4px)") ||
    CSS.supports("-webkit-backdrop-filter", "blur(4px)");
  return backdropFilterCache;
}