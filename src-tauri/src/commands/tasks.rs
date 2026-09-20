//! 任务创建与控制命令。

use std::sync::Arc;

use serde::Deserialize;
use tauri::State;

use crate::core::model::{DownloadTask, MediaStream, ResolvedMedia, StreamSelection, TaskError, TaskKind, TaskStatus};
use crate::downloader::ControlState;
use crate::error::{AppError, AppResult};
use crate::state::{build_task, AppState, NewTaskInput};
use crate::storage;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskPayload {
    pub resolved_id: String,
    pub selection: StreamSelection,
    pub target_dir: String,
    pub title: String,
    #[serde(default)]
    pub idempotency_key: String,
}

#[tauri::command]
pub async fn create_task(
    state: State<'_, Arc<AppState>>,
    payload: CreateTaskPayload,
) -> AppResult<DownloadTask> {
    let resolved = state
        .get_resolved(&payload.resolved_id)
        .ok_or_else(AppError::provider_expired)?;

    // 防空标题：解析结果标题为空时回退到站点名
    let title = if payload.title.trim().is_empty() {
        resolved.title.clone()
    } else {
        payload.title.clone()
    };

    let task_id = uuid::Uuid::new_v4().to_string();
    let scratch = storage::scratch_dir_for(
        std::path::Path::new(&payload.target_dir),
        &task_id,
    );

    let mut task = build_task(
        NewTaskInput {
            resolved,
            selection: payload.selection,
            target_dir: payload.target_dir.clone(),
            title,
        },
        task_id.clone(),
    )?;
    task.scratch_dir = Some(scratch.to_string_lossy().to_string());

    state.insert_task(task.clone());
    state.emit_task_updated(&task);

    // 立即尝试调度
    state.wake();
    state.pump();

    // 必须返回「调度之后」的当前状态：pump 可能已经把任务推进到 downloading 并
    // 发出了 task.updated 事件。若这里仍返回上面那份 queued 快照，前端在
    // await 结束时会用它覆盖掉刚收到的新状态，界面就会停在「等待中 / 立即开始」，
    // 直到用户手动点一次按钮（那次命令返回的是最新状态）才继续。
    Ok(state.get_task(&task_id).unwrap_or(task))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskActionPayload {
    pub task_id: String,
    pub action: String,
    #[serde(default)]
    pub idempotency_key: String,
}

#[tauri::command]
pub async fn task_action(
    state: State<'_, Arc<AppState>>,
    payload: TaskActionPayload,
) -> AppResult<DownloadTask> {
    let task = state
        .get_task(&payload.task_id)
        .ok_or_else(|| AppError::new("TASK_NOT_FOUND", "任务不存在", false))?;

    match payload.action.as_str() {
        "pause" => {
            if !(task.status.is_running() || task.status.is_schedulable()) {
                // 幂等：状态已满足时直接返回当前任务
                return Ok(task);
            }
            if let Some(token) = state.control_token(&task.id) {
                token.request_pause();
            }
            let updated = state
                .mutate(&task.id, |t| {
                    t.status = if t.status.is_running() {
                        TaskStatus::Pausing
                    } else {
                        TaskStatus::Paused
                    };
                })
                .unwrap_or(task);
            Ok(updated)
        }
        "resume" => {
            if task.status.is_running() || task.status.is_terminal() {
                return Ok(task);
            }
            // needs_reparse：地址已失效，继续下载没有意义，必须走「重新解析」
            if task.status == TaskStatus::NeedsReparse {
                return Err(AppError::provider_expired());
            }
            let task_id = task.id.clone();
            // 恢复第一步是「校验中」（项目书 §3.4）：先校验本地断点是否还能用，
            // 断点文件已不在就丢弃检查点从头下载，而不是带着坏数据继续。
            state.mutate(&task_id, |t| {
                t.status = TaskStatus::Verifying;
                t.error = None;
            });
            let checkpoint_ok = checkpoint_usable(&task);
            let updated = state
                .mutate(&task_id, |t| {
                    if !checkpoint_ok {
                        t.resume = None;
                        t.downloaded_bytes = 0;
                    }
                    t.status = TaskStatus::Queued;
                    t.error = None;
                })
                .unwrap_or(task);
            state.wake();
            state.pump();
            // pump 可能已经把它推进到 downloading，返回最新状态，
            // 避免前端拿到 queued 快照把刚收到的事件覆盖回去
            Ok(state.get_task(&task_id).unwrap_or(updated))
        }
        // 手动置顶：优先级只在排队阶段起作用，运行中的任务不会被抢占（项目书 §3.2）
        "prioritize" | "unprioritize" => {
            let top = payload.action == "prioritize";
            let updated = state
                .mutate(&task.id, |t| {
                    t.priority = if top { 1 } else { 0 };
                })
                .unwrap_or(task);
            state.wake();
            state.pump();
            Ok(updated)
        }
        "cancel" => {
            if task.status.is_terminal() {
                return Ok(task);
            }
            if let Some(token) = state.control_token(&task.id) {
                token.request_cancel();
            } else {
                // 未在运行：直接清理临时产物并置为已取消
                if let Some(scratch) = &task.scratch_dir {
                    storage::cleanup_scratch(std::path::Path::new(scratch));
                }
                if let Some(target) = &task.target_path {
                    let (part, resume) = storage::temp_paths(std::path::Path::new(target));
                    let _ = std::fs::remove_file(part);
                    let _ = std::fs::remove_file(resume);
                }
            }
            let updated = state
                .mutate(&task.id, |t| {
                    t.status = TaskStatus::Cancelled;
                    t.speed_bps = 0.0;
                    t.eta_sec = None;
                })
                .unwrap_or(task);
            Ok(updated)
        }
        "retry" => {
            if task.status.is_running() {
                return Ok(task);
            }
            if task.status == TaskStatus::NeedsReparse {
                return Err(AppError::provider_expired());
            }
            let task_id = task.id.clone();
            let updated = state
                .mutate(&task_id, |t| {
                    t.status = TaskStatus::Queued;
                    t.retry_count += 1;
                    t.error = None;
                    // 保留 resume 检查点，让引擎自行判断能否续传
                })
                .unwrap_or(task);
            state.wake();
            state.pump();
            Ok(state.get_task(&task_id).unwrap_or(updated))
        }
        other => Err(AppError::new(
            "TASK_BAD_ACTION",
            format!("不支持的操作：{other}"),
            false,
        )),
    }
}

/// 断点是否仍然可用（项目书 §3.4「恢复」）：临时分片还在才算可续传。
/// 没有断点信息属于正常情况（从头下载），不当作失败。
fn checkpoint_usable(task: &DownloadTask) -> bool {
    let Some(cp) = &task.resume else {
        return true;
    };
    let Some(temp) = cp.temp_path.as_deref().filter(|p| !p.trim().is_empty()) else {
        return false;
    };
    let path = std::path::Path::new(temp);
    match std::fs::metadata(path) {
        // yt-dlp 场景记录的是临时目录：目录里还有分片才算可用
        Ok(meta) if meta.is_dir() => std::fs::read_dir(path)
            .map(|mut it| it.next().is_some())
            .unwrap_or(false),
        Ok(meta) => meta.len() > 0,
        Err(_) => false,
    }
}

/// 重新解析任务来源链接，并把新地址就地写回任务（项目书 §3.1 第 4 条）。
///
/// 用于「解析过期 / 403 / 404」之后的恢复路径：重新取一份媒体地址，尽量沿用原来的
/// 清晰度与容器选择，然后重新排队。断点数据保留，由引擎按 ETag / 分片自行校验。
#[tauri::command]
pub async fn reparse_task(
    state: State<'_, Arc<AppState>>,
    task_id: String,
) -> AppResult<DownloadTask> {
    let task = state
        .get_task(&task_id)
        .ok_or_else(|| AppError::new("TASK_NOT_FOUND", "任务不存在", false))?;

    if task.status.is_running() {
        return Ok(task);
    }
    if task.kind != TaskKind::Download {
        return Err(AppError::internal("只有下载任务需要重新解析"));
    }

    // needs_reparse → resolving（项目书 §4.2 状态机）
    state.mutate(&task_id, |t| {
        t.status = TaskStatus::Resolving;
        t.error = None;
    });

    let fresh: AppResult<(ResolvedMedia, MediaStream, Option<MediaStream>)> =
        match crate::commands::resolve_media(&state, &task.source_url, None).await {
            Ok(media) => match crate::core::model::pick_stream(&media, &task).cloned() {
                Some(stream) => {
                    let audio = crate::core::model::pick_audio(&media, &stream, &task).cloned();
                    Ok((media, stream, audio))
                }
                // 解析成功但没有任何可下载流：按过期处理，让用户看到同一类提示
                None => Err(AppError::provider_expired()),
            },
            Err(err) => Err(err),
        };

    match fresh {
        Ok((media, stream, audio)) => {
            let (engine, stream_url, audio_url) =
                crate::core::model::engine_targets(&stream, audio.as_ref());
            let thumbnail = media.thumbnail_url.clone().or_else(|| task.thumbnail_url.clone());
            let quality = DownloadTask::quality_from_selection(&stream);
            let container = stream.container.clone();
            let audio_label = stream.audio_label.clone();
            let estimated = stream.estimated_bytes;
            let provider_id = media.provider_id.clone();
            // 新解析结果可能带新的内容哈希，一起换掉（项目书 §3.3）
            let expected_sha256 = stream.sha256.clone();
            // 新解析结果的直链可能需要不同的 Referer（见 MediaStream::referer）
            let referer = stream.referer.clone();
            // HLS 恢复时要用时长继续推进度
            let duration_sec = media.duration_sec;

            // 新解析结果同样进缓存，界面上再次「加入下载」可以直接用
            state.store_resolved(media.clone());

            let updated = state
                .mutate(&task_id, |t| {
                    t.provider_id = Some(provider_id.clone());
                    t.thumbnail_url = thumbnail.clone();
                    t.quality_label = Some(quality.clone());
                    t.container = Some(container.clone());
                    t.audio_label = audio_label.clone();
                    if estimated.is_some() {
                        t.total_bytes = estimated;
                    }
                    t.engine = engine;
                    t.source_stream_url = stream_url.clone();
                    t.source_audio_url = audio_url.clone();
                    t.referer = referer.clone();
                    t.duration_sec = duration_sec;
                    t.expected_sha256 = expected_sha256.clone();
                    t.status = TaskStatus::Queued;
                    t.error = None;
                })
                .unwrap_or(task);

            state.wake();
            state.pump();
            Ok(state.get_task(&task_id).unwrap_or(updated))
        }
        Err(err) => {
            let code = err.code.clone();
            let message = err.message.clone();
            let retryable = err.retryable;
            let detail = err.detail.clone();
            // 解析仍然失败：退回「需重新解析」，保留错误让用户决定下一步
            state.mutate(&task_id, |t| {
                t.status = TaskStatus::NeedsReparse;
                t.speed_bps = 0.0;
                t.eta_sec = None;
                t.error = Some(TaskError {
                    code: code.clone(),
                    message: message.clone(),
                    retryable,
                    detail: detail.clone(),
                });
            });
            Err(err)
        }
    }
}

/// 用于诊断：返回指定任务的即时控制状态
#[tauri::command]
pub fn task_control_state(
    state: State<'_, Arc<AppState>>,
    task_id: String,
) -> Option<String> {
    let token = state.control_token(&task_id)?;
    Some(match token.state() {
        ControlState::Run => "run",
        ControlState::Pause => "pause",
        ControlState::Cancel => "cancel",
    }
    .to_string())
}

/// 删除任务对应的视频文件（移入系统回收站），并把任务标记为「已删除」。
///
/// 记录会保留在列表里：用户需要看到「已完成 → 已删除」这个变化，才能确认文件确实
/// 处理掉了。删文件与改状态放在同一个命令里完成，避免前端分两步时出现
/// 「文件没了但状态还是已完成」的不一致。
#[tauri::command]
pub async fn delete_task_file(
    state: State<'_, Arc<AppState>>,
    task_id: String,
) -> AppResult<DownloadTask> {
    let task = state
        .get_task(&task_id)
        .ok_or_else(|| AppError::new("TASK_NOT_FOUND", "任务不存在", false))?;

    let path = task
        .target_path
        .clone()
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| AppError::storage("这条记录没有关联的本地文件"))?;

    crate::platform::delete_to_trash(&path)?;

    let updated = state
        .mutate(&task_id, |t| {
            t.status = TaskStatus::Deleted;
            t.speed_bps = 0.0;
            t.eta_sec = None;
            t.error = None;
            t.resume = None;
        })
        .unwrap_or(task);
    Ok(updated)
}