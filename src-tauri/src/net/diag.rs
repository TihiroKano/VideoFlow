//! 网络诊断：分别测「本机 DNS」「直连」「系统代理」「自定义代理」能否访问 X，
//! 并给出一句可操作的结论。
//!
//! 与 `net::proxy` 的分工：那边决定**怎么出去**，这边只负责**回答现在能不能出去**。
//! 解析失败时也复用这里的探测（`probe_target` / `probe_proxy_endpoint`），
//! 用来把「网络不可达」与「代理不可用」分开，而不是把它们伪装成解析器错误。
//!
//! 纯判定逻辑（`dns_verdict` / `is_fake_ip` / `verdict`）与 IO 分开，前者可离线单测。

use std::net::IpAddr;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::proxy::{ProxyConfig, ProxyMode};
use super::{build_client, ClientSpec};

/// 探测目标：X 的静态小文件（体积小、无重定向、不需要登录态）
pub const X_PROBE_URL: &str = "https://x.com/robots.txt";
/// 站点名（用于文案）
pub const X_TARGET_LABEL: &str = "X";
/// DNS 探测的两个域名：站点本身与它下载用的 CDN
const DNS_HOSTS: [&str; 2] = ["x.com", "video.twimg.com"];

/// 一条出口路径的探测结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathProbe {
    pub label: String,
    pub available: bool,
    pub detail: String,
    pub latency_ms: Option<u64>,
}

impl PathProbe {
    fn ok(label: &str, detail: impl Into<String>, latency_ms: u64) -> Self {
        Self {
            label: label.to_string(),
            available: true,
            detail: detail.into(),
            latency_ms: Some(latency_ms),
        }
    }

    fn fail(label: &str, detail: impl Into<String>) -> Self {
        Self {
            label: label.to_string(),
            available: false,
            detail: detail.into(),
            latency_ms: None,
        }
    }

    /// 这条路径压根没得测（例如自定义代理没填地址）
    fn absent(label: &str, detail: impl Into<String>) -> Self {
        Self::fail(label, detail)
    }
}

/// DNS 解析结果的定性
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsVerdict {
    /// 解析到正常公网地址
    Normal,
    /// 解析到内网 / 回环地址：域名污染或分流
    Polluted,
    /// 解析到 `198.18.0.0/15`：代理的 fake-IP，TUN 模式下正常
    FakeIp,
    /// 一个地址都没解析出来
    Unresolved,
}

/// 代理 fake-IP 常用网段（`198.18.0.0/15`，RFC 2544 基准测试段）
pub fn is_fake_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == 198 && (octets[1] == 18 || octets[1] == 19)
        }
        IpAddr::V6(_) => false,
    }
}

/// DNS 结果定性：**污染优先**——只要出现内网 / 回环地址就按污染处理，
/// 因为那种答案根本连不到目标；fake-IP 是可用的，单独标出来避免误报成污染。
pub fn dns_verdict(ips: &[IpAddr]) -> DnsVerdict {
    if ips.is_empty() {
        return DnsVerdict::Unresolved;
    }
    if ips.iter().any(|ip| crate::resolver::is_blocked_ip(*ip)) {
        return DnsVerdict::Polluted;
    }
    if ips.iter().any(|ip| is_fake_ip(*ip)) {
        return DnsVerdict::FakeIp;
    }
    DnsVerdict::Normal
}

/// 完整诊断结果（前端直接展示）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkDiagnostics {
    /// 当前模式：direct / system / custom
    pub mode: String,
    pub mode_label: String,
    /// 当前实际使用的代理（已脱敏），None = 直连
    pub active_proxy: Option<String>,
    pub active_proxy_source: String,
    /// 本机 DNS 解析 x.com / video.twimg.com 的结果
    pub dns: PathProbe,
    /// 代理端口本身的连通性（无代理时为 None）
    pub active_proxy_endpoint: Option<PathProbe>,
    pub direct: PathProbe,
    pub system: PathProbe,
    pub custom: PathProbe,
    /// 当前出口配置有没有问题（前端据此决定要不要弹提示）
    pub proxy_ok: bool,
    /// 当前出口能不能访问 X
    pub x_reachable: bool,
    /// 一句话结论 + 该怎么做
    pub verdict: String,
}

/// 结论计算的输入
pub struct VerdictInput<'a> {
    pub mode: ProxyMode,
    pub active_proxy: Option<&'a str>,
    /// 自定义地址是否已填写且合法
    pub custom_configured: bool,
    pub dns: &'a PathProbe,
    pub direct: &'a PathProbe,
    pub system: &'a PathProbe,
    pub custom: &'a PathProbe,
}

/// 结论：结论文案 + 当前出口配置是否有问题 + 能否访问 X
pub struct Verdict {
    pub text: String,
    pub proxy_ok: bool,
    pub x_reachable: bool,
}

