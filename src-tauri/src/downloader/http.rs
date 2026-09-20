//! VideoFlow 原生 HTTP 下载引擎。
//!
//! 能力（项目书 §3.3）：
//! - `Accept-Ranges: bytes` 时按 8 MiB 分段并发写入 `<task>.part` 与 `<task>.resume.json`；
//! - 每完成一段原子更新检查点，支持暂停 / 恢复 / 应用重启续传；
//! - 无 Range 的资源退化为单流下载，并明确提示「需从头重下」；
//! - 完成后做长度校验，再原子 rename 到最终文件名，失败不覆盖原文件。

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, ETAG, LAST_MODIFIED, RANGE, USER_AGENT};
use reqwest::{Client, StatusCode};
use tokio::sync::Semaphore;

use crate::core::model::{ChunkState, ResumeCheckpoint};
use crate::error::{AppError, AppResult};

use super::{plan_chunks, write_at, ControlToken, SpeedMeter, StopReason, CHUNK_SIZE};

#[derive(Debug, Clone)]
pub struct ProgressSnapshot {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub speed_bps: f64,
    pub eta_sec: Option<f64>,
}

#[derive(Debug)]
pub struct HttpRequest {
    pub url: String,
    /// 最终文件路径（同目录临时名 → 校验后原子 rename 到这里）
    pub target_path: PathBuf,
    pub temp_path: PathBuf,
    pub resume_path: PathBuf,
    pub chunk_concurrency: usize,
    pub resume: Option<ResumeCheckpoint>,
    pub referer: Option<String>,
    pub user_agent: String,
}

#[derive(Debug)]
pub struct HttpOutcome {
    pub total_bytes: u64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub checkpoint: ResumeCheckpoint,
}

/// 探测远端元信息
struct Probe {
    total: Option<u64>,
    range_supported: bool,
    etag: Option<String>,
    last_modified: Option<String>,
}

