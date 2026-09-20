//! Browser Resolver 的纯逻辑部分。
//!
//! 抖音的 Web 接口要求页面 JS 现算的 `a_bogus` 签名，纯 HTTP 客户端（含 yt-dlp）
//! 一律被 403 拦下——实测把真实登录态与 `Uifid` 头都补齐后，错误只是从
//! `Uifid Not Found` 前进到 `Signature Not Found`。所以这里让页面自己去算签名，
//! 我们只在旁边把结果捞回来。
//!
//! 本模块**不依赖 Tauri**：哨兵地址的编解码、抓取载荷到 `ResolvedMedia` 的映射
//! 都可以脱离 GUI 单测。真正驱动 WebView2 的部分在 `driver.rs`。

#[cfg(feature = "gui")]
pub mod driver;

use serde::Deserialize;

use crate::core::model::{MediaStream, ResolvedMedia, StreamKind};
use crate::error::{AppError, AppResult};
use crate::resolver::RESOLVE_TTL_SECS;

/// 注入脚本把结果发回来的哨兵地址。
///
/// 用 `.invalid` 顶级域（RFC 2606 保留，永远不会解析到真实主机），
/// 页面导航到这里会被 `on_navigation` 拦下并取消，不会真的产生网络请求。
pub const SENTINEL_HOST: &str = "vf-capture.invalid";
/// 哨兵地址里承载载荷的参数名
pub const SENTINEL_PARAM: &str = "d";

/// 一个用内嵌浏览器解析的站点
#[derive(Debug, Clone, Copy)]
pub struct BrowserSite {
    /// 稳定标识
    pub key: &'static str,
    /// 显示名
    pub label: &'static str,
    /// 归属域名（精确匹配，避免 `notdouyin.com` 被误判）
    pub hosts: &'static [&'static str],
    /// 下载直链时要带的 Referer；站点不校验来源时为 None
    pub referer: Option<&'static str>,
    /// `true`：yt-dlp 优先，只有它解析失败才兜底到内嵌浏览器；
    /// `false`：yt-dlp 处理不了（或不支持），直接用内嵌浏览器。
    pub prefers_ytdlp: bool,
}

/// 可用内嵌浏览器解析的站点表。
///
/// `prefers_ytdlp=false` 的两个是有实测依据的：
/// - 抖音：详情接口要求页面 JS 现算的 `a_bogus` 签名，yt-dlp 拿不到（一律 403）；
/// - 快手：yt-dlp **完全没有** extractor，返回 `Unsupported URL`。
pub const SITES: &[BrowserSite] = &[
    BrowserSite {
        key: "douyin",
        label: "抖音",
        hosts: &["douyin.com", "www.douyin.com", "v.douyin.com", "m.douyin.com", "iesdouyin.com", "www.iesdouyin.com"],
        // 抖音 CDN 直链校验来源，缺失会被 403（已实测）
        referer: Some("https://www.douyin.com/"),
        prefers_ytdlp: false,
    },
    BrowserSite {
        key: "kuaishou",
        label: "快手",
        hosts: &["kuaishou.com", "www.kuaishou.com", "v.kuaishou.com", "m.kuaishou.com"],
        // 实测快手 CDN 直链不需要 Referer
        referer: None,
        prefers_ytdlp: false,
    },
    BrowserSite {
        key: "twitter",
        label: "X（推特）",
        hosts: &["twitter.com", "www.twitter.com", "mobile.twitter.com", "x.com", "www.x.com", "t.co"],
        referer: Some("https://twitter.com/"),
        // yt-dlp 支持完整，这里只作保险
        prefers_ytdlp: true,
    },
    BrowserSite {
        key: "bilibili",
        label: "Bilibili",
        hosts: &["bilibili.com", "www.bilibili.com", "m.bilibili.com", "b23.tv"],
        referer: Some("https://www.bilibili.com/"),
        prefers_ytdlp: true,
    },
    BrowserSite {
        key: "youtube",
        label: "YouTube",
        hosts: &["youtube.com", "www.youtube.com", "m.youtube.com", "youtu.be"],
        referer: Some("https://www.youtube.com/"),
        prefers_ytdlp: true,
    },
];

