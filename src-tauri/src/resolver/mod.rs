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
pub mod ytdlp;

use std::net::IpAddr;

use url::Url;

use crate::core::model::ResolvedMedia;
use crate::error::{AppError, AppResult};

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

/// 内网 / 回环 / 链路本地地址判定
fn is_blocked_ip(ip: IpAddr) -> bool {
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

/// SSRF 防护：scheme、主机名、并做一次 DNS 解析后的 IP 复校（项目书 §3.1 第 2 条）
pub async fn guard_url(url: &str) -> AppResult<Url> {
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

    // DNS 解析后再次校验，避免 DNS 重绑定指向内网
    let port = parsed.port_or_known_default().unwrap_or(443);
    match tokio::net::lookup_host((host.as_str(), port)).await {
        Ok(addrs) => {
            let mut any = false;
            for addr in addrs {
                any = true;
                if is_blocked_ip(addr.ip()) {
                    return Err(AppError::url_private());
                }
            }
            if !any {
                return Err(AppError::network("域名无法解析"));
            }
        }
        Err(_) => return Err(AppError::network("域名无法解析")),
    }

    Ok(parsed)
}

/// 解析前的统一判定：SSRF 防护 + Provider 选择。
///
/// 拆出来是给 GUI 侧的派发用：`Browser` 需要 AppHandle 才能开窗口，
/// 而本模块要保持不依赖 Tauri，所以调用方先在这里拿到种类再决定走哪条路。
pub async fn classify(url: &str) -> AppResult<(Url, ProviderKind)> {
    let parsed = guard_url(url).await?;
    let kind = registry(parsed.as_str())?;
    Ok((parsed, kind))
}

/// 统一解析入口
///
/// `cookies_source` 为登录态来源（见 `ytdlp::append_cookie_args`）：yt-dlp 会复用
/// 该浏览器已登录的会话，或读取导出的 cookies.txt。
///
/// 注意：`ProviderKind::Browser` 不走这里——它需要 `AppHandle` 开内嵌浏览器，
/// 由 GUI 侧派发（见 `commands::resolve_media`）。
pub async fn resolve(
    client: &reqwest::Client,
    url: &str,
    cookies_source: Option<&str>,
) -> AppResult<ResolvedMedia> {
    let (parsed, kind) = classify(url).await?;
    match kind {
        ProviderKind::Direct => direct::resolve(client, &parsed).await,
        ProviderKind::Hls => hls::resolve(client, &parsed).await,
        ProviderKind::YtDlp => ytdlp::resolve(client, &parsed, cookies_source).await,
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

    #[tokio::test]
    async fn 守卫拒绝回环与内网字面量() {
        assert_eq!(
            guard_url("https://127.0.0.1/a.mp4").await.unwrap_err().code,
            "URL_PRIVATE"
        );
        assert_eq!(
            guard_url("https://localhost/a.mp4").await.unwrap_err().code,
            "URL_PRIVATE"
        );
        assert_eq!(
            guard_url("https://192.168.1.1/a.mp4").await.unwrap_err().code,
            "URL_PRIVATE"
        );
    }

    #[tokio::test]
    async fn 守卫允许公网字面量() {
        assert!(guard_url("https://8.8.8.8/a.mp4").await.is_ok());
    }
}