//! 统一网络出口层。
//!
//! 出口只有一个真相源（`ProxyConfig`），五条链路都从这里取：
//! reqwest 主客户端 / XDown 短超时客户端 / yt-dlp 的 `--proxy` /
//! FFmpeg 的 `-http_proxy` / WebView2 的 `--proxy-server`。
//!
//! 分层：`proxy` 决定怎么出去（纯逻辑），`diag` 回答现在能不能出去（探测 + 判定），
//! 本文件放跨两边的粘合（建库、失败归类、HLS 代理参数）。全模块不依赖 Tauri。

pub mod diag;
pub mod proxy;

use std::time::Duration;

use crate::error::AppError;

pub use diag::{NetworkDiagnostics, PathProbe};
pub use proxy::{ProxyConfig, ProxyMode, ProxyPolicy};

/// 各客户端统一的 UA
pub const USER_AGENT: &str = "VideoFlow/0.1 (Windows)";

/// 建库参数：同一套参数保证「代理一致」之外的超时行为也一致
#[derive(Debug, Clone)]
pub struct ClientSpec {
    pub user_agent: String,
    pub connect_timeout: Duration,
    pub timeout: Duration,
    pub pool_max_idle_per_host: usize,
}

impl ClientSpec {
    /// 主客户端：下载用，总超时给得很长
    pub fn main() -> Self {
        Self {
            user_agent: USER_AGENT.to_string(),
            connect_timeout: Duration::from_secs(20),
            timeout: Duration::from_secs(600),
            pool_max_idle_per_host: 8,
        }
    }

    /// XDown 兜底：连接与总超时都收紧，不与主客户端混用
    pub fn short() -> Self {
        Self {
            user_agent: USER_AGENT.to_string(),
            connect_timeout: Duration::from_secs(5),
            timeout: Duration::from_secs(15),
            pool_max_idle_per_host: 2,
        }
    }

    /// 网络探测：越快越好，避免让用户等诊断
    pub fn probe() -> Self {
        Self {
            user_agent: USER_AGENT.to_string(),
            connect_timeout: Duration::from_secs(5),
            timeout: Duration::from_secs(6),
            pool_max_idle_per_host: 1,
        }
    }
}

/// 按当前出口配置建一个 reqwest 客户端。
///
/// 关键点：reqwest 默认会自动跟随系统代理，**显式设代理与 `no_proxy()` 都会把它关掉**
/// （`reqwest-0.12.28/src/async_impl/client.rs:1413-1430`）。所以直连模式必须显式
/// `no_proxy()`，否则「直连」只是名义上的。
pub fn build_client(cfg: &ProxyConfig, spec: &ClientSpec) -> reqwest::Result<reqwest::Client> {
    let builder = reqwest::Client::builder()
        .user_agent(spec.user_agent.clone())
        .connect_timeout(spec.connect_timeout)
        .timeout(spec.timeout)
        .pool_max_idle_per_host(spec.pool_max_idle_per_host);

    let builder = match cfg.policy() {
        ProxyPolicy::Direct => builder.no_proxy(),
        ProxyPolicy::System => match proxy::system_proxy_url() {
            Some(url) => builder.proxy(reqwest::Proxy::all(&url)?),
            // 系统没开代理：显式直连，避免建库后又自动跟随一次（行为不可预测）
            None => builder.no_proxy(),
        },
        ProxyPolicy::Explicit(url) => builder.proxy(reqwest::Proxy::all(&url)?),
    };

    builder.build()
}

/// 网络类错误：换解析器、换浏览器都解决不了，只能改网络出口
pub fn is_network_error(err: &AppError) -> bool {
    matches!(
        err.code.as_str(),
        "NETWORK_ERROR"
            | "NETWORK_UNREACHABLE"
            | "NETWORK_DNS_POLLUTED"
            | "PROXY_UNAVAILABLE"
            | "PROVIDER_TIMEOUT"
    )
}

/// X 链路失败时的现场（由命令层探测后填入）
pub struct XFailureContext<'a> {
    /// 用当前出口访问 X 的探测结果
    pub target: Option<&'a PathProbe>,
    /// 代理端口本身的连通性（无代理时为 None）
    pub proxy_endpoint: Option<&'a PathProbe>,
    /// 当前出口的脱敏代理地址
    pub active_proxy: Option<&'a str>,
}

