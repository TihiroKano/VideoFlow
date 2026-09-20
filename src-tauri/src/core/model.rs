//! 领域模型（对应 docs/VideoFlow-项目书.md §4.1 与前端 src/services/types.ts）。
//!
//! TS 与 Rust 的字段名必须保持一致，序列化统一使用 camelCase。

use serde::{Deserialize, Serialize};

/// 任务状态机（项目书 §4.2）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Draft,
    Resolving,
    Queued,
    Downloading,
    Pausing,
    Paused,
    RetryWait,
    Verifying,
    Merging,
    Completed,
    Failed,
    Cancelled,
    NeedsReparse,
    /// 文件已被删除（移入系统回收站），记录保留在列表里
    Deleted,
}

impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Deleted
        )
    }

    /// 占用调度槽位的状态
    pub fn is_running(self) -> bool {
        matches!(self, Self::Downloading | Self::Verifying | Self::Merging)
    }

    /// 可以被调度器取出的状态
    pub fn is_schedulable(self) -> bool {
        matches!(self, Self::Queued | Self::RetryWait)
    }

    /// 进程结束后没有对应运行实体的状态。
    ///
    /// 队列快照落盘与启动恢复时用：这些状态必须记为 `paused`，
    /// 否则重启后会显示成「下载中」但没有任何东西在跑（项目书 §3.2）。
    pub fn requires_pause_on_restore(self) -> bool {
        matches!(
            self,
            Self::Downloading | Self::Verifying | Self::Merging | Self::Pausing
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Resolving => "resolving",
            Self::Queued => "queued",
            Self::Downloading => "downloading",
            Self::Pausing => "pausing",
            Self::Paused => "paused",
            Self::RetryWait => "retry_wait",
            Self::Verifying => "verifying",
            Self::Merging => "merging",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::NeedsReparse => "needs_reparse",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Download,
    Convert,
}

/// 转换任务的模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConvertMode {
    /// 视频转换：重新编码视频与音频，或换容器
    Video,
    /// 音频提取：丢掉视频轨，只留音轨
    AudioExtract,
}

/// 一个转换任务的完整方案。
///
/// 这个结构**刻意不加 `#[serde(skip)]`**：`DownloadTask` 里那几个被 skip 的字段
/// 都需要 `PersistedTask` 外壳单独兜住才能恢复，而转码方案不含敏感信息、可以下发，
/// 只要正常序列化，`#[serde(flatten)]` 就会带它一起落盘与恢复——
/// 持久化层与 `restore_tasks` 因此**零改动**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertPlan {
    /// 源文件绝对路径
    pub source_path: String,
    pub mode: ConvertMode,
    /// 目标容器 key（见 `media::capabilities`）
    pub container: String,
    /// 视频编码 key；音频提取模式下无意义
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<String>,
    /// 音频编码 key
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_codec: Option<String>,
    /// 缩放到的高度；`None` 表示保持原始分辨率
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_height: Option<u32>,
    /// 是否允许走硬件（GPU）编码。
    ///
    /// 只是「允许」：具体能不能走由 `capabilities::hardware_encoder_for` 现场判断，
    /// 没有硬件路径或探测失败时静默回落软件编码。
    /// **默认 false**：重启后恢复的旧任务没有这个字段，让它们保持原来的软件行为，
    /// 而不是突然换一条全新的编码路径。
    #[serde(default)]
    pub use_hardware: bool,
    /// 点名要走哪条硬件路径（FFmpeg 编码器名，如 `h264_nvenc`）。
    ///
    /// `None` = 自动：按能力表里的优先级取第一个探测可用的。
    /// 点名的那条若不在候选里、或本机探测不可用，会**静默回落自动**，
    /// 所以这个字段永远只是「偏好」，不会让任务失败。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware_encoder: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    Video,
    Audio,
    Muxed,
}

