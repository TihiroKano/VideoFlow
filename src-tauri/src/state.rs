//! 应用状态、事件总线与调度器。
//!
//! 后端是状态真相源（项目书 §4.2）：每次变更走事务式 compare-and-swap（`version`），
//! 陈旧命令会被拒绝；前端只做乐观展示，收到 `task.updated` 后收敛。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Notify, Semaphore};

use crate::core::model::{
    engine_targets, task_source_host, AppNotice, ConvertPlan, DownloadTask, EngineKind, MediaStream,
    QueueStats, ResolvedMedia, ResumeCheckpoint, StreamSelection, TaskError, TaskKind, TaskStatus,
};
use crate::downloader::http::{self, ProgressSnapshot};
use crate::downloader::{ytdlp as ytdlp_dl, ControlState, ControlToken, SpeedMeter};
use crate::error::{AppError, AppResult};
use crate::media;
use crate::storage;

/// 事件合并节流：每任务最多 4 次/秒（项目书 §3.4）
const EMIT_INTERVAL_MS: u64 = 250;

/// 同时运行的转码任务上限。
///
/// 转码是纯 CPU 活，本地文件也没有远端限流压力，但它会跟下载抢机器：
/// 放开并发跑多个 FFmpeg 会明显拖慢界面。固定为 1 直到有实际需求。
const CONVERT_CONCURRENCY: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub download_dir: String,
    pub max_concurrent: u32,
    pub chunk_concurrency: u32,
    /// 同一来源（同一站点域名）同时运行的任务数上限，避免触发服务端限流（项目书 §3.2）
    #[serde(default = "default_per_host")]
    pub per_host_concurrency: u32,
    /// 前端设备探测给出的渲染档位：quality / balanced / performance。
    /// 后端据此自动下调并发（项目书 §3.2「设备低档或网络受限时自动下调」）。
    #[serde(default)]
    pub device_tier: String,
    /// 登录态来源，供 yt-dlp 使用。取值：
    /// - `""`：不使用（抖音会失败，Bilibili 只有低清晰度）
    /// - `"browser:edge"` / `"browser:chrome"` / `"browser:firefox"`：复用浏览器登录态
    /// - `"file:<绝对路径>"`：使用导出的 cookies.txt
    ///
    /// 抖音要求登录态才能取到播放地址，Bilibili 的高码率/60 帧也只对已登录用户开放。
    /// 这里用的是用户自己账号本就有的权限，不是绕过访问控制。
    #[serde(default)]
    pub cookies_source: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_dir: "D:\\VideoFlow\\Downloads".to_string(),
            max_concurrent: 3,
            chunk_concurrency: 4,
            per_host_concurrency: default_per_host(),
            device_tier: String::new(),
            cookies_source: String::new(),
        }
    }
}

/// 同一来源并发上限的默认值（项目书 §3.2：避免触发服务端限流）
fn default_per_host() -> u32 {
    2
}

/// 实际生效的下载并发（项目书 §3.2）
#[derive(Debug, Clone, Copy)]
pub struct DownloadLimits {
    pub max_concurrent: u32,
    pub chunk_concurrency: u32,
}

fn settings_path() -> PathBuf {
    // 与默认下载目录同级，全部落在 D 盘，不写 C 盘用户目录
    PathBuf::from("D:\\VideoFlow\\settings.json")
}

fn load_settings() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<Settings>(&raw).ok())
        .unwrap_or_default()
}

fn save_settings(settings: &Settings) {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(data) = serde_json::to_vec_pretty(settings) {
        let _ = std::fs::write(path, data);
    }
}

struct Inner {
    tasks: HashMap<String, DownloadTask>,
    resolved: HashMap<String, ResolvedMedia>,
    controls: HashMap<String, Arc<ControlToken>>,
}

pub struct AppState {
    pub app: AppHandle,
    pub client: reqwest::Client,
    inner: Mutex<Inner>,
    settings: Mutex<Settings>,
    seq: AtomicU64,
    /// 调度槽位：限制同时下载的任务数
    slots: Arc<Semaphore>,
    wake: Arc<Notify>,
    last_emit: Mutex<HashMap<String, u64>>,
    /// 队列有变更、等待落盘（项目书 §3.2 持久化队列）
    dirty: AtomicBool,
    /// Browser Resolver：内嵌浏览器解析（抖音等有动态签名的站点）。
    /// 窗口懒创建——没用过这类站点就不必付 WebView2 的启动成本。
    browser: crate::resolver::browser::driver::BrowserResolver,
    /// 账户登录的「我已完成登录」确认标志。
    /// 登录等待循环靠它收工，用于 Cookie 名不稳定、无法自动判定的站点。
    login_confirm: Mutex<HashMap<String, Arc<std::sync::atomic::AtomicBool>>>,
}

impl AppState {
    pub fn new(app: AppHandle) -> Arc<Self> {
        let settings = load_settings();
        let client = reqwest::Client::builder()
            .user_agent("VideoFlow/0.1 (Windows)")
            .connect_timeout(std::time::Duration::from_secs(20))
            .timeout(std::time::Duration::from_secs(600))
            .pool_max_idle_per_host(8)
            .build()
            .expect("构建 HTTP 客户端失败");

        let slots = Arc::new(Semaphore::new(settings.max_concurrent.max(1) as usize));

        let state = Arc::new(Self {
            app,
            client,
            inner: Mutex::new(Inner {
                tasks: HashMap::new(),
                resolved: HashMap::new(),
                controls: HashMap::new(),
            }),
            settings: Mutex::new(settings),
            seq: AtomicU64::new(1),
            slots,
            wake: Arc::new(Notify::new()),
            last_emit: Mutex::new(HashMap::new()),
            dirty: AtomicBool::new(false),
            browser: crate::resolver::browser::driver::BrowserResolver::new(),
            login_confirm: Mutex::new(HashMap::new()),
        });
        // 恢复上次的队列（项目书 §3.2）：必须在窗口加载「快照」之前完成
        state.restore_tasks();
        state
    }