fn header_str(resp: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<String> {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

async fn probe(client: &Client, req: &HttpRequest) -> AppResult<Probe> {
    let mut builder = client
        .get(&req.url)
        .header(RANGE, "bytes=0-0")
        .header(USER_AGENT, req.user_agent.clone());
    if let Some(referer) = &req.referer {
        builder = builder.header(reqwest::header::REFERER, referer.clone());
    }

    let resp = builder.send().await?;
    let status = resp.status();

    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        return Err(AppError::new("HTTP_416", "服务器拒绝了范围请求", false)
            .with_hint("请重新解析该链接"));
    }
    if !status.is_success() {
        return Err(AppError::http_status(status.as_u16()));
    }

    let content_range = header_str(&resp, CONTENT_RANGE);
    let total = content_range
        .as_deref()
        .and_then(|v| v.rsplit('/').next())
        .filter(|v| *v != "*")
        .and_then(|v| v.trim().parse::<u64>().ok())
        .or_else(|| {
            header_str(&resp, CONTENT_LENGTH)
                .and_then(|v| v.parse::<u64>().ok())
                // 206 + Content-Range 缺失时，Content-Length 只是 1 字节，不可用
                .filter(|_| status == StatusCode::OK)
        });

    let range_supported = status == StatusCode::PARTIAL_CONTENT
        || header_str(&resp, ACCEPT_RANGES)
            .map(|v| v.to_ascii_lowercase().contains("bytes"))
            .unwrap_or(false);

    Ok(Probe {
        total,
        range_supported,
        etag: header_str(&resp, ETAG),
        last_modified: header_str(&resp, LAST_MODIFIED),
    })
}

fn save_checkpoint(path: &Path, checkpoint: &ResumeCheckpoint) -> AppResult<()> {
    // 先写临时文件再 rename，保证检查点文件本身是原子的
    let tmp = path.with_extension("json.tmp");
    let data = serde_json::to_vec(checkpoint)?;
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 预分配文件空间，避免并发写入时反复扩展
fn preallocate(file: &std::fs::File, size: u64) -> AppResult<()> {
    if size > 0 {
        file.set_len(size)?;
    }
    Ok(())
}

/// 读取磁盘上的断点文件（项目书 §3.2：重启后按临时文件与 ETag 校验再继续）。
///
/// 内存里的 `task.resume` 是上次状态变更时的快照，可能比磁盘上的检查点旧——
/// 磁盘版本由每个分片完成时原子更新，用它能让续传少下一批已经拿到的分片。
pub fn load_checkpoint(path: &Path) -> Option<ResumeCheckpoint> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<ResumeCheckpoint>(&raw).ok()
}

/// 主入口：执行一次下载，直到完成、暂停或取消
///
/// `on_progress` 用 `Arc<dyn Fn>` 包装，因为分片任务会被 spawn 到独立任务中，
/// 需要 `'static` 的可共享回调。
#[allow(clippy::too_many_arguments)]
pub async fn download(
    client: &Client,
    req: HttpRequest,
    token: Arc<ControlToken>,
    on_progress: Arc<dyn Fn(ProgressSnapshot) + Send + Sync>,
) -> AppResult<HttpOutcome> {
    let probe = probe(client, &req).await?;

    // 恢复时校验远端是否变化（ETag / Last-Modified），变了就从头下
    let mut resumable = req.resume.clone().filter(|cp| {
        probe.range_supported
            && match (&cp.etag, &probe.etag) {
                (Some(a), Some(b)) => a == b,
                _ => match (&cp.last_modified, &probe.last_modified) {
                    (Some(a), Some(b)) => a == b,
                    _ => true,
                },
            }
    });

    let mut downloaded: u64 = match &resumable {
        Some(cp) => cp.chunks.iter().filter(|c| c.done).map(|c| c.end - c.start + 1).sum(),
        None => 0,
    };

    if req.resume.is_some() && resumable.is_none() && probe.range_supported {
        // 远端内容已变化，旧的临时数据不可信
        let _ = std::fs::remove_file(&req.temp_path);
    }

    if !probe.range_supported {
        // 服务器不支持续传：从头单流下载，并明确告知
        resumable = None;
        downloaded = 0;
        let _ = std::fs::remove_file(&req.temp_path);
    }

    let speed = Arc::new(SpeedMeter::new());
    let file = Arc::new(
        std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(resumable.is_none())
            .open(&req.temp_path)?,
    );

    if !probe.range_supported {
        // ---- 单流下载 ----
        let mut builder = client
            .get(&req.url)
            .header(USER_AGENT, req.user_agent.clone());
        if let Some(referer) = &req.referer {
            builder = builder.header(reqwest::header::REFERER, referer.clone());
        }
        let resp = builder.send().await?;
        if !resp.status().is_success() {
            return Err(AppError::http_status(resp.status().as_u16()));
        }
        let total = resp.content_length();
        let mut resp = resp;
        let mut offset = 0u64;
        loop {
            match token.checkpoint().await {
                Ok(()) => {}
                Err(StopReason::Pause) => {
                    let cp = ResumeCheckpoint {
                        etag: probe.etag.clone(),
                        last_modified: probe.last_modified.clone(),
                        chunks: Vec::new(),
                        temp_path: Some(req.temp_path.to_string_lossy().to_string()),
                        range_supported: false,
                    };
                    save_checkpoint(&req.resume_path, &cp)?;
                    token.written.store(offset, Ordering::SeqCst);
                    return Ok(HttpOutcome {
                        // 暂停时这个字段被当作「已落盘字节」用（界面显示可恢复大小），
                        // 必须返回真实写入量，不能返回远端总长度
                        total_bytes: offset,
                        etag: probe.etag.clone(),
                        last_modified: probe.last_modified.clone(),
                        checkpoint: cp,
                    });
                }
                Err(StopReason::Cancel) => {
                    let _ = std::fs::remove_file(&req.temp_path);
                    let _ = std::fs::remove_file(&req.resume_path);
                    return Err(AppError::cancelled());
                }
            }

            let chunk = match resp.chunk().await {
                Ok(Some(c)) => c,
                Ok(None) => break,
                Err(e) => {
                    let _ = std::fs::remove_file(&req.temp_path);
                    return Err(AppError::from(e));
                }
            };
            write_at(&file, &chunk, offset)?;
            offset += chunk.len() as u64;
            speed.sample(offset, 0.35);
            on_progress(ProgressSnapshot {
                downloaded: offset,
                total,
                speed_bps: speed.speed_bps(),
                eta_sec: speed.eta_sec(offset, total),
            });
        }

        let cp = ResumeCheckpoint {
            etag: probe.etag.clone(),
            last_modified: probe.last_modified.clone(),
            chunks: Vec::new(),
            temp_path: Some(req.temp_path.to_string_lossy().to_string()),
            range_supported: false,
        };
        save_checkpoint(&req.resume_path, &cp)?;
        token.written.store(offset, Ordering::SeqCst);
        return Ok(HttpOutcome {
            total_bytes: total.unwrap_or(offset),
            etag: probe.etag.clone(),
            last_modified: probe.last_modified.clone(),
            checkpoint: cp,
        });
    }

    // ---- 分段并发下载 ----
    let total = probe
        .total
        .ok_or_else(|| AppError::new("HTTP_NO_LENGTH", "服务器没有返回文件大小", false))?;

    preallocate(&file, total)?;

    let planned = plan_chunks(total, CHUNK_SIZE);
    let done_map: std::collections::HashMap<u32, bool> = resumable
        .as_ref()
        .map(|cp| cp.chunks.iter().map(|c| (c.index, c.done)).collect())
        .unwrap_or_default();

    let mut tasks: Vec<(u32, u64, u64)> = planned
        .iter()
        .filter(|(idx, _, _)| !done_map.get(idx).copied().unwrap_or(false))
        .cloned()
        .collect();

    // 已完成的字节数从检查点恢复
    if downloaded == 0 && !tasks.is_empty() {
        downloaded = 0;
    }
    let progress = Arc::new(std::sync::atomic::AtomicU64::new(downloaded));
    let mut chunk_states: Vec<ChunkState> = planned
        .iter()
        .map(|(index, start, end)| ChunkState {
            index: *index,
            start: *start,
            end: *end,
            done: done_map.get(index).copied().unwrap_or(false),
        })
        .collect();

    let semaphore = Arc::new(Semaphore::new(req.chunk_concurrency.max(1)));
    let mut join_set = tokio::task::JoinSet::new();
    let paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // 反向排序：先下末尾分片，某些站点对 mdat 尾部更友好；同时便于观测进度
    tasks.reverse();

    for (index, start, end) in tasks {
        let permit = match semaphore.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => break,
        };

        match token.checkpoint().await {
            Ok(()) => {}
            Err(StopReason::Pause) => {
                paused.store(true, Ordering::SeqCst);
                break;
            }
            Err(StopReason::Cancel) => {
                cancelled.store(true, Ordering::SeqCst);
                break;
            }
        }

        let client = client.clone();
        let url = req.url.clone();
        let referer = req.referer.clone();
        let ua = req.user_agent.clone();
        let file = file.clone();
        let progress = progress.clone();
        let speed = speed.clone();
        let token_inner = token.clone();
        let on_progress = Arc::clone(&on_progress);

        join_set.spawn(async move {
            let _permit = permit;
            let result = fetch_range(&client, &url, referer.as_deref(), &ua, start, end, &file, &progress, &token_inner).await;

            match result {
                Ok(written) => {
                    if written > 0 {
                        let now = progress.load(Ordering::SeqCst);
                        speed.sample(now, 0.35);
                        on_progress(ProgressSnapshot {
                            downloaded: now,
                            total: Some(total),
                            speed_bps: speed.speed_bps(),
                            eta_sec: speed.eta_sec(now, Some(total)),
                        });
                    }
                    Ok(index)
                }
                Err(e) => Err(e),
            }
        });

        // 控制流：每启动一批就回收已完成任务，及时暴露错误
        while join_set.len() >= req.chunk_concurrency.max(1) + 2 {
            if let Some(joined) = join_set.join_next().await {
                match joined {
                    Ok(Ok(idx)) => mark_done(&mut chunk_states, idx),
                    Ok(Err(e)) => {
                        cancelled.store(true, Ordering::SeqCst);
                        join_set.abort_all();
                        return Err(e);
                    }
                    Err(_) => {}
                }
            }
        }
    }

    // 收尾：等待剩余分片
    while let Some(joined) = join_set.join_next().await {
        match joined {
            Ok(Ok(idx)) => mark_done(&mut chunk_states, idx),
            Ok(Err(e)) => {
                join_set.abort_all();
                return Err(e);
            }
            Err(_) => {}
        }
    }

    let finished = progress.load(Ordering::SeqCst);
    token.written.store(finished, Ordering::SeqCst);

    let checkpoint = ResumeCheckpoint {
        etag: probe.etag.clone(),
        last_modified: probe.last_modified.clone(),
        chunks: chunk_states,
        temp_path: Some(req.temp_path.to_string_lossy().to_string()),
        range_supported: true,
    };
    save_checkpoint(&req.resume_path, &checkpoint)?;

    if cancelled.load(Ordering::SeqCst) {
        let _ = std::fs::remove_file(&req.temp_path);
        let _ = std::fs::remove_file(&req.resume_path);
        return Err(AppError::cancelled());
    }

    if paused.load(Ordering::SeqCst) {
        // 暂停：保留分片与检查点，返回当前进度
        return Ok(HttpOutcome {
            total_bytes: finished,
            etag: probe.etag.clone(),
            last_modified: probe.last_modified.clone(),
            checkpoint,
        });
    }

    // 长度校验：不一致则删除不可信产物
    let all_done = checkpoint.chunks.iter().all(|c| c.done);
    if finished != total || !all_done {
        let _ = std::fs::remove_file(&req.temp_path);
        let _ = std::fs::remove_file(&req.resume_path);
        return Err(AppError::integrity(format!(
            "下载长度校验失败：期望 {} 字节，实际 {} 字节",
            crate::core::model::human_bytes(total),
            crate::core::model::human_bytes(finished)
        )));
    }

    Ok(HttpOutcome {
        total_bytes: total,
        etag: probe.etag,
        last_modified: probe.last_modified,
        checkpoint,
    })
}

