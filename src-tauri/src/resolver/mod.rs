//! Provider 注册表与解析入口（项目书 §3.1）。
//!
//! 约束：
//! - UI 不感知具体站点，只拿到标准化 `ResolvedMedia`；
//! - 解析前做 SSRF 防护（scheme、私网、DNS 重校验）；
//! - 不支持、需登录或被访问控制的资源返回明确分类错误，绝不尝试绕过。

pub mod browser;
pub mod direct;
pub mod hls;
pub mod m3u8;
pub mod xdown;
pub mod ytdlp;

use std::net::IpAddr;

use url::Url;

use crate::core::model::ResolvedMedia;
use crate::error::{AppError, AppResult};
use crate::net::ProxyConfig;

/// 可用的 Provider 种类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// 直链媒体：HEAD/Range 探测
    Direct,
    /// 站点链接：交给 yt-dlp 解析
    YtDlp,
    /// 有浏览器验证与动态签名的站点：用内嵌浏览器让页面自己去算签名
    Browser,
    /// HLS（m3u8）清单：解析选流后交给 FFmpeg 下载封装
    Hls,
}

/// Provider 注册表：按域名与路径选择允许且已启用的 Provider
pub fn registry(url: &str) -> AppResult<ProviderKind> {
    let parsed = Url::parse(url).map_err(|_| AppError::url_malformed())?;
    if parsed.scheme() != "https" {
        return Err(AppError::url_scheme());
    }
    // 直链媒体扩展名（不含 .m3u8，清单要单独解析）
    if is_likely_direct_media(&parsed) {
        return Ok(ProviderKind::Direct);
    }
    if m3u8::looks_like_hls(&parsed) {
        return Ok(ProviderKind::Hls);
    }
    // 有浏览器验证或动态签名的站点：yt-dlp 拿不到签名（抖音一律 403），
    // 或压根没有 extractor（快手 Unsupported URL），必须让内嵌浏览器里的页面自己去请求。
    // 优先 yt-dlp 的站点（推特/YouTube/Bilibili）仍走 yt-dlp，失败时才兜底到浏览器。
    if let Some(site) = browser::site_for(&parsed) {
        if !site.prefers_ytdlp {
            return Ok(ProviderKind::Browser);
        }
    }
    Ok(ProviderKind::YtDlp)
}

/// yt-dlp 失败后是否值得改用内嵌浏览器兜底。
///
/// 只对站点表里「优先 yt-dlp」的站点兜底，且仅限「解析不了」类错误——
/// 网络不通、需要付费这类问题换浏览器也解决不了。
pub fn should_fallback_to_browser(url: &str, err: &AppError) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    let Some(site) = browser::site_for(&parsed) else {
        return false;
    };
    site.prefers_ytdlp
        && matches!(
            err.code.as_str(),
            "PROVIDER_UNSUPPORTED" | "PROVIDER_ERROR" | "PROVIDER_EMPTY" | "PROVIDER_NO_STREAM"
        )
}

/// 直链判定：路径以常见媒体扩展名结尾
fn is_likely_direct_media(url: &Url) -> bool {
    const MEDIA_EXT: [&str; 14] = [
        ".mp4", ".m4v", ".mov", ".webm", ".mkv", ".avi", ".flv", ".ts", ".m4s", ".mp3", ".m4a",
        ".aac", ".flac", ".wav",
    ];
    let path = url.path().to_ascii_lowercase();
    MEDIA_EXT.iter().any(|ext| path.ends_with(ext))
}

/// 内网 / 回环 / 链路本地地址判定（`net::diag` 的 DNS 定性也用它）
pub(crate) fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.octets()[0] == 0
                || (v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1])) // CGNAT
        }
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
    }
}

/// DNS 复校的判定：解析不出地址说明域名不可用；出现内网 / 回环地址则按**域名污染**处理。
///
/// 与「用户自己填了内网地址」区分开：那种情况走 `URL_PRIVATE`，这里链接本身是公网
/// 域名，是本机 DNS 给出了内网答案（污染或分流），文案与解决办法都不同。
fn check_resolved(host: &str, ips: &[IpAddr]) -> AppResult<()> {
    if ips.is_empty() {
        return Err(AppError::network("域名无法解析"));
    }
    if ips.iter().any(|ip| is_blocked_ip(*ip)) {
        return Err(AppError::network_dns_polluted(host));
    }
    Ok(())
}

