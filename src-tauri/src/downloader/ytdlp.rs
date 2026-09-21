//! 站点授权流下载：交给 yt-dlp 执行，并把它的人类可读进度转换为结构化事件。
//!
//! 参数全部固定化，禁止把 URL 或文件名拼接成 shell 命令（项目书 §7.3）。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::core::model::ResumeCheckpoint;
use crate::error::{AppError, AppResult};
use crate::platform::sidecar;
use crate::resolver::ytdlp::YtDlpProxy;

use super::http::ProgressSnapshot;
use super::{ControlState, ControlToken, SpeedMeter};

#[derive(Debug)]
pub struct YtDlpRequest {
    pub page_url: String,
    /// 形如 `137+140` 的格式选择器
    pub format_selector: String,
    /// 临时目录，yt-dlp 在其中产出分片与最终文件
    pub scratch_dir: PathBuf,
    /// 文件名前缀（任务 id），用于事后定位产物
    pub stem: String,
    /// 最终落盘路径
    pub target_path: PathBuf,
    pub referer: Option<String>,
    /// 登录态来源（`Settings::cookies_source`）；站点要求登录时必须提供
    pub cookies_source: Option<String>,
    /// 出口代理：与解析链路用同一份，保证「解析走哪条、下载就走哪条」
    pub proxy: YtDlpProxy,
}

#[derive(Debug)]
pub struct YtDlpOutcome {
    pub downloaded_bytes: u64,
    /// yt-dlp 实际产出的文件
    pub produced: PathBuf,
}

fn build_args(req: &YtDlpRequest) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--no-playlist".into(),
        "--no-warnings".into(),
        "--newline".into(),
        "--no-mtime".into(),
        "--retries".into(),
        "5".into(),
        "--fragment-retries".into(),
        "5".into(),
        "--concurrent-fragments".into(),
        "4".into(),
        // 机器可读进度：VF <格式id> <本段已下载> <本段总量> <本段估算总量> <速度> <剩余秒>
        // 带上格式 id 才能识别「yt-dlp 已经开始下一段流」，进而把总量累加，
        // 否则第二个流的 downloaded 会从 0 重新计数、进度条直接倒退。
        "--progress-template".into(),
        "download:VF %(info.format_id)s %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s %(progress.speed)s %(progress.eta)s".into(),
        "-f".into(),
        req.format_selector.clone(),
        "--merge-output-format".into(),
        "mp4".into(),
    ];

    if let Some(ffmpeg) = sidecar::ffmpeg_path() {
        if let Some(dir) = ffmpeg.parent() {
            args.push("--ffmpeg-location".into());
            args.push(dir.to_string_lossy().to_string());
        }
    }

    if let Some(referer) = &req.referer {
        args.push("--referer".into());
        args.push(referer.clone());
    }

    // 复用登录态：抖音必须登录才能取到播放地址，
    // Bilibili 的高码率/60 帧同样只对已登录账号开放
    crate::resolver::ytdlp::append_cookie_args(&mut args, req.cookies_source.as_deref());

    // 出口：与解析链路同一个代理（直连时不传参数并清环境变量）
    req.proxy.append_args(&mut args);

    let template = req
        .scratch_dir
        .join(format!("{}.%(ext)s", req.stem))
        .to_string_lossy()
        .to_string();
    args.push("-o".into());
    args.push(template);
    args.push(req.page_url.clone());
    args
}

/// 从一行进度里解析出的「当前流」信息
struct StreamProgress {
    format_id: String,
    downloaded: u64,
    total: Option<u64>,
    speed_bps: f64,
    eta_sec: Option<f64>,
}

/// 解析一行进度输出
fn parse_progress(line: &str) -> Option<StreamProgress> {
    let rest = line.strip_prefix("VF ")?;
    let mut parts = rest.split_whitespace();
    let format_id = parts.next()?.to_string();
    let downloaded = parse_num(parts.next()?)?;
    let total = parse_num(parts.next().unwrap_or("NA"));
    let estimate = parse_num(parts.next().unwrap_or("NA"));
    let speed_bps = parse_num(parts.next().unwrap_or("NA")).unwrap_or(0) as f64;
    let eta_sec = parse_num(parts.next().unwrap_or("NA")).map(|v| v as f64);
    Some(StreamProgress {
        format_id,
        downloaded,
        total: total.or(estimate).filter(|v| *v > 0),
        speed_bps,
        eta_sec,
    })
}

fn parse_num(raw: &str) -> Option<u64> {
    let t = raw.trim();
    if t.is_empty() || t == "NA" || t == "None" {
        return None;
    }
    t.split('.').next()?.parse::<u64>().ok()
}

fn classify_failure(stderr: &str) -> AppError {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("requested format is not available") {
        return AppError::provider_expired()
            .with_detail("解析结果中的格式已失效，请重新解析链接");
    }
    // 登录态问题优先归类：有明确的解决办法，不该被压成笼统的失败
    if lower.contains("fresh cookies")
        || lower.contains("cookies")
        || lower.contains("sign in")
        || lower.contains("login required")
    {
        return crate::resolver::ytdlp::cookies_hint();
    }
    if lower.contains("drm") || lower.contains("members-only") {
        return AppError::provider_restricted();
    }
    if lower.contains("http error 403") || lower.contains("http error 404") {
        return AppError::new("HTTP_403", "资源地址已失效", true)
            .with_hint("请重新解析该链接");
    }
    AppError::new("PROVIDER_ERROR", "站点流下载失败", true).with_detail(stderr.trim().to_string())
}