/// 按域名找站点；不在表内返回 None（白名单语义）
pub fn site_for(url: &url::Url) -> Option<&'static BrowserSite> {
    let host = url.host_str()?.to_ascii_lowercase();
    SITES.iter().find(|s| s.hosts.contains(&host.as_str()))
}

/// 站点显示名
pub fn site_name(url: &url::Url) -> &'static str {
    site_for(url).map(|s| s.label).unwrap_or("站点")
}

/// 按站点推导下载时需要的 Referer
pub fn referer_for(page_url: &url::Url) -> Option<String> {
    site_for(page_url)?.referer.map(str::to_string)
}

/// 注入脚本回传的媒体载荷（字段名与 hook.js 里的输出一一对应）
#[derive(Debug, Clone, Deserialize)]
pub struct CapturePayload {
    /// 数据来源：aweme（站点结构化）/ dom（video 元素）/ sniff（响应体扫描）
    #[serde(default)]
    pub source: Option<String>,
    /// 视频标题
    pub desc: Option<String>,
    /// 页面标题（结构化数据没有标题时的兜底）
    #[serde(default, rename = "pageTitle")]
    pub page_title: Option<String>,
    /// 时长（毫秒）
    #[serde(rename = "durationMs")]
    pub duration_ms: Option<f64>,
    /// 封面地址
    pub cover: Option<String>,
    /// 无水印直链候选
    #[serde(default)]
    pub play: Vec<String>,
    /// 清晰度阶梯（仅站点结构化提取时有）
    #[serde(default)]
    pub ladder: Vec<LadderEntry>,
    /// 页面上 `<video>` 元素正在播的地址：最可信的一条（快手走这里）
    #[serde(default)]
    pub dom: Vec<String>,
    /// 响应体扫描出的地址：页面里常混着推荐流的下一条视频（实测快手就是），
    /// 只在前一项为空时使用
    #[serde(default)]
    pub sniff: Vec<String>,
}

/// 清晰度阶梯的一档
#[derive(Debug, Clone, Deserialize)]
pub struct LadderEntry {
    /// 分辨率字符串，如 `1080p` / `720p`
    pub label: Option<String>,
    /// 码率（bps）
    #[serde(rename = "bitRate")]
    pub bit_rate: Option<u64>,
    /// 视频编码标识（avc1 / hvc1 / av01），注入脚本按兼容性挑过一档
    #[serde(default)]
    pub codec: Option<String>,
    #[serde(default)]
    pub urls: Vec<String>,
}

/// 从哨兵地址里取出载荷并解码为 `CapturePayload`。
///
/// 返回 `None` 表示这不是我们的哨兵地址（可能是站点自身的导航，必须放行）。
pub fn parse_sentinel(url: &url::Url) -> Option<AppResult<CapturePayload>> {
    if url.host_str() != Some(SENTINEL_HOST) {
        return None;
    }
    let raw = url
        .query_pairs()
        .find(|(k, _)| k == SENTINEL_PARAM)
        .map(|(_, v)| v.to_string())?;
    Some(decode_payload(&raw))
}

/// 解码注入脚本回传的原始载荷字符串（哨兵地址里 query 的取值）
pub fn decode_captured(raw: &str) -> AppResult<CapturePayload> {
    decode_payload(raw)
}

/// 解码 base64url（无填充）承载的 JSON 载荷
fn decode_payload(raw: &str) -> AppResult<CapturePayload> {
    use base64::Engine;

    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|e| {
            AppError::new("PROVIDER_BROWSER_EMPTY", "无法解析浏览器返回的数据", true)
                .with_detail(format!("base64 解码失败：{e}"))
        })?;
    serde_json::from_slice(&bytes).map_err(|e| {
        AppError::new("PROVIDER_BROWSER_EMPTY", "无法解析浏览器返回的数据", true)
            .with_detail(format!("JSON 解析失败：{e}"))
    })
}