/// 一条可下载的媒体流
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaStream {
    pub id: String,
    pub container: String,
    pub kind: StreamKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_label: Option<String>,
    /// 估算字节数；无法确认时为 None，UI 显示「待确认」
    #[serde(default)]
    pub estimated_bytes: Option<u64>,
    /// 编码标识，如 avc1.640028
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    /// Provider 提供的内容哈希（SHA-256，十六进制小写）。
    /// 为 `None` 时跳过哈希校验，只做长度校验（项目书 §3.3）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// 是否需要音视频合并
    pub needs_merge: bool,
    /// 实际下载地址（仅主进程使用，不下发到 UI 之外的地方）
    #[serde(skip_serializing)]
    pub url: String,
    /// 配套音频流的下载地址（yt-dlp 的 bestvideo+bestaudio 场景）
    #[serde(skip_serializing)]
    pub audio_url: Option<String>,
    /// 下载该流时必须携带的 Referer。
    ///
    /// 抖音的 CDN 直链校验来源：带 `https://www.douyin.com/` 返回 206，不带直接 403。
    /// 与 `url` 一样不下发前端。
    #[serde(skip_serializing, default)]
    pub referer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleTrack {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedMedia {
    pub resolved_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub source_name: String,
    pub source_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_sec: Option<f64>,
    pub streams: Vec<MediaStream>,
    pub subtitles: Vec<SubtitleTrack>,
    pub expires_at: Option<String>,
    pub provider_id: String,
    pub provider_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamSelection {
    pub stream_id: String,
    #[serde(default)]
    pub audio_stream_id: Option<String>,
    #[serde(default)]
    pub subtitle_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkState {
    pub index: u32,
    pub start: u64,
    pub end: u64,
    pub done: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeCheckpoint {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub chunks: Vec<ChunkState>,
    pub temp_path: Option<String>,
    pub range_supported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTask {
    pub id: String,
    pub kind: TaskKind,
    pub source_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub title: String,
    pub status: TaskStatus,
    pub priority: i32,
    pub created_at: String,
    pub updated_at: String,
    /// 事务式 compare-and-swap 版本号，用于拒绝过期命令
    pub version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_label: Option<String>,
    pub target_path: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bps: f64,
    pub eta_sec: Option<f64>,
    pub retry_count: u32,
    pub error: Option<TaskError>,
    pub resume: Option<ResumeCheckpoint>,

    // ---- 仅主进程内部使用，不下发到前端 ----
    /// 已选流的真实下载地址
    #[serde(skip)]
    pub source_stream_url: Option<String>,
    #[serde(skip)]
    pub source_audio_url: Option<String>,
    /// 下载时携带的 Referer（抖音 CDN 直链必需，见 `MediaStream::referer`）
    #[serde(skip)]
    pub referer: Option<String>,
    /// 媒体时长（秒）。HLS 用它与体积估算一起推进度；其余引擎仅展示用途
    #[serde(skip)]
    pub duration_sec: Option<f64>,
    /// Provider 提供的内容哈希（SHA-256 十六进制小写），下载完成后校验（项目书 §3.3）
    #[serde(skip)]
    pub expected_sha256: Option<String>,
    /// 该任务使用的下载引擎（下发给界面，用于展示「下载详情」）
    pub engine: EngineKind,
    /// 临时分片目录
    #[serde(skip)]
    pub scratch_dir: Option<String>,
    /// 转换任务的方案（仅 `kind == Convert` 时有值）。
    ///
    /// 不加 `#[serde(skip)]`：见 `ConvertPlan` 的说明，靠正常序列化落盘与恢复。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub convert: Option<ConvertPlan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    /// 原生 HTTP 引擎：Range 分段、断点续传
    #[default]
    NativeHttp,
    /// 站点授权流：交给 yt-dlp 下载
    YtDlp,
    /// HLS 清单：交给 FFmpeg 拉流并封装（AES/byterange/fMP4 由它原生处理）
    Hls,
    /// 格式转换：交给 FFmpeg 重新编码（本地文件，无网络）
    Convert,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgress {
    pub task_id: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bps: f64,
    pub eta_sec: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueStats {
    pub active_count: u32,
    pub queued_count: u32,
    pub total_speed_bps: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppNotice {
    pub seq: u64,
    pub level: String,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

impl DownloadTask {
    pub fn progress(&self) -> TaskProgress {
        TaskProgress {
            task_id: self.id.clone(),
            downloaded_bytes: self.downloaded_bytes,
            total_bytes: self.total_bytes,
            speed_bps: self.speed_bps,
            eta_sec: self.eta_sec,
        }
    }

    /// 展示用清晰度标签
    pub fn quality_from_selection(stream: &MediaStream) -> String {
        if let Some(label) = &stream.quality_label {
            return label.clone();
        }
        if let Some(h) = stream.height {
            return format!("{h}P");
        }
        match stream.kind {
            StreamKind::Audio => "音频".to_string(),
            _ => "默认".to_string(),
        }
    }
}

/// 由选中的流推导下载引擎与真实下载地址。
///
/// yt-dlp 用 `video+audio` 选择器；原生引擎直接用流地址。任务创建与「重新解析」
/// 共用这一份逻辑，避免两处各写一遍导致行为走偏。
///
/// 放在领域层（而不是 state）是为了让它能脱离 GUI 依赖被单测覆盖。
pub fn engine_targets(
    stream: &MediaStream,
    audio: Option<&MediaStream>,
) -> (EngineKind, Option<String>, Option<String>) {
    // HLS：交给 FFmpeg 拉清单，selector 位置存清单地址
    if stream.id.starts_with("hls-") {
        return (EngineKind::Hls, Some(stream.url.clone()), None);
    }
    if stream.id.starts_with("yt-") {
        let video_id = stream.id.trim_start_matches("yt-");
        let selector = match (stream.needs_merge, audio) {
            (true, Some(a)) => format!("{}+{}", video_id, a.id.trim_start_matches("yt-")),
            (true, None) => format!("{}+bestaudio", video_id),
            (false, _) => video_id.to_string(),
        };
        (EngineKind::YtDlp, Some(selector), None)
    } else {
        (
            EngineKind::NativeHttp,
            Some(stream.url.clone()),
            stream.audio_url.clone(),
        )
    }
}

/// 重新解析后挑选与任务原选择最接近的流：先按「清晰度 + 容器」精确匹配，
/// 再退到清晰度、容器，最后退回首个不需要合并的流（免合并更稳）。
pub fn pick_stream<'a>(media: &'a ResolvedMedia, task: &DownloadTask) -> Option<&'a MediaStream> {
    let candidates: Vec<&MediaStream> = media
        .streams
        .iter()
        .filter(|s| s.kind != StreamKind::Audio)
        .collect();
    if candidates.is_empty() {
        return media.streams.first();
    }

    let label = task.quality_label.as_deref();
    let container = task.container.as_deref();

    if let Some(label) = label {
        if let Some(s) = candidates.iter().find(|s| {
            s.quality_label.as_deref() == Some(label) && container == Some(s.container.as_str())
        }) {
            return Some(s);
        }
        if let Some(s) = candidates
            .iter()
            .find(|s| s.quality_label.as_deref() == Some(label))
        {
            return Some(s);
        }
    }
    if let Some(container) = container {
        if let Some(s) = candidates.iter().find(|s| s.container == container) {
            return Some(s);
        }
    }
    candidates
        .iter()
        .find(|s| !s.needs_merge)
        .copied()
        .or_else(|| candidates.first().copied())
}

/// 重新解析后为需要合并的流挑配套音频：优先沿用原来的音频描述
pub fn pick_audio<'a>(
    media: &'a ResolvedMedia,
    stream: &MediaStream,
    task: &DownloadTask,
) -> Option<&'a MediaStream> {
    if !stream.needs_merge {
        return None;
    }
    let audios: Vec<&MediaStream> = media
        .streams
        .iter()
        .filter(|s| s.kind == StreamKind::Audio)
        .collect();
    if audios.is_empty() {
        return None;
    }
    if let Some(label) = task.audio_label.as_deref() {
        if let Some(a) = audios
            .iter()
            .find(|a| a.audio_label.as_deref() == Some(label))
        {
            return Some(a);
        }
    }
    if let Some(a) = audios.iter().find(|a| a.container == stream.container) {
        return Some(a);
    }
    audios.first().copied()
}

/// 设备档位对应的下载并发上限（项目书 §3.2「设备低档或网络受限时自动下调」）。
///
/// 返回 `(同时下载任务数上限, 单任务分段并发上限)`；档位未知时不下压，
/// 由用户设置值决定。
pub fn device_tier_caps(tier: &str) -> (u32, u32) {
    match tier {
        "performance" => (2, 2),
        "balanced" => (3, 3),
        _ => (6, 8),
    }
}

/// 任务来源的站点域名（小写，去掉 `www.`）。
///
/// 调度器用它做「同一来源并发上限」（项目书 §3.2）：同一个站点同时开太多任务容易触发
/// 服务端限流。这里刻意用**页面来源**而不是媒体地址——限流按站点算而不是按 CDN 节点算，
/// 而且 yt-dlp 任务根本没有单独的媒体地址（只有 `137+140` 这样的选择器）。
pub fn task_source_host(task: &DownloadTask) -> Option<String> {
    let url = url::Url::parse(&task.source_url).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    Some(host.strip_prefix("www.").unwrap_or(&host).to_string())
}

/// 把字节数格式化为人类可读文本（用于日志与诊断）
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(id: &str, label: &str, container: &str, kind: StreamKind) -> MediaStream {
        MediaStream {
            id: id.to_string(),
            container: container.to_string(),
            kind,
            quality_label: Some(label.to_string()),
            width: None,
            height: None,
            fps: None,
            audio_label: (kind == StreamKind::Audio).then(|| format!("{label}")),
            estimated_bytes: None,
            codec: None,
            sha256: None,
            needs_merge: kind != StreamKind::Audio,
            url: format!("https://cdn.example.com/{id}"),
            audio_url: None,
            referer: None,
        }
    }

    fn media_of(streams: Vec<MediaStream>) -> ResolvedMedia {
        ResolvedMedia {
            resolved_id: "r1".to_string(),
            title: "测试视频".to_string(),
            description: None,
            source_name: "测试站".to_string(),
            source_url: "https://example.com/video/1".to_string(),
            thumbnail_url: None,
            duration_sec: None,
            streams,
            subtitles: Vec::new(),
            expires_at: None,
            provider_id: "test".to_string(),
            provider_version: "1".to_string(),
        }
    }

    fn task_with(
        quality: Option<&str>,
        container: Option<&str>,
        audio_label: Option<&str>,
    ) -> DownloadTask {
        DownloadTask {
            id: "t1".to_string(),
            kind: TaskKind::Download,
            source_url: "https://example.com/video/1".to_string(),
            provider_id: Some("test".to_string()),
            title: "测试".to_string(),
            status: TaskStatus::NeedsReparse,
            priority: 0,
            created_at: "2026-09-19T00:00:00Z".to_string(),
            updated_at: "2026-09-19T00:00:00Z".to_string(),
            version: 1,
            thumbnail_url: None,
            quality_label: quality.map(|s| s.to_string()),
            container: container.map(|s| s.to_string()),
            audio_label: audio_label.map(|s| s.to_string()),
            target_path: None,
            downloaded_bytes: 0,
            total_bytes: None,
            speed_bps: 0.0,
            eta_sec: None,
            retry_count: 0,
            error: None,
            resume: None,
            source_stream_url: None,
            source_audio_url: None,
            referer: None,
            duration_sec: None,
            expected_sha256: None,
            engine: EngineKind::NativeHttp,
            scratch_dir: None,
            convert: None,
        }
    }

    #[test]
    fn 重新解析优先沿用原清晰度与容器() {
        let media = media_of(vec![
            stream("v-720", "720P", "mp4", StreamKind::Video),
            stream("v-1080", "1080P", "webm", StreamKind::Video),
            stream("v-1080-mp4", "1080P", "mp4", StreamKind::Video),
        ]);
        let task = task_with(Some("1080P"), Some("mp4"), None);
        let picked = pick_stream(&media, &task).expect("应能选到流");
        assert_eq!(picked.id, "v-1080-mp4", "清晰度与容器都要对上");
    }

    #[test]
    fn 重新解析不会选中音频流() {
        let media = media_of(vec![
            stream("audio-1", "128kbps", "m4a", StreamKind::Audio),
            stream("v-1080", "1080P", "mp4", StreamKind::Video),
        ]);
        let task = task_with(Some("1080P"), Some("mp4"), None);
        assert_eq!(pick_stream(&media, &task).unwrap().id, "v-1080");
    }

    #[test]
    fn 配套音频沿用原音频描述() {
        let media = media_of(vec![
            stream("v-1080", "1080P", "mp4", StreamKind::Video),
            stream("a-aac", "AAC 128kbps", "m4a", StreamKind::Audio),
            stream("a-opus", "Opus 64kbps", "webm", StreamKind::Audio),
        ]);
        let mut video = stream("v-1080", "1080P", "mp4", StreamKind::Video);
        video.needs_merge = true;
        let task = task_with(Some("1080P"), Some("mp4"), Some("Opus 64kbps"));
        let audio = pick_audio(&media, &video, &task).expect("应挑到音频");
        assert_eq!(audio.id, "a-opus");
    }

    #[test]
    fn 重启后需要暂停的状态判定() {
        // 落盘与恢复时会把这些状态记成 paused（项目书 §3.2）
        for status in [
            TaskStatus::Downloading,
            TaskStatus::Verifying,
            TaskStatus::Merging,
            TaskStatus::Pausing,
        ] {
            assert!(status.requires_pause_on_restore(), "{status:?} 应降级为已暂停");
        }
        for status in [
            TaskStatus::Queued,
            TaskStatus::Paused,
            TaskStatus::Completed,
            TaskStatus::NeedsReparse,
            TaskStatus::Cancelled,
        ] {
            assert!(!status.requires_pause_on_restore(), "{status:?} 不应被改动");
        }
    }

    #[test]
    fn 设备档位决定下载并发上限() {
        // 未知档位不下压，由用户设置决定
        assert_eq!(device_tier_caps(""), (6, 8));
        assert_eq!(device_tier_caps("quality"), (6, 8));
        // 低档自动下调（项目书 §3.2）
        assert_eq!(device_tier_caps("balanced"), (3, 3));
        assert_eq!(device_tier_caps("performance"), (2, 2));
    }

    #[test]
    fn 来源域名归一化并去掉_www() {
        let mut task = task_with(None, None, None);

        task.source_url = "https://www.bilibili.com/video/BV1xx".to_string();
        assert_eq!(task_source_host(&task).as_deref(), Some("bilibili.com"));

        // 大小写归一
        task.source_url = "https://WWW.Bilibili.COM/video/BV1xx".to_string();
        assert_eq!(task_source_host(&task).as_deref(), Some("bilibili.com"));

        // 没有 www 的原样保留
        task.source_url = "https://cdn.example.com/a.mp4".to_string();
        assert_eq!(task_source_host(&task).as_deref(), Some("cdn.example.com"));

        // 非法地址不参与限流计算
        task.source_url = "not a url".to_string();
        assert_eq!(task_source_host(&task), None);
    }

    #[test]
    fn yt_dlp_流生成选择器而原生流用直链() {
        let video = stream("yt-137", "1080P", "mp4", StreamKind::Video);
        let audio = stream("yt-140", "AAC 128kbps", "m4a", StreamKind::Audio);
        let (engine, selector, audio_url) = engine_targets(&video, Some(&audio));
        assert_eq!(engine, EngineKind::YtDlp);
        assert_eq!(selector.as_deref(), Some("137+140"));
        assert!(audio_url.is_none(), "yt-dlp 场景不使用单独的音频地址");

        let native = stream("direct-1", "1080P", "mp4", StreamKind::Video);
        let (engine, url, _) = engine_targets(&native, None);
        assert_eq!(engine, EngineKind::NativeHttp);
        assert_eq!(url.as_deref(), Some("https://cdn.example.com/direct-1"));
    }

    #[test]
    fn hls_流交给_ffmpeg_引擎() {
        // HLS 的 url 存的是媒体清单地址，交给 FFmpeg 去拉分片
        let hls = stream("hls-0", "720P", "mp4", StreamKind::Muxed);
        let (engine, playlist, audio) = engine_targets(&hls, None);
        assert_eq!(engine, EngineKind::Hls);
        assert_eq!(playlist.as_deref(), Some("https://cdn.example.com/hls-0"));
        assert!(audio.is_none(), "HLS 清单已含音轨，不需要单独音频地址");
    }
}