//! 站点 Provider：通过 yt-dlp 解析页面并标准化为 `ResolvedMedia`。
//!
//! 合规边界（项目书 §0.2）：只处理用户有权下载的公开资源；
//! 遇到登录、会员、地区或访问控制限制时返回 `PROVIDER_RESTRICTED`，不做任何绕过。

use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;
use url::Url;

use crate::core::model::{MediaStream, ResolvedMedia, StreamKind, SubtitleTrack};
use crate::error::{AppError, AppResult};
use crate::net::ProxyConfig;
use crate::platform::sidecar;
use crate::resolver::RESOLVE_TTL_SECS;

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(90);

/// 直连模式必须从子进程环境里清掉的代理变量。
///
/// yt-dlp（Python 的 urllib）会自己读这些变量，不清掉的话用户在应用里选了「直连」，
/// 子进程照样走代理——「名义直连、实际走代理」是最难排查的一类不一致。
const PROXY_ENV_KEYS: [&str; 6] = [
    "ALL_PROXY",
    "all_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
];

/// yt-dlp 的出口参数：一次算好 `--proxy` 与环境清理，避免重复探测系统代理。
///
/// 解析链路与下载链路共用，保证「解析用哪个出口、下载就用哪个出口」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YtDlpProxy {
    /// `--proxy` 的取值；None = 不传（由 yt-dlp 自己决定，但环境已被清理）
    pub url: Option<String>,
    /// 是否需要清理子进程环境里的代理变量
    pub clear_env: bool,
}

impl YtDlpProxy {
    pub fn from_config(proxy: &ProxyConfig) -> Self {
        let url = proxy.effective_url();
        Self {
            clear_env: url.is_none(),
            url,
        }
    }

    /// 追加到命令行
    pub fn append_args(&self, args: &mut Vec<String>) {
        if let Some(url) = &self.url {
            args.push("--proxy".into());
            args.push(url.clone());
        }
    }