/// 把抓取到的载荷标准化为 `ResolvedMedia`。
///
/// `source_url` 用规范化后的页面地址：它同时决定任务重解析时的入口，
/// 也是我们推导 Referer 的依据。
pub fn to_resolved_media(
    payload: CapturePayload,
    page_url: &url::Url,
    provider_version: &str,
) -> AppResult<ResolvedMedia> {
    let referer = referer_for(page_url);

    // 每一档清晰度一条流；阶梯为空时退化为直链候选
    let mut streams: Vec<MediaStream> = payload
        .ladder
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| {
            let stream_url = entry.urls.first()?.clone();
            let height = entry.label.as_deref().and_then(parse_label_height);
            Some(MediaStream {
                id: format!("browser-{idx}"),
                container: "mp4".to_string(),
                kind: StreamKind::Muxed,
                quality_label: entry.label.clone(),
                width: None,
                height,
                fps: None,
                audio_label: Some("随视频".to_string()),
                // bit_rate 是 bps，乘时长得到估算体积
                estimated_bytes: estimate_bytes(entry.bit_rate, payload.duration_ms),
                codec: entry.codec.clone(),
                // 直链带签名会过期，且没有整文件摘要可校验
                sha256: None,
                needs_merge: false,
                url: stream_url,
                audio_url: None,
                referer: referer.clone(),
            })
        })
        .collect();

    // 阶梯缺失时用直链候选兜底，至少有一条能下
    if streams.is_empty() {
        // 优先级：页面上正在播的那条 > 响应体扫描结果 > 站点结构化给的直链。
        // 中间那档最容易混进推荐流的下一条视频，所以 DOM 有货就不看它。
        let candidates: Vec<String> = if !payload.dom.is_empty() {
            payload.dom.clone()
        } else if !payload.sniff.is_empty() {
            payload.sniff.clone()
        } else {
            payload.play.clone()
        };
        streams = candidates
            .iter()
            .enumerate()
            .map(|(idx, stream_url)| {
                let hls = is_hls_url(stream_url);
                MediaStream {
                    // HLS 用 hls- 前缀，engine_targets 会把它交给 FFmpeg 拉清单，
                    // 直接复用已有的 HLS 引擎
                    id: if hls {
                        format!("hls-{idx}")
                    } else {
                        format!("browser-plain-{idx}")
                    },
                    container: "mp4".to_string(),
                    kind: StreamKind::Muxed,
                    // 没有分辨率信息，交给界面显示「待确认」
                    quality_label: None,
                    width: None,
                    height: None,
                    fps: None,
                    audio_label: Some("随视频".to_string()),
                    estimated_bytes: None,
                    codec: None,
                    sha256: None,
                    needs_merge: false,
                    url: stream_url.clone(),
                    audio_url: None,
                    referer: referer.clone(),
                }
            })
            .collect();
    }

    if streams.is_empty() {
        return Err(AppError::new(
            "PROVIDER_BROWSER_EMPTY",
            "没有从页面里抓到可下载的媒体地址",
            true,
        )
        .with_hint("可能是该视频受限，或页面结构已变化；请重试或换一个链接"));
    }

    // 清晰度高者在先，同清晰度先比体积再比码率
    streams.sort_by(|a, b| {
        b.height
            .unwrap_or(0)
            .cmp(&a.height.unwrap_or(0))
            .then_with(|| {
                b.estimated_bytes
                    .unwrap_or(0)
                    .cmp(&a.estimated_bytes.unwrap_or(0))
            })
    });

    let expires = chrono::Utc::now() + chrono::Duration::seconds(RESOLVE_TTL_SECS);

    Ok(ResolvedMedia {
        resolved_id: uuid::Uuid::new_v4().to_string(),
        title: payload
            .desc
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty())
            // 通用嗅探没有结构化标题，用页面标题兜底
            .or_else(|| {
                payload
                    .page_title
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
            })
            .unwrap_or_else(|| "未命名视频".to_string()),
        description: None,
        source_name: site_name(page_url).to_string(),
        source_url: page_url.to_string(),
        thumbnail_url: payload.cover,
        duration_sec: payload.duration_ms.map(|ms| ms / 1000.0),
        streams,
        subtitles: Vec::new(),
        expires_at: Some(expires.to_rfc3339()),
        provider_id: "browser".to_string(),
        provider_version: provider_version.to_string(),
    })
}

