//! 直链媒体 Provider：对 `.mp4` 等直链做 HEAD / Range 探测。

use base64::Engine;
use reqwest::header::{HeaderMap, CONTENT_LENGTH, CONTENT_TYPE};
use url::Url;

use crate::core::model::{MediaStream, ResolvedMedia, StreamKind};
use crate::error::{AppError, AppResult};
use crate::resolver::RESOLVE_TTL_SECS;

/// 从响应头里取 Provider 提供的 SHA-256（项目书 §3.3「Provider 提供时校验 SHA-256」）。
///
/// 支持三种来源：
/// - RFC 9530 `Content-Digest` / RFC 3230 `Digest`：`sha-256=:BASE64:`
/// - S3 的 `x-amz-checksum-sha256`：BASE64
fn digest_sha256(headers: &HeaderMap) -> Option<String> {
    for name in ["content-digest", "digest"] {
        if let Some(raw) = headers.get(name).and_then(|v| v.to_str().ok()) {
            if let Some(hex) = parse_digest_sha256(raw) {
                return Some(hex);
            }
        }
    }
    headers
        .get("x-amz-checksum-sha256")
        .and_then(|v| v.to_str().ok())
        .and_then(decode_hash_token)
}

/// 解析 `Digest` 系列头里的 sha-256，统一转成小写十六进制
fn parse_digest_sha256(value: &str) -> Option<String> {
    for part in value.split(',') {
        let Some((alg, rest)) = part.split_once('=') else {
            continue;
        };
        let alg = alg.trim();
        if !alg.eq_ignore_ascii_case("sha-256") && !alg.eq_ignore_ascii_case("sha256") {
            continue;
        }
        if let Some(hex) = decode_hash_token(rest) {
            return Some(hex);
        }
    }
    None
}

/// 哈希令牌可能是十六进制，也可能是 base64（RFC 9530 会用冒号包起来）
fn decode_hash_token(token: &str) -> Option<String> {
    let trimmed = token.trim().trim_matches(':').trim();
    if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(trimmed.to_ascii_lowercase());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(trimmed)
        .ok()?;
    if bytes.len() != 32 {
        return None;
    }
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn extension_of(url: &Url) -> Option<String> {
    url.path()
        .rsplit('.')
        .next()
        .filter(|s| s.len() <= 5 && !s.contains('/'))
        .map(|s| s.to_ascii_lowercase())
}

fn container_of(ext: Option<&str>, content_type: Option<&str>) -> String {
    if let Some(ext) = ext {
        return ext.to_string();
    }
    match content_type {
        Some(ct) if ct.contains("webm") => "webm".into(),
        Some(ct) if ct.contains("matroska") => "mkv".into(),
        Some(ct) if ct.contains("mpeg") => "mp3".into(),
        Some(ct) if ct.contains("mp4") || ct.contains("m4a") => "mp4".into(),
        _ => "bin".into(),
    }
}

fn kind_of(content_type: Option<&str>, ext: Option<&str>) -> StreamKind {
    let ct = content_type.unwrap_or("");
    if ct.starts_with("audio/") {
        return StreamKind::Audio;
    }
    if ct.starts_with("video/") {
        return StreamKind::Muxed;
    }
    match ext {
        Some("mp3") | Some("m4a") | Some("aac") | Some("flac") | Some("wav") => StreamKind::Audio,
        _ => StreamKind::Muxed,
    }
}

/// 标题取路径最后一段，去掉扩展名，并做百分号解码
fn title_of(url: &Url) -> String {
    let raw = url.path_segment_decoded_last().unwrap_or_else(|| "未命名文件".to_string());
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        "未命名文件".to_string()
    } else {
        trimmed.to_string()
    }
}

/// `Url::path_segments` 的便捷封装：取最后一段并解码
trait PathSegmentDecoded {
    fn path_segment_decoded_last(&self) -> Option<String>;
}

impl PathSegmentDecoded for Url {
    fn path_segment_decoded_last(&self) -> Option<String> {
        let seg = self.path_segments()?.next_back()?;
        if seg.is_empty() {
            return None;
        }
        let decoded = percent_decode_str_local(seg);
        let without_ext = decoded
            .rsplit_once('.')
            .map(|(head, _)| head.to_string())
            .unwrap_or(decoded);
        Some(without_ext)
    }
}

