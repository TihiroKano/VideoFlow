//! 网络出口配置：Direct / System / Custom 三种模式与代理地址归一化。
//!
//! 这里是**唯一的出口真相源**：reqwest 客户端、XDown 客户端、yt-dlp 的 `--proxy`、
//! FFmpeg 的 `-http_proxy`、WebView2 的 `--proxy-server` 都从 `ProxyConfig` 取地址，
//! 避免出现「应用走代理、下载不走代理」这类不一致。
//!
//! 纯逻辑，不依赖 Tauri，可进 `--no-default-features` 测试集。

use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::{AppError, AppResult};

/// 代理地址长度上限：正常代理串不会超过这个长度，超了多半是粘贴错了
const MAX_PROXY_URL_LEN: usize = 256;

/// 设置里的默认模式。
///
/// **必须是 system 而不是 direct**：reqwest 默认跟随系统代理，改默认值会让现有
/// 依赖系统代理的用户全部回归（直连失败）。
pub const DEFAULT_PROXY_MODE: &str = "system";

/// 网络出口模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    /// 直连：显式关闭系统代理与环境变量代理
    Direct,
    /// 跟随系统代理（Windows 读 HKCU 的 Internet Settings）
    System,
    /// 自定义代理（http / https / socks5 / socks5h）
    Custom,
}

impl ProxyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ProxyMode::Direct => "direct",
            ProxyMode::System => "system",
            ProxyMode::Custom => "custom",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ProxyMode::Direct => "直连",
            ProxyMode::System => "系统代理",
            ProxyMode::Custom => "自定义代理",
        }
    }

    /// 解析设置里的字符串；取值非法时返回 None（调用方按默认值处理）
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "direct" => Some(ProxyMode::Direct),
            "system" => Some(ProxyMode::System),
            "custom" => Some(ProxyMode::Custom),
            _ => None,
        }
    }
}

/// 应用设置中的网络出口
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyConfig {
    pub mode: ProxyMode,
    /// 自定义代理地址（含 scheme），仅 Custom 模式使用
    pub custom_url: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            mode: ProxyMode::System,
            custom_url: String::new(),
        }
    }
}

impl ProxyConfig {
    /// 从设置字段构造；mode 取值非法时退回默认（system）
    pub fn from_settings(mode: &str, custom_url: &str) -> Self {
        Self {
            mode: ProxyMode::parse(mode).unwrap_or(ProxyMode::System),
            custom_url: custom_url.trim().to_string(),
        }
    }

    /// 出口决策（纯函数，不读环境与注册表，便于单测断言）
    pub fn policy(&self) -> ProxyPolicy {
        match self.mode {
            ProxyMode::Direct => ProxyPolicy::Direct,
            ProxyMode::System => ProxyPolicy::System,
            ProxyMode::Custom => match normalize(&self.custom_url) {
                Ok(url) => ProxyPolicy::Explicit(url),
                Err(_) => ProxyPolicy::Direct,
            },
        }
    }

    /// 本次请求**实际**会用的代理地址；None 表示直连。
    ///
    /// System 模式在这里实时读环境变量与注册表：系统代理开关是应用外部状态，
    /// 缓存住会变成「改了系统代理必须重启应用」。
    pub fn effective_url(&self) -> Option<String> {
        match self.policy() {
            ProxyPolicy::Direct => None,
            ProxyPolicy::System => system_proxy_url(),
            ProxyPolicy::Explicit(url) => Some(url),
        }
    }

    /// 是否真的会走代理。
    ///
    /// SSRF guard 的 DNS 复校只看这个：走代理时域名由代理端解析，本机 DNS 结果
    /// 既不参与连接、在污染网络下还会把公网域名判成内网地址。
    pub fn is_active(&self) -> bool {
        self.effective_url().is_some()
    }

    /// 当前出口的地址来源说明（诊断面板用）
    pub fn source_label(&self) -> &'static str {
        match self.mode {
            ProxyMode::Direct => "直连（不使用代理）",
            ProxyMode::System => "系统代理",
            ProxyMode::Custom => "自定义代理",
        }
    }

    /// 供显示与日志用的脱敏地址（去掉用户名密码）
    pub fn redacted(&self) -> Option<String> {
        self.effective_url().map(|u| redact(&u))
    }

    /// 自定义地址是否已填写且合法（设置页据此给校验提示）
    pub fn custom_valid(&self) -> bool {
        matches!(self.policy(), ProxyPolicy::Explicit(_))
    }
}

/// 出口决策：纯函数产物，builder 与单测都断言它
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyPolicy {
    /// 显式直连（关掉 reqwest 的自动系统代理）
    Direct,
    /// 跟随系统代理（地址在真正建库时实时探测）
    System,
    /// 使用这个地址
    Explicit(String),
}