/// 地址是否指向 HLS 清单（决定交给 HLS 引擎还是原生引擎）
fn is_hls_url(url: &str) -> bool {
    url.split('?').next().unwrap_or(url).to_ascii_lowercase().ends_with(".m3u8")
}

/// `1080p` → `1080`；解析不出来返回 `None`
fn parse_label_height(label: &str) -> Option<u32> {
    let digits: String = label
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse::<u32>().ok().filter(|h| *h > 0)
}

/// 用码率与时长估算体积
fn estimate_bytes(bit_rate: Option<u64>, duration_ms: Option<f64>) -> Option<u64> {
    let bps = bit_rate.filter(|b| *b > 0)?;
    let secs = duration_ms.filter(|d| *d > 0.0)? / 1000.0;
    Some((bps as f64 * secs / 8.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(id: &str) -> url::Url {
        url::Url::parse(&format!("https://www.douyin.com/video/{id}")).unwrap()
    }

    fn payload_of(json: &str) -> CapturePayload {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn 抖音各域名都走浏览器解析() {
        for host in [
            "https://www.douyin.com/video/1",
            "https://v.douyin.com/ABC/",
            "https://m.douyin.com/share/video/1",
            "https://www.iesdouyin.com/share/video/1/",
            "https://douyin.com/video/1",
        ] {
            let u = url::Url::parse(host).unwrap();
            let site = site_for(&u).expect("应命中站点表");
            assert_eq!(site.key, "douyin", "{host} 应归到抖音");
            assert!(!site.prefers_ytdlp, "抖音必须直接走浏览器");
        }
    }

    #[test]
    fn 快手域名命中且不优先_ytdlp() {
        // yt-dlp 完全没有快手 extractor（实测 Unsupported URL），Browser 是唯一通路
        for host in [
            "https://www.kuaishou.com/short-video/3xabc",
            "https://v.kuaishou.com/ABC",
            "https://m.kuaishou.com/short-video/1",
        ] {
            let u = url::Url::parse(host).unwrap();
            let site = site_for(&u).expect("应命中站点表");
            assert_eq!(site.key, "kuaishou");
            assert!(!site.prefers_ytdlp);
            // 实测快手 CDN 直链不需要 Referer
            assert_eq!(site.referer, None);
            assert_eq!(referer_for(&u), None);
        }
    }

    #[test]
    fn 推特与_youtube_优先_ytdlp_但仍在表内() {
        for (host, key) in [
            ("https://x.com/u/status/123", "twitter"),
            ("https://twitter.com/u/status/123", "twitter"),
            ("https://t.co/abc", "twitter"),
            ("https://www.youtube.com/watch?v=abc", "youtube"),
            ("https://youtu.be/abc", "youtube"),
            ("https://www.bilibili.com/video/BV1xx", "bilibili"),
            ("https://b23.tv/abc", "bilibili"),
        ] {
            let u = url::Url::parse(host).unwrap();
            let site = site_for(&u).unwrap_or_else(|| panic!("{host} 应命中站点表"));
            assert_eq!(site.key, key, "{host} 站点判定错");
            // 这些站点 yt-dlp 处理得好，浏览器只作兜底
            assert!(site.prefers_ytdlp, "{host} 应优先 yt-dlp");
        }
    }

    #[test]
    fn 其它站点不在白名单内() {
        for host in [
            "https://cdn.example.com/a.mp4",
            // 同后缀但不同域：不能用 ends_with 粗暴匹配
            "https://notdouyin.com/video/1",
            "https://douyin.com.evil.test/video/1",
            "https://notkuaishou.com/x",
        ] {
            let u = url::Url::parse(host).unwrap();
            assert!(site_for(&u).is_none(), "{host} 不该命中白名单");
            assert_eq!(site_name(&u), "站点");
        }
    }

    #[test]
    fn 哨兵地址能往返编解码() {
        use base64::Engine;
        let json = r#"{"desc":"标题","durationMs":323167,"play":["https://cdn/a.mp4"]}"#;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json);
        let sentinel =
            url::Url::parse(&format!("https://{SENTINEL_HOST}/?{SENTINEL_PARAM}={encoded}"))
                .unwrap();

        let parsed = parse_sentinel(&sentinel).expect("应识别为哨兵地址");
        let payload = parsed.expect("应能解码");
        assert_eq!(payload.desc.as_deref(), Some("标题"));
        assert_eq!(payload.duration_ms, Some(323167.0));
        assert_eq!(payload.play, vec!["https://cdn/a.mp4"]);
    }

    #[test]
    fn 非哨兵地址必须放行() {
        // 站点自身的导航（含抖音的短链重定向）绝不能被误拦
        for other in [
            "https://www.douyin.com/video/1",
            "https://www.iesdouyin.com/share/video/1/",
            "https://vf-capture.invalid.example.com/?d=x",
        ] {
            let u = url::Url::parse(other).unwrap();
            assert!(parse_sentinel(&u).is_none(), "{other} 不该被当成哨兵");
        }
    }

    #[test]
    fn 哨兵地址缺参数时不误判成功() {
        let u = url::Url::parse(&format!("https://{SENTINEL_HOST}/")).unwrap();
        // host 命中但没有载荷参数：属于哨兵地址，但解析应失败而不是 panic
        assert!(parse_sentinel(&u).is_none());
    }

    #[test]
    fn 载荷损坏时返回分类错误() {
        let u = url::Url::parse(&format!("https://{SENTINEL_HOST}/?{SENTINEL_PARAM}=!!!bad!!!"))
            .unwrap();
        let err = parse_sentinel(&u).unwrap().unwrap_err();
        assert_eq!(err.code, "PROVIDER_BROWSER_EMPTY");
    }

    #[test]
    fn 阶梯映射为清晰度列表并带_referer() {
        let payload = payload_of(
            r#"{
              "desc":"绿裤子教我猛攻绝航",
              "durationMs":323167,
              "cover":"https://p3.douyinpic.com/cover.jpeg",
              "play":["https://v11-weba.douyinvod.com/a.mp4"],
              "ladder":[
                {"label":"1080p","bitRate":3000503,"urls":["https://v11-weba.douyinvod.com/1080.mp4"]},
                {"label":"720p","bitRate":2073956,"urls":["https://v11-weba.douyinvod.com/720.mp4"]}
              ]
            }"#,
        );
        let media = to_resolved_media(payload, &page("7686809202295131426"), "webview2").unwrap();

        assert_eq!(media.provider_id, "browser");
        assert_eq!(media.source_name, "抖音");
        assert_eq!(media.title, "绿裤子教我猛攻绝航");
        assert_eq!(media.duration_sec, Some(323.167));
        assert_eq!(media.streams.len(), 2);
        // 1080 排在 720 前面
        assert_eq!(media.streams[0].quality_label.as_deref(), Some("1080p"));
        assert_eq!(media.streams[0].height, Some(1080));
        assert_eq!(media.streams[1].height, Some(720));
        // 抖音 CDN 直链必须带 Referer，否则 403
        for s in &media.streams {
            assert_eq!(s.referer.as_deref(), Some("https://www.douyin.com/"));
            assert_eq!(s.container, "mp4");
            assert_eq!(s.kind, StreamKind::Muxed);
            assert!(!s.needs_merge);
        }
        // 估算体积：3_000_503bps × 323.167s / 8
        let expect = (3_000_503f64 * 323.167 / 8.0) as u64;
        assert_eq!(media.streams[0].estimated_bytes, Some(expect));
    }

    #[test]
    fn 阶梯缺失时用直链兜底() {
        let payload = payload_of(
            r#"{"desc":"无阶梯","play":["https://cdn/a.mp4","https://cdn/b.mp4"]}"#,
        );
        let media = to_resolved_media(payload, &page("1"), "webview2").unwrap();
        assert_eq!(media.streams.len(), 2);
        // 没有分辨率信息时不编造清晰度
        assert_eq!(media.streams[0].quality_label, None);
        assert_eq!(media.streams[0].estimated_bytes, None);
        assert_eq!(media.streams[0].referer.as_deref(), Some("https://www.douyin.com/"));
    }

    #[test]
    fn 完全没有可用地址时报错() {
        let payload = payload_of(r#"{"desc":"空","play":[],"ladder":[]}"#);
        let err = to_resolved_media(payload, &page("1"), "webview2").unwrap_err();
        assert_eq!(err.code, "PROVIDER_BROWSER_EMPTY");
    }

    #[test]
    fn 通用嗅探的直链映射为原生引擎流() {
        // 快手走的就是这条路：从 <video> 元素拿到 CDN 直链
        let payload = payload_of(
            r#"{"source":"sniff","pageTitle":"某个快手视频","dom":["https://k0u7.djvod.ndcimgs.com/a.mp4?tag=1-un"]}"#,
        );
        let media = to_resolved_media(
            payload,
            &url::Url::parse("https://www.kuaishou.com/short-video/3xabc").unwrap(),
            "webview2",
        )
        .unwrap();

        assert_eq!(media.title, "某个快手视频", "没有结构化标题时用页面标题");
        assert_eq!(media.source_name, "快手");
        assert_eq!(media.streams.len(), 1);
        // 非 m3u8：走原生 HTTP 引擎
        assert!(media.streams[0].id.starts_with("browser-plain-"));
        assert_eq!(media.streams[0].referer, None, "快手实测不需要 Referer");
    }

    #[test]
    fn m3u8_候选交给_hls_引擎() {
        // 推特等 HLS 站点的兜底：识别出清单地址后复用已有的 HLS 引擎
        let payload = payload_of(
            r#"{"source":"sniff","dom":["https://video.twimg.com/amplify_video/1/pl/abc.m3u8?tag=1","https://cdn/a.mp4"]}"#,
        );
        let media = to_resolved_media(
            payload,
            &url::Url::parse("https://x.com/u/status/1").unwrap(),
            "webview2",
        )
        .unwrap();

        assert_eq!(media.streams.len(), 2);
        let hls = media.streams.iter().find(|s| s.id.starts_with("hls-"));
        assert!(hls.is_some(), "带 query 的 m3u8 也要认出来");
        assert!(hls.unwrap().url.contains(".m3u8"));
        // 推特要带 Referer
        assert_eq!(
            hls.unwrap().referer.as_deref(),
            Some("https://twitter.com/")
        );
        assert!(media.streams.iter().any(|s| s.id.starts_with("browser-plain-")));
    }

    #[test]
    fn 有清晰度阶梯时优先用阶梯而不是通用候选() {
        let payload = payload_of(
            r#"{"source":"aweme","ladder":[{"label":"1080p","bitRate":3000000,"urls":["https://cdn/1080.mp4"]}],
                "dom":["https://cdn/other.mp4"]}"#,
        );
        let media = to_resolved_media(payload, &page("1"), "webview2").unwrap();
        assert_eq!(media.streams.len(), 1);
        assert_eq!(media.streams[0].quality_label.as_deref(), Some("1080p"));
    }

    #[test]
    fn dom_有候选时不用响应体扫描结果() {
        // 实测快手：页面正在播的是 dom 里那条，而响应体里混着推荐流的下一条视频。
        // 两者都收进列表会让用户下到别人的视频，所以 dom 有货就不看 sniff。
        let payload = payload_of(
            r#"{"source":"sniff","pageTitle":"某条快手视频",
                "dom":["https://cdn/playing.mp4"],
                "sniff":["https://cdn/next-video.mp4","https://cdn/another.mp4"]}"#,
        );
        let media = to_resolved_media(
            payload,
            &url::Url::parse("https://www.kuaishou.com/short-video/3xabc").unwrap(),
            "webview2",
        )
        .unwrap();

        assert_eq!(media.streams.len(), 1, "只保留 DOM 里正在播的那条");
        assert_eq!(media.streams[0].url, "https://cdn/playing.mp4");
    }

    #[test]
    fn dom_为空时退回响应体扫描结果() {
        // 推特这类用 MSE 的站点，<video> 是 blob: 地址，只能靠响应体扫描
        let payload = payload_of(
            r#"{"source":"sniff","sniff":["https://video.twimg.com/a.m3u8"]}"#,
        );
        let media = to_resolved_media(
            payload,
            &url::Url::parse("https://x.com/u/status/1").unwrap(),
            "webview2",
        )
        .unwrap();
        assert_eq!(media.streams.len(), 1);
        assert!(media.streams[0].id.starts_with("hls-"));
    }

    #[test]
    fn 阶梯里的编码会下发给前端() {
        // 抖音 4K 档只有 H.265，Windows 自带播放器解不了——必须让界面能显示编码，
        // 否则用户看到「下载成功但打不开」无从判断原因。
        let payload = payload_of(
            r#"{"ladder":[{"label":"2160p","bitRate":4792000,"codec":"hvc1","urls":["https://cdn/4k.mp4"]},
                {"label":"1080p","bitRate":3000503,"codec":"avc1","urls":["https://cdn/1080.mp4"]}]}"#,
        );
        let media = to_resolved_media(payload, &page("1"), "webview2").unwrap();
        assert_eq!(media.streams[0].codec.as_deref(), Some("hvc1"));
        assert_eq!(media.streams[1].codec.as_deref(), Some("avc1"));
    }

    #[test]
    fn 阶梯缺编码字段时不报错() {
        // 老版本 hook 或站点改字段时不能让整次解析失败
        let payload = payload_of(
            r#"{"ladder":[{"label":"1080p","bitRate":3000000,"urls":["https://cdn/a.mp4"]}]}"#,
        );
        let media = to_resolved_media(payload, &page("1"), "webview2").unwrap();
        assert_eq!(media.streams[0].codec, None);
    }

    #[test]
    fn hls_地址识别忽略_query() {
        assert!(is_hls_url("https://c.com/a.m3u8"));
        assert!(is_hls_url("https://c.com/a.m3u8?token=x"));
        assert!(!is_hls_url("https://c.com/a.mp4"));
        assert!(!is_hls_url("https://c.com/m3u8"));
    }

    #[test]
    fn 没有标题时给兜底名() {
        let payload = payload_of(r#"{"play":["https://cdn/a.mp4"]}"#);
        let media = to_resolved_media(payload, &page("1"), "webview2").unwrap();
        assert_eq!(media.title, "未命名视频");
    }

    #[test]
    fn 清晰度标签解析只认开头数字() {
        assert_eq!(parse_label_height("1080p"), Some(1080));
        assert_eq!(parse_label_height("720P"), Some(720));
        assert_eq!(parse_label_height("p1080"), None);
        assert_eq!(parse_label_height(""), None);
        assert_eq!(parse_label_height("0p"), None);
    }
}