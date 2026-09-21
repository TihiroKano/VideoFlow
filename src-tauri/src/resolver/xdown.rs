//! X（Twitter）专用兜底解析：借 `xdown.app` 的公开搜索接口取回**原始 MP4** 地址。
//!
//! 定位（对应本轮需求）：
//! - **yt-dlp 仍是 X 链接的第一解析链路**（见 `resolver::ytdlp`）。本模块只在
//!   X URL 的 yt-dlp 解析失败后，由 `commands::resolve_media` 调用；它失败时
//!   仍会按原有规则退回内嵌浏览器，其它站点完全不受影响。
//! - **不依赖任何登录态**：不读 X 的登录 Cookie / `auth_token` / `ct0`，也不碰
//!   站点页面里的 `k_token` 之类的脚本变量——请求只带 `q` 与 `lang` 两个表单字段。
//! - **只提取原始媒体地址**：仅收 `https://video.twimg.com/` 下的 `.mp4`；
//!   站点自己的 `dl.snapcdn.app` 中转链接一律丢弃——那是它的下载层，不是我们的。
//!   站点把高清档地址放在中转链接的 JWT 载荷里（载荷里就是原始 twimg 地址），
//!   这里只把它当**容器**读出来，绝不拿它去访问站点下载层。
//! - **脱敏日志**：只记录主机名、条数与错误码；不落响应体，也不落完整媒体地址。

use std::collections::HashSet;

use base64::Engine;
use serde::Deserialize;
use url::Url;

use crate::core::model::{MediaStream, ResolvedMedia, StreamKind};
use crate::error::{AppError, AppResult};
use crate::resolver::RESOLVE_TTL_SECS;

/// 站点的公开搜索接口
const ENDPOINT: &str = "https://xdown.app/api/ajaxSearch";
/// 接口语言参数（需求指定）
const LANG: &str = "zh-cn";
/// 最多重试次数，不含首次（需求指定 1 次）
const MAX_RETRIES: usize = 1;

/// 只认这个前缀下的原始媒体
const MEDIA_PREFIX: &str = "https://video.twimg.com/";
/// 封面图前缀
const THUMB_PREFIX: &str = "https://pbs.twimg.com/";
/// 站点中转下载层（发现它的出现位置只为从载荷里取回原始地址）
const REDIRECT_MARKER: &str = "dl.snapcdn.app/get";

/// X 主机表。
///
/// 刻意不含 `t.co`：短链要先跟随重定向才知道真实推文，那是 yt-dlp 的活；
/// 拿短链去问兜底接口只会白白失败一次。
const X_HOSTS: &[&str] = &[
    "x.com",
    "www.x.com",
    "twitter.com",
    "www.twitter.com",
    "mobile.twitter.com",
];

/// 路径里可能出现的编码段（`/vid/avc1/720x1078/xxx.mp4`）
const CODEC_TOKENS: &[&str] = &[
    "avc1", "avc3", "hvc1", "hev1", "av01", "vp09", "vp9", "h264", "h265", "hevc",
];

/// 该 URL 是否属于 X（决定要不要在 yt-dlp 失败后尝试兜底）
pub fn is_x_url(url: &Url) -> bool {
    url.host_str()
        .map(|h| {
            let lower = h.to_ascii_lowercase();
            X_HOSTS.contains(&lower.as_str())
        })
        .unwrap_or(false)
}

/// 兜底解析入口：返回可直接交给**原生 HTTP 引擎**的统一模型。
///
/// `client` 由调用方按当前网络出口提供（短超时客户端，见 `net::ClientSpec::short`），
/// 这里不再自己持有客户端——出口要能随设置变化，`OnceLock` 那种写法做不到。
pub async fn resolve(client: &reqwest::Client, url: &Url) -> AppResult<ResolvedMedia> {
    if !is_x_url(url) {
        return Err(AppError::new(
            "PROVIDER_UNSUPPORTED",
            "XDown 兜底只处理 X 链接",
            false,
        ));
    }

    let mut attempt = 0usize;
    loop {
        attempt += 1;
        match fetch(client, url).await {
            Ok(html) => return build_media(url, &html),
            Err(err) => {
                // 日志只带错误码、主机名与第几次尝试
                eprintln!(
                    "[videoflow] XDown 兜底请求失败（{}，第 {} 次）：host={}",
                    err.code,
                    attempt,
                    host_of(url)
                );
                if attempt <= MAX_RETRIES && err.retryable {
                    continue;
                }
                return Err(err);
            }
        }
    }
}