/// 结论判定（纯函数）
///
/// `proxy_ok` 的语义是「当前出口配置有没有问题」，用来决定启动时要不要提醒：
/// 直连用户永远不提醒（那是他自己的选择），自定义代理不通或系统代理没开又直连不通才提醒。
pub fn verdict(input: &VerdictInput<'_>) -> Verdict {
    // 「当前出口」在 System 模式下要跟着实际情况走：探测到系统代理就用它，
    // 没探测到就等于直连（与 `build_client` 的行为一致）。否则会出现
    // 「系统代理没开、直连明明能通，却报告访问不了 X」这种自相矛盾的结论。
    let active = match input.mode {
        ProxyMode::Direct => input.direct,
        ProxyMode::System => {
            if input.active_proxy.is_some() {
                input.system
            } else {
                input.direct
            }
        }
        ProxyMode::Custom => input.custom,
    };
    let x_reachable = active.available;
    let proxy_ok = match input.mode {
        ProxyMode::Direct => true,
        ProxyMode::Custom => input.custom.available,
        ProxyMode::System => input.system.available || input.direct.available,
    };

    let polluted_note = if input.dns.available {
        ""
    } else {
        "；本机 DNS 把 X 解析到了内网地址（域名污染）"
    };

    let text = if x_reachable {
        match input.mode {
            ProxyMode::Direct => format!("直连可以访问 {X_TARGET_LABEL}"),
            ProxyMode::System => {
                if input.direct.available {
                    format!("经系统代理可以访问 {X_TARGET_LABEL}（直连也通）")
                } else {
                    format!("经系统代理可以访问 {X_TARGET_LABEL}（直连不通）")
                }
            }
            ProxyMode::Custom => format!("经自定义代理可以访问 {X_TARGET_LABEL}"),
        }
    } else {
        match input.mode {
            ProxyMode::Direct => format!(
                "当前网络无法访问 {X_TARGET_LABEL}，请启用系统代理/TUN 或配置 VideoFlow 代理{polluted_note}"
            ),
            ProxyMode::System => match input.active_proxy {
                Some(addr) => format!(
                    "系统代理 {addr} 不可用（{}），请在代理软件里检查后点「重新检测」",
                    input.system.detail
                ),
                None => format!(
                    "系统代理未启用，且当前网络无法直连 {X_TARGET_LABEL}：请开启系统代理/TUN，或在 VideoFlow 里配置自定义代理{polluted_note}"
                ),
            },
            ProxyMode::Custom => {
                if !input.custom_configured {
                    "自定义代理还没填好（需要形如 127.0.0.1:7890 的地址），填好后点「重新检测」"
                        .to_string()
                } else {
                    match input.active_proxy {
                        Some(addr) => format!(
                            "自定义代理 {addr} 不可用（{}），请检查代理软件与端口",
                            input.custom.detail
                        ),
                        None => "自定义代理地址非法，请检查后重试".to_string(),
                    }
                }
            }
        }
    };

    Verdict {
        text,
        proxy_ok,
        x_reachable,
    }
}

/// 探测「用这套出口能不能访问 X」
pub async fn probe_target(cfg: &ProxyConfig) -> PathProbe {
    let label = cfg.mode.label();
    if cfg.mode == ProxyMode::Custom && !cfg.custom_valid() {
        return PathProbe::absent(label, "未配置或地址非法");
    }

    let client = match build_client(cfg, &ClientSpec::probe()) {
        Ok(client) => client,
        Err(e) => return PathProbe::fail(label, format!("客户端构建失败：{e}")),
    };

    let started = Instant::now();
    match client.get(X_PROBE_URL).send().await {
        Ok(resp) => PathProbe::ok(
            label,
            format!("HTTP {}", resp.status().as_u16()),
            started.elapsed().as_millis() as u64,
        ),
        Err(e) => PathProbe::fail(label, describe(&e)),
    }
}

/// 探测代理端口本身是否在监听（把「代理没在跑」与「目标不通」分开）
pub async fn probe_proxy_endpoint(cfg: &ProxyConfig) -> Option<PathProbe> {
    let url = cfg.effective_url()?;
    let parsed = url::Url::parse(&url).ok()?;
    let host = parsed.host_str()?.to_string();
    let port = parsed.port()?;
    let display = super::proxy::redact(&url);

    let started = Instant::now();
    let target = (host.as_str(), port);
    match tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(target),
    )
    .await
    {
        Ok(Ok(_)) => Some(PathProbe::ok(
            "代理端口",
            format!("{display} 可连接"),
            started.elapsed().as_millis() as u64,
        )),
        Ok(Err(e)) => Some(PathProbe::fail("代理端口", format!("{display} 连不上：{e}"))),
        Err(_) => Some(PathProbe::fail("代理端口", format!("{display} 连接超时"))),
    }
}

