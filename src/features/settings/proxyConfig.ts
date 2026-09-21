/**
 * 网络出口（代理）配置的纯函数助手（项目书 §3.1 的网络层）。
 *
 * 后端只认两样东西：模式（direct / system / custom）与一个完整的代理地址字符串。
 * 界面上拆成「类型 + 地址」两栏更好填，所以合成与反解都在这里做，并配单测。
 *
 * 与后端 `src-tauri/src/net/proxy.rs` 的 `normalize` 保持同一套规则：
 * 只收 http / https / socks5 / socks5h，必须带端口，不接受路径与查询。
 *
 * 这里**刻意不用 `URL` 解析端口**：`new URL("http://a:80")` 会把默认端口吃掉
 * （`port` 变成空串），往返一圈地址就被改坏了。地址形状很窄，手写拆分更可靠。
 */

/** 代理类型：值即 URL scheme */
export type ProxyType = "http" | "https" | "socks5" | "socks5h";

export type ProxyMode = "direct" | "system" | "custom";

export const PROXY_TYPES: { value: ProxyType; label: string; hint: string }[] = [
  { value: "http", label: "HTTP", hint: "最常见的本地代理端口（Clash 的混合端口也支持）" },
  { value: "https", label: "HTTPS", hint: "代理本身走 TLS，较少见" },
  { value: "socks5", label: "SOCKS5", hint: "域名由本机解析，DNS 被污染时仍会失败" },
  {
    value: "socks5h",
    label: "SOCKS5H",
    hint: "域名交给代理端解析，DNS 被污染时用这个",
  },
];

export const PROXY_MODES: { value: ProxyMode; label: string; hint: string }[] = [
  {
    value: "system",
    label: "系统代理",
    hint: "跟随 Windows 的「系统代理」开关（Clash / v2rayN 的系统代理模式）",
  },
  { value: "direct", label: "直连", hint: "不使用任何代理，忽略系统代理与代理环境变量" },
  { value: "custom", label: "自定义代理", hint: "本应用单独走这个代理，不影响系统设置" },
];

export interface ProxyForm {
  type: ProxyType;
  /** host:port（也允许直接粘贴带 scheme 的完整地址） */
  host: string;
}

const MAX_LENGTH = 256;
const ALLOWED_TYPES: ProxyType[] = ["http", "https", "socks5", "socks5h"];

interface Authority {
  /** 形如 `user:pass@`，没有凭据时为空串 */
  userinfo: string;
  host: string;
  port: string;
}

/** 拆 `[user:pass@]host:port`；形状不对返回 null */
function splitAuthority(rest: string): Authority | null {
  const matched = /^(?:([^@/?#]+)@)?(\[[^\]]+\]|[^:/?#@]+):(\d{1,5})$/.exec(rest.trim());
  if (!matched) return null;
  return {
    userinfo: matched[1] ? `${matched[1]}@` : "",
    host: matched[2] ?? "",
    port: matched[3] ?? "",
  };
}

/** 拆出 scheme 与后面的部分；没写 scheme 时返回 null */
function splitScheme(raw: string): { scheme: string; rest: string } | null {
  const index = raw.indexOf("://");
  if (index < 0) return null;
  return { scheme: raw.slice(0, index).toLowerCase(), rest: raw.slice(index + 3) };
}

/** 解析失败时返回原因；null 表示合法 */
export function validateProxyHost(raw: string): string | null {
  const trimmed = raw.trim();
  if (!trimmed) return "请填写代理地址，例如 127.0.0.1:7890";
  if (trimmed.length > MAX_LENGTH) return "地址过长，请检查是否粘贴错了";

  const withScheme = splitScheme(trimmed);
  if (withScheme && !ALLOWED_TYPES.includes(withScheme.scheme as ProxyType)) {
    return `不支持的代理类型 ${withScheme.scheme}，只支持 HTTP / HTTPS / SOCKS5 / SOCKS5H`;
  }

  const authority = splitAuthority(withScheme ? withScheme.rest : trimmed);
  if (!authority) return "地址需要写成 主机:端口，例如 127.0.0.1:7890";
  if (authority.port === "0") return "端口不能为 0";
  if (Number(authority.port) > 65535) return "端口超出范围";
  return null;
}

/** 表单 → 后端要的完整地址；地址非法时返回空串（后端会按直连处理） */
export function composeProxyUrl(form: ProxyForm): string {
  if (validateProxyHost(form.host) !== null) return "";
  const trimmed = form.host.trim();
  const withScheme = splitScheme(trimmed);
  const authority = splitAuthority(withScheme ? withScheme.rest : trimmed);
  if (!authority) return "";
  // 类型以下拉所选为准：粘贴的完整地址里带的 scheme 只是它的来源
  return `${form.type}://${authority.userinfo}${authority.host}:${authority.port}`;
}

/** 完整地址 → 表单（用于回显已保存的设置） */
export function parseProxyUrl(url: string): ProxyForm {
  const trimmed = url.trim();
  if (!trimmed) return { type: "http", host: "" };

  const withScheme = splitScheme(trimmed);
  const scheme = withScheme ? withScheme.scheme : "http";
  const type = ALLOWED_TYPES.includes(scheme as ProxyType) ? (scheme as ProxyType) : "http";
  const authority = splitAuthority(withScheme ? withScheme.rest : trimmed);
  if (!authority) return { type, host: trimmed };
  return { type, host: `${authority.host}:${authority.port}` };
}

/** 显示用：去掉凭据 */
export function redactProxyUrl(url: string): string {
  const trimmed = url.trim();
  if (!trimmed) return "";
  const withScheme = splitScheme(trimmed);
  const authority = splitAuthority(withScheme ? withScheme.rest : trimmed);
  if (!authority) return trimmed;
  const scheme = withScheme ? withScheme.scheme : "http";
  return `${scheme}://${authority.host}:${authority.port}`;
}
