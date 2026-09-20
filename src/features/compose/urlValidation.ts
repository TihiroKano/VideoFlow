/**
 * URL 规范化与校验（项目书 §3.1）。
 * 前端只做格式与 scheme 检查；SSRF 防护（私网、回环、DNS 重校验）由主进程负责。
 */

export const MAX_URL_LENGTH = 2048;
/** 主进程一次最多解析的链接数，与 src-tauri/src/commands/resolve.rs 保持一致 */
export const MAX_URLS_PER_BATCH = 50;

export interface UrlIssue {
  code: "URL_EMPTY" | "URL_TOO_LONG" | "URL_MALFORMED" | "URL_SCHEME" | "URL_PRIVATE";
  message: string;
}

export interface UrlCheckResult {
  ok: boolean;
  normalized: string;
  issue?: UrlIssue;
}

/** 私网 / 回环 / 链路本地与本地文件协议：前端先拦一层，主进程再校验一次 */
const PRIVATE_HOST_PATTERNS: RegExp[] = [
  /^localhost$/i,
  /^127\./,
  /^10\./,
  /^192\.168\./,
  /^172\.(1[6-9]|2\d|3[01])\./,
  /^169\.254\./,
  /^0\./,
  /^\[?::1\]?$/,
  /\.local$/i,
];

export function isPrivateHost(hostname: string): boolean {
  if (!hostname) return true;
  return PRIVATE_HOST_PATTERNS.some((re) => re.test(hostname));
}

/**
 * 从整段文本里挑出第一个链接。
 *
 * 抖音、快手的「分享」永远是「口令 + 链接 + 提示语」一整段，用户习惯全选复制，
 * 粘进来的并不是纯链接。先摘出链接，否则会被判成「格式不正确」。
 * 中文字符与全角标点一律排除，避免把紧跟其后的说明文字吃进链接里。
 */
const URL_IN_TEXT = /https?:\/\/[^\s\u4e00-\u9fff\u3000-\u303f\uff00-\uffef`"'<>[\](){}【】]+/i;

export function extractUrl(raw: string): string {
  const trimmed = raw.trim();
  return URL_IN_TEXT.exec(trimmed)?.[0] ?? trimmed;
}

export function checkUrl(raw: string): UrlCheckResult {
  const trimmed = extractUrl(raw);
  if (!trimmed) {
    return { ok: false, normalized: "", issue: { code: "URL_EMPTY", message: "请输入视频链接" } };
  }
  if (trimmed.length > MAX_URL_LENGTH) {
    return {
      ok: false,
      normalized: trimmed,
      issue: { code: "URL_TOO_LONG", message: `链接长度超过 ${MAX_URL_LENGTH} 字符` },
    };
  }

  // 用户常直接粘贴 "www.xxx.com/..."，补全 scheme 后再校验
  const candidate = /^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed) ? trimmed : `https://${trimmed}`;

  let url: URL;
  try {
    url = new URL(candidate);
  } catch {
    return {
      ok: false,
      normalized: trimmed,
      issue: { code: "URL_MALFORMED", message: "链接格式不正确，请检查后重试" },
    };
  }

  if (url.protocol !== "https:") {
    return {
      ok: false,
      normalized: trimmed,
      issue: {
        code: "URL_SCHEME",
        message: `仅支持 https 链接，当前为 ${url.protocol.replace(":", "")}`,
      },
    };
  }

  if (isPrivateHost(url.hostname)) {
    return {
      ok: false,
      normalized: trimmed,
      issue: { code: "URL_PRIVATE", message: "出于安全考虑，不解析内网或本机地址" },
    };
  }

  return { ok: true, normalized: url.toString() };
}