/// 只处理 %XX 的最小百分号解码，避免为此引入额外依赖
fn percent_decode_str_local(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

pub async fn resolve(client: &reqwest::Client, url: &Url) -> AppResult<ResolvedMedia> {
    // 先 HEAD 拿元信息；部分服务器不支持 HEAD，退回 Range: bytes=0-0
    let mut content_length: Option<u64> = None;
    let mut content_type: Option<String> = None;
    let mut sha256: Option<String> = None;

    let head = client.head(url.as_str()).send().await;
    match head {
        Ok(resp) if resp.status().is_success() => {
            content_length = resp
                .headers()
                .get(CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            content_type = resp
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.to_string());
            sha256 = digest_sha256(resp.headers());
        }
        Ok(resp) => {
            let status = resp.status().as_u16();
            // 405 表示服务器不支持 HEAD，继续用 GET 探测
            if status != 405 && status != 501 {
                return Err(AppError::http_status(status));
            }
        }
        Err(err) => return Err(AppError::from(err)),
    }

    if content_length.is_none() || content_type.is_none() || sha256.is_none() {
        let probe = client
            .get(url.as_str())
            .header(reqwest::header::RANGE, "bytes=0-0")
            .send()
            .await?;
        let status = probe.status();
        if !status.is_success() {
            return Err(AppError::http_status(status.as_u16()));
        }
        if content_length.is_none() {
            content_length = probe
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_total_from_content_range);
        }
        if content_type.is_none() {
            content_type = probe
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.to_string());
        }
        // 只有服务器忽略了 Range、返回完整 200 响应时，头里的摘要才对应整个文件；
        // 206 部分响应的摘要是针对那一段的，绝不能拿来校验整文件。
        if sha256.is_none() && status == reqwest::StatusCode::OK {
            sha256 = digest_sha256(probe.headers());
        }
    }

    // 直链资源不应是 HTML 页面；出现 HTML 说明这其实是网页而不是媒体文件
    if let Some(ct) = &content_type {
        if ct.contains("text/html") {
            return Err(AppError::provider_unsupported());
        }
    }

    let ext = extension_of(url);
    let container = container_of(ext.as_deref(), content_type.as_deref());
    let kind = kind_of(content_type.as_deref(), ext.as_deref());

    let stream = MediaStream {
        id: "direct-0".to_string(),
        container,
        kind,
        quality_label: None,
        width: None,
        height: None,
        fps: None,
        audio_label: if kind == StreamKind::Video {
            None
        } else {
            Some("随文件".to_string())
        },
        estimated_bytes: content_length,
        codec: None,
        sha256,
        needs_merge: false,
        url: url.to_string(),
        audio_url: None,
        referer: None,
    };

    let expires = chrono::Utc::now() + chrono::Duration::seconds(RESOLVE_TTL_SECS);

    Ok(ResolvedMedia {
        resolved_id: uuid::Uuid::new_v4().to_string(),
        title: title_of(url),
        description: Some("直链媒体文件".to_string()),
        source_name: url.host_str().unwrap_or("直链").to_string(),
        source_url: url.to_string(),
        thumbnail_url: None,
        duration_sec: None,
        streams: vec![stream],
        subtitles: Vec::new(),
        expires_at: Some(expires.to_rfc3339()),
        provider_id: "direct".to_string(),
        provider_version: "1.0.0".to_string(),
    })
}

/// 从 `Content-Range: bytes 0-0/12345` 中解析总长度
fn parse_total_from_content_range(value: &str) -> Option<u64> {
    let after_slash = value.rsplit('/').next()?;
    if after_slash == "*" {
        return None;
    }
    after_slash.trim().parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析_content_range_中的总长度() {
        assert_eq!(parse_total_from_content_range("bytes 0-0/12345"), Some(12345));
        assert_eq!(parse_total_from_content_range("bytes 100-200/300"), Some(300));
        assert_eq!(parse_total_from_content_range("bytes 0-0/*"), None);
        assert_eq!(parse_total_from_content_range("garbage"), None);
        assert_eq!(parse_total_from_content_range(""), None);
    }

    #[test]
    fn 按扩展名识别容器() {
        assert_eq!(container_of(Some("mp4"), None), "mp4");
        assert_eq!(container_of(Some("webm"), None), "webm");
        // 没有扩展名时退回 Content-Type
        assert_eq!(container_of(None, Some("video/webm; codecs=vp9")), "webm");
        assert_eq!(container_of(None, Some("audio/mpeg")), "mp3");
        assert_eq!(container_of(None, None), "bin");
    }

    #[test]
    fn 按_content_type_识别媒体类型() {
        assert_eq!(kind_of(Some("audio/mpeg"), Some("mp3")), StreamKind::Audio);
        assert_eq!(kind_of(Some("video/mp4"), Some("mp4")), StreamKind::Muxed);
        // 无 Content-Type 时按扩展名判断
        assert_eq!(kind_of(None, Some("m4a")), StreamKind::Audio);
        assert_eq!(kind_of(None, Some("mp4")), StreamKind::Muxed);
    }

    #[test]
    fn 从_url_推导标题并解码百分号() {
        let url = Url::parse("https://cdn.example.com/a/%E5%A4%A9%E6%B0%94%E4%B9%8B%E5%AD%90.mp4")
            .unwrap();
        assert_eq!(title_of(&url), "天气之子");
    }

    #[test]
    fn 标题在路径无文件名时给出兜底() {
        let url = Url::parse("https://cdn.example.com/").unwrap();
        assert_eq!(title_of(&url), "未命名文件");
    }

    #[test]
    fn 解析_digest_头里的_sha256() {
        // 空字符串的 SHA-256，作为对照常量
        const EMPTY_SHA256: &str =
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        // RFC 9530：base64，用冒号包裹
        assert_eq!(
            parse_digest_sha256("sha-256=:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=:"),
            Some(EMPTY_SHA256.to_string())
        );
        // 十六进制原样（大小写都接受，统一转小写）
        assert_eq!(
            parse_digest_sha256(&format!("sha-256={}", EMPTY_SHA256.to_uppercase())),
            Some(EMPTY_SHA256.to_string())
        );
        // 多种算法并存时挑 sha-256
        assert_eq!(
            parse_digest_sha256("md5=abc,sha-256=:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=:"),
            Some(EMPTY_SHA256.to_string())
        );
        // 没有 sha-256 或格式不对时返回 None
        assert_eq!(parse_digest_sha256("md5=abc"), None);
        assert_eq!(parse_digest_sha256(""), None);
        assert_eq!(parse_digest_sha256("sha-256=not-base64!!"), None);
    }

    #[test]
    fn s3_校验和头按_base64_解析() {
        assert_eq!(
            decode_hash_token("47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string())
        );
        // 长度不是 32 字节的一律拒绝
        assert_eq!(decode_hash_token("YWJj"), None);
    }
}