/// 接口响应。成功时给 `data`（HTML 片段）；失败时给 `statusCode` + `msg`。
#[derive(Debug, Deserialize)]
struct SearchResponse {
    status: Option<String>,
    #[serde(rename = "statusCode")]
    status_code: Option<u16>,
    msg: Option<String>,
    data: Option<String>,
}

async fn fetch(client: &reqwest::Client, url: &Url) -> AppResult<String> {
    // 请求体只有 q 与 lang 两个字段——不带任何登录态、页面变量或站点 Token
    let form = [("q", url.as_str()), ("lang", LANG)];
    let resp = client
        .post(ENDPOINT)
        .form(&form)
        .send()
        .await
        .map_err(AppError::from)?;

    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::http_status(status.as_u16()));
    }

    let body = resp.text().await.map_err(AppError::from)?;
    // 解析失败时不把响应体塞进错误里（脱敏）
    let parsed: SearchResponse = serde_json::from_str(&body).map_err(|_| {
        AppError::new("PROVIDER_ERROR", "XDown 返回了无法解析的内容", false)
    })?;

    if parsed.status.as_deref() != Some("ok") {
        return Err(
            AppError::new("PROVIDER_ERROR", "XDown 接口返回异常状态", false)
                .with_detail(format!("status={}", parsed.status.as_deref().unwrap_or("缺省"))),
        );
    }
    if let Some(code) = parsed.status_code {
        if code >= 400 {
            return Err(no_stream(parsed.msg));
        }
    }
    parsed.data.ok_or_else(|| no_stream(parsed.msg))
}

/// 「没有可下载的原始视频」：站点文案原样带给上层（仅在日志/详情里出现）
fn no_stream(msg: Option<String>) -> AppError {
    let err = AppError::new("PROVIDER_NO_STREAM", "该链接没有可下载的原始视频", false);
    match msg.map(|m| m.trim().to_string()).filter(|m| !m.is_empty()) {
        Some(m) => err.with_detail(truncate(&m, 120)),
        None => err,
    }
}

/// 把 HTML 组装成统一模型
fn build_media(page_url: &Url, html: &str) -> AppResult<ResolvedMedia> {
    let candidates = collect_candidates(html);
    if candidates.is_empty() {
        eprintln!(
            "[videoflow] XDown 兜底未提取到原始 MP4：host={}",
            host_of(page_url)
        );
        return Err(no_stream(None));
    }

    let streams = to_streams(candidates);
    eprintln!(
        "[videoflow] XDown 兜底解析成功：{} 档清晰度（host={}）",
        streams.len(),
        host_of(page_url)
    );

    let title = extract_title(html)
        .or_else(|| tweet_id(page_url).map(|id| format!("X 视频 {id}")))
        .unwrap_or_else(|| "X 视频".to_string());

    let expires = chrono::Utc::now() + chrono::Duration::seconds(RESOLVE_TTL_SECS);

    Ok(ResolvedMedia {
        resolved_id: uuid::Uuid::new_v4().to_string(),
        title,
        description: Some("来自 X 的原始 MP4".to_string()),
        source_name: "X（推特）".to_string(),
        source_url: page_url.to_string(),
        thumbnail_url: first_thumbnail(html),
        duration_sec: None,
        streams,
        subtitles: Vec::new(),
        // 与其它 Provider 一致：过期后要求重新解析
        expires_at: Some(expires.to_rfc3339()),
        provider_id: "xdown".to_string(),
        provider_version: "1.0.0".to_string(),
    })
}

/// 一条候选地址及其从路径解析出的元信息
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    url: String,
    width: Option<u32>,
    height: Option<u32>,
    codec: Option<String>,
}

/// 收集候选：先收字面量地址，再从站点中转链接的载荷里补齐高清档，最后去重。
fn collect_candidates(html: &str) -> Vec<Candidate> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<Candidate> = Vec::new();

    let literal = collect_literals(html, MEDIA_PREFIX);
    let from_tokens = collect_from_redirect_tokens(html);

    for raw in literal.iter().chain(from_tokens.iter()) {
        let Some(url) = as_media_url(raw) else { continue };
        if !seen.insert(url.clone()) {
            continue;
        }
        let Ok(parsed) = Url::parse(&url) else { continue };
        let (width, height, codec) = path_meta(&parsed);
        out.push(Candidate {
            url,
            width,
            height,
            codec,
        });
    }
    out
}