/// SSRF 防护：scheme、主机名，并在「域名由本机解析」时做一次 DNS 后的 IP 复校
/// （项目书 §3.1 第 2 条）。
///
/// 复校只在请求真的会用本机 DNS 时才有意义。代理生效时（`ProxyConfig::is_active`）
/// 域名交给代理端解析，本机这份结果既不参与连接，在域名被污染或分流的网络里还会把
/// 公网域名解析成 127.0.0.1 —— 那样 `x.com` 会被判成内网地址直接拦下，连 yt-dlp 都
/// 起不来，而用户填的明明是公网链接。
///
/// 需要如实记录的原理上限：走代理时**无法**做 DNS 重绑定复校（域名解析发生在代理端，
/// 本机拿不到代理最终连的地址）。所以字面量判定必须独立于这一层：
/// 非 https、`localhost` / `*.local` / `*.internal`、以及字面量的内网 / 回环 /
/// 链路本地 / CGNAT 地址，任何模式下都照拦不误。
pub async fn guard_url(url: &str, proxy: &ProxyConfig) -> AppResult<Url> {
    let parsed = Url::parse(url).map_err(|_| AppError::url_malformed())?;

    if parsed.scheme() != "https" {
        return Err(AppError::url_scheme());
    }

    let host = parsed.host_str().ok_or_else(AppError::url_malformed)?.to_string();
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".local") || lower.ends_with(".internal") {
        return Err(AppError::url_private());
    }

    // 主机本身就是 IP 时直接判定
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_ip(ip) {
            return Err(AppError::url_private());
        }
        return Ok(parsed);
    }

    if proxy.is_active() {
        return Ok(parsed);
    }

    // DNS 解析后再次校验，避免 DNS 重绑定指向内网
    let port = parsed.port_or_known_default().unwrap_or(443);
    match tokio::net::lookup_host((host.as_str(), port)).await {
        Ok(addrs) => {
            let ips: Vec<IpAddr> = addrs.map(|addr| addr.ip()).collect();
            check_resolved(&host, &ips)?;
        }
        Err(_) => return Err(AppError::network("域名无法解析")),
    }

    Ok(parsed)
}

/// 解析前的统一判定：SSRF 防护 + Provider 选择。
///
/// 拆出来是给 GUI 侧的派发用：`Browser` 需要 AppHandle 才能开窗口，
/// 而本模块要保持不依赖 Tauri，所以调用方先在这里拿到种类再决定走哪条路。
pub async fn classify(url: &str, proxy: &ProxyConfig) -> AppResult<(Url, ProviderKind)> {
    let parsed = guard_url(url, proxy).await?;
    let kind = registry(parsed.as_str())?;
    Ok((parsed, kind))
}

/// 统一解析入口
///
/// `cookies_source` 为登录态来源（见 `ytdlp::append_cookie_args`）：yt-dlp 会复用
/// 该浏览器已登录的会话，或读取导出的 cookies.txt。
/// `proxy` 决定出口，yt-dlp 侧转成 `--proxy`。
///
/// 注意：`ProviderKind::Browser` 不走这里——它需要 `AppHandle` 开内嵌浏览器，
/// 由 GUI 侧派发（见 `commands::resolve_media`）。
pub async fn resolve(
    client: &reqwest::Client,
    url: &str,
    cookies_source: Option<&str>,
    proxy: &ProxyConfig,
) -> AppResult<ResolvedMedia> {
    let (parsed, kind) = classify(url, proxy).await?;
    match kind {
        ProviderKind::Direct => direct::resolve(client, &parsed).await,
        ProviderKind::Hls => hls::resolve(client, &parsed).await,
        ProviderKind::YtDlp => ytdlp::resolve(&parsed, cookies_source, proxy).await,
        ProviderKind::Browser => Err(AppError::new(
            "PROVIDER_BROWSER_UNAVAILABLE",
            "该站点需要通过内嵌浏览器解析",
            true,
        )
        .with_hint("请从界面发起解析；若持续失败请反馈")),
    }
}