/// X 链接失败归类（纯函数，离线可测）。
///
/// 用户要的是「一眼看出是网络问题还是解析器问题」，所以这里按
/// **代理不可用 → 网络不可达 → XDown 失败 → 原样返回 yt-dlp 错误** 的顺序判定，
/// 并且把两条链路的原因都留在 `detail` 里，不再像以前那样丢掉 XDown 的失败原因。
pub fn classify_x_failure(
    ytdlp: &AppError,
    xdown: Option<&AppError>,
    ctx: &XFailureContext<'_>,
) -> AppError {
    // ① 代理端口都连不上：问题在代理，不在目标站点
    if let Some(endpoint) = ctx.proxy_endpoint {
        if !endpoint.available {
            let addr = ctx.active_proxy.unwrap_or("当前代理");
            return AppError::proxy_unavailable(addr, &endpoint.detail);
        }
    }

    // ② 用当前出口访问 X 失败：网络不可达（目标被阻断或出口不通）
    if let Some(target) = ctx.target {
        if !target.available {
            let err = AppError::network_unreachable(diag::X_TARGET_LABEL);
            return match ctx.active_proxy {
                Some(addr) => err.with_detail(format!("{addr}：{}", target.detail)),
                None => err.with_detail(target.detail.clone()),
            };
        }
    }

    // ③ yt-dlp 自己就报网络类错误：换 XDown 也白搭
    if is_network_error(ytdlp) {
        return AppError::network_unreachable(diag::X_TARGET_LABEL)
            .with_detail(format!("yt-dlp：{}", ytdlp));
    }

    // ④ XDown 也失败了：两条链路的原因都留下
    if let Some(x) = xdown {
        if is_network_error(x) {
            return AppError::network_unreachable(diag::X_TARGET_LABEL)
                .with_detail(format!("yt-dlp：{ytdlp}；XDown：{x}"));
        }
        return AppError::provider_xdown_failed(&ytdlp.to_string(), &x.to_string());
    }

    // ⑤ 只有 yt-dlp 失败且不值得兜底：保留它自己的分类（例如需要登录态）
    ytdlp.clone()
}

/// FFmpeg 能用的代理参数。
///
/// FFmpeg 的 `-http_proxy` 只支持 http/https 代理，**不支持 SOCKS**，
/// 所以 SOCKS 出口下这里返回 None（HLS 任务会退回直连并给出提示）。
pub fn hls_proxy_arg(cfg: &ProxyConfig) -> Option<String> {
    let url = cfg.effective_url()?;
    let scheme = url.split("://").next()?.to_ascii_lowercase();
    matches!(scheme.as_str(), "http" | "https").then_some(url)
}

