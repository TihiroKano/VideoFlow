//! 端到端下载链路测试：真实解析 → 真实下载 → 落盘校验。
//!
//! 覆盖的路径与产品运行时一致（除 Tauri IPC 层外）：
//!   resolver::resolve  →  storage 命名/冲突策略  →  downloader::http  →  原子 rename
//!
//! 需要网络，默认跳过。运行：
//!   cargo test --no-default-features --test download_e2e -- --ignored --nocapture

use std::path::PathBuf;
use std::sync::Arc;

use videoflow_lib::downloader::http::{self, ProgressSnapshot};
use videoflow_lib::downloader::ControlToken;
use videoflow_lib::{resolver, storage};

/// 公开可用的直链样本，支持 Range
const DIRECT_MP4: &str =
    "https://test-videos.co.uk/vids/bigbuckbunny/mp4/h264/360/Big_Buck_Bunny_360_10s_1MB.mp4";

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vf-e2e-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    dir
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("VideoFlow/0.1 (test)")
        .build()
        .expect("构建 HTTP 客户端失败")
}

/// 完整链路：解析直链 → 生成目标路径 → 分段下载 → 原子 rename → 校验文件
#[tokio::test]
#[ignore = "需要网络访问，默认跳过"]
async fn 直链从解析到落盘完整跑通() {
    let client = client();

    // ---- 1. 解析 ----
    let media = resolver::resolve(&client, DIRECT_MP4, None)
        .await
        .expect("解析直链应当成功");

    assert!(!media.streams.is_empty(), "解析结果必须包含至少一条媒体流");
    assert_eq!(media.provider_id, "direct");

    let stream = &media.streams[0];
    assert!(
        stream.estimated_bytes.unwrap_or(0) > 900_000,
        "样本大小应接近 1 MB，实际 {:?}",
        stream.estimated_bytes
    );
    assert!(!stream.url.is_empty(), "流必须带有可下载地址");
    println!(
        "解析成功：title={:?} container={} size={:?}",
        media.title, stream.container, stream.estimated_bytes
    );

    // ---- 2. 目标路径与冲突策略 ----
    let dir = temp_dir("direct");
    storage::ensure_dir(&dir).expect("目录应可写");

    let stem = storage::sanitize_stem(&media.title);
    let target = storage::unique_path(&dir, &stem, &stream.container);
    storage::ensure_within(&dir, &target).expect("目标路径必须位于下载目录内");

    // 预置同名文件，验证不会覆盖
    std::fs::write(&target, b"pre-existing").expect("写入占位文件失败");
    let target = storage::unique_path(&dir, &stem, &stream.container);
    assert!(
        !target.exists(),
        "冲突策略应返回一个尚未被占用的路径：{}",
        target.display()
    );
    assert!(
        target
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("(1)"),
        "冲突时应追加序号，实际 {}",
        target.display()
    );

    // ---- 3. 下载 ----
    let (part, resume_path) = storage::temp_paths(&target);
    let progress_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&progress_calls);

    let req = http::HttpRequest {
        url: stream.url.clone(),
        target_path: target.clone(),
        temp_path: part.clone(),
        resume_path: resume_path.clone(),
        chunk_concurrency: 4,
        resume: None,
        referer: None,
        user_agent: "VideoFlow/0.1 (test)".to_string(),
    };

    let token = ControlToken::new();
    let outcome = http::download(
        &client,
        req,
        token,
        Arc::new(move |_snapshot: ProgressSnapshot| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }),
    )
    .await
    .expect("下载应当成功");

    assert!(outcome.total_bytes > 900_000, "下载总字节数应接近 1 MB");
    assert!(
        outcome.checkpoint.chunks.iter().all(|c| c.done),
        "所有分片都应标记为完成"
    );
    assert!(
        progress_calls.load(std::sync::atomic::Ordering::SeqCst) > 0,
        "下载过程中应当上报过进度"
    );

    // ---- 4. 原子 rename 并校验最终产物 ----
    assert!(part.exists(), "下载完成后应存在 .part 临时文件");
    std::fs::rename(&part, &target).expect("rename 到最终路径应当成功");
    let _ = std::fs::remove_file(&resume_path);

    let written = std::fs::metadata(&target).expect("最终文件应存在").len();
    assert_eq!(
        written, outcome.total_bytes,
        "最终文件大小应与下载声明一致"
    );
    assert!(!part.exists(), "临时文件应已被移走");
    assert!(
        !resume_path.exists(),
        "断点文件应在完成后清理：{}",
        resume_path.display()
    );

    // 产物应是真实的 MP4（前 12 字节含 ftyp box）
    let head = std::fs::read(&target).expect("读取产物失败");
    let head = &head[..head.len().min(12)];
    assert!(
        head.windows(4).any(|w| w == b"ftyp"),
        "产物应当是有效的 MP4，实际头部 {:?}",
        head
    );

    println!(
        "下载完成：{} 字节，进度回调 {} 次，产物 {}",
        written,
        progress_calls.load(std::sync::atomic::Ordering::SeqCst),
        target.display()
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// 站点链接解析：验证 yt-dlp 适配器能返回标准化结果
#[tokio::test]
#[ignore = "需要网络访问，默认跳过"]
async fn 站点链接可解析出可用清晰度() {
    let client = client();

    // 公开视频，仅用于验证解析链路
    let media = match resolver::resolve(
        &client,
        "https://www.bilibili.com/video/BV1GJ411x7h7",
        None,
    )
    .await
    {
        Ok(m) => m,
        Err(e) => {
            // 站点风控、地区限制或 yt-dlp 缺失都可能导致失败；
            // 这里只要求失败时给出明确分类错误，而不是崩溃或静默成功。
            println!("站点解析未成功（可接受）：{e}");
            assert!(
                !e.code.is_empty(),
                "失败时必须返回明确的错误码"
            );
            return;
        }
    };

    assert!(!media.title.is_empty(), "解析结果必须有标题");
    assert!(!media.streams.is_empty(), "解析结果必须有可下载流");
    assert_eq!(media.provider_id, "yt-dlp");

    let videos: Vec<_> = media
        .streams
        .iter()
        .filter(|s| s.kind != videoflow_lib::core::model::StreamKind::Audio)
        .collect();
    assert!(!videos.is_empty(), "应当至少有一条视频流");

    // 需要合并的流必须带上音频地址，否则下载后没有声音
    for s in &videos {
        if s.needs_merge {
            assert!(
                s.audio_url.is_some(),
                "需要合并的流必须提供音频地址（format_id={}）",
                s.id
            );
        }
    }

    println!(
        "站点解析成功：title={:?} 流数量={} 字幕数量={}",
        media.title,
        media.streams.len(),
        media.subtitles.len()
    );
}
