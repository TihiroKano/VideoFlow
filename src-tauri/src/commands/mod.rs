//! IPC 命令注册（项目书 §4.3）。
//!
//! 所有 mutation 都需要 idempotencyKey；失败统一返回 `{ code, message, retryable, hint }`。

pub mod account;
pub mod convert;
pub mod files;
pub mod history;
pub mod resolve;
pub mod settings;
pub mod tasks;

use tauri::State;
use std::sync::Arc;

use crate::core::model::{DownloadTask, QueueStats};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotPayload {
    pub seq: u64,
    pub tasks: Vec<DownloadTask>,
    pub queue: QueueStats,
}

/// 统一的解析入口（GUI 侧）。
///
/// `resolver` 模块本身不依赖 Tauri，但 Browser Resolver 必须开内嵌浏览器窗口，
/// 所以派发放在这一层：先做 SSRF 防护与 Provider 判定，再决定走哪条路。
pub async fn resolve_media(
    state: &AppState,
    url: &str,
    token: Option<Arc<crate::downloader::ControlToken>>,
) -> AppResult<crate::core::model::ResolvedMedia> {
    use crate::resolver::{classify, should_fallback_to_browser, ProviderKind};

    let (parsed, kind) = classify(url).await?;
    match kind {
        ProviderKind::Browser => capture_with_browser(state, &parsed, token).await,
        _ => {
            let cookies = state.cookies_source();
            match crate::resolver::resolve(&state.client, url, cookies.as_deref()).await {
                Ok(media) => Ok(media),
                // 保险：站点在表内且属于「yt-dlp 处理不了」类错误时，改用内嵌浏览器重试。
                // 表外的站点不兜底，避免把陌生网页加载进内嵌浏览器。
                Err(err) if should_fallback_to_browser(url, &err) => {
                    eprintln!(
                        "[videoflow] yt-dlp 解析失败（{}），改用内嵌浏览器重试：{url}",
                        err.code
                    );
                    capture_with_browser(state, &parsed, token).await
                }
                Err(err) => Err(err),
            }
        }
    }
}

/// 用内嵌浏览器抓取页面上的媒体数据并标准化
async fn capture_with_browser(
    state: &AppState,
    page_url: &url::Url,
    token: Option<Arc<crate::downloader::ControlToken>>,
) -> AppResult<crate::core::model::ResolvedMedia> {
    use crate::resolver::browser;

    let payload = state.browser_capture(page_url, token).await?;
    let version = tauri::webview_version().unwrap_or_else(|_| "unknown".to_string());
    browser::to_resolved_media(payload, page_url, &version)
}

#[tauri::command]
pub fn get_snapshot(state: State<'_, Arc<AppState>>, after_seq: Option<u64>) -> SnapshotPayload {
    let _ = after_seq;
    SnapshotPayload {
        seq: state.next_seq(),
        tasks: state.snapshot(),
        queue: state.queue_stats(),
    }
}

#[tauri::command]
pub fn remove_task(state: State<'_, Arc<AppState>>, task_id: String) -> AppResult<()> {
    state.remove_task(&task_id);
    state.emit_task_removed(&task_id);
    Ok(())
}

#[tauri::command]
pub async fn pick_directory(
    app: tauri::AppHandle,
    default_path: Option<String>,
) -> AppResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;

    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut builder = app.dialog().file();
        if let Some(dir) = default_path.filter(|d| !d.trim().is_empty()) {
            builder = builder.set_directory(dir);
        }
        builder.blocking_pick_folder()
    })
    .await
    .map_err(|e| AppError::internal(format!("目录选择器失败：{e}")))?;

    Ok(picked.and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().to_string()))
}

/// 读取本地图片并转为 data URL，供背景控制台预览与持久化使用
#[tauri::command]
pub fn read_image_as_data_url(path: String) -> AppResult<String> {
    use base64::Engine;

    let p = std::path::Path::new(&path);
    if !p.is_file() {
        return Err(AppError::storage("图片文件不存在"));
    }

    let ext = p
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let mime = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        _ => {
            return Err(AppError::new(
                "STORAGE_UNSUPPORTED_IMAGE",
                "只支持 JPG、PNG、WebP 静态图片",
                false,
            ))
        }
    };

    let bytes = std::fs::read(p)?;
    if bytes.len() > 24 * 1024 * 1024 {
        return Err(AppError::storage("图片体积超过 24 MB"));
    }

    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime};base64,{encoded}"))
}

#[tauri::command]
pub fn ensure_download_dir(dir: String) -> AppResult<String> {
    let path = std::path::PathBuf::from(&dir);
    crate::storage::ensure_dir(&path)?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn sidecar_status() -> crate::platform::sidecar::SidecarStatus {
    // 这个探测要真的把 yt-dlp 与 ffmpeg 拉起来取版本，本机实测约 2–3 秒。
    // 放到阻塞线程池里执行，任何情况下都不占用 UI 相关线程。
    tauri::async_runtime::spawn_blocking(crate::platform::sidecar::status)
        .await
        .unwrap_or_else(|e| {
            eprintln!("[videoflow] sidecar 探测任务失败：{e}");
            crate::platform::sidecar::SidecarStatus {
                yt_dlp: crate::platform::sidecar::ToolStatus::missing(),
                ffmpeg: crate::platform::sidecar::ToolStatus::missing(),
            }
        })
}