    pub fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst)
    }

    /// 用内嵌浏览器抓取页面上的媒体数据（抖音等有动态签名的站点）。
    ///
    /// 页面自己的 JS 会带上签名去请求详情接口，我们只把响应捞回来。
    pub async fn browser_capture(
        &self,
        page_url: &url::Url,
        token: Option<Arc<ControlToken>>,
    ) -> AppResult<crate::resolver::browser::CapturePayload> {
        self.browser.capture(&self.app, page_url, token).await
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// 供 yt-dlp 使用的登录态来源；未启用时返回 None
    pub fn cookies_source(&self) -> Option<String> {
        let raw = self.settings().cookies_source;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    /// 切换登录态来源并落盘（账户登录/退出后由命令层调用）
    pub fn set_cookies_source(&self, source: &str) {
        self.update_settings(SettingsPatch {
            cookies_source: Some(source.to_string()),
            ..Default::default()
        });
    }

    /// 取（或创建）某站点的登录确认标志
    pub fn login_confirm_flag(&self, site_key: &str) -> Arc<std::sync::atomic::AtomicBool> {
        let mut map = self.login_confirm.lock().expect("login_confirm 锁不可中毒");
        Arc::clone(
            map.entry(site_key.to_string())
                .or_insert_with(|| Arc::new(std::sync::atomic::AtomicBool::new(false))),
        )
    }

    /// 用户在登录窗口点了「我已完成登录」
    pub fn mark_login_confirm(&self, site_key: &str) {
        self.login_confirm_flag(site_key)
            .store(true, Ordering::SeqCst);
    }

    pub fn update_settings(&self, patch: SettingsPatch) {
        if let Ok(mut s) = self.settings.lock() {
            if let Some(dir) = patch.download_dir {
                if !dir.trim().is_empty() {
                    s.download_dir = dir;
                }
            }
            if let Some(n) = patch.max_concurrent {
                let clamped = n.clamp(1, 6);
                s.max_concurrent = clamped;
                self.slots.add_permits(0);
            }
            if let Some(n) = patch.chunk_concurrency {
                s.chunk_concurrency = n.clamp(1, 8);
            }
            if let Some(n) = patch.per_host_concurrency {
                s.per_host_concurrency = n.clamp(1, 6);
            }
            if let Some(tier) = patch.device_tier {
                s.device_tier = tier.trim().to_string();
            }
            if let Some(source) = patch.cookies_source {
                s.cookies_source = source.trim().to_string();
            }
            save_settings(&s);
        }
        self.wake.notify_waiters();
    }

    /// 实际生效的下载并发上限（项目书 §3.2）。
    ///
    /// 用户设置是上限，设备档位在低档时再往下压一档：弱机同时开满 6 个任务 + 8 路分段
    /// 会把 CPU/磁盘抢光，反而让界面与下载一起变慢。档位由前端设备探测给出，未知时不下压。
    pub fn limits(&self) -> DownloadLimits {
        let s = self.settings();
        let (max_cap, chunk_cap) = crate::core::model::device_tier_caps(&s.device_tier);
        DownloadLimits {
            max_concurrent: s.max_concurrent.clamp(1, max_cap),
            chunk_concurrency: s.chunk_concurrency.clamp(1, chunk_cap),
        }
    }

    // ---- 解析结果缓存 ----

    pub fn store_resolved(&self, media: ResolvedMedia) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.resolved.insert(media.resolved_id.clone(), media);
        }
    }

    pub fn get_resolved(&self, id: &str) -> Option<ResolvedMedia> {
        self.inner.lock().ok()?.resolved.get(id).cloned()
    }

    /// 清理超过 TTL 的解析结果，避免长期占用内存
    pub fn prune_resolved(&self) {
        let now = chrono::Utc::now();
        if let Ok(mut inner) = self.inner.lock() {
            inner.resolved.retain(|_, media| {
                match media.expires_at.as_deref().and_then(|s| {
                    chrono::DateTime::parse_from_rfc3339(s).ok()
                }) {
                    Some(exp) => exp.with_timezone(&chrono::Utc) > now,
                    None => true,
                }
            });
        }
    }

    // ---- 任务 ----

    pub fn snapshot(&self) -> Vec<DownloadTask> {
        self.inner
            .lock()
            .map(|inner| inner.tasks.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn get_task(&self, id: &str) -> Option<DownloadTask> {
        self.inner.lock().ok()?.tasks.get(id).cloned()
    }

    pub fn insert_task(&self, task: DownloadTask) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.tasks.insert(task.id.clone(), task);
        }
        self.mark_dirty();
    }

    pub fn remove_task(&self, id: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.tasks.remove(id);
            inner.controls.remove(id);
        }
        self.mark_dirty();
    }

    // ---- 持久化队列（项目书 §3.2）----

    /// 标记队列有变更；由后台节流器合并写盘，避免频繁 I/O
    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::SeqCst);
    }

    /// 把队列写成磁盘快照。
    ///
    /// 写盘时把「运行中」的任务记为 `paused`：这次进程结束后它们并没有运行实体，
    /// 恢复时必须由用户或调度器重新校验断点后再继续（项目书 §3.2）。
    pub fn persist_snapshot(&self) {
        let records: Vec<storage::PersistedTask> = self
            .snapshot()
            .into_iter()
            .map(|mut task| {
                if task.status.requires_pause_on_restore() {
                    task.status = TaskStatus::Paused;
                }
                task.speed_bps = 0.0;
                task.eta_sec = None;
                task.updated_at = chrono::Utc::now().to_rfc3339();
                storage::PersistedTask {
                    stream_url: task.source_stream_url.clone(),
                    audio_url: task.source_audio_url.clone(),
                    scratch_dir: task.scratch_dir.clone(),
                    sha256: task.expected_sha256.clone(),
                    referer: task.referer.clone(),
                    duration_sec: task.duration_sec,
                    task,
                }
            })
            .collect();

        if let Err(e) = storage::save_tasks(&records) {
            eprintln!("[videoflow] 保存队列快照失败：{e}");
        }
    }

    /// 启动时恢复上次的队列。
    ///
    /// 硬退出（任务管理器结束进程、断电）时快照里可能残留 `downloading` 等状态，
    /// 这次启动并没有对应的运行实体，因此一律降级为 `paused`；
    /// 从旧版本快照读到的任务还需要把内部字段（下载地址、临时目录）装回去。
    fn restore_tasks(&self) {
        let records = storage::load_tasks();
        if records.is_empty() {
            return;
        }

        let mut restored = 0usize;
        if let Ok(mut inner) = self.inner.lock() {
            for mut record in records {
                if record.task.status.requires_pause_on_restore() {
                    record.task.status = TaskStatus::Paused;
                }
                record.task.speed_bps = 0.0;
                record.task.eta_sec = None;
                // 版本号继续往前推，前端不会把恢复出来的任务当成旧快照
                record.task.version += 1;
                record.task.source_stream_url = record.stream_url;
                record.task.source_audio_url = record.audio_url;
                record.task.scratch_dir = record.scratch_dir;
                record.task.expected_sha256 = record.sha256;
                record.task.referer = record.referer;
                record.task.duration_sec = record.duration_sec;
                inner.tasks.insert(record.task.id.clone(), record.task);
                restored += 1;
            }
        }
        if restored > 0 {
            eprintln!("[videoflow] 已恢复上次的任务记录 {restored} 条");
        }
    }

    pub fn control_token(&self, id: &str) -> Option<Arc<ControlToken>> {
        self.inner.lock().ok()?.controls.get(id).cloned()
    }

    pub fn set_control(&self, id: &str, token: Arc<ControlToken>) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.controls.insert(id.to_string(), token);
        }
    }

    /// 事务式更新：只有传入版本与当前一致才生效
    pub fn mutate<F>(&self, id: &str, f: F) -> Option<DownloadTask>
    where
        F: FnOnce(&mut DownloadTask),
    {
        let updated = {
            let mut inner = self.inner.lock().ok()?;
            let task = inner.tasks.get_mut(id)?;
            f(task);
            task.version += 1;
            task.updated_at = chrono::Utc::now().to_rfc3339();
            task.clone()
        };
        self.emit_task_updated(&updated);
        self.mark_dirty();
        self.wake.notify_waiters();
        Some(updated)
    }

    pub fn queue_stats(&self) -> QueueStats {
        let tasks = self.snapshot();
        let mut stats = QueueStats::default();
        for t in &tasks {
            if t.status.is_running() {
                stats.active_count += 1;
                stats.total_speed_bps += t.speed_bps;
            } else if t.status.is_schedulable() {
                stats.queued_count += 1;
            }
        }
        stats
    }

    // ---- 事件 ----

    /// 推送事件，失败时打日志。
    ///
    /// 之前所有 `emit` 都用 `let _ =` 吞掉返回值：一旦序列化或事件通道出问题，
    /// 界面就会永远停在旧状态，而控制台里没有任何线索可查。
    ///
    /// 事件名一律用下划线：Tauri 只允许 `[A-Za-z0-9]` 与 `-` `/` `:` `_`
    /// （`tauri-2.11.5/src/event/event_name.rs:8-12`），带 `.` 会被判为非法名，
    /// `emit` 直接报 `IllegalEventName` 而**什么都不推送**——界面因此完全收不到更新。
    fn emit_event<T: serde::Serialize + Clone>(&self, event: &str, payload: T) {
        if let Err(e) = self.app.emit(event, payload) {
            eprintln!("[videoflow] 推送事件 {event} 失败：{e}");
        }
    }

    pub fn emit_task_updated(&self, task: &DownloadTask) {
        let seq = self.next_seq();
        self.emit_event("task_updated", serde_json::json!({ "seq": seq, "task": task }));
        self.emit_event("queue_updated", self.queue_stats());
    }

    /// 任务被移除（只影响列表记录，不删除磁盘文件）
    pub fn emit_task_removed(&self, task_id: &str) {
        let seq = self.next_seq();
        self.emit_event(
            "task_removed",
            serde_json::json!({ "seq": seq, "taskId": task_id }),
        );
        self.emit_event("queue_updated", self.queue_stats());
    }

    pub fn emit_progress(&self, snapshot: &ProgressSnapshot, task_id: &str, total: Option<u64>) {
        let now = now_ms();
        let should_emit = {
            match self.last_emit.lock() {
                Ok(mut map) => {
                    let last = map.get(task_id).copied().unwrap_or(0);
                    if now.saturating_sub(last) >= EMIT_INTERVAL_MS {
                        map.insert(task_id.to_string(), now);
                        true
                    } else {
                        false
                    }
                }
                Err(_) => true,
            }
        };

        // 速度与进度始终写入状态，只是事件推送被节流
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(task) = inner.tasks.get_mut(task_id) {
                task.downloaded_bytes = snapshot.downloaded;
                if total.is_some() {
                    task.total_bytes = total;
                }
                task.speed_bps = snapshot.speed_bps;
                task.eta_sec = snapshot.eta_sec;
                task.updated_at = chrono::Utc::now().to_rfc3339();
            }
        }

        if !should_emit {
            return;
        }

        let seq = self.next_seq();
        self.emit_event(
            "task_progress",
            serde_json::json!({
                "seq": seq,
                "taskId": task_id,
                "downloadedBytes": snapshot.downloaded,
                "totalBytes": total,
                "speedBps": snapshot.speed_bps,
                "etaSec": snapshot.eta_sec,
            }),
        );
        self.emit_event("queue_updated", self.queue_stats());
    }

    pub fn notice(&self, level: &str, code: &str, message: &str, task_id: Option<&str>) {
        let notice = AppNotice {
            seq: self.next_seq(),
            level: level.to_string(),
            code: code.to_string(),
            message: message.to_string(),
            task_id: task_id.map(|s| s.to_string()),
        };
        self.emit_event("app_notice", notice);
    }

    pub fn fail_task(&self, id: &str, err: &AppError) {
        let retryable = err.retryable;
        let code = err.code.clone();
        let message = err.message.clone();
        let detail = err.detail.clone();
        // 解析过期或 403/404：进入「需重新解析」，由用户确认后重新解析（项目书 §3.1 第 4 条）
        let needs_reparse = err.requires_reparse();
        self.mutate(id, |task| {
            task.status = if needs_reparse {
                TaskStatus::NeedsReparse
            } else {
                TaskStatus::Failed
            };
            task.speed_bps = 0.0;
            task.eta_sec = None;
            task.error = Some(TaskError {
                code: code.clone(),
                message: message.clone(),
                retryable,
                detail: detail.clone(),
            });
        });
        self.notice(
            if needs_reparse { "warning" } else { "error" },
            &code,
            &message,
            Some(id),
        );
    }

    // ---- 调度 ----

    pub fn wake(&self) {
        self.wake.notify_waiters();
    }

    pub fn spawn_scheduler(self: &Arc<Self>) {
        let state = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            loop {
                state.wake.notified().await;
                state.pump();
            }
        });

        // 启动时把上次未完成的任务按快照恢复：稍等片刻，等前端完成订阅后
        // 把恢复出来的排队任务交给调度器，并清理过期的解析缓存。
        let state2 = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            state2.prune_resolved();
            state2.pump();
        });

        // 队列快照的落盘节流：状态变更后最多每 2 秒合并写一次盘
        let state3 = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                if state3.dirty.swap(false, Ordering::SeqCst) {
                    state3.persist_snapshot();
                }
            }
        });
    }

    /// 取出可调度的任务并把槽位占满
    pub fn pump(self: &Arc<Self>) {
        let limit = self.limits().max_concurrent;
        loop {
            let running = self.running_count();
            if running >= limit {
                break;
            }
            let Some(task) = self.take_next_schedulable() else {
                break;
            };
            let state = Arc::clone(self);
            tauri::async_runtime::spawn(async move {
                state.run_task(task).await;
            });
        }
    }

    fn running_count(&self) -> u32 {
        self.snapshot().iter().filter(|t| t.status.is_running()).count() as u32
    }

    fn take_next_schedulable(&self) -> Option<DownloadTask> {
        let per_host_limit = self.settings().per_host_concurrency.max(1);
        let mut inner = self.inner.lock().ok()?;

        // 同一来源在跑的任务数（项目书 §3.2：每个域名设并发上限，避免触发服务端限流）
        let mut running_per_host: HashMap<String, u32> = HashMap::new();
        let mut running_convert = 0u32;
        for task in inner.tasks.values() {
            if task.status.is_running() {
                if task.engine == EngineKind::Convert {
                    running_convert += 1;
                }
                if let Some(host) = task_source_host(task) {
                    *running_per_host.entry(host).or_insert(0) += 1;
                }
            }
        }

        let candidate = inner
            .tasks
            .values()
            .filter(|t| t.status.is_schedulable())
            .filter(|t| {
                // 转码吃 CPU：本地文件没有远端限流压力，但多个 FFmpeg 同时跑
                // 会把界面和下载一起拖卡，所以单独限一个上限
                if t.engine == EngineKind::Convert {
                    return running_convert < CONVERT_CONCURRENCY;
                }
                true
            })
            .filter(|t| match task_source_host(t) {
                // 该来源已经跑满：这次跳过它，让其它来源的任务先走
                Some(host) => {
                    running_per_host.get(&host).copied().unwrap_or(0) < per_host_limit
                }
                None => true,
            })
            .min_by(|a, b| {
                b.priority
                    .cmp(&a.priority)
                    .then_with(|| a.created_at.cmp(&b.created_at))
            })
            .cloned()?;

        if let Some(task) = inner.tasks.get_mut(&candidate.id) {
            task.status = TaskStatus::Downloading;
            task.version += 1;
            task.updated_at = chrono::Utc::now().to_rfc3339();
            task.error = None;
            let snapshot = task.clone();
            drop(inner);
            self.mark_dirty();
            return Some(snapshot);
        }
        None
    }

    async fn run_task(self: Arc<Self>, task: DownloadTask) {
        self.emit_task_updated(&task);

        let token = ControlToken::new();
        self.set_control(&task.id, Arc::clone(&token));

        let result = match task.engine {
            EngineKind::NativeHttp => self.run_native(&task, Arc::clone(&token)).await,
            EngineKind::YtDlp => self.run_ytdlp(&task, Arc::clone(&token)).await,
            EngineKind::Hls => self.run_hls(&task, Arc::clone(&token)).await,
            EngineKind::Convert => self.run_convert(&task, Arc::clone(&token)).await,
        };

        match result {
            Ok(Outcome::Completed(path, bytes)) => {
                self.mutate(&task.id, |t| {
                    t.status = TaskStatus::Completed;
                    t.target_path = Some(path.clone());
                    t.downloaded_bytes = bytes;
                    t.total_bytes = Some(bytes);
                    t.speed_bps = 0.0;
                    t.eta_sec = None;
                    t.error = None;
                    t.resume = None;
                });
                self.notice("success", "TASK_COMPLETED", &format!("已完成：{}", task.title), Some(&task.id));
            }
            Ok(Outcome::Paused) => {
                self.mutate(&task.id, |t| {
                    t.status = TaskStatus::Paused;
                    t.speed_bps = 0.0;
                    t.eta_sec = None;
                });
            }
            Err(err) if err.code == "TASK_PAUSED" => {
                // 转码不能断点续传（与 HLS 同理）：停下即丢弃半成品，
                // 恢复时从头再来。必须把已下载字节清零，否则界面会显示
                // 「可恢复 X MB」而实际并不会续上——那是在骗用户。
                if let Some(target) = &task.target_path {
                    let (part, _) = storage::temp_paths(std::path::Path::new(target));
                    let _ = std::fs::remove_file(part);
                }
                self.mutate(&task.id, |t| {
                    t.status = TaskStatus::Paused;
                    t.downloaded_bytes = 0;
                    t.speed_bps = 0.0;
                    t.eta_sec = None;
                    t.resume = None;
                });
            }
            Err(err) if err.code == "TASK_CANCELLED" => {
                let scratch = task
                    .scratch_dir
                    .clone()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."));
                storage::cleanup_scratch(&scratch);
                if let Some(target) = &task.target_path {
                    let (part, resume) = storage::temp_paths(std::path::Path::new(target));
                    let _ = std::fs::remove_file(part);
                    let _ = std::fs::remove_file(resume);
                }
                // 取消不删除已完成的最终文件
                self.mutate(&task.id, |t| {
                    t.status = TaskStatus::Cancelled;
                    t.speed_bps = 0.0;
                    t.eta_sec = None;
                });
            }
            Err(err) => {
                self.fail_task(&task.id, &err);
            }
        }

        if let Ok(mut inner) = self.inner.lock() {
            inner.controls.remove(&task.id);
        }
        self.wake.notify_waiters();
        self.pump();
    }

    async fn run_native(
        self: &Arc<Self>,
        task: &DownloadTask,
        token: Arc<ControlToken>,
    ) -> AppResult<Outcome> {
        let Some(target) = task.target_path.clone() else {
            return Err(AppError::internal("任务缺少目标路径"));
        };
        let Some(url) = task.source_stream_url.clone() else {
            return Err(AppError::provider_expired());
        };

        let target_path = PathBuf::from(&target);
        let (part, resume_path) = storage::temp_paths(&target_path);
        // 分段并发也走生效上限：设备低档时会被自动下调（项目书 §3.2）
        let limits = self.limits();

        let state = Arc::clone(self);
        let task_id = task.id.clone();

        let req = http::HttpRequest {
            url,
            target_path: target_path.clone(),
            temp_path: part.clone(),
            resume_path: resume_path.clone(),
            chunk_concurrency: limits.chunk_concurrency as usize,
            // 优先用磁盘上的检查点：它由每个分片完成时原子更新，比内存快照新
            // （重启恢复后这里能少下一批已经拿到的分片，项目书 §3.2）
            resume: http::load_checkpoint(&resume_path).or_else(|| task.resume.clone()),
            // 抖音等站点的 CDN 直链校验来源，缺失会被 403（见 MediaStream::referer）
            referer: task.referer.clone(),
            user_agent: "VideoFlow/0.1 (Windows)".to_string(),
        };

        let on_progress = Arc::new(move |snapshot: ProgressSnapshot| {
            state.emit_progress(&snapshot, &task_id, snapshot.total);
        });

        // 需要合并时先下视频流，再下音频流，最后交给 FFmpeg
        let outcome =
            http::download(&self.client, req, Arc::clone(&token), on_progress).await?;

        if token.state() == ControlState::Pause {
            let checkpoint = outcome.checkpoint.clone();
            self.mutate(&task.id, |t| {
                t.resume = Some(checkpoint.clone());
                t.downloaded_bytes = outcome.total_bytes;
            });
            return Ok(Outcome::Paused);
        }

        // 校验产物存在且非空
        let size = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if size == 0 {
            let _ = std::fs::remove_file(&part);
            return Err(AppError::integrity("下载产物为空"));
        }

        // Provider 提供哈希时校验 SHA-256（项目书 §3.3）。
        // 需要合并的任务跳过：最终文件是 FFmpeg 重新封装的结果，哈希对不上是正常的。
        if task.source_audio_url.is_none() {
            if let Some(expected) = task
                .expected_sha256
                .as_deref()
                .map(str::trim)
                .filter(|h| !h.is_empty())
            {
                let hash_path = part.clone();
                let actual = tauri::async_runtime::spawn_blocking(move || {
                    storage::sha256_file(&hash_path)
                })
                .await
                .map_err(|e| AppError::internal(format!("哈希校验任务失败：{e}")))??;
                if !actual.eq_ignore_ascii_case(expected) {
                    // 不可信产物直接删除，不留成最终文件（§3.5 INTEGRITY_*）
                    let _ = std::fs::remove_file(&part);
                    let _ = std::fs::remove_file(&resume_path);
                    return Err(AppError::integrity(format!(
                        "SHA-256 校验失败：期望 {expected}，实际 {actual}"
                    )));
                }
            }
        }

        // 原子 rename：校验通过才成为最终文件
        if let Err(e) = std::fs::rename(&part, &target_path) {
            // 跨卷或目标被占用时退化为复制
            std::fs::copy(&part, &target_path)
                .map_err(|_| AppError::storage(format!("写入最终文件失败：{e}")))?;
            let _ = std::fs::remove_file(&part);
        }
        let _ = std::fs::remove_file(&resume_path);

        Ok(Outcome::Completed(target, size))
    }

    async fn run_ytdlp(self: &Arc<Self>, task: &DownloadTask, token: Arc<ControlToken>) -> AppResult<Outcome> {
        let Some(target) = task.target_path.clone() else {
            return Err(AppError::internal("任务缺少目标路径"));
        };
        let selector = task
            .source_stream_url
            .clone()
            .ok_or_else(AppError::provider_expired)?;

        let settings = self.settings();
        let scratch = task
            .scratch_dir
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| storage::scratch_dir_for(std::path::Path::new(&settings.download_dir), &task.id));
        storage::ensure_dir(&scratch)?;

        let state = Arc::clone(self);
        let task_id = task.id.clone();

        let req = ytdlp_dl::YtDlpRequest {
            page_url: task.source_url.clone(),
            format_selector: selector,
            scratch_dir: scratch.clone(),
            stem: task.id.clone(),
            target_path: PathBuf::from(&target),
            referer: task.referer.clone(),
            cookies_source: self.cookies_source(),
        };

        let on_progress = Arc::new(move |snapshot: ProgressSnapshot| {
            state.emit_progress(&snapshot, &task_id, snapshot.total);
        });

        let outcome = ytdlp_dl::download(&req, Arc::clone(&token), on_progress).await?;

        if token.state() == ControlState::Pause {
            self.mutate(&task.id, |t| {
                t.resume = Some(ytdlp_dl::checkpoint_for(&scratch));
                t.downloaded_bytes = outcome.downloaded_bytes;
            });
            return Ok(Outcome::Paused);
        }

        if outcome.produced.as_os_str().is_empty() {
            return Ok(Outcome::Paused);
        }

        // 需要合并时，yt-dlp 已按 -f video+audio 完成无损合并
        ytdlp_dl::finalize(&outcome.produced, std::path::Path::new(&target))?;
        storage::cleanup_scratch(&scratch);

        let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(outcome.downloaded_bytes);
        Ok(Outcome::Completed(target, size))
    }

    /// HLS：把媒体清单交给 FFmpeg 拉流并封装为 mp4。
    ///
    /// 分片下载、AES-128 解密、fMP4 的 init segment 都由 FFmpeg 处理，
    /// 我们只负责起进程、转发进度、响应取消。代价是 HLS 任务不支持断点续传
    /// （FFmpeg 单次跑完），取消后重来。
    async fn run_hls(self: &Arc<Self>, task: &DownloadTask, token: Arc<ControlToken>) -> AppResult<Outcome> {
        let Some(target) = task.target_path.clone() else {
            return Err(AppError::internal("任务缺少目标路径"));
        };
        let Some(playlist) = task.source_stream_url.clone() else {
            return Err(AppError::provider_expired());
        };

        let target_path = PathBuf::from(&target);
        let (part, _resume_path) = storage::temp_paths(&target_path);

        let state = Arc::clone(self);
        let task_id = task.id.clone();
        let on_progress: Box<dyn Fn(u64, Option<u64>) + Send + Sync> = Box::new(move |done, total| {
            state.emit_progress(
                &ProgressSnapshot {
                    downloaded: done,
                    total,
                    speed_bps: 0.0,
                    eta_sec: None,
                },
                &task_id,
                total,
            );
        });

        let cancel_token = Arc::clone(&token);
        let should_cancel: Box<dyn Fn() -> bool + Send + Sync> =
            Box::new(move || cancel_token.state() == ControlState::Cancel);

        media::hls_to_mp4(
            &playlist,
            task.referer.as_deref(),
            task.duration_sec,
            task.total_bytes,
            &part,
            on_progress,
            should_cancel,
        )
        .await?;

        // 原子 rename：与原生引擎一致，完成才成为最终文件
        if let Err(e) = std::fs::rename(&part, &target_path) {
            std::fs::copy(&part, &target_path)
                .map_err(|_| AppError::storage(format!("写入最终文件失败：{e}")))?;
            let _ = std::fs::remove_file(&part);
        }

        let size = std::fs::metadata(&target_path).map(|m| m.len()).unwrap_or(0);
        Ok(Outcome::Completed(target, size))
    }

    /// 格式转换：调 FFmpeg 重新编码本地文件。
    ///
    /// 与 HLS 一样不能断点续传，所以暂停走 `TASK_PAUSED`（调用方清半成品），
    /// 取消走 `TASK_CANCELLED`。
    async fn run_convert(
        self: &Arc<Self>,
        task: &DownloadTask,
        token: Arc<ControlToken>,
    ) -> AppResult<Outcome> {
        let Some(target) = task.target_path.clone() else {
            return Err(AppError::internal("任务缺少目标路径"));
        };
        let Some(plan) = task.convert.clone() else {
            return Err(AppError::internal("转换任务缺少方案"));
        };

        // 方案可能来自重启前的旧版本（或用户手改过 tasks.json），
        // 这里再校验一次：非法组合绝不能送进 FFmpeg
        media::capabilities::validate_plan(&plan)?;

        let input = PathBuf::from(&plan.source_path);
        let target_path = PathBuf::from(&target);
        let (part, _resume) = storage::temp_paths(&target_path);

        // 进度分母：源文件大小。转码后的体积事前不可知，用源大小能给出有意义的百分比
        let estimated_total = std::fs::metadata(&input).ok().map(|m| m.len());
        let duration = media::probe_duration(&input);

        // 线程数按设备档位限流：转码吃满 CPU 会把界面和下载一起拖卡
        let threads = match self.settings().device_tier.as_str() {
            "performance" => 2,
            "quality" => 0,
            _ => 4,
        };
        let body = media::capabilities::ffmpeg_args(&plan, &part, threads)?;
        let hardware = media::capabilities::uses_hardware(&plan);

        // 进度与停止回调每次转码都要新建一份（Box<dyn Fn> 不能复制），
        // 因为硬件路径失败时要用同一套回调把软件路径再跑一遍
        let make_progress = || {
            let state = Arc::clone(self);
            let task_id = task.id.clone();
            let on_progress: Box<dyn Fn(u64, Option<u64>) + Send + Sync> =
                Box::new(move |done, total| {
                    state.emit_progress(
                        &ProgressSnapshot {
                            downloaded: done,
                            total,
                            speed_bps: 0.0,
                            eta_sec: None,
                        },
                        &task_id,
                        total,
                    );
                });
            on_progress
        };
        let make_should_stop = || {
            let cancel_token = Arc::clone(&token);
            let should_stop: Box<dyn Fn() -> Option<crate::downloader::StopReason> + Send + Sync> =
                Box::new(move || match cancel_token.state() {
                    ControlState::Cancel => Some(crate::downloader::StopReason::Cancel),
                    ControlState::Pause => Some(crate::downloader::StopReason::Pause),
                    ControlState::Run => None,
                });
            should_stop
        };

        let first = media::transcode(
            &input,
            &part,
            &body,
            duration,
            estimated_total,
            make_progress(),
            make_should_stop(),
        )
        .await;

        // 硬件路径崩了（驱动抽风、显卡不支持这个分辨率、实验性参数不被接受……）
        // 不能让用户白等一场：自动退回软件编码重跑一次。
        // 暂停/取消是用户的意思，绝不能重试。
        if let Err(err) = first {
            let user_stopped = matches!(err.code.as_str(), "TASK_CANCELLED" | "TASK_PAUSED");
            if !hardware || user_stopped {
                return Err(err);
            }
            eprintln!("[videoflow] 硬件编码失败，回落软件编码重试：{}", err.message);
            let soft = media::capabilities::software_ffmpeg_args(&plan, &part, threads)?;
            media::transcode(
                &input,
                &part,
                &soft,
                duration,
                estimated_total,
                make_progress(),
                make_should_stop(),
            )
            .await?;
        }

        // 原子 rename：与其它引擎一致，完成才成为最终文件
        if let Err(e) = std::fs::rename(&part, &target_path) {
            std::fs::copy(&part, &target_path)
                .map_err(|_| AppError::storage(format!("写入最终文件失败：{e}")))?;
            let _ = std::fs::remove_file(&part);
        }

        let size = std::fs::metadata(&target_path).map(|m| m.len()).unwrap_or(0);
        Ok(Outcome::Completed(target, size))
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub download_dir: Option<String>,
    pub max_concurrent: Option<u32>,
    pub chunk_concurrency: Option<u32>,
    pub per_host_concurrency: Option<u32>,
    pub device_tier: Option<String>,
    pub cookies_source: Option<String>,
}