/// 归一化代理地址。
///
/// 接受三种输入：`127.0.0.1:7897`（按 http 处理）、`http://…` / `https://…`、
/// `socks5://…` / `socks5h://…`。拒绝：空、超长、未知 scheme、缺端口、带路径或查询。
pub fn normalize(raw: &str) -> AppResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::setting_invalid("代理地址不能为空"));
    }
    if trimmed.len() > MAX_PROXY_URL_LEN {
        return Err(AppError::setting_invalid("代理地址过长，请检查是否粘贴错了"));
    }

    // 用户更可能直接填 127.0.0.1:7897，没写 scheme 时按 http 处理
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };

    let parsed = Url::parse(&with_scheme)
        .map_err(|_| AppError::setting_invalid(format!("代理地址无法识别：{trimmed}")))?;

    match parsed.scheme() {
        "http" | "https" | "socks5" | "socks5h" => {}
        other => {
            return Err(AppError::setting_invalid(format!(
                "不支持的代理类型 {other}，只支持 HTTP / HTTPS / SOCKS5 / SOCKS5H"
            )))
        }
    }

    let host = parsed.host_str().unwrap_or("").trim().to_string();
    if host.is_empty() {
        return Err(AppError::setting_invalid("代理地址缺少主机名"));
    }
    let Some(port) = parsed.port() else {
        return Err(AppError::setting_invalid("代理地址需要写端口，例如 127.0.0.1:7890"));
    };
    if port == 0 {
        return Err(AppError::setting_invalid("代理端口不能为 0"));
    }
    if !matches!(parsed.path(), "" | "/") || parsed.query().is_some() {
        return Err(AppError::setting_invalid("代理地址不应带路径或查询参数"));
    }

    // 自己拼回去：`Url::to_string()` 会给 http(s) 补一个尾斜杠，写进 yt-dlp 与
    // 注册表对比时都不好看。凭据（若有）原样保留，显示时由 `redact` 去掉。
    let userinfo = if parsed.username().is_empty() {
        String::new()
    } else {
        match parsed.password() {
            Some(pwd) => format!("{}:{}@", parsed.username(), pwd),
            None => format!("{}@", parsed.username()),
        }
    };
    Ok(format!("{}://{userinfo}{host}:{port}", parsed.scheme()))
}

/// 脱敏：显示与日志里不出现代理凭据
pub fn redact(url: &str) -> String {
    let Ok(parsed) = Url::parse(url) else {
        return url.to_string();
    };
    if parsed.username().is_empty() {
        return url.to_string();
    }
    let host = parsed.host_str().unwrap_or("");
    let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
    format!("{}://{host}{port}", parsed.scheme())
}

/// 解析 Windows 注册表里的 `ProxyServer`。
///
/// 三种真实写法都要认：
/// - `127.0.0.1:7897`
/// - `http=127.0.0.1:7897;https=127.0.0.1:7897`（Clash / v2rayN 常见）
/// - `socks=127.0.0.1:1080`
///
/// 同一份值里有多个协议时优先 https，其次 http，最后 socks：HTTPS 请求是我们
/// 的主要流量，取它最贴近实际行为。
pub fn parse_registry_proxy_server(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if !raw.contains('=') {
        return normalize(raw).ok();
    }

    let mut https = None;
    let mut http = None;
    let mut socks = None;
    for part in raw.split(';') {
        let Some((kind, value)) = part.split_once('=') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let kind = kind.trim().to_ascii_lowercase();
        match kind.as_str() {
            "https" => https = entry_url(&kind, value),
            "http" => http = entry_url(&kind, value),
            "socks" | "socks5" | "socks4" => socks = entry_url(&kind, value),
            _ => {}
        }
    }
    https.or(http).or(socks)
}

/// 注册表条目 → 归一化地址：值里没写 scheme 时按条目类型补
fn entry_url(kind: &str, value: &str) -> Option<String> {
    if value.contains("://") {
        return normalize(value).ok();
    }
    match kind {
        "socks" | "socks5" | "socks4" => normalize(&format!("socks5://{value}")).ok(),
        _ => normalize(value).ok(),
    }
}

