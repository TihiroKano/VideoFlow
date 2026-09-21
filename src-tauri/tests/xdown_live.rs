//! XDown 兜底解析的真实链路测试：**真实 X URL → 真实接口 → 原始 MP4 可下载**。
//!
//! 覆盖的需求点：
//! - 兜底只产出 `https://video.twimg.com/` 下的原始 MP4（绝不含站点中转层）；
//! - 带尺寸/编码元信息，能直接交给原生 HTTP 引擎；
//! - 取回的地址**不依赖任何登录态**：这里用一个全新的、没有任何 Cookie 的客户端
//!   直接发 Range 请求，必须能拿到 206/200 + `video/mp4`。
//!
//! 需要网络（且能访问 x.com 与 xdown.app），默认跳过。运行：
//!   cargo test --no-default-features --test xdown_live -- --ignored --nocapture

use reqwest::header::RANGE;
use url::Url;
use videoflow_lib::core::model::{engine_targets, EngineKind, StreamKind};
use videoflow_lib::net::{build_client, ClientSpec, ProxyConfig, ProxyMode};
use videoflow_lib::resolver::xdown;

/// 公开视频推文（yt-dlp 测试集里长期有效的样本），用于稳定断言
const PUBLIC_TWEET: &str = "https://x.com/oshtru/status/1577855540407197696";
/// 敏感内容推文：X 的游客接口只回 tombstone，正是兜底要解决的场景
const SENSITIVE_TWEET: &str = "https://x.com/mineBUQ/status/2101206859013296412/video/1?s=46";

/// 测试出口：设了 `VF_TEST_PROXY` 就走它（例如 `socks5h://127.0.0.1:7897`），否则直连。
///
/// 同一套真实链路测试因此既能在直连下跑，也能在自定义代理下跑——验收
/// 「不开 TUN 也能解析 X」时就用后者。
fn proxy_config() -> ProxyConfig {
    match std::env::var("VF_TEST_PROXY") {
        Ok(url) if !url.trim().is_empty() => ProxyConfig {
            mode: ProxyMode::Custom,
            custom_url: url,
        },
        _ => ProxyConfig {
            mode: ProxyMode::Direct,
            custom_url: String::new(),
        },
    }
}

fn client() -> reqwest::Client {
    build_client(&proxy_config(), &ClientSpec::short()).expect("构建 HTTP 客户端失败")
}

/// 断言候选地址全部是原始 MP4，且引擎派发到原生 HTTP
fn assert_native_original_mp4(media: &videoflow_lib::core::model::ResolvedMedia) {
    assert_eq!(media.provider_id, "xdown");
    assert!(!media.streams.is_empty(), "必须至少有一条可下载流");

    for stream in &media.streams {
        assert!(
            stream.url.starts_with("https://video.twimg.com/"),
            "只允许原始前缀，实际 {}",
            stream.url
        );
        assert!(
            stream.url.contains(".mp4"),
            "只允许 MP4，实际 {}",
            stream.url
        );
        assert!(
            !stream.url.contains("snapcdn"),
            "站点中转层不能作为下载地址：{}",
            stream.url
        );
        assert_eq!(stream.kind, StreamKind::Muxed, "原始 MP4 自带音轨");
        assert!(!stream.needs_merge, "自带音轨的流不该要求合并");
        assert!(stream.width.is_some() && stream.height.is_some(), "应带尺寸元信息");

        let (engine, target, audio) = engine_targets(stream, None);
        assert_eq!(engine, EngineKind::NativeHttp, "必须交给原生 HTTP 引擎");
        assert_eq!(target.as_deref(), Some(stream.url.as_str()));
        assert!(audio.is_none());
    }
}

/// 不带任何 Cookie 直接发 Range 请求，验证地址确实可用
async fn assert_downloadable(client: &reqwest::Client, url: &str) {
    let resp = client
        .get(url)
        .header(RANGE, "bytes=0-0")
        .send()
        .await
        .expect("原始地址应当可访问");

    let status = resp.status();
    assert!(
        status == reqwest::StatusCode::PARTIAL_CONTENT || status == reqwest::StatusCode::OK,
        "期望 206/200，实际 {status}"
    );
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.contains("video/mp4") || content_type.contains("application/octet-stream"),
        "期望视频内容，实际 Content-Type={content_type}"
    );
}

#[tokio::test]
#[ignore = "需要网络访问，默认跳过"]
async fn 公开推文可解析出原始_mp4_并可直接下载() {
    let url = Url::parse(PUBLIC_TWEET).unwrap();
    let media = xdown::resolve(&client(), &url)
        .await
        .expect("公开推文应当解析成功");

    assert_native_original_mp4(&media);
    println!(
        "解析成功：title={:?} 流数量={} 下载地址样本={}",
        media.title,
        media.streams.len(),
        media.streams[0].url
    );

    assert_downloadable(&client(), &media.streams[0].url).await;
    println!("Range 探测通过：原始地址无需任何登录态即可下载");
}

/// 敏感推文：yt-dlp 只会拿到 tombstone，兜底接口能给出原始 MP4。
///
/// 站点侧随时可能调整，因此这里允许「解析失败」但必须是明确的分类错误——
/// 绝不能出现崩溃或静默成功。
#[tokio::test]
#[ignore = "需要网络访问，默认跳过"]
async fn 敏感推文走兜底也能拿到原始_mp4() {
    let url = Url::parse(SENSITIVE_TWEET).unwrap();
    match xdown::resolve(&client(), &url).await {
        Ok(media) => {
            assert_native_original_mp4(&media);
            println!(
                "敏感推文兜底成功：title={:?} 清晰度={:?}",
                media.title,
                media
                    .streams
                    .iter()
                    .map(|s| s.quality_label.clone().unwrap_or_default())
                    .collect::<Vec<_>>()
            );
            assert_downloadable(&client(), &media.streams[0].url).await;
        }
        Err(err) => {
            assert!(!err.code.is_empty(), "失败必须带明确错误码");
            println!("兜底未成功（可接受，但要有分类错误）：{err}");
        }
    }
}