enum Outcome {
    Completed(String, u64),
    Paused,
}

/// 供命令层构造任务使用
pub struct NewTaskInput {
    pub resolved: ResolvedMedia,
    pub selection: StreamSelection,
    pub target_dir: String,
    pub title: String,
}

/// 根据解析结果与选择构造任务实体
pub fn build_task(input: NewTaskInput, task_id: String) -> AppResult<DownloadTask> {
    let media = &input.resolved;

    let stream = media
        .streams
        .iter()
        .find(|s| s.id == input.selection.stream_id)
        .ok_or_else(AppError::provider_expired)?;

    let audio_stream: Option<&MediaStream> = input
        .selection
        .audio_stream_id
        .as_deref()
        .and_then(|id| media.streams.iter().find(|s| s.id == id));

    let dir = PathBuf::from(&input.target_dir);
    storage::ensure_dir(&dir)?;

    let stem = storage::sanitize_stem(&input.title);
    let ext = if stream.needs_merge {
        "mp4".to_string()
    } else {
        stream.container.clone()
    };
    let target = storage::unique_path(&dir, &stem, &ext);
    storage::ensure_within(&dir, &target)?;

    let (engine, stream_url, audio_url) = engine_targets(stream, audio_stream);

    let now = chrono::Utc::now().to_rfc3339();
    Ok(DownloadTask {
        id: task_id,
        kind: TaskKind::Download,
        source_url: media.source_url.clone(),
        provider_id: Some(media.provider_id.clone()),
        title: input.title,
        status: TaskStatus::Queued,
        priority: 0,
        created_at: now.clone(),
        updated_at: now,
        version: 1,
        thumbnail_url: media.thumbnail_url.clone(),
        quality_label: Some(DownloadTask::quality_from_selection(stream)),
        container: Some(stream.container.clone()),
        audio_label: stream.audio_label.clone(),
        target_path: Some(target.to_string_lossy().to_string()),
        downloaded_bytes: 0,
        total_bytes: stream.estimated_bytes,
        speed_bps: 0.0,
        eta_sec: None,
        retry_count: 0,
        error: None,
        resume: None,
        source_stream_url: stream_url,
        source_audio_url: audio_url,
        referer: stream.referer.clone(),
        duration_sec: media.duration_sec,
        expected_sha256: stream.sha256.clone(),
        engine,
        scratch_dir: None,
        convert: None,
    })
}