/// 本机 DNS 探测：解析站点与 CDN，并判定答案是否被污染
async fn probe_dns() -> PathProbe {
    let mut parts: Vec<String> = Vec::new();
    let mut ips: Vec<IpAddr> = Vec::new();

    for host in DNS_HOSTS {
        match tokio::net::lookup_host((host, 443)).await {
            Ok(addrs) => {
                let mut seen: Vec<String> = Vec::new();
                for addr in addrs {
                    ips.push(addr.ip());
                    let text = addr.ip().to_string();
                    if !seen.contains(&text) {
                        seen.push(text);
                    }
                }
                if seen.is_empty() {
                    parts.push(format!("{host} → 无结果"));
                } else {
                    parts.push(format!("{host} → {}", seen.join(", ")));
                }
            }
            Err(_) => parts.push(format!("{host} → 解析失败")),
        }
    }

    let detail = parts.join("；");
    match dns_verdict(&ips) {
        DnsVerdict::Polluted => PathProbe::fail(
            "本机 DNS",
            format!("{detail}（内网/回环地址，疑似域名污染）"),
        ),
        DnsVerdict::FakeIp => PathProbe::ok(
            "本机 DNS",
            format!("{detail}（代理 fake-IP，TUN 模式下正常）"),
            0,
        ),
        DnsVerdict::Normal => PathProbe::ok("本机 DNS", detail, 0),
        DnsVerdict::Unresolved => PathProbe::fail("本机 DNS", detail),
    }
}

/// 完整诊断：DNS + 三条出口路径 + 代理端口
pub async fn diagnose(cfg: &ProxyConfig) -> NetworkDiagnostics {
    let direct_cfg = ProxyConfig {
        mode: ProxyMode::Direct,
        custom_url: String::new(),
    };
    let system_cfg = ProxyConfig {
        mode: ProxyMode::System,
        custom_url: String::new(),
    };
    let custom_cfg = ProxyConfig {
        mode: ProxyMode::Custom,
        custom_url: cfg.custom_url.clone(),
    };

    // 三条路径互不依赖，并发跑；每条 5s 上限，最坏总耗时也就 5s 出头
    let (dns, direct, system, custom, active_proxy, active_endpoint) = tokio::join!(
        probe_dns(),
        probe_target(&direct_cfg),
        probe_target(&system_cfg),
        probe_target(&custom_cfg),
        async { cfg.redacted() },
        probe_proxy_endpoint(cfg),
    );

    let verdict_result = verdict(&VerdictInput {
        mode: cfg.mode,
        active_proxy: active_proxy.as_deref(),
        custom_configured: cfg.custom_valid(),
        dns: &dns,
        direct: &direct,
        system: &system,
        custom: &custom,
    });

    NetworkDiagnostics {
        mode: cfg.mode.as_str().to_string(),
        mode_label: cfg.mode.label().to_string(),
        active_proxy,
        active_proxy_source: cfg.source_label().to_string(),
        dns,
        active_proxy_endpoint: active_endpoint,
        direct,
        system,
        custom,
        proxy_ok: verdict_result.proxy_ok,
        x_reachable: verdict_result.x_reachable,
        verdict: verdict_result.text,
    }
}

/// 把 reqwest 错误说成人话，并带上底层原因（connection refused / 超时 都在里面）
fn describe(e: &reqwest::Error) -> String {
    let head = if e.is_timeout() {
        "连接超时"
    } else if e.is_connect() {
        "无法连接"
    } else if e.is_request() {
        "请求失败"
    } else {
        "失败"
    };
    match source_text(e) {
        Some(reason) => format!("{head}：{reason}"),
        None => head.to_string(),
    }
}