/// 按字面前缀扫描地址：命中后一直读到第一个终结字符。
///
/// 不引入正则：地址的字符集很窄（`"` `'` `<` `>` `\` 反引号、空白、右括号都视为结束），
/// 手写扫描足够且没有任何额外依赖。
fn collect_literals(text: &str, prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(pos) = text[cursor..].find(prefix) {
        let start = cursor + pos;
        let rest = &text[start..];
        let end = rest
            .find(|c: char| {
                matches!(c, '"' | '\'' | '<' | '>' | '\\' | '`' | ')') || c.is_whitespace()
            })
            .unwrap_or(rest.len());
        out.push(rest[..end].to_string());
        cursor = start + end.max(1);
    }
    out
}

/// 站点把高清档包在 `dl.snapcdn.app/get?token=<JWT>` 里，载荷带原始 twimg 地址。
///
/// 只把它当容器读取原始地址；不校验签名、不使用该链接下载。
fn collect_from_redirect_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(pos) = text[cursor..].find(REDIRECT_MARKER) {
        let start = cursor + pos;
        let tail = &text[start..];
        let Some(offset) = tail.find("token=") else {
            cursor = start + REDIRECT_MARKER.len();
            continue;
        };
        let token_start = start + offset + "token=".len();
        let rest = &text[token_start..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
            .unwrap_or(rest.len());
        if end > 0 {
            if let Some(url) = media_url_from_token(&rest[..end]) {
                out.push(url);
            }
        }
        cursor = token_start + end.max(1);
    }
    out
}

/// 解码 JWT 载荷并取出其中的原始媒体地址
fn media_url_from_token(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let trimmed = payload.trim_end_matches('=');
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(trimmed)
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let url = json.get("url")?.as_str()?;
    as_media_url(url)
}

/// 校验并规范化：必须是 `https://video.twimg.com/` 下的 `.mp4`
fn as_media_url(raw: &str) -> Option<String> {
    // HTML 里的 & 会被写成 &amp;
    let cleaned = raw.replace("&amp;", "&");
    let parsed = Url::parse(&cleaned).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    if !parsed
        .host_str()?
        .eq_ignore_ascii_case("video.twimg.com")
    {
        return None;
    }
    if !parsed.path().to_ascii_lowercase().ends_with(".mp4") {
        return None;
    }
    Some(parsed.to_string())
}

/// 从路径里解析尺寸与编码：`/vid/avc1/720x1078/x.mp4` → (720, 1078, Some("avc1"))
fn path_meta(url: &Url) -> (Option<u32>, Option<u32>, Option<String>) {
    let mut dims: Option<(u32, u32)> = None;
    let mut codec: Option<String> = None;
    for seg in url.path_segments().into_iter().flatten() {
        let lower = seg.to_ascii_lowercase();
        if dims.is_none() {
            if let Some((w, h)) = parse_dims(&lower) {
                dims = Some((w, h));
            }
        }
        if codec.is_none() && CODEC_TOKENS.contains(&lower.as_str()) {
            codec = Some(lower);
        }
    }
    match dims {
        Some((w, h)) => (Some(w), Some(h), codec),
        None => (None, None, codec),
    }
}

/// `720x1078` → (720, 1078)；其它形状一律 None
fn parse_dims(segment: &str) -> Option<(u32, u32)> {
    let (w, h) = segment.split_once('x')?;
    let w: u32 = w.parse().ok()?;
    let h: u32 = h.parse().ok()?;
    if w == 0 || h == 0 {
        return None;
    }
    Some((w, h))
}

/// 候选 → 流：按短边降序排；同档只留一条（与 yt-dlp 那条链路的去重口径一致）
fn to_streams(mut candidates: Vec<Candidate>) -> Vec<MediaStream> {
    candidates.sort_by(|a, b| {
        short_side(b)
            .unwrap_or(0)
            .cmp(&short_side(a).unwrap_or(0))
            .then_with(|| a.url.cmp(&b.url))
    });

    let mut used_labels: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for cand in candidates {
        let quality_label = short_side(&cand).map(|s| format!("{s}P"));
        if let Some(label) = &quality_label {
            if !used_labels.insert(label.clone()) {
                continue;
            }
        }
        out.push(MediaStream {
            id: format!("xdown-{}", out.len()),
            container: "mp4".to_string(),
            kind: StreamKind::Muxed,
            quality_label,
            width: cand.width,
            height: cand.height,
            fps: None,
            audio_label: Some("随文件".to_string()),
            estimated_bytes: None,
            codec: cand.codec,
            sha256: None,
            needs_merge: false,
            url: cand.url,
            audio_url: None,
            referer: None,
        });
    }
    out
}