pub async fn download(
    req: &YtDlpRequest,
    token: Arc<ControlToken>,
    on_progress: Arc<dyn Fn(ProgressSnapshot) + Send + Sync>,
) -> AppResult<YtDlpOutcome> {
    let exe = sidecar::yt_dlp_path().ok_or_else(AppError::provider_tool_missing)?;
    std::fs::create_dir_all(&req.scratch_dir)?;

    let args = build_args(req);

    let mut cmd = Command::new(&exe);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // 直连模式：清掉子进程环境里的代理变量，避免「名义直连、实际走代理」
    req.proxy.apply_env(&mut cmd);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::provider_tool_missing().with_detail(format!("启动 yt-dlp 失败：{e}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::internal("无法读取 yt-dlp 输出"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::internal("无法读取 yt-dlp 错误输出"))?;

    // stdout 交给独立任务读取，再用 channel 送回来。
    // 不能把 next_line() 直接放进 select!：它会和 400ms 的暂停轮询竞争，
    // 一旦 sleep 先完成，next_line 的 future 就被丢弃，而它内部已经读进缓冲区、
    // 还没遇到换行的字节会一并丢失，进度行就此残缺或消失——表现就是
    // 「下载明明在跑，界面却卡住不动」。
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(512);
    let reader_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send(line).await.is_err() {
                break;
            }
        }
    });

    // stderr 必须一直读到 EOF：中途停止读取会让子进程写满管道缓冲区后
    // 永久阻塞在写 stderr 上，整个下载随之冻结。缓冲区只保留前 16 KB 供报错。
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if buf.len() < 16 * 1024 {
                buf.push_str(&line);
                buf.push('\n');
            }
        }
        buf
    });

    let speed = SpeedMeter::new();
    // `-f video+audio` 是两段串行下载，第二段的 downloaded 从 0 重新开始，
    // 所以要把上一段的总量作为基数累加，否则进度条会倒退到 0。
    let mut base_bytes = 0u64;
    let mut current_format = String::new();
    let mut current_total = 0u64;
    let mut total_seen = 0u64;
    let mut last_downloaded = 0u64;
    let mut interrupted: Option<ControlState> = None;

    loop {
        tokio::select! {
            line = rx.recv() => {
                let Some(text) = line else { break };
                let Some(p) = parse_progress(&text) else { continue };

                if p.format_id != current_format {
                    // 换到下一段流：把上一段的总量并进基数
                    base_bytes += current_total;
                    current_format = p.format_id;
                    current_total = 0;
                }
                if let Some(t) = p.total {
                    current_total = t;
                }

                let downloaded = base_bytes + p.downloaded;
                // 总量只增不减，避免第二段流刚开始时进度条从 100% 大幅回落
                total_seen = total_seen.max(base_bytes + current_total);
                let total = if total_seen > 0 { Some(total_seen) } else { None };

                last_downloaded = downloaded;
                if p.speed_bps > 0.0 {
                    speed.sample(downloaded, 0.35);
                }
                let speed_bps = if p.speed_bps > 0.0 { p.speed_bps } else { speed.speed_bps() };
                on_progress(ProgressSnapshot {
                    downloaded,
                    total,
                    speed_bps,
                    eta_sec: p.eta_sec.or_else(|| speed.eta_sec(downloaded, total)),
                });
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(400)) => {
                match token.state() {
                    ControlState::Run => {}
                    other => {
                        interrupted = Some(other);
                        let _ = child.start_kill();
                        break;
                    }
                }
            }
        }
    }
    // 子进程退出后 stdout 会 EOF，读取任务自然结束；中断路径下显式收尾
    reader_task.abort();

    let status = child.wait().await;
    let stderr_text = stderr_task.await.unwrap_or_default();

    if let Some(state) = interrupted {
        if state == ControlState::Cancel {
            let _ = std::fs::remove_dir_all(&req.scratch_dir);
            return Err(AppError::cancelled());
        }
        return Ok(YtDlpOutcome {
            downloaded_bytes: last_downloaded,
            produced: PathBuf::new(),
        });
    }

    match status {
        Ok(s) if s.success() => {}
        _ => return Err(classify_failure(&stderr_text)),
    }

    // 定位实际产物
    let produced = find_produced(&req.scratch_dir, &req.stem)
        .ok_or_else(|| AppError::media_failed("yt-dlp 没有产出可用的文件"))?;

    let size = std::fs::metadata(&produced).map(|m| m.len()).unwrap_or(last_downloaded);

    Ok(YtDlpOutcome {
        downloaded_bytes: size,
        produced,
    })
}

/// 在临时目录中查找本次任务的产物（排除 .part 与中间分片）
fn find_produced(dir: &Path, stem: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut candidates: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            name.starts_with(stem)
                && !name.ends_with(".part")
                && !name.ends_with(".ytdl")
                && !name.ends_with(".json")
        })
        .collect();

    candidates.sort_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0));
    candidates.pop()
}

/// 把产物移动到最终路径（同卷用 rename，跨卷退化为复制后删除）
pub fn finalize(produced: &Path, target: &Path) -> AppResult<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::rename(produced, target) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(produced, target)?;
            std::fs::remove_file(produced)?;
            Ok(())
        }
    }
}

/// yt-dlp 场景下的检查点：分片与断点由 yt-dlp 自己在临时目录中维护，
/// 这里只记录临时目录位置，供恢复时继续使用。
pub fn checkpoint_for(scratch: &Path) -> ResumeCheckpoint {
    ResumeCheckpoint {
        etag: None,
        last_modified: None,
        chunks: Vec::new(),
        temp_path: Some(scratch.to_string_lossy().to_string()),
        range_supported: true,
    }
}