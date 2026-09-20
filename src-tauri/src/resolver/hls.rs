//! HLS Provider：拉取 m3u8 清单并标准化为 `ResolvedMedia`。
//!
//! 只做解析与选流，实际下载交给 FFmpeg——它原生处理 AES-128 解密、
//! `EXT-X-BYTERANGE` 与 fMP4 的 init segment，自己重写一遍只会引入偏差。

use url::Url;

use crate::core::model::{MediaStream, ResolvedMedia, StreamKind};
use crate::error::{AppError, AppResult};
use crate::resolver::m3u8;
use crate::resolver::RESOLVE_TTL_SECS;

/// 清单体积上限：m3u8 是文本，正常几十 KB；超过这个量级基本可以判定不是清单
const MAX_PLAYLIST_BYTES: usize = 4 * 1024 * 1024;

/// 拉取清单文本
async fn fetch_playlist(client: &reqwest::Client, url: &Url) -> AppResult<String> {
    let resp = client
        .get(url.clone())
        .header(reqwest::header::ACCEPT, "*/*")
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::http_status(status.as_u16()));
    }

    let text = resp.text().await?;
    if text.len() > MAX_PLAYLIST_BYTES {
        return Err(AppError::new(
            "PROVIDER_UNSUPPORTED",
            "该地址返回的不是 HLS 清单",
            false,
        ));
    }
    if !text.trim_start().starts_with("#EXTM3U") {
        return Err(AppError::new(
            "PROVIDER_UNSUPPORTED",
            "该地址返回的不是 HLS 清单",
            false,
        )
        .with_detail(format!("前 80 字符：{}", text.chars().take(80).collect::<String>())));
    }
    Ok(text)
}

/// 解析一个 .m3u8 链接
pub async fn resolve(client: &reqwest::Client, url: &Url) -> AppResult<ResolvedMedia> {
    let text = fetch_playlist(client, url).await?;

    let (streams, duration_sec) = if m3u8::is_master(&text) {
        let variants = m3u8::parse_master(&text, url);
        if variants.is_empty() {
            return Err(AppError::new(
                "PROVIDER_NO_STREAM",
                "HLS 清单里没有可用的清晰度",
                false,
            ));
        }
        // 取最高档的 media 清单拿总时长：master 自身不含 EXTINF，
        // 而时长要用来展示与估算体积，值得多一次请求
        let duration = match fetch_playlist(client, &url_for(&variants[0].playlist_url)?).await {
            Ok(media) => Some(m3u8::total_duration(&media)).filter(|d| *d > 0.0),
            // 拿不到时长不影响下载，只是不显示
            Err(_) => None,
        };

        let streams = variants
            .iter()
            .enumerate()
            .map(|(idx, v)| {
                let height = v
                    .resolution
                    .as_deref()
                    .and_then(|r| r.split('x').nth(1))
                    .and_then(|h| h.parse::<u32>().ok())
                    .filter(|h| *h > 0);
                MediaStream {
                    id: format!("hls-{idx}"),
                    // 最终由 FFmpeg 封装为 mp4
                    container: "mp4".to_string(),
                    kind: StreamKind::Muxed,
                    quality_label: height.map(|h| format!("{h}P")),
                    width: v
                        .resolution
                        .as_deref()
                        .and_then(|r| r.split('x').next())
                        .and_then(|w| w.parse().ok()),
                    height,
                    fps: None,
                    audio_label: Some("随视频".to_string()),
                    estimated_bytes: estimate(v.bandwidth, duration),
                    codec: v.codecs.clone(),
                    // HLS 没有整文件摘要，只做长度校验
                    sha256: None,
                    needs_merge: false,
                    // 交给 FFmpeg 的是媒体清单地址，不是单个分片
                    url: v.playlist_url.clone(),
                    audio_url: None,
                    referer: None,
                }
            })
            .collect::<Vec<_>>();
        (streams, duration)
    } else {
        let duration = Some(m3u8::total_duration(&text)).filter(|d| *d > 0.0);
        let stream = MediaStream {
            id: "hls-0".to_string(),
            container: "mp4".to_string(),
            kind: StreamKind::Muxed,
            // 媒体清单不含分辨率信息，不编造清晰度
            quality_label: None,
            width: None,
            height: None,
            fps: None,
            audio_label: Some("随视频".to_string()),
            estimated_bytes: None,
            codec: None,
            sha256: None,
            needs_merge: false,
            url: url.to_string(),
            audio_url: None,
            referer: None,
        };
        (vec![stream], duration)
    };

    let expires = chrono::Utc::now() + chrono::Duration::seconds(RESOLVE_TTL_SECS);

    Ok(ResolvedMedia {
        resolved_id: uuid::Uuid::new_v4().to_string(),
        title: title_from_url(url),
        description: None,
        source_name: url.host_str().unwrap_or("站点").to_string(),
        source_url: url.to_string(),
        thumbnail_url: None,
        duration_sec,
        streams,
        subtitles: Vec::new(),
        expires_at: Some(expires.to_rfc3339()),
        provider_id: "hls".to_string(),
        provider_version: "native".to_string(),
    })
}

fn url_for(raw: &str) -> AppResult<Url> {
    Url::parse(raw).map_err(|_| AppError::url_malformed())
}

/// 清单文件名去掉扩展名当标题
fn title_from_url(url: &Url) -> String {
    url.path_segments()
        .and_then(|mut s| s.next_back())
        .map(|name| name.trim_end_matches(".m3u8").to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "HLS 媒体".to_string())
}

/// 码率 × 时长 → 估算体积
fn estimate(bandwidth: Option<u64>, duration: Option<f64>) -> Option<u64> {
    let bps = bandwidth.filter(|b| *b > 0)?;
    let secs = duration.filter(|d| *d > 0.0)?;
    Some((bps as f64 * secs / 8.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 标题取清单文件名() {
        assert_eq!(
            title_from_url(&Url::parse("https://c.com/a/b/index.m3u8").unwrap()),
            "index"
        );
        assert_eq!(
            title_from_url(&Url::parse("https://c.com/").unwrap()),
            "HLS 媒体"
        );
    }

    #[test]
    fn 体积按码率与时长估算() {
        // 4 Mbps × 100s = 50 MB
        assert_eq!(estimate(Some(4_000_000), Some(100.0)), Some(50_000_000));
        assert_eq!(estimate(None, Some(100.0)), None);
        assert_eq!(estimate(Some(4_000_000), None), None);
        assert_eq!(estimate(Some(0), Some(100.0)), None);
    }
}