/// 环境变量里的代理地址（与 hyper-util 的读取口径一致：ALL_PROXY 优先）
pub fn env_proxy_url(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    const KEYS: [&str; 6] = [
        "ALL_PROXY",
        "all_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ];
    KEYS.iter().find_map(|key| {
        let value = get(key)?;
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        normalize(value).ok()
    })
}

/// 系统代理地址：环境变量优先，其次 Windows 注册表
pub fn system_proxy_url_from(
    env_get: impl Fn(&str) -> Option<String>,
    registry_raw: Option<&str>,
) -> Option<String> {
    if let Some(url) = env_proxy_url(env_get) {
        return Some(url);
    }
    registry_raw.and_then(parse_registry_proxy_server)
}

/// 实时探测本机的系统代理地址（None = 没有可用代理）
pub fn system_proxy_url() -> Option<String> {
    system_proxy_url_from(|key| std::env::var(key).ok(), registry_proxy_server().as_deref())
}

/// Chromium / WebView2 的代理启动参数。
///
/// - Direct → `--no-proxy-server`：真正直连，而不是「跟随系统」；
/// - System → None：WebView2 默认就跟随系统代理，不要画蛇添足；
/// - Custom → `--proxy-server=<url>`。Chromium **不认 `socks5h`**（也不支持带认证的
///   SOCKS），所以 socks5h 降级成 socks5（域名改由本机解析）、凭据一律去掉。
pub fn browser_proxy_arg(cfg: &ProxyConfig) -> Option<String> {
    match cfg.mode {
        ProxyMode::Direct => Some("--no-proxy-server".to_string()),
        ProxyMode::System => None,
        ProxyMode::Custom => {
            // 地址非法时保持 WebView2 默认行为，而不是悄悄改成直连
            let url = normalize(&cfg.custom_url).ok()?;
            let parsed = Url::parse(&url).ok()?;
            let host = parsed.host_str()?;
            let port = parsed.port()?;
            let scheme = match parsed.scheme() {
                "socks5h" => "socks5",
                other => other,
            };
            Some(format!("--proxy-server={scheme}://{host}:{port}"))
        }
    }
}

/// Windows 的「系统代理」开关（Clash、v2rayN 等的系统代理模式就写在这里）
#[cfg(windows)]
fn registry_proxy_server() -> Option<String> {
    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    let settings = windows_registry::CURRENT_USER.open(KEY).ok()?;
    if settings.get_u32("ProxyEnable").unwrap_or(0) == 0 {
        return None;
    }
    settings
        .get_string("ProxyServer")
        .ok()
        .filter(|v| !v.trim().is_empty())
}

#[cfg(not(windows))]
fn registry_proxy_server() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 没写_scheme_时按_http_处理() {
        assert_eq!(normalize("127.0.0.1:7897").unwrap(), "http://127.0.0.1:7897");
        assert_eq!(
            normalize(" 192.168.1.2:1080 ").unwrap(),
            "http://192.168.1.2:1080"
        );
    }

    #[test]
    fn 四种_scheme_都接受且去掉尾斜杠() {
        assert_eq!(
            normalize("http://127.0.0.1:7890/").unwrap(),
            "http://127.0.0.1:7890"
        );
        assert_eq!(
            normalize("https://proxy.example.com:8443").unwrap(),
            "https://proxy.example.com:8443"
        );
        assert_eq!(
            normalize("socks5://127.0.0.1:1080").unwrap(),
            "socks5://127.0.0.1:1080"
        );
        // socks5h：域名交给代理端解析，污染网络下必须用这个
        assert_eq!(
            normalize("socks5h://127.0.0.1:1080").unwrap(),
            "socks5h://127.0.0.1:1080"
        );
    }

    #[test]
    fn 凭据保留但显示时脱敏() {
        let normalized = normalize("http://user:secret@127.0.0.1:7890").unwrap();
        assert!(normalized.contains("user:secret@"), "实际：{normalized}");
        let redacted = redact(&normalized);
        assert_eq!(redacted, "http://127.0.0.1:7890");
        assert!(!redacted.contains("secret"));
        // 没有凭据时原样返回
        assert_eq!(redact("socks5h://127.0.0.1:1080"), "socks5h://127.0.0.1:1080");
    }

    #[test]
    fn 非法地址被拒绝() {
        for bad in [
            "",
            "   ",
            "http://",
            "ftp://127.0.0.1:21",
            "127.0.0.1",          // 缺端口
            "http://127.0.0.1:0", // 端口 0
            "http://127.0.0.1:7890/path",
            "http://127.0.0.1:7890/?a=1",
        ] {
            assert!(normalize(bad).is_err(), "{bad} 应被拒绝");
        }
        assert!(normalize(&format!("http://127.0.0.1:{}", 1)).is_ok());
        assert!(normalize(&"a".repeat(MAX_PROXY_URL_LEN + 1)).is_err());
    }

    #[test]
    fn 注册表三种写法都能解析() {
        // 纯 host:port
        assert_eq!(
            parse_registry_proxy_server("127.0.0.1:7897").unwrap(),
            "http://127.0.0.1:7897"
        );
        // 按协议列出（Clash / v2rayN）：优先 https
        assert_eq!(
            parse_registry_proxy_server("http=127.0.0.1:7897;https=127.0.0.1:7898").unwrap(),
            "http://127.0.0.1:7898"
        );
        // 只有 http 时取 http
        assert_eq!(
            parse_registry_proxy_server("http=127.0.0.1:7897").unwrap(),
            "http://127.0.0.1:7897"
        );
        // socks 条目补上 socks5 scheme
        assert_eq!(
            parse_registry_proxy_server("socks=127.0.0.1:1080").unwrap(),
            "socks5://127.0.0.1:1080"
        );
        // 空值、垃圾值、只有分号
        assert!(parse_registry_proxy_server("").is_none());
        assert!(parse_registry_proxy_server("   ").is_none());
        assert!(parse_registry_proxy_server(";;").is_none());
        assert!(parse_registry_proxy_server("ftp=x:1").is_none());
    }

    #[test]
    fn 环境变量代理优先于注册表() {
        let env = |k: &str| (k == "HTTPS_PROXY").then(|| "http://127.0.0.1:1111".to_string());
        assert_eq!(
            system_proxy_url_from(env, Some("127.0.0.1:7897")).unwrap(),
            "http://127.0.0.1:1111"
        );

        // 环境变量为空串等于没设
        let empty = |_: &str| Some(String::new());
        assert_eq!(
            system_proxy_url_from(empty, Some("127.0.0.1:7897")).unwrap(),
            "http://127.0.0.1:7897"
        );
        // 两边都没有 → None
        assert!(system_proxy_url_from(|_| None, None).is_none());
        assert!(system_proxy_url_from(|_| None, Some("")).is_none());
    }

    #[test]
    fn 出口决策按模式给出() {
        let direct = ProxyConfig {
            mode: ProxyMode::Direct,
            custom_url: "http://127.0.0.1:7890".into(),
        };
        // 直连模式忽略自定义地址
        assert_eq!(direct.policy(), ProxyPolicy::Direct);

        let system = ProxyConfig {
            mode: ProxyMode::System,
            custom_url: String::new(),
        };
        assert_eq!(system.policy(), ProxyPolicy::System);

        let custom = ProxyConfig {
            mode: ProxyMode::Custom,
            custom_url: "socks5h://127.0.0.1:1080".into(),
        };
        assert_eq!(
            custom.policy(),
            ProxyPolicy::Explicit("socks5h://127.0.0.1:1080".to_string())
        );
        assert!(custom.custom_valid());

        // 自定义地址非法 → 退化为直连（不 panic，设置页会给校验提示）
        let broken = ProxyConfig {
            mode: ProxyMode::Custom,
            custom_url: "ftp://x".into(),
        };
        assert_eq!(broken.policy(), ProxyPolicy::Direct);
        assert!(!broken.custom_valid());
    }

    #[test]
    fn 设置里的模式字符串解析() {
        assert_eq!(ProxyMode::parse("direct"), Some(ProxyMode::Direct));
        assert_eq!(ProxyMode::parse(" System "), Some(ProxyMode::System));
        assert_eq!(ProxyMode::parse("CUSTOM"), Some(ProxyMode::Custom));
        assert_eq!(ProxyMode::parse("tun"), None);
        // 取值非法时按默认（system）处理，不能悄悄变成直连
        let cfg = ProxyConfig::from_settings("tun", "");
        assert_eq!(cfg.mode, ProxyMode::System);
        assert_eq!(cfg.mode.as_str(), DEFAULT_PROXY_MODE);
    }

    #[test]
    fn 浏览器代理参数按模式给出() {
        let custom = ProxyConfig {
            mode: ProxyMode::Custom,
            custom_url: "http://user:pwd@127.0.0.1:7890".into(),
        };
        // 凭据不进命令行
        assert_eq!(
            browser_proxy_arg(&custom).as_deref(),
            Some("--proxy-server=http://127.0.0.1:7890")
        );

        // Chromium 不认 socks5h：降级成 socks5
        let socks = ProxyConfig {
            mode: ProxyMode::Custom,
            custom_url: "socks5h://127.0.0.1:1080".into(),
        };
        assert_eq!(
            browser_proxy_arg(&socks).as_deref(),
            Some("--proxy-server=socks5://127.0.0.1:1080")
        );

        // 直连要显式关掉，System 保持 WebView2 默认
        let direct = ProxyConfig {
            mode: ProxyMode::Direct,
            custom_url: String::new(),
        };
        assert_eq!(browser_proxy_arg(&direct).as_deref(), Some("--no-proxy-server"));
        let system = ProxyConfig {
            mode: ProxyMode::System,
            custom_url: String::new(),
        };
        assert!(browser_proxy_arg(&system).is_none());

        // 地址非法时不给参数（保持默认），也不该 panic
        let broken = ProxyConfig {
            mode: ProxyMode::Custom,
            custom_url: "nonsense".into(),
        };
        assert!(browser_proxy_arg(&broken).is_none());
    }

    #[test]
    fn 探测系统代理不panic() {
        // 结果取决于当前机器（注册表 / 环境变量），这里只要求它能安全返回
        let _ = system_proxy_url();
    }
}