/// 取错误链里最靠下的一句（reqwest 的顶层信息只有 URL，原因在 source 里）
fn source_text(e: &reqwest::Error) -> Option<String> {
    let mut cursor: Option<&(dyn std::error::Error + 'static)> = Some(e);
    let mut last: Option<String> = None;
    while let Some(current) = cursor {
        let text = current.to_string();
        if !text.trim().is_empty() {
            last = Some(truncate(&text, 120));
        }
        cursor = current.source();
    }
    last
}

fn truncate(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::proxy::ProxyConfig;

    fn probe(label: &str, available: bool, detail: &str) -> PathProbe {
        PathProbe {
            label: label.to_string(),
            available,
            detail: detail.to_string(),
            latency_ms: None,
        }
    }

    #[test]
    fn fake_ip_只认代理常用网段() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        // Clash 的 fake-ip-range 默认 198.18.0.1/16
        assert!(is_fake_ip(ip("198.18.0.12")));
        assert!(is_fake_ip(ip("198.19.255.1")));
        assert!(!is_fake_ip(ip("198.20.0.1")));
        assert!(!is_fake_ip(ip("8.8.8.8")));
        assert!(!is_fake_ip(ip("::1")));
    }

    #[test]
    fn dns_定性污染优先于_fake_ip() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert_eq!(dns_verdict(&[]), DnsVerdict::Unresolved);
        assert_eq!(dns_verdict(&[ip("8.8.8.8")]), DnsVerdict::Normal);
        assert_eq!(dns_verdict(&[ip("198.18.0.12")]), DnsVerdict::FakeIp);
        assert_eq!(dns_verdict(&[ip("127.0.0.1")]), DnsVerdict::Polluted);
        // 公网 + 内网混合时按污染处理：内网那条根本连不到
        assert_eq!(
            dns_verdict(&[ip("8.8.8.8"), ip("192.168.1.1")]),
            DnsVerdict::Polluted
        );
    }

    #[test]
    fn 结论按模式给出且直连不唠叨() {
        let dns_ok = probe("本机 DNS", true, "x.com → 104.244.42.1");
        let dns_polluted = probe("本机 DNS", false, "x.com → 127.0.0.1（疑似域名污染）");
        let up = probe("直连", true, "HTTP 200");
        let down = probe("直连", false, "无法连接：connection refused");
        let absent = probe("自定义代理", false, "未配置或地址非法");

        // 直连可用：没问题、可达
        let v = verdict(&VerdictInput {
            mode: ProxyMode::Direct,
            active_proxy: None,
            custom_configured: false,
            dns: &dns_ok,
            direct: &up,
            system: &down,
            custom: &absent,
        });
        assert!(v.x_reachable && v.proxy_ok);
        assert!(v.text.contains("直连可以访问"));

        // 直连模式但网络不通：可达性为假，但不提醒（用户自己的选择）
        let v = verdict(&VerdictInput {
            mode: ProxyMode::Direct,
            active_proxy: None,
            custom_configured: false,
            dns: &dns_polluted,
            direct: &down,
            system: &down,
            custom: &absent,
        });
        assert!(!v.x_reachable);
        assert!(v.proxy_ok, "直连模式不该被当成代理问题");
        assert!(v.text.contains("当前网络无法访问 X"));
        assert!(v.text.contains("启用系统代理/TUN 或配置 VideoFlow 代理"));
        assert!(v.text.contains("域名污染"));

        // 系统代理没开、直连也不通：要提醒
        let v = verdict(&VerdictInput {
            mode: ProxyMode::System,
            active_proxy: None,
            custom_configured: false,
            dns: &dns_polluted,
            direct: &down,
            system: &down,
            custom: &absent,
        });
        assert!(!v.proxy_ok);
        assert!(v.text.contains("系统代理未启用"));

        // 系统代理没开但直连能通：不提醒
        let v = verdict(&VerdictInput {
            mode: ProxyMode::System,
            active_proxy: None,
            custom_configured: false,
            dns: &dns_ok,
            direct: &up,
            system: &down,
            custom: &absent,
        });
        assert!(v.proxy_ok);
        assert!(v.x_reachable);

        // 系统代理开了但不可用：提醒并点名地址
        let v = verdict(&VerdictInput {
            mode: ProxyMode::System,
            active_proxy: Some("http://127.0.0.1:7899"),
            custom_configured: false,
            dns: &dns_ok,
            direct: &down,
            system: &down,
            custom: &absent,
        });
        assert!(!v.proxy_ok);
        assert!(v.text.contains("127.0.0.1:7899"));

        // 自定义代理没填好
        let v = verdict(&VerdictInput {
            mode: ProxyMode::Custom,
            active_proxy: None,
            custom_configured: false,
            dns: &dns_ok,
            direct: &down,
            system: &down,
            custom: &absent,
        });
        assert!(!v.proxy_ok);
        assert!(v.text.contains("还没填好"));

        // 自定义代理填好且可用
        let v = verdict(&VerdictInput {
            mode: ProxyMode::Custom,
            active_proxy: Some("socks5h://127.0.0.1:7897"),
            custom_configured: true,
            dns: &dns_polluted,
            direct: &down,
            system: &down,
            custom: &up,
        });
        assert!(v.proxy_ok && v.x_reachable);
        assert!(v.text.contains("经自定义代理可以访问"));
    }

    #[tokio::test]
    async fn 自定义代理未配置时不发探测请求() {
        let cfg = ProxyConfig {
            mode: ProxyMode::Custom,
            custom_url: String::new(),
        };
        let probe = probe_target(&cfg).await;
        assert!(!probe.available);
        assert!(probe.detail.contains("未配置"));
        assert!(probe.latency_ms.is_none());
    }
}
