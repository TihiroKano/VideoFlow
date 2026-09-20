//! M3U8 / HLS 清单解析（纯逻辑，可脱离 GUI 单测）。
//!
//! 只负责把清单解析成可下载的媒体信息；实际下载交给 FFmpeg
//! （AES-128 解密、byterange、fMP4 init segment 由 FFmpeg 原生处理）。

use url::Url;

/// 一份 HLS 变体（通常来自 master 清单的 `EXT-X-STREAM-INF`）
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    /// 该变体的媒体清单地址（已解析为绝对地址）
    pub playlist_url: String,
    /// 码率（bps）
    pub bandwidth: Option<u64>,
    /// 分辨率，形如 `1280x720`
    pub resolution: Option<String>,
    /// 视频编码，形如 `avc1.640028,mp4a.40.2`
    pub codecs: Option<String>,
}

/// media 清单的总时长（各分片 `EXTINF` 之和）
pub fn total_duration(text: &str) -> f64 {
    text.lines()
        .filter_map(|l| {
            let value = l.trim().strip_prefix("#EXTINF:")?;
            // `#EXTINF:<时长>,<标题>`，标题可能为空
            value.split(',').next()?.trim().parse::<f64>().ok()
        })
        .sum()
}

/// 是否是 master 清单：包含 `EXT-X-STREAM-INF`
pub fn is_master(text: &str) -> bool {
    text.lines().any(|l| l.trim().starts_with("#EXT-X-STREAM-INF"))
}

/// 把相对 URI 解析成绝对地址；空 URI 返回 None
fn resolve_uri(base: &Url, uri: &str) -> Option<String> {
    let uri = uri.trim();
    if uri.is_empty() {
        return None;
    }
    base.join(uri).ok().map(|u| u.to_string())
}

/// 解析 `EXT-X-STREAM-INF` 行的属性到键值表
fn parse_attributes(line: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    // 去掉行首的 #EXT-X-STREAM-INF:
    let attrs = line
        .trim()
        .trim_start_matches("#EXT-X-STREAM-INF")
        .trim_start_matches(':')
        .trim();
    if attrs.is_empty() {
        return map;
    }
    // 属性可能是逗号分隔，但值可能带引号（引号内不含逗号）
    for part in split_top_level(attrs) {
        if let Some((k, v)) = part.split_once('=') {
            let k = k.trim().to_string();
            let v = v.trim().trim_matches('"').to_string();
            if !k.is_empty() {
                map.insert(k, v);
            }
        }
    }
    map
}

/// 顶层逗号切分：忽略引号内的逗号。
///
/// 按字节下标切分（属性值都是 ASCII），避免字符下标与字节下标错位。
fn split_top_level(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'"' => in_quotes = !in_quotes,
            b',' if !in_quotes => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

fn parse_bandwidth(v: &str) -> Option<u64> {
    v.parse::<u64>().ok().filter(|b| *b > 0)
}

fn parse_height(resolution: Option<&str>) -> Option<u32> {
    resolution?.split('x').nth(1)?.parse().ok().filter(|h| *h > 0)
}

/// 从 master 清单里列出所有变体
pub fn parse_master(text: &str, base: &Url) -> Vec<Variant> {
    let mut variants = Vec::new();
    let mut pending: Option<std::collections::HashMap<String, String>> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("#EXT-X-STREAM-INF") {
            pending = Some(parse_attributes(trimmed));
            continue;
        }
        // 空行或再一个 tag（未跟媒体 URI）则丢弃待定属性
        if pending.is_some() && (trimmed.is_empty() || trimmed.starts_with('#')) {
            continue;
        }
        if let Some(attrs) = pending.take() {
            if let Some(url) = resolve_uri(base, trimmed) {
                let resolution = attrs.get("RESOLUTION").cloned();
                variants.push(Variant {
                    playlist_url: url,
                    bandwidth: attrs.get("BANDWIDTH").and_then(|v| parse_bandwidth(v)),
                    resolution,
                    codecs: attrs.get("CODECS").cloned(),
                });
            }
        }
    }

    // 按分辨率降序，同分辨率按码率降序
    variants.sort_by(|a, b| {
        let pa = parse_height(a.resolution.as_deref()).unwrap_or(0);
        let pb = parse_height(b.resolution.as_deref()).unwrap_or(0);
        pb.cmp(&pa)
            .then_with(|| b.bandwidth.unwrap_or(0).cmp(&a.bandwidth.unwrap_or(0)))
    });
    variants
}