/// 解析结果的 TTL（项目书 §3.1 第 4 条）
pub const RESOLVE_TTL_SECS: i64 = 15 * 60;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 直链媒体按扩展名识别() {
        assert_eq!(
            registry("https://cdn.example.com/a/b.mp4").unwrap(),
            ProviderKind::Direct
        );
        assert_eq!(
            registry("https://cdn.example.com/x.m4a").unwrap(),
            ProviderKind::Direct
        );
        // 带 query 的直链同样识别
        assert_eq!(
            registry("https://cdn.example.com/video.mp4?token=abc").unwrap(),
            ProviderKind::Direct
        );
    }

    #[test]
    fn 站点链接走_ytdlp() {
        assert_eq!(
            registry("https://www.bilibili.com/video/BV1xx411c7mD").unwrap(),
            ProviderKind::YtDlp
        );
        assert_eq!(
            registry("https://www.youtube.com/watch?v=abc").unwrap(),
            ProviderKind::YtDlp
        );
    }

    #[test]
    fn m3u8链接走_HLS_解析() {
        assert_eq!(
            registry("https://cdn.example.com/live/index.m3u8").unwrap(),
            ProviderKind::Hls
        );
        // 带 query 的清单同样识别
        assert_eq!(
            registry("https://cdn.example.com/a.m3u8?token=x").unwrap(),
            ProviderKind::Hls
        );
    }

    #[test]
    fn 白名单兜底只在表内站点且错误可兜底时成立() {
        use crate::error::AppError;

        // 推特在表内且优先 yt-dlp：解析不了时值得换浏览器
        assert!(should_fallback_to_browser(
            "https://x.com/u/status/1",
            &AppError::provider_unsupported()
        ));
        assert!(should_fallback_to_browser(
            "https://www.youtube.com/watch?v=a",
            &AppError::new("PROVIDER_ERROR", "解析失败", false)
        ));

        // 表外站点不兜底：不把陌生网页加载进内嵌浏览器
        assert!(!should_fallback_to_browser(
            "https://example.com/video/1",
            &AppError::provider_unsupported()
        ));

        // 网络/付费这类问题换浏览器也解决不了，不兜底
        assert!(!should_fallback_to_browser(
            "https://x.com/u/status/1",
            &AppError::network("断网")
        ));
        assert!(!should_fallback_to_browser(
            "https://www.bilibili.com/video/BV1xx",
            &AppError::provider_restricted()
        ));

        // 抖音本来就直连浏览器，不存在「兜底」这一步
        assert!(!should_fallback_to_browser(
            "https://v.douyin.com/abc",
            &AppError::provider_unsupported()
        ));
    }

    #[test]
    fn 抖音走浏览器解析() {
        assert_eq!(
            registry("https://v.douyin.com/AMva9tRuEqU/").unwrap(),
            ProviderKind::Browser
        );
        assert_eq!(
            registry("https://www.douyin.com/video/7686809202295131426").unwrap(),
            ProviderKind::Browser
        );
    }

    #[test]
    fn 非_https_被拒绝() {
        let err = registry("http://example.com/a.mp4").unwrap_err();
        assert_eq!(err.code, "URL_SCHEME");
    }

    #[test]
    fn 私网地址判定覆盖常见网段() {
        use std::net::IpAddr;
        let blocked = [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.1",
            "172.16.0.1",
            "172.31.255.254",
            "169.254.1.1",
            "0.0.0.0",
            "100.64.0.1",
            "::1",
        ];
        for ip in blocked {
            let parsed: IpAddr = ip.parse().unwrap();
            assert!(is_blocked_ip(parsed), "{ip} 应被拦截");
        }
        let allowed = ["8.8.8.8", "1.1.1.1", "172.32.0.1", "203.0.113.1"];
        for ip in allowed {
            let parsed: IpAddr = ip.parse().unwrap();
            assert!(!is_blocked_ip(parsed), "{ip} 不应被拦截");
        }
    }

    /// 直连出口：会做本地 DNS 复校
    fn direct_proxy() -> ProxyConfig {
        ProxyConfig {
            mode: crate::net::ProxyMode::Direct,
            custom_url: String::new(),
        }
    }

    /// 走代理的出口：跳过本地 DNS 复校（不产生任何网络 IO，`is_active` 只看配置）
    fn active_proxy() -> ProxyConfig {
        ProxyConfig {
            mode: crate::net::ProxyMode::Custom,
            custom_url: "http://127.0.0.1:7890".into(),
        }
    }

    #[tokio::test]
    async fn 守卫拒绝回环与内网字面量() {
        assert_eq!(
            guard_url("https://127.0.0.1/a.mp4", &direct_proxy())
                .await
                .unwrap_err()
                .code,
            "URL_PRIVATE"
        );
        assert_eq!(
            guard_url("https://localhost/a.mp4", &direct_proxy())
                .await
                .unwrap_err()
                .code,
            "URL_PRIVATE"
        );
        assert_eq!(
            guard_url("https://192.168.1.1/a.mp4", &direct_proxy())
                .await
                .unwrap_err()
                .code,
            "URL_PRIVATE"
        );
    }

    #[tokio::test]
    async fn 守卫允许公网字面量() {
        assert!(guard_url("https://8.8.8.8/a.mp4", &direct_proxy())
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn 代理生效时字面内网地址与内网主机名照拦不误() {
        // 跳过 DNS 复校只针对「域名解析」这一步，其余判定不受影响
        for blocked in [
            "https://127.0.0.1/a.mp4",
            "https://192.168.1.1/a.mp4",
            "https://169.254.1.1/a.mp4",
            "https://localhost/a.mp4",
            "https://nas.local/a.mp4",
            "https://git.internal/a.mp4",
            "http://example.com/a.mp4",
        ] {
            let err = guard_url(blocked, &active_proxy()).await.unwrap_err();
            assert!(
                matches!(err.code.as_str(), "URL_PRIVATE" | "URL_SCHEME"),
                "{blocked} 有代理时也该被拦，实际 {}",
                err.code
            );
        }
    }

    #[tokio::test]
    async fn 代理生效时公网域名不再被本机解析结果拦下() {
        // 污染网络下 x.com 在本机解析为 127.0.0.1，此前会被判成「内网地址」。
        // 代理生效时域名由代理端解析，这一步不该再拦。
        let url = guard_url("https://x.com/u/status/1", &active_proxy()).await;
        assert!(url.is_ok(), "有代理时不该因本机 DNS 结果被拦：{url:?}");
        assert_eq!(url.unwrap().host_str(), Some("x.com"));
    }

    #[test]
    fn 解析结果含内网地址按域名污染处理() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        // 任一地址落在内网就拒绝：公网 + 内网混合时同样不能放过。
        // 注意错误码是 NETWORK_DNS_POLLUTED 而不是 URL_PRIVATE——链接本身是公网域名，
        // 是本机 DNS 给出了内网答案，解决办法（开代理）完全不同。
        assert_eq!(
            check_resolved("x.com", &[ip("8.8.8.8"), ip("10.0.0.1")])
                .unwrap_err()
                .code,
            "NETWORK_DNS_POLLUTED"
        );
        assert_eq!(
            check_resolved("x.com", &[ip("127.0.0.1")]).unwrap_err().code,
            "NETWORK_DNS_POLLUTED"
        );
        assert_eq!(
            check_resolved("x.com", &[ip("::1")]).unwrap_err().code,
            "NETWORK_DNS_POLLUTED"
        );
        // 全是公网地址才放行
        assert!(check_resolved("x.com", &[ip("8.8.8.8"), ip("1.1.1.1")]).is_ok());
    }

    #[test]
    fn 解析不出地址按域名不可用处理() {
        assert_eq!(
            check_resolved("x.com", &[]).unwrap_err().code,
            "NETWORK_ERROR",
            "解析结果为空是网络问题，不是内网地址"
        );
    }
}