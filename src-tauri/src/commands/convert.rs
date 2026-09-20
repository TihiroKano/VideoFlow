//! 格式转换命令：能力表、源文件探测、创建转换任务。

use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use tauri::State;

use crate::core::model::{ConvertPlan, DownloadTask};
use crate::error::{AppError, AppResult};
use crate::media::{self, capabilities::ConvertCapabilities};
use crate::state::{build_convert_task, AppState};

/// 下发给界面的转码能力表（容器 / 编码 / 分辨率，以及它们之间的合法组合）。
///
/// 界面据此做联动过滤，防止用户选到必然失败的组合；后端在创建任务时还会再校验一次。
#[tauri::command]
pub async fn convert_capabilities() -> ConvertCapabilities {
    // 硬件编码器要真编一帧才知道能不能用（单次 1~2 秒），不能占着主线程；
    // 失败（线程池炸了）时退回「没有硬件」的软件能力表，功能不受影响
    tauri::async_runtime::spawn_blocking(media::capabilities::capabilities)
        .await
        .unwrap_or_else(|_| media::capabilities::build_capabilities(&[]))
}

/// 探测源文件的基本信息，供界面回显（时长、分辨率、编码）
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub duration_sec: Option<f64>,
    /// 文件大小（字节）
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// 视频编码名（ffprobe 的 codec_name，如 h264 / hevc）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_codec: Option<String>,
    /// 音频码率（bps）；读不到时为 None，界面据此决定要不要提示音质
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_bitrate: Option<u64>,
    /// 是否含视频轨（音频文件为 false）
    pub has_video: bool,
}

#[tauri::command]
pub async fn probe_media_info(path: String) -> AppResult<MediaInfo> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err(AppError::new("MEDIA_SOURCE_MISSING", "文件不存在", false));
    }
    let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    let duration = media::probe_duration(&p);
    let streams = media::probe_streams(&p);

    let video = streams.iter().find(|s| s.kind == "video");
    let audio = streams.iter().find(|s| s.kind == "audio");

    // 只在真有音轨时才去读码率：没有音轨的文件读出来的只会是视频码率
    let audio_bitrate = audio.and_then(|_| media::probe_audio_bitrate(&p, video.is_some()));

    Ok(MediaInfo {
        duration_sec: duration,
        size,
        width: video.and_then(|v| v.width),
        height: video.and_then(|v| v.height),
        video_codec: video.map(|v| v.codec.clone()),
        audio_codec: audio.map(|a| a.codec.clone()),
        audio_bitrate,
        has_video: video.is_some(),
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateConvertPayload {
    /// 源文件绝对路径
    pub source_path: String,
    /// `video` / `audio_extract`
    pub mode: String,
    pub container: String,
    #[serde(default)]
    pub video_codec: Option<String>,
    #[serde(default)]
    pub audio_codec: Option<String>,
    /// 目标高度；`None` 表示保持原始分辨率
    #[serde(default)]
    pub scale_height: Option<u32>,
    /// 是否允许硬件（GPU）编码；缺省为 false（老界面不带这个字段）
    #[serde(default)]
    pub use_hardware: bool,
    /// 点名走哪条硬件路径（如 `h264_nvenc`）；缺省 = 自动按优先级挑
    #[serde(default)]
    pub hardware_encoder: Option<String>,
    /// 产物目录
    pub target_dir: String,
}

#[tauri::command]
pub async fn create_convert_task(
    state: State<'_, Arc<AppState>>,
    payload: CreateConvertPayload,
) -> AppResult<DownloadTask> {
    let mode = match payload.mode.as_str() {
        "video" => crate::core::model::ConvertMode::Video,
        "audio_extract" => crate::core::model::ConvertMode::AudioExtract,
        other => {
            return Err(AppError::new(
                "MEDIA_UNSUPPORTED_TARGET",
                format!("未知的转换模式：{other}"),
                false,
            ))
        }
    };

    let plan = ConvertPlan {
        source_path: payload.source_path.clone(),
        mode,
        container: payload.container.clone(),
        video_codec: payload.video_codec.clone(),
        audio_codec: payload.audio_codec.clone(),
        scale_height: payload.scale_height,
        use_hardware: payload.use_hardware,
        hardware_encoder: payload.hardware_encoder.clone(),
    };

    let task_id = uuid::Uuid::new_v4().to_string();
    // 排队中的任务还没落盘，只靠 unique_path 会算出同一个目标路径
    let taken: Vec<String> = state
        .snapshot()
        .into_iter()
        .filter_map(|t| t.target_path)
        .collect();

    let task = build_convert_task(plan, &payload.target_dir, task_id, &taken)?;

    state.insert_task(task.clone());
    state.emit_task_updated(&task);
    state.wake();
    state.pump();

    // 与 create_task 同理：必须返回「调度之后」的状态，
    // 否则前端会用这份 queued 快照覆盖掉刚收到的 downloading 事件
    Ok(state.get_task(&task.id).unwrap_or(task))
}