    /// 直连时把环境里的代理变量清掉
    pub fn apply_env(&self, cmd: &mut Command) {
        if self.clear_env {
            for key in PROXY_ENV_KEYS {
                cmd.env_remove(key);
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawFormat {
    format_id: Option<String>,
    ext: Option<String>,
    vcodec: Option<String>,
    acodec: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<f64>,
    filesize: Option<u64>,
    filesize_approx: Option<u64>,
    format_note: Option<String>,
    abr: Option<f64>,
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawInfo {
    title: Option<String>,
    description: Option<String>,
    thumbnail: Option<String>,
    duration: Option<f64>,
    extractor_key: Option<String>,
    webpage_url: Option<String>,
    formats: Option<Vec<RawFormat>>,
    #[serde(default)]
    subtitles: std::collections::HashMap<String, Vec<RawSubtitle>>,
}

#[derive(Debug, Deserialize)]
struct RawSubtitle {
    name: Option<String>,
}

fn has_video(f: &RawFormat) -> bool {
    f.vcodec.as_deref().map(|v| v != "none").unwrap_or(false)
}

fn has_audio(f: &RawFormat) -> bool {
    f.acodec.as_deref().map(|v| v != "none").unwrap_or(false)
}

fn size_of(f: &RawFormat) -> Option<u64> {
    f.filesize.or(f.filesize_approx)
}

/// 把 http 封面地址升到 https。
///
/// 站点给的封面常是 http（例如 Bilibili 的 `http://i2.hdslb.com/...`），
/// 那个 CDN 同时做了防盗链：带本应用的 Referer 会返回 403，只有不带 Referer
/// 或带站点自身的 Referer 才能取到。这里统一升到 https（该 CDN 两种协议都支持），
/// 前端再配合 `referrerpolicy="no-referrer"` 请求。
fn upgrade_thumbnail(url: String) -> String {
    match url.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => url,
    }
}

/// 视频编码兼容性排序：数字越小越优先。
/// 同一清晰度优先挑 H.264（到处都能播），其次 H.265，最后才是 AV1 / VP9。
fn video_codec_rank(s: &MediaStream) -> u8 {
    match s.codec.as_deref().map(|c| c.to_ascii_lowercase()) {
        Some(c) if c.starts_with("avc") || c.starts_with("h264") => 0,
        Some(c) if c.starts_with("hev") || c.starts_with("hvc") || c.starts_with("h265") => 1,
        Some(c) if c.starts_with("av01") => 2,
        Some(c) if c.starts_with("vp9") || c.starts_with("vp09") => 3,
        Some(_) => 4,
        // 编码未知时排在已知编码之后，避免顶掉确定的兼容选项
        None => 5,
    }
}

/// 站点要求登录态时的统一提示
///
/// 抖音没有登录态就拿不到播放地址，Bilibili 的高码率与 60 帧也只对已登录账号开放。
/// 这里复用用户自己浏览器里已有的会话，取到的仍是该账号本来就有权观看的内容。
pub fn cookies_hint() -> AppError {
    AppError::new("PROVIDER_NEEDS_COOKIES", "该站点需要登录状态才能解析", true).with_hint(
        "到「设置 → 下载 → 账户登录」登录该站点；想用浏览器里已有的登录态，可在同一处的「其他方式」里选择浏览器或 cookies.txt。抖音必须开启，Bilibili 的高清晰度也需要它",
    )
}

/// 登录态来源（对应 `Settings::cookies_source`）
pub const COOKIES_BROWSER_PREFIX: &str = "browser:";
pub const COOKIES_FILE_PREFIX: &str = "file:";
/// 应用内账户登录：Cookie 由本应用从内嵌浏览器导出，存放在固定路径
pub const COOKIES_ACCOUNT_PREFIX: &str = "account:";

/// 把登录态参数追加到 yt-dlp 命令行
///
/// 三种来源各有取舍：
/// - `account:`：应用内账户登录，Cookie 由内嵌浏览器导出到固定路径。
///   推荐方式——不用关浏览器，也不用手动导出文件；
/// - `browser:<名>`：复用浏览器已有登录态，最省事，但 Chromium 系浏览器运行时
///   会锁住 Cookie 数据库，yt-dlp 复制不出来（上游 issue #7271），
///   此时必须完全退出浏览器；
/// - `file:<路径>`：用户自己导出的 cookies.txt，不受浏览器是否运行影响。
pub fn append_cookie_args(args: &mut Vec<String>, source: Option<&str>) {
    let Some(source) = source.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    if let Some(browser) = source.strip_prefix(COOKIES_BROWSER_PREFIX) {
        let browser = browser.trim();
        if !browser.is_empty() {
            args.push("--cookies-from-browser".into());
            args.push(browser.to_string());
        }
    } else if let Some(path) = source.strip_prefix(COOKIES_FILE_PREFIX) {
        let path = path.trim();
        if !path.is_empty() {
            args.push("--cookies".into());
            args.push(path.to_string());
        }
    } else if source.starts_with(COOKIES_ACCOUNT_PREFIX) {
        // 路径由应用管理，忽略前缀后面的内容，避免设置被改坏后指向别的文件
        args.push("--cookies".into());
        args.push(crate::account::cookie_store_path().to_string_lossy().to_string());
    }
}

/// 从登录态来源里取出浏览器键名（`browser:edge` → `edge`）
fn browser_key(source: Option<&str>) -> Option<&str> {
    source
        .map(str::trim)?
        .strip_prefix(COOKIES_BROWSER_PREFIX)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// 把 yt-dlp 的错误输出映射为分类错误
fn classify_stderr(text: &str, cookies_source: Option<&str>) -> AppError {
    let lower = text.to_ascii_lowercase();

    // 浏览器锁库必须排在登录态判断之前：它的原文是「Could not copy Chrome cookie
    // database」，虽然说的是 cookie，但成因是浏览器还开着，不是没登录。
    // 混进「需要登录」里会让用户白折腾登录状态却始终解析不了。
    if lower.contains("could not copy") && lower.contains("cookie database") {
        return AppError::provider_cookie_locked(browser_key(cookies_source).unwrap_or(""));
    }

    // 登录态问题优先归类：这类失败有明确的解决办法，不该被压成笼统的「不支持」
    const NEEDS_LOGIN: [&str; 5] = [
        "fresh cookies",
        "cookies",
        "login required",
        "sign in",
        "registered users",
    ];
    if NEEDS_LOGIN.iter().any(|k| lower.contains(k)) {
        return cookies_hint();
    }

    const RESTRICTED: [&str; 4] = [
        "members-only",
        "premium",
        "drm",
        "not available in your country",
    ];
    if RESTRICTED.iter().any(|k| lower.contains(k)) {
        return AppError::provider_restricted();
    }
    if lower.contains("unsupported url") || lower.contains("no suitable extractor") {
        return AppError::provider_unsupported();
    }
    if lower.contains("unable to download webpage")
        || lower.contains("connection")
        || lower.contains("timed out")
    {
        return AppError::network("无法访问该站点");
    }
    if lower.contains("video unavailable") || lower.contains("404") {
        return AppError::new("PROVIDER_UNAVAILABLE", "该视频不可用或已被删除", false);
    }
    AppError::new("PROVIDER_ERROR", "解析失败", false).with_detail(text.trim().to_string())
}

async fn run_ytdlp(
    args: &[String],
    cookies_source: Option<&str>,
    proxy: &YtDlpProxy,
) -> AppResult<String> {
    let exe = sidecar::yt_dlp_path().ok_or_else(AppError::provider_tool_missing)?;

    let mut cmd = Command::new(&exe);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PYTHONIOENCODING", "utf-8")
        .kill_on_drop(true);
    // 直连模式：把环境里的代理变量清掉，避免子进程偷偷走代理
    proxy.apply_env(&mut cmd);

    #[cfg(windows)]
    {
        // 避免弹出控制台窗口
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let child = cmd.spawn().map_err(|e| {
        AppError::provider_tool_missing().with_detail(format!("启动 yt-dlp 失败：{e}"))
    })?;

    let output = match tokio::time::timeout(RESOLVE_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(AppError::internal(format!("yt-dlp 执行失败：{e}"))),
        Err(_) => {
            return Err(AppError::new(
                "PROVIDER_TIMEOUT",
                "解析超时",
                true,
            )
            .with_hint("站点响应较慢，请稍后重试"))
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(classify_stderr(&stderr, cookies_source));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// 用 yt-dlp 解析站点页面。
///
/// `proxy` 决定出口：会转成 `--proxy`（直连时不传，并清掉子进程的代理环境变量）。
pub async fn resolve(
    url: &Url,
    cookies_source: Option<&str>,
    proxy: &ProxyConfig,
) -> AppResult<ResolvedMedia> {
    let mut args: Vec<String> = vec![
        "--dump-single-json".into(),
        "--no-warnings".into(),
        "--no-playlist".into(),
        "--no-progress".into(),
        "--socket-timeout".into(),
        "20".into(),
    ];
    // 复用登录态：抖音的播放地址与 Bilibili 的高清晰度都要求已登录
    append_cookie_args(&mut args, cookies_source);
    let ytdlp_proxy = YtDlpProxy::from_config(proxy);
    ytdlp_proxy.append_args(&mut args);
    args.push(url.to_string());

    let stdout = run_ytdlp(&args, cookies_source, &ytdlp_proxy).await?;
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Err(AppError::new("PROVIDER_EMPTY", "解析没有返回任何内容", false));
    }

    let info: RawInfo = serde_json::from_str(trimmed).map_err(|e| {
        AppError::internal(format!("无法理解 yt-dlp 的返回：{e}"))
    })?;

    let formats = info.formats.unwrap_or_default();
    let mut video_streams: Vec<MediaStream> = Vec::new();
    let mut audio_streams: Vec<MediaStream> = Vec::new();

    for f in &formats {
        let Some(fid) = f.format_id.clone() else { continue };
        let Some(furl) = f.url.clone() else { continue };
        // 跳过没有实际媒体内容的条目（例如纯 storyboard）
        let is_v = has_video(f);
        let is_a = has_audio(f);
        if !is_v && !is_a {
            continue;
        }

        let container = f.ext.clone().unwrap_or_else(|| "mp4".into());
        // 竖屏视频的 height 是长边（例如 1920），直接拿它当「P 数」会得到
        // 「1920P」这种并不存在的清晰度；以短边为准才是通行叫法。
        let short_side = match (f.width, f.height) {
            (Some(w), Some(h)) => Some(w.min(h)),
            (None, Some(h)) => Some(h),
            (Some(w), None) => Some(w),
            (None, None) => None,
        };
        let quality_label = f
            .format_note
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| short_side.map(|s| format!("{s}P")));

        let audio_label = if is_a {
            f.abr
                .map(|abr| format!("{} {}kbps", f.acodec.clone().unwrap_or_else(|| "AAC".into()).to_uppercase(), abr.round() as i64))
                .or_else(|| Some(f.acodec.clone().unwrap_or_else(|| "音频".into()).to_uppercase()))
        } else {
            None
        };

        let stream = MediaStream {
            id: format!("yt-{fid}"),
            container,
            kind: if is_v && is_a {
                StreamKind::Muxed
            } else if is_v {
                StreamKind::Video
            } else {
                StreamKind::Audio
            },
            quality_label,
            width: f.width,
            height: f.height,
            fps: f.fps,
            audio_label,
            estimated_bytes: size_of(f),
            codec: f.vcodec.clone().filter(|v| v != "none"),
            // yt-dlp 不提供内容哈希（DASH/HLS 流没有整文件摘要），只做长度校验
            sha256: None,
            needs_merge: is_v && !is_a,
            url: furl,
            audio_url: None,
            // yt-dlp 取到的地址不校验来源站，无需 Referer
            referer: None,
        };

        if is_v {
            video_streams.push(stream.clone());
        }
        if is_a {
            let mut only_audio = stream.clone();
            only_audio.needs_merge = false;
            audio_streams.push(only_audio);
        }
        // 音视频一体的流同样要加入音频候选之外的列表（作为可合并音轨的备份）
        if is_v && is_a {
            audio_streams.push(stream);
        }
    }

    // 视频按分辨率降序；同分辨率优先音视频一体（无需合并），再按编码兼容性，
    // 最后才比体积
    video_streams.sort_by(|a, b| {
        let pa = a.height.unwrap_or(0);
        let pb = b.height.unwrap_or(0);
        pb.cmp(&pa)
            .then_with(|| a.needs_merge.cmp(&b.needs_merge))
            .then_with(|| video_codec_rank(a).cmp(&video_codec_rank(b)))
            .then_with(|| a.estimated_bytes.unwrap_or(u64::MAX).cmp(&b.estimated_bytes.unwrap_or(u64::MAX)))
    });

    // 音频只保留纯音轨，按码率降序
    audio_streams.retain(|s| s.kind == StreamKind::Audio);
    audio_streams.sort_by(|a, b| {
        b.estimated_bytes
            .unwrap_or(0)
            .cmp(&a.estimated_bytes.unwrap_or(0))
    });

    // 去重：同一清晰度只保留最优的一条，避免下拉框出现大量重复项
    //
    // 同一清晰度通常有 h264 / h265 / AV1 多个版本，排序已让兼容性最好的排在最前，
    // 因此这里保留第一条即可。此前是按体积升序留最小值，结果每个清晰度都选中
    // 体积最小但兼容性最差的 AV1，下载下来的文件很多播放器打不开。
    let mut seen: std::collections::HashSet<(u32, bool, String)> = std::collections::HashSet::new();
    video_streams.retain(|s| {
        let key = (
            s.height.unwrap_or(0),
            s.needs_merge,
            s.container.clone(),
        );
        seen.insert(key)
    });

    // 依次把音频附到需要合并的视频流上（选码率最低的，体积更可控）
    let anchor_audio = audio_streams.last().cloned();
    for s in video_streams.iter_mut() {
        if s.needs_merge {
            s.audio_url = anchor_audio.as_ref().map(|a| a.url.clone());
            if let Some(a) = &anchor_audio {
                s.audio_label = a.audio_label.clone();
            }
        }
    }

    if video_streams.is_empty() && audio_streams.is_empty() {
        return Err(AppError::new(
            "PROVIDER_NO_STREAM",
            "该资源没有可直接下载的媒体流",
            false,
        )
        .with_hint("可能是直播、图文或 DAZN 等未支持类型"));
    }

    let mut streams = video_streams;
    streams.extend(audio_streams);

    let mut subtitles: Vec<SubtitleTrack> = info
        .subtitles
        .iter()
        .map(|(lang, tracks)| SubtitleTrack {
            id: format!("sub-{lang}"),
            label: tracks
                .first()
                .and_then(|t| t.name.clone())
                .unwrap_or_else(|| lang.clone()),
            language: Some(lang.clone()),
            // ext 仅在需要下载字幕时使用
        })
        .collect();
    subtitles.sort_by(|a, b| a.id.cmp(&b.id));

    let expires = chrono::Utc::now() + chrono::Duration::seconds(RESOLVE_TTL_SECS);

    Ok(ResolvedMedia {
        resolved_id: uuid::Uuid::new_v4().to_string(),
        title: info.title.unwrap_or_else(|| "未命名视频".to_string()),
        description: info
            .description
            .map(|d| d.lines().next().unwrap_or("").trim().to_string())
            .filter(|d| !d.is_empty()),
        source_name: info.extractor_key.unwrap_or_else(|| {
            url.host_str().unwrap_or("站点").to_string()
        }),
        source_url: info.webpage_url.unwrap_or_else(|| url.to_string()),
        thumbnail_url: info.thumbnail.map(upgrade_thumbnail),
        duration_sec: info.duration,
        streams,
        subtitles,
        expires_at: Some(expires.to_rfc3339()),
        provider_id: "yt-dlp".to_string(),
        provider_version: sidecar::yt_dlp_path()
            .and_then(|p| sidecar::version_of(&p, &["--version"]))
            .unwrap_or_else(|| "unknown".to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// yt-dlp 在浏览器开着时的真实报错（实测输出）
    const LOCKED: &str = "ERROR: Could not copy Chrome cookie database. See  https://github.com/yt-dlp/yt-dlp/issues/7271  for more info";
    /// 抖音在没有任何 Cookie 时的真实报错（实测输出）
    const FRESH: &str = "ERROR: [Douyin] 7686809202295131426: Fresh cookies (not necessarily logged in) are needed";

    #[test]
    fn 浏览器锁库归类为需退出浏览器而不是需要登录() {
        let err = classify_stderr(LOCKED, Some("browser:edge"));
        assert_eq!(err.code, "PROVIDER_COOKIE_LOCKED");
        // 文案要指名道姓，用户才知道该退哪个浏览器
        assert!(err.message.contains("Microsoft Edge"), "实际：{}", err.message);
        assert!(err.hint.unwrap().contains("Microsoft Edge"));
    }

    #[test]
    fn 锁库报错在没配置来源时也能归类() {
        let err = classify_stderr(LOCKED, None);
        assert_eq!(err.code, "PROVIDER_COOKIE_LOCKED");
    }

    #[test]
    fn 抖音缺登录态归类为需要登录() {
        let err = classify_stderr(FRESH, None);
        assert_eq!(err.code, "PROVIDER_NEEDS_COOKIES");
    }

    #[test]
    fn 锁库与缺登录态不会互相混淆() {
        // 「cookie database」里没有 cookies 这个词，顺序错了就会掉进兜底的
        // PROVIDER_ERROR，用户只能看到一句「解析失败」
        assert_ne!(classify_stderr(LOCKED, Some("browser:edge")).code, "PROVIDER_NEEDS_COOKIES");
        assert_ne!(classify_stderr(FRESH, None).code, "PROVIDER_COOKIE_LOCKED");
    }

    #[test]
    fn 浏览器键名从登录态来源里解析() {
        assert_eq!(browser_key(Some("browser:edge")), Some("edge"));
        assert_eq!(browser_key(Some(" browser:chrome ")), Some("chrome"));
        assert_eq!(browser_key(Some("file:D:\\a.txt")), None);
        assert_eq!(browser_key(Some("")), None);
        assert_eq!(browser_key(None), None);
    }

    #[test]
    fn 登录态参数按来源拼装() {
        let mut args = Vec::new();
        append_cookie_args(&mut args, Some("browser:edge"));
        assert_eq!(args, vec!["--cookies-from-browser", "edge"]);

        let mut args = Vec::new();
        append_cookie_args(&mut args, Some("file:D:\\cookies.txt"));
        assert_eq!(args, vec!["--cookies", "D:\\cookies.txt"]);

        // 账户登录：路径由应用管理，不受前缀后面的内容影响
        let mut args = Vec::new();
        append_cookie_args(&mut args, Some("account:"));
        assert_eq!(
            args,
            vec![
                "--cookies",
                crate::account::cookie_store_path().to_string_lossy().as_ref()
            ]
        );

        // 未配置时不能凭空加参数，否则 yt-dlp 会因为空路径直接报错
        let mut args = Vec::new();
        append_cookie_args(&mut args, None);
        append_cookie_args(&mut args, Some(""));
        assert!(args.is_empty());
    }

    #[test]
    fn 出口参数按模式拼装() {
        use crate::net::{ProxyConfig, ProxyMode};

        // 自定义代理：传 --proxy，不动环境变量
        let custom = ProxyConfig {
            mode: ProxyMode::Custom,
            custom_url: "socks5h://127.0.0.1:7897".into(),
        };
        let ytdlp_proxy = YtDlpProxy::from_config(&custom);
        assert_eq!(ytdlp_proxy.url.as_deref(), Some("socks5h://127.0.0.1:7897"));
        assert!(!ytdlp_proxy.clear_env);
        let mut args = Vec::new();
        ytdlp_proxy.append_args(&mut args);
        assert_eq!(args, vec!["--proxy", "socks5h://127.0.0.1:7897"]);

        // 直连：不传 --proxy，但必须清环境变量（否则子进程偷偷走代理）
        let direct = ProxyConfig {
            mode: ProxyMode::Direct,
            custom_url: "socks5h://127.0.0.1:7897".into(),
        };
        let ytdlp_proxy = YtDlpProxy::from_config(&direct);
        assert!(ytdlp_proxy.url.is_none());
        assert!(ytdlp_proxy.clear_env);
        let mut args = Vec::new();
        ytdlp_proxy.append_args(&mut args);
        assert!(args.is_empty(), "直连不该出现 --proxy");

        // 地址写坏时按直连处理：宁可直连，也不能把非法串塞给 yt-dlp
        let broken = ProxyConfig {
            mode: ProxyMode::Custom,
            custom_url: "ftp://127.0.0.1:21".into(),
        };
        let ytdlp_proxy = YtDlpProxy::from_config(&broken);
        assert!(ytdlp_proxy.url.is_none());
        assert!(ytdlp_proxy.clear_env);
    }
}