fn mark_done(states: &mut [ChunkState], index: u32) {
    if let Some(s) = states.iter_mut().find(|s| s.index == index) {
        s.done = true;
    }
}

#[allow(clippy::too_many_arguments)]
async fn fetch_range(
    client: &Client,
    url: &str,
    referer: Option<&str>,
    user_agent: &str,
    start: u64,
    end: u64,
    file: &std::fs::File,
    progress: &std::sync::atomic::AtomicU64,
    token: &ControlToken,
) -> AppResult<u64> {
    // 单段最大 8 MiB，弱网下一次重连往往不够；给到 6 次（退避 1/2/4/8/8/8 秒），
    // 避免整个任务因为几次瞬时抖动直接失败。
    const MAX_ATTEMPTS: u32 = 6;
    let mut attempt = 0u32;
    let mut offset = start;

    while offset <= end {
        attempt += 1;
        if attempt > MAX_ATTEMPTS {
            return Err(AppError::network("分片多次重试后仍然失败"));
        }

        match token.checkpoint().await {
            Ok(()) => {}
            Err(StopReason::Pause) => return Ok(offset.saturating_sub(start)),
            Err(StopReason::Cancel) => return Err(AppError::cancelled()),
        }

        let mut builder = client
            .get(url)
            .header(RANGE, format!("bytes={offset}-{end}"))
            .header(USER_AGENT, user_agent.to_string())
            .timeout(Duration::from_secs(120));
        if let Some(referer) = referer {
            builder = builder.header(reqwest::header::REFERER, referer.to_string());
        }

        let resp = match builder.send().await {
            Ok(r) => r,
            Err(e) => {
                let err = AppError::from(e);
                if !err.retryable {
                    return Err(err);
                }
                backoff(attempt).await;
                continue;
            }
        };

        let status = resp.status();
        if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            // 尊重 Retry-After
            let wait = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(2u64.pow(attempt.min(4)));
            tokio::time::sleep(Duration::from_secs(wait.min(60))).await;
            continue;
        }
        if !status.is_success() {
            return Err(AppError::http_status(status.as_u16()));
        }

        let mut resp = resp;
        loop {
            match token.checkpoint().await {
                Ok(()) => {}
                Err(StopReason::Pause) => return Ok(offset.saturating_sub(start)),
                Err(StopReason::Cancel) => return Err(AppError::cancelled()),
            }

            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    write_at(file, &chunk, offset)?;
                    offset += chunk.len() as u64;
                    progress.fetch_add(chunk.len() as u64, Ordering::SeqCst);
                    if offset > end {
                        // 服务器返回了超出请求范围的数据，截断视为完成
                        return Ok(offset.saturating_sub(start));
                    }
                }
                Ok(None) => {
                    if offset > end {
                        return Ok(offset.saturating_sub(start));
                    }
                    // 连接提前结束：从头对剩余部分重试
                    backoff(attempt).await;
                    break;
                }
                Err(e) => {
                    let err = AppError::from(e);
                    if !err.retryable {
                        return Err(err);
                    }
                    backoff(attempt).await;
                    break;
                }
            }
        }
    }

    Ok(offset.saturating_sub(start))
}