/// 竖屏视频拿长边当「P 数」会得到不存在的清晰度，统一按短边（与 yt-dlp 链路一致）
fn short_side(cand: &Candidate) -> Option<u32> {
    match (cand.width, cand.height) {
        (Some(w), Some(h)) => Some(w.min(h)),
        (Some(w), None) => Some(w),
        (None, Some(h)) => Some(h),
        (None, None) => None,
    }
}

/// 标题取推文正文（`<h3>`），去掉标签与实体后压成一行
fn extract_title(html: &str) -> Option<String> {
    let start = html.find("<h3>")? + "<h3>".len();
    let end = start + html[start..].find("</h3>")?;
    let text = decode_entities(&strip_tags(&html[start..end]));
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        None
    } else {
        Some(truncate(&collapsed, 80))
    }
}

/// 封面取页面上的第一张 `pbs.twimg.com` 图片
fn first_thumbnail(html: &str) -> Option<String> {
    for raw in collect_literals(html, THUMB_PREFIX) {
        let cleaned = raw.replace("&amp;", "&");
        let Ok(parsed) = Url::parse(&cleaned) else { continue };
        if parsed.scheme() != "https" {
            continue;
        }
        let path = parsed.path().to_ascii_lowercase();
        if [".jpg", ".jpeg", ".png", ".webp"]
            .iter()
            .any(|ext| path.ends_with(ext))
        {
            return Some(parsed.to_string());
        }
    }
    None
}

/// 从链接里取推文 id（`/status/<id>`），作为标题兜底
fn tweet_id(url: &Url) -> Option<String> {
    let segments: Vec<&str> = url.path_segments()?.collect();
    for pair in segments.windows(2) {
        if pair[0] == "status" || pair[0] == "statuses" {
            if pair[1].chars().all(|c| c.is_ascii_digit()) && !pair[1].is_empty() {
                return Some(pair[1].to_string());
            }
        }
    }
    None
}

fn strip_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