/// 是否是 HLS 清单（通过扩展名判断）
pub fn looks_like_hls(url: &Url) -> bool {
    url.path().to_ascii_lowercase().ends_with(".m3u8")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(u: &str) -> Url {
        Url::parse(u).unwrap()
    }

    #[test]
    fn 识别主清单与媒体清单() {
        assert!(is_master("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000\nhttp://c/720.m3u8"));
        assert!(!is_master("#EXTM3U\n#EXTINF:10.0,\ndog.mp4"));
    }

    #[test]
    fn 主清单解析出变体并按清晰度排序() {
        let text = concat!(
            "#EXTM3U\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1280x720,CODECS=\"avc1.64001f,mp4a.40.2\"\n",
            "720/index.m3u8\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=4000000,RESOLUTION=1920x1080\n",
            "1080/index.m3u8\n",
        );
        let variants = parse_master(text, &base("https://cdn.example.com/a/master.m3u8"));
        assert_eq!(variants.len(), 2);
        // 1080 在前
        assert_eq!(variants[0].resolution.as_deref(), Some("1920x1080"));
        assert_eq!(variants[0].bandwidth, Some(4_000_000));
        assert!(
            variants[0]
                .playlist_url
                .starts_with("https://cdn.example.com/a/1080/")
        );
        assert_eq!(variants[1].resolution.as_deref(), Some("1280x720"));
    }

    #[test]
    fn 相对路径解析为绝对地址() {
        let text = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=500000\nsub/medium.m3u8\n";
        let variants = parse_master(text, &base("https://cdn.example.com/master.m3u8"));
        assert_eq!(
            variants[0].playlist_url,
            "https://cdn.example.com/sub/medium.m3u8"
        );
    }

    #[test]
    fn 绝对地址直接使用() {
        let text = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=500000\nhttps://other.com/low.m3u8\n";
        let variants = parse_master(text, &base("https://cdn.example.com/master.m3u8"));
        assert_eq!(variants[0].playlist_url, "https://other.com/low.m3u8");
    }

    #[test]
    fn 值为空或乱序时稳健处理() {
        let text = "#EXTM3U\n#EXT-X-STREAM-INF:RESOLUTION=720x1280\nvid.m3u8\n";
        let variants = parse_master(text, &base("https://c.com/master.m3u8"));
        assert_eq!(variants.len(), 1);
        assert_eq!(variants[0].bandwidth, None);
        assert_eq!(variants[0].resolution.as_deref(), Some("720x1280"));
    }

    #[test]
    fn 媒体清单统计总时长() {
        let text = concat!(
            "#EXTM3U\n",
            "#EXT-X-TARGETDURATION:10\n",
            "#EXTINF:9.009,\n",
            "seg1.ts\n",
            "#EXTINF:9.009,\n",
            "seg2.ts\n",
            "#EXT-X-ENDLIST\n",
        );
        let duration = total_duration(text);
        assert!((duration - 18.018).abs() < 0.001);
    }

    #[test]
    fn 编码带引号逗号时正确取值() {
        let attrs = parse_attributes(
            "#EXT-X-STREAM-INF:BANDWIDTH=800000,CODECS=\"avc1.64001f,mp4a.40.2\"",
        );
        assert_eq!(attrs.get("BANDWIDTH").map(String::as_str), Some("800000"));
        assert_eq!(
            attrs.get("CODECS").map(String::as_str),
            Some("avc1.64001f,mp4a.40.2")
        );
    }

    #[test]
    fn 顶层逗号切分忽略引号内逗号() {
        let parts = split_top_level(r#"BANDWIDTH=800000,CODECS="a,b",RESOLUTION=720x1280"#);
        assert_eq!(parts, vec!["BANDWIDTH=800000", r#"CODECS="a,b""#, "RESOLUTION=720x1280"]);
    }

    #[test]
    fn hls链接识别() {
        assert!(looks_like_hls(&base("https://c.com/live/index.m3u8")));
        assert!(looks_like_hls(&base("https://c.com/a.m3u8?token=x")));
        assert!(!looks_like_hls(&base("https://c.com/video.mp4")));
        assert!(!looks_like_hls(&base("https://c.com/master")));
    }
}