/// 出口是 SOCKS 时，HLS 任务为什么不能用它的说明（给用户看）
pub fn hls_socks_hint(cfg: &ProxyConfig) -> Option<&'static str> {
    let url = cfg.effective_url()?;
    let scheme = url.split("://").next()?.to_ascii_lowercase();
    scheme.starts_with("socks").then_some(
        "当前出口是 SOCKS 代理，FFmpeg 不支持；HLS 下载请改用 HTTP 代理端口或开启 TUN",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(label: &str, available: bool, detail: &str) -> PathProbe {
        PathProbe {
            label: label.to_string(),
            available,
            detail: detail.to_string(),
            latency_ms: None,
        }
    }

    #[test]
    fn 代理参数只给_http_系() {
        let http = ProxyConfig {
            mode: ProxyMode::Custom,
            custom_url: "http://127.0.0.1:7890".into(),
        };
        assert_eq!(
            hls_proxy_arg(&http).as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert!(hls_socks_hint(&http).is_none());

        let socks = ProxyConfig {
            mode: ProxyMode::Custom,
            custom_url: "socks5h://127.0.0.1:7897".into(),
        };
        assert!(hls_proxy_arg(&socks).is_none(), "FFmpeg 不支持 SOCKS");
        assert!(hls_socks_hint(&socks).is_some());

        // 直连：没有代理参数，也没有 SOCKS 提示
        let direct = ProxyConfig {
            mode: ProxyMode::Direct,
            custom_url: "http://127.0.0.1:7890".into(),
        };
        assert!(hls_proxy_arg(&direct).is_none());
        assert!(hls_socks_hint(&direct).is_none());
    }

    #[test]
    fn 网络类错误集合() {
        assert!(is_network_error(&AppError::network("断网")));
        assert!(is_network_error(&AppError::network_unreachable("X")));
        assert!(is_network_error(&AppError::network_dns_polluted("x.com")));
        assert!(is_network_error(&AppError::proxy_unavailable("p", "d")));
        assert!(!is_network_error(&AppError::provider_unsupported()));
        assert!(!is_network_error(&AppError::provider_xdown_failed("a", "b")));
    }

    #[test]
    fn x_链接失败归类优先看代理与网络() {
        let ytdlp = AppError::new("PROVIDER_ERROR", "解析失败", false);
        let xdown = AppError::new("PROVIDER_NO_STREAM", "没有可下载的原始视频", false);

        // ① 代理端口连不上 → 代理不可用，点名地址
        let endpoint_down = probe("代理端口", false, "http://127.0.0.1:7899 连不上");
        let target_ok = probe("自定义代理", true, "HTTP 200");
        let err = classify_x_failure(
            &ytdlp,
            Some(&xdown),
            &XFailureContext {
                target: Some(&target_ok),
                proxy_endpoint: Some(&endpoint_down),
                active_proxy: Some("http://127.0.0.1:7899"),
            },
        );
        assert_eq!(err.code, "PROXY_UNAVAILABLE");
        assert!(err.message.contains("127.0.0.1:7899"));

        // ② 代理端口通但目标不通 → 网络不可达
        let endpoint_ok = probe("代理端口", true, "可连接");
        let target_down = probe("自定义代理", false, "无法连接：连接被重置");
        let err = classify_x_failure(
            &ytdlp,
            Some(&xdown),
            &XFailureContext {
                target: Some(&target_down),
                proxy_endpoint: Some(&endpoint_ok),
                active_proxy: Some("http://127.0.0.1:7897"),
            },
        );
        assert_eq!(err.code, "NETWORK_UNREACHABLE");
        assert!(err.hint.unwrap().contains("系统代理/TUN"));

        // ③ 网络没问题、两条链路都因内容失败 → XDown 失败，两条原因都在 detail 里
        let err = classify_x_failure(
            &ytdlp,
            Some(&xdown),
            &XFailureContext {
                target: Some(&target_ok),
                proxy_endpoint: Some(&endpoint_ok),
                active_proxy: Some("http://127.0.0.1:7897"),
            },
        );
        assert_eq!(err.code, "PROVIDER_XDOWN_FAILED");
        let detail = err.detail.unwrap();
        assert!(detail.contains("yt-dlp"));
        assert!(detail.contains("XDown"));

        // ④ yt-dlp 报网络错误、没有探测结果 → 仍归为网络不可达
        let err = classify_x_failure(
            &AppError::network("无法访问该站点"),
            None,
            &XFailureContext {
                target: None,
                proxy_endpoint: None,
                active_proxy: None,
            },
        );
        assert_eq!(err.code, "NETWORK_UNREACHABLE");

        // ⑤ 只有 yt-dlp 失败且不值得兜底 → 原样返回（例如需要登录态）
        let cookies = crate::resolver::ytdlp::cookies_hint();
        let err = classify_x_failure(
            &cookies,
            None,
            &XFailureContext {
                target: None,
                proxy_endpoint: None,
                active_proxy: None,
            },
        );
        assert_eq!(err.code, "PROVIDER_NEEDS_COOKIES");
    }

    #[test]
    fn 直连模式建库不会启用系统代理() {
        // 直连必须显式关掉自动系统代理，否则「直连」只是名义上的
        let cfg = ProxyConfig {
            mode: ProxyMode::Direct,
            custom_url: String::new(),
        };
        assert!(build_client(&cfg, &ClientSpec::probe()).is_ok());
        assert!(cfg.effective_url().is_none());
    }
}