fn decode_entities(input: &str) -> String {
    input
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

/// 日志里只出现主机名
fn host_of(url: &Url) -> &str {
    url.host_str().unwrap_or("unknown")
}

fn truncate(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut out: String = input.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{engine_targets, EngineKind};

    /// 按站点真实结构拼一份响应 HTML：中转链接带 JWT 载荷、字面量最差档、封面、标题
    fn fixture_html() -> String {
        let payload = |url: &str, name: &str| {
            let json = format!(
                r#"{{"url":"{url}","filename":"XDown.app_{name}","nbf":1789922520,"exp":1789926120,"iat":1789922520}}"#
            );
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.as_bytes())
        };
        let token_1080 = payload(
            "https://video.twimg.com/amplify_video/2101206639034687488/vid/avc1/720x1078/V7w6-FC7a2VxrjPr.mp4?tag=14",
            "V7w6-FC7a2VxrjPr_1078p.mp4",
        );
        let token_720 = payload(
            "https://video.twimg.com/amplify_video/2101206639034687488/vid/avc1/480x718/iyDZ8U-UCxLmBT4o.mp4?tag=14",
            "iyDZ8U-UCxLmBT4o_718p.mp4",
        );
        format!(
            r##"        <div class="tw-video">
            <img src="https://pbs.twimg.com/amplify_video_thumb/2101206639034687488/img/qbXWv_nQHKazWc5c.jpg">
            <p><a href="https://dl.snapcdn.app/get?token=eyJhbGciOiJIUzI1NiJ9.{token_1080}.sig1" class="tw-button-dl">下载 MP4 (1078p)</a></p>
            <p><a href="https://dl.snapcdn.app/get?token=eyJhbGciOiJIUzI1NiJ9.{token_720}.sig2" class="tw-button-dl">下载 MP4 (718p)</a></p>
            <p><a href="#" data-audioUrl="https://video.twimg.com/amplify_video/2101206639034687488/vid/avc1/320x478/aVr7jsyLybybVp4W.mp4?tag=14" class="action-convert">转换为 MP3</a></p>
            <h3>今天的任务被主人安排<b>在户外</b>展示 &amp; 身材</h3>
        </div>"##
        )
    }

    #[test]
    fn 只认_x_主机的链接() {
        for ok in [
            "https://x.com/u/status/1",
            "https://www.x.com/u/status/1",
            "https://twitter.com/u/status/1",
            "https://mobile.twitter.com/u/status/1",
        ] {
            let url = Url::parse(ok).unwrap();
            assert!(is_x_url(&url), "{ok} 应判定为 X");
        }
        for no in [
            // 短链要先跟重定向，交给 yt-dlp，不在这里处理
            "https://t.co/abc",
            "https://www.douyin.com/video/1",
            "https://video.twimg.com/a.mp4",
            "https://notx.com/u/status/1",
        ] {
            let url = Url::parse(no).unwrap();
            assert!(!is_x_url(&url), "{no} 不该判定为 X");
        }
    }

    #[test]
    fn 只提取原始_mp4_并忽略中转与封面() {
        let html = fixture_html();
        let candidates = collect_candidates(&html);
        assert_eq!(candidates.len(), 3, "应拿到三档原始地址");
        for cand in &candidates {
            assert!(
                cand.url.starts_with(MEDIA_PREFIX),
                "只允许原始前缀，实际 {}",
                cand.url
            );
            assert!(
                !cand.url.contains("snapcdn"),
                "绝不能把站点中转链接当下载地址"
            );
        }
    }

    #[test]
    fn 去重同一条地址只留一次() {
        let html = fixture_html();
        let literal = collect_literals(&html, MEDIA_PREFIX);
        assert_eq!(literal.len(), 1, "字面量只有一条（最差档）");
        // 同一份 HTML 反复收集，候选数不变
        assert_eq!(collect_candidates(&html).len(), 3);
    }

    #[test]
    fn 拒绝非原始前缀与非_mp4() {
        // 中转下载层的地址即使出现在字面量里也不能收
        assert!(as_media_url("https://dl.snapcdn.app/get?token=abc").is_none());
        // 同域但不是 mp4 的一律拒绝
        assert!(as_media_url("https://video.twimg.com/x/master.m3u8").is_none());
        assert!(as_media_url("https://video.twimg.com/a.jpg").is_none());
        // http 不行
        assert!(as_media_url("http://video.twimg.com/a.mp4").is_none());
        // 正常的收下
        assert_eq!(
            as_media_url("https://video.twimg.com/ext_tw_video/1/pu/vid/320x400/a.mp4?tag=12"),
            Some("https://video.twimg.com/ext_tw_video/1/pu/vid/320x400/a.mp4?tag=12".to_string())
        );
    }

    #[test]
    fn html_实体在地址里被还原() {
        let raw = "https://video.twimg.com/a/b.mp4?tag=14&amp;x=1";
        assert_eq!(
            as_media_url(raw),
            Some("https://video.twimg.com/a/b.mp4?tag=14&x=1".to_string())
        );
    }

    #[test]
    fn 从路径解析尺寸与编码() {
        let url =
            Url::parse("https://video.twimg.com/amplify_video/1/vid/avc1/720x1078/x.mp4").unwrap();
        assert_eq!(path_meta(&url), (Some(720), Some(1078), Some("avc1".into())));

        // 老格式没有编码段
        let url = Url::parse("https://video.twimg.com/ext_tw_video/1/pu/vid/320x400/x.mp4").unwrap();
        assert_eq!(path_meta(&url), (Some(320), Some(400), None));

        // 没有尺寸段时只回编码
        let url = Url::parse("https://video.twimg.com/x/av01/y.mp4").unwrap();
        assert_eq!(path_meta(&url), (None, None, Some("av01".into())));

        // 尺寸段不合法不臆造
        assert_eq!(parse_dims("0x400"), None);
        assert_eq!(parse_dims("abc"), None);
        assert_eq!(parse_dims("axb"), None);
    }

    #[test]
    fn 令牌载荷只在无法解码时放弃() {
        // 正常载荷
        let json = r#"{"url":"https://video.twimg.com/a/b.mp4","filename":"x.mp4"}"#;
        let token = format!(
            "h.{}.s",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
        );
        assert_eq!(
            media_url_from_token(&token),
            Some("https://video.twimg.com/a/b.mp4".to_string())
        );
        // 载荷里不是原始前缀（例如封面）→ 丢弃
        let json = r#"{"url":"https://pbs.twimg.com/a.jpg"}"#;
        let token = format!(
            "h.{}.s",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
        );
        assert_eq!(media_url_from_token(&token), None);
        // 不是 JWT / 不是 JSON
        assert_eq!(media_url_from_token("not-a-jwt"), None);
        assert_eq!(media_url_from_token("h.bm90anNvbg.s"), None);
    }

    #[test]
    fn 清晰度按短边排序同档去重() {
        let candidates = collect_candidates(&fixture_html());
        let streams = to_streams(candidates);
        assert_eq!(streams.len(), 3, "三档各留一条");
        let labels: Vec<_> = streams
            .iter()
            .map(|s| s.quality_label.clone().unwrap_or_default())
            .collect();
        // 竖屏按短边：478 是最小档（长边 1078 不能当成清晰度名）
        assert_eq!(labels, vec!["720P", "480P", "320P"]);
        assert_eq!(streams[0].width, Some(720));
        assert_eq!(streams[0].height, Some(1078));
        assert_eq!(streams[0].codec.as_deref(), Some("avc1"));
        for s in &streams {
            assert_eq!(s.container, "mp4");
            assert_eq!(s.kind, StreamKind::Muxed);
            assert!(!s.needs_merge, "原始 MP4 自带音轨，不需要合并");
            assert!(s.referer.is_none(), "twimg 直链不校验来源");
        }
    }

    #[test]
    fn 结果交给原生_http_引擎() {
        let streams = to_streams(collect_candidates(&fixture_html()));
        let (engine, target, audio) = engine_targets(&streams[0], None);
        // 流 id 不能带 yt-/hls- 前缀，否则会被派给 yt-dlp 或 FFmpeg
        assert_eq!(engine, EngineKind::NativeHttp);
        assert_eq!(target.as_deref(), Some(streams[0].url.as_str()));
        assert!(audio.is_none());
    }

    #[test]
    fn 组装出统一模型() {
        let page =
            Url::parse("https://x.com/mineBUQ/status/2101206859013296412/video/1?s=46").unwrap();
        let media = build_media(&page, &fixture_html()).unwrap();
        assert_eq!(media.provider_id, "xdown");
        assert_eq!(media.source_url, page.to_string());
        assert_eq!(media.streams.len(), 3);
        assert_eq!(media.title, "今天的任务被主人安排在户外展示 & 身材");
        assert_eq!(
            media.thumbnail_url.as_deref(),
            Some("https://pbs.twimg.com/amplify_video_thumb/2101206639034687488/img/qbXWv_nQHKazWc5c.jpg")
        );
        assert!(media.expires_at.is_some(), "与其它 Provider 一致，带有效期");
    }

    #[test]
    fn 没有标题时用推文_id_兜底() {
        let page =
            Url::parse("https://x.com/mineBUQ/status/2101206859013296412/video/1?s=46").unwrap();
        let media = build_media(&page, &fixture_html().replace("<h3>", "<h4>")).unwrap();
        assert_eq!(media.title, "X 视频 2101206859013296412");
    }

    #[test]
    fn 提取不到原始地址时是明确的空结果错误() {
        let page = Url::parse("https://x.com/u/status/1").unwrap();
        let err = build_media(&page, "<div>没有媒体</div>").unwrap_err();
        assert_eq!(err.code, "PROVIDER_NO_STREAM");
        assert!(!err.retryable);
    }

    #[test]
    fn 失败响应按站点状态码分类() {
        let body = r#"{"status":"ok","statusCode":404,"msg":"未找到视频。也许视频是私人的或被阻止的。"}"#;
        let parsed: SearchResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.status_code, Some(404));
        let err = no_stream(parsed.msg);
        assert_eq!(err.code, "PROVIDER_NO_STREAM");
        assert_eq!(
            err.detail.as_deref(),
            Some("未找到视频。也许视频是私人的或被阻止的。")
        );
        // 站点文案过长时截断，避免把整段 HTML 带进错误里
        let long = "字".repeat(300);
        let err = no_stream(Some(long));
        assert!(err.detail.unwrap().chars().count() <= 121);
    }
}