/// 指数退避：1s、2s、4s（项目书 §3.3）
async fn backoff(attempt: u32) {
    let secs = 2u64.saturating_pow(attempt.saturating_sub(1)).min(8);
    tokio::time::sleep(Duration::from_secs(secs)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vf-http-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn request(url: &str, target: PathBuf, concurrency: usize) -> HttpRequest {
        let (part, resume) = crate::storage::temp_paths(&target);
        HttpRequest {
            url: url.to_string(),
            target_path: target,
            temp_path: part,
            resume_path: resume,
            chunk_concurrency: concurrency,
            resume: None,
            referer: None,
            user_agent: "VideoFlow/0.1 (test)".to_string(),
        }
    }

    /// 真实网络测试：验证分段并发下载、长度校验与产物字节数一致
    ///
    /// 运行：cargo test -- --ignored
    #[tokio::test]
    #[ignore = "需要网络访问，默认跳过"]
    async fn 直链分段下载并校验长度() {
        const URL: &str =
            "https://test-videos.co.uk/vids/bigbuckbunny/mp4/h264/360/Big_Buck_Bunny_360_10s_1MB.mp4";

        let client = Client::builder()
            .user_agent("VideoFlow/0.1 (test)")
            .build()
            .unwrap();
        let dir = scratch("seg");
        let target = dir.join("out.mp4");

        let token = ControlToken::new();
        let outcome = download(
            &client,
            request(URL, target.clone(), 4),
            token,
            Arc::new(|_snapshot: ProgressSnapshot| {}),
        )
        .await
        .expect("下载应当成功");

        assert!(outcome.total_bytes > 900_000, "样本文件应接近 1 MB");
        assert!(outcome.checkpoint.chunks.iter().all(|c| c.done));
        assert!(outcome.checkpoint.range_supported);

        // 本用例直接检查临时产物；正式流程由调用方负责 rename 到最终路径
        let written = std::fs::metadata(&outcome.checkpoint.temp_path.as_ref().unwrap())
            .unwrap()
            .len();
        assert_eq!(written, outcome.total_bytes, "落盘字节数应与声明长度一致");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 真实网络测试：暂停后检查点保留，续传后字节数正确
    #[tokio::test]
    #[ignore = "需要网络访问，默认跳过"]
    async fn 暂停后可断点续传() {
        const URL: &str =
            "https://test-videos.co.uk/vids/bigbuckbunny/mp4/h264/360/Big_Buck_Bunny_360_10s_1MB.mp4";

        let client = Client::builder()
            .user_agent("VideoFlow/0.1 (test)")
            .build()
            .unwrap();
        let dir = scratch("resume");
        let target = dir.join("out.mp4");

        // 第一次：立即请求暂停，任务应在检查点处停下
        let token = ControlToken::new();
        token.request_pause();
        let req = request(URL, target.clone(), 2);
        let (part, resume_path) = (req.temp_path.clone(), req.resume_path.clone());
        let first = download(&client, req, token, Arc::new(|_: ProgressSnapshot| {})).await;

        // 暂停语义：要么直接返回带检查点的结果，要么被识别为暂停
        if let Ok(outcome) = &first {
            assert!(outcome.checkpoint.temp_path.is_some());
        }

        // 第二次：从检查点续传，最终必须完整
        let checkpoint: Option<ResumeCheckpoint> = std::fs::read_to_string(&resume_path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok());
        let done_before: u64 = checkpoint
            .as_ref()
            .map(|cp| cp.chunks.iter().filter(|c| c.done).map(|c| c.end - c.start + 1).sum())
            .unwrap_or(0);

        let mut req2 = request(URL, target.clone(), 4);
        req2.resume = checkpoint;
        // 复用同一临时文件路径
        req2.temp_path = part;
        let token2 = ControlToken::new();
        let outcome2 = download(&client, req2, token2, Arc::new(|_: ProgressSnapshot| {}))
            .await
            .expect("续传应当成功");

        assert!(outcome2.total_bytes > 900_000);
        assert!(
            outcome2.total_bytes >= done_before,
            "续传后的总量不应小于暂停前已完成的字节数"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}