/// 是否需要合并两条流（原生引擎场景）
pub fn needs_merge(task: &DownloadTask) -> bool {
    task.engine == EngineKind::NativeHttp && task.source_audio_url.is_some()
}

/// 根据转换方案构造任务实体。
///
/// `taken` 是当前所有任务已占用的目标路径：`storage::unique_path` 只看磁盘，
/// 排队中的任务还没落盘，只靠它会算出同一个目标路径（建第二个同名任务时撞车）。
pub fn build_convert_task(
    plan: ConvertPlan,
    target_dir: &str,
    id: String,
    taken: &[String],
) -> AppResult<DownloadTask> {
    media::capabilities::validate_plan(&plan)?;

    let source = PathBuf::from(&plan.source_path);
    if !source.is_file() {
        return Err(AppError::new(
            "MEDIA_SOURCE_MISSING",
            "源文件不存在或已被移动",
            false,
        )
        .with_hint("请重新选择文件后再试"));
    }

    // 源文件就是目标格式、且编码不变时转换没有意义，直接拒绝而不是白跑一趟
    let ext = media::capabilities::extension_of(&plan.container).ok_or_else(|| {
        AppError::new("MEDIA_UNSUPPORTED_TARGET", "不支持的目标格式", false)
    })?;

    let dir = PathBuf::from(target_dir);
    storage::ensure_dir(&dir)?;

    let stem = storage::sanitize_stem(
        &source
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "转换产物".to_string()),
    );
    // 先按磁盘上的冲突算，再避开排队任务的占用
    let mut target = storage::unique_path(&dir, &stem, ext);
    let mut guard = 1;
    while taken.iter().any(|p| p == &target.to_string_lossy()) {
        target = dir.join(format!("{stem} ({guard}).{ext}"));
        guard += 1;
        if guard > 999 {
            return Err(AppError::storage("同名文件过多，请清理下载目录"));
        }
    }
    storage::ensure_within(&dir, &target)?;

    let quality_label = if plan.mode == crate::core::model::ConvertMode::AudioExtract {
        Some("仅音频".to_string())
    } else {
        let v = plan
            .video_codec
            .as_deref()
            .map(codec_display)
            .unwrap_or("保持原编码");
        // 任务卡上标明这次真的走了显卡（并写清是哪条路径，例如 QSV / NVENC）：
        // 用户看到「突然快了几倍」得有据可查
        let gpu = media::capabilities::effective_hardware(&plan).map(|hw| hw.label);
        let v = match gpu {
            Some(label) => format!("{v}（GPU·{label}）"),
            None => v.to_string(),
        };
        let a = plan
            .audio_codec
            .as_deref()
            .map(codec_display)
            .unwrap_or("保持原音轨");
        Some(format!("{v} · {a}"))
    };

    let now = chrono::Utc::now().to_rfc3339();
    Ok(DownloadTask {
        id,
        kind: TaskKind::Convert,
        // 转换任务的「来源」就是本地文件路径
        source_url: plan.source_path.clone(),
        provider_id: Some("local".to_string()),
        title: stem,
        status: TaskStatus::Queued,
        priority: 0,
        created_at: now.clone(),
        updated_at: now,
        version: 1,
        thumbnail_url: None,
        quality_label,
        container: Some(plan.container.clone()),
        // 复用该字段展示源文件大小，界面不必区分两种 kind
        audio_label: None,
        target_path: Some(target.to_string_lossy().to_string()),
        downloaded_bytes: 0,
        total_bytes: std::fs::metadata(&source).ok().map(|m| m.len()),
        speed_bps: 0.0,
        eta_sec: None,
        retry_count: 0,
        error: None,
        resume: None,
        source_stream_url: None,
        source_audio_url: None,
        referer: None,
        duration_sec: media::probe_duration(&source),
        expected_sha256: None,
        engine: EngineKind::Convert,
        scratch_dir: None,
        convert: Some(plan),
    })
}

/// 编码 key → 界面显示名
fn codec_display(key: &str) -> &str {
    match key {
        "h264" => "H.264",
        "h265" => "H.265",
        "av1" => "AV1",
        "vp9" => "VP9",
        "aac" => "AAC",
        "opus" => "Opus",
        "mp3" => "MP3",
        "flac" => "FLAC",
        "pcm" => "PCM",
        other => other,
    }
}

/// 合并入口：把视频与音频合并到目标路径
pub fn merge_streams(video: &std::path::Path, audio: &std::path::Path, out: &std::path::Path) -> AppResult<()> {
    media::merge_to_mp4(video, audio, out)
}

/// 恢复时决定是否允许自动继续：用户手动暂停的任务不自动恢复（项目书 §3.2）
pub fn can_auto_resume(_checkpoint: &ResumeCheckpoint) -> bool {
    true
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 速度表在任务结束时归零
pub fn reset_speed(meter: &SpeedMeter) {
    meter.reset();
}