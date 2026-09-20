//! 媒体处理：无损 remux / 音视频合并 / HLS 封装 / 格式转换（转码）。
//!
//! 前几项一律 `-c copy` 不重新编码；只有 `transcode` 会真正转码，
//! 它的可选组合与限制见 `capabilities`。

use std::path::Path;
use std::process::Command;

use crate::downloader::StopReason;
use crate::error::{AppError, AppResult};
use crate::platform::sidecar;

pub mod capabilities;

fn run_ffmpeg(args: &[String]) -> AppResult<String> {
    let exe = sidecar::ffmpeg_path().ok_or_else(AppError::media_tool_missing)?;

    let mut cmd = Command::new(&exe);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = cmd
        .output()
        .map_err(|e| AppError::media_failed(format!("无法启动 FFmpeg：{e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .lines()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        return Err(AppError::media_failed("FFmpeg 执行失败").with_detail(tail));
    }

    Ok(String::from_utf8_lossy(&output.stderr).to_string())
}

/// 无损合并视频流与音频流到 mp4；失败不覆盖已有文件
pub fn merge_to_mp4(video: &Path, audio: &Path, out: &Path) -> AppResult<()> {
    if out.exists() {
        return Err(AppError::media_failed("目标文件已存在，拒绝覆盖"));
    }

    let args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-i".into(),
        video.to_string_lossy().to_string(),
        "-i".into(),
        audio.to_string_lossy().to_string(),
        // 无损：直接复制编码，不做任何转码
        "-c".into(),
        "copy".into(),
        "-map".into(),
        "0:v:0".into(),
        "-map".into(),
        "1:a:0".into(),
        "-movflags".into(),
        "+faststart".into(),
        out.to_string_lossy().to_string(),
    ];

    run_ffmpeg(&args)?;

    let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        let _ = std::fs::remove_file(out);
        return Err(AppError::media_failed("合并产物为空"));
    }
    Ok(())
}

/// 把任意容器无损重封装为 mp4（不重新编码）
pub fn remux_to_mp4(input: &Path, out: &Path) -> AppResult<()> {
    if out.exists() {
        return Err(AppError::media_failed("目标文件已存在，拒绝覆盖"));
    }

    let args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-i".into(),
        input.to_string_lossy().to_string(),
        "-c".into(),
        "copy".into(),
        "-movflags".into(),
        "+faststart".into(),
        out.to_string_lossy().to_string(),
    ];

    run_ffmpeg(&args)?;
    Ok(())
}

/// 把 HLS 清单拉下来并无损封装为 mp4。
///
/// 交给 FFmpeg 而不是自己下分片：AES-128 解密、`EXT-X-BYTERANGE`、fMP4 的 init
/// segment 它都原生处理，自己重写一遍只会引入偏差（离线环境也没有 AES 相关 crate）。
///
/// - `referer`：需要校验来源的源（部分 HLS 源要求）
/// - `duration_sec` + `total_bytes`：两者都有时按时间比例推进度；否则只报总时长
/// - `on_progress` 收到 (已下载字节估算, 总字节估算)
/// - `should_cancel` 返回 true 时杀掉 FFmpeg 进程
pub async fn hls_to_mp4(
    playlist_url: &str,
    referer: Option<&str>,
    duration_sec: Option<f64>,
    total_bytes: Option<u64>,
    out: &Path,
    on_progress: Box<dyn Fn(u64, Option<u64>) + Send + Sync + 'static>,
    should_cancel: Box<dyn Fn() -> bool + Send + Sync + 'static>,
) -> AppResult<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let exe = sidecar::ffmpeg_path().ok_or_else(AppError::media_tool_missing)?;

    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        // 用进度管道而不是解析人类可读输出，格式稳定
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        "-y".into(),
    ];
    if let Some(referer) = referer.map(str::trim).filter(|r| !r.is_empty()) {
        // 该参数是单个字符串，多个头之间用 \r\n 分隔
        args.push("-headers".into());
        args.push(format!("Referer: {referer}\r\n"));
    }
    args.extend([
        "-i".into(),
        playlist_url.to_string(),
        // 无损：直接复制编码，不转码（项目书 §1.3）
        "-c".into(),
        "copy".into(),
        "-movflags".into(),
        "+faststart".into(),
        // 临时文件是 `.part` 结尾，FFmpeg 推不出容器格式，必须显式指定
        "-f".into(),
        "mp4".into(),
        out.to_string_lossy().to_string(),
    ]);

    let mut cmd = tokio::process::Command::new(&exe);
    cmd.args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    #[cfg(windows)]
    {
        // tokio 的 Command 自带 creation_flags，无需引入 std 的扩展 trait
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::media_failed(format!("无法启动 FFmpeg：{e}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::media_failed("无法读取 FFmpeg 进度"))?;
    let mut lines = BufReader::new(stdout).lines();

    // FFmpeg 的 out_time_us 是已处理的媒体时间（微秒）。HLS 没有可用的
    // Content-Length，所以用「已处理时长 / 总时长」摊出已下载字节，纯展示用。
    let mut cancel_tick = 0u32;
    loop {
        // 取消检查要够密：卡在某个分片上时用户按取消得能立刻停
        cancel_tick = cancel_tick.wrapping_add(1);
        if cancel_tick % 8 == 0 && should_cancel() {
            let _ = child.start_kill();
            let _ = child.wait().await;
            let _ = std::fs::remove_file(out);
            return Err(AppError::cancelled());
        }

        match lines.next_line().await {
            Ok(Some(line)) => {
                let Some(value) = line.trim().strip_prefix("out_time_us=") else {
                    continue;
                };
                let Ok(micros) = value.trim().parse::<i64>() else {
                    continue;
                };
                if micros <= 0 {
                    continue;
                }
                let done = progress_bytes(micros, duration_sec, total_bytes);
                on_progress(done, total_bytes);
            }
            // 进度管道结束即 FFmpeg 收尾
            Ok(None) => break,
            Err(_) => break,
        }
    }

    // 必须收进程状态，否则失败会被当成成功
    let status = child
        .wait()
        .await
        .map_err(|e| AppError::media_failed(format!("FFmpeg 执行失败：{e}")))?;

    if !status.success() {
        let mut detail = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            use tokio::io::AsyncReadExt;
            let mut buf = String::new();
            let _ = stderr.read_to_string(&mut buf).await;
            detail = buf
                .lines()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
        }
        let _ = std::fs::remove_file(out);
        return Err(AppError::media_failed("HLS 下载失败").with_detail(detail));
    }

    let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        let _ = std::fs::remove_file(out);
        return Err(AppError::media_failed("HLS 产物为空"));
    }
    on_progress(size, Some(size));
    Ok(())
}

/// 用「已处理时长 / 总时长」把已下载字节摊出来（仅用于进度展示）。
///
/// 缺任一项就返回 0：宁可进度条不动，也不要给出编造的数字。
fn progress_bytes(out_time_us: i64, duration_sec: Option<f64>, total_bytes: Option<u64>) -> u64 {
    let total = match total_bytes.filter(|t| *t > 0) {
        Some(t) => t,
        None => return 0,
    };
    let duration = match duration_sec.filter(|d| *d > 0.0) {
        Some(d) => d,
        None => return 0,
    };
    let ratio = (out_time_us as f64 / 1_000_000.0 / duration).clamp(0.0, 1.0);
    (ratio * total as f64) as u64
}

/// 读取媒体时长（秒），用于校验与展示；失败返回 None
pub fn probe_duration(input: &Path) -> Option<f64> {
    let exe = sidecar::ffmpeg_path()?;
    let ffprobe = exe.with_file_name("ffprobe.exe");
    let probe = if ffprobe.exists() { ffprobe } else { exe };

    let mut cmd = Command::new(&probe);
    cmd.args([
        "-v",
        "error",
        "-show_entries",
        "format=duration",
        "-of",
        "default=noprint_wrappers=1:nokey=1",
        &input.to_string_lossy(),
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse::<f64>().ok()
}

/// ffprobe 单值输出 → 数字。
///
/// 实测会碰到三种输出：正常数字（`192000`）、**纯音频文件的 flac 流级 `N/A`**、
/// 以及失败时的空串。后两种都必须当成「读不到」而不是 0，
/// 否则界面上会出现「0 kbps 转 192 kbps 会提升音质」这种胡说。
fn parse_bitrate(raw: &str) -> Option<u64> {
    let text = raw.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("N/A") {
        return None;
    }
    text.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
        .map(|v| v as u64)
}

/// 用 `-show_entries` 取一个数字字段
fn probe_number(probe: &Path, args: &[&str], input: &Path) -> Option<u64> {
    let mut cmd = Command::new(probe);
    cmd.args(args);
    cmd.arg(input);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_bitrate(&String::from_utf8_lossy(&output.stdout))
}

/// 读音频码率（bps），读不到返回 None。
///
/// 两级回退，**缺一不可**（实测）：
/// 1. 流级 `stream=bit_rate`：mp3 / aac 这类能直接读到（192000）；
/// 2. **flac 的流级返回 `N/A`**，只能退到容器级 `format=bit_rate`（136193）。
///
/// 容器级码率是**整个文件**的，所以只在没有视频轨时才拿它兜底——
/// 否则视频文件会把视频码率当成音频码率报上去，质量提示直接失真。
pub fn probe_audio_bitrate(input: &Path, has_video: bool) -> Option<u64> {
    let exe = sidecar::ffmpeg_path()?;
    let ffprobe = exe.with_file_name("ffprobe.exe");
    let probe = if ffprobe.exists() { ffprobe } else { exe };

    let stream = probe_number(
        &probe,
        &[
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=bit_rate",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ],
        input,
    );
    if stream.is_some() {
        return stream;
    }
    if has_video {
        return None;
    }
    probe_number(
        &probe,
        &[
            "-v",
            "error",
            "-show_entries",
            "format=bit_rate",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ],
        input,
    )
}

/// 一条流的基本信息（探测用）
#[derive(Debug, Clone)]
pub struct StreamInfo {
    /// `video` / `audio` / `subtitle`
    pub kind: String,
    /// ffprobe 的 codec_name，如 h264 / hevc / aac
    pub codec: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// 读取文件的流构成（视频/音频编码与分辨率）。
///
/// 转换页要回显源文件的实际情况，用户才知道「保持原始分辨率」是什么、
/// 以及能不能提取音频。读不到时返回空列表，由界面按「未知」展示。
pub fn probe_streams(input: &Path) -> Vec<StreamInfo> {
    let Some(exe) = sidecar::ffmpeg_path() else {
        return Vec::new();
    };
    let ffprobe = exe.with_file_name("ffprobe.exe");
    let probe = if ffprobe.exists() { ffprobe } else { exe };

    let mut cmd = Command::new(&probe);
    cmd.args([
        "-v",
        "error",
        "-show_entries",
        "stream=codec_type,codec_name,width,height",
        "-of",
        "csv=p=0",
        &input.to_string_lossy(),
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let Ok(output) = cmd.output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_probe_line)
        .collect()
}

/// 解析 ffprobe `-of csv=p=0` 的一行。
///
/// **不能按下标取字段**：csv 输出用的是 ffprobe 自己的字段顺序，与
/// `-show_entries` 里写的顺序无关。实测：
/// - 视频流 → `h264,video,1280,720`（编码在前、类型在后）
/// - 音频流 → `aac,audio`（没有宽高，字段数都不一样）
///
/// 所以这里先定位类型字段，再在其余字段里挑编码与宽高。
/// 这条规则写错过一次（按 `type,codec,w,h` 取），导致源文件信息全部读成空，
/// 转换页的回显与「有没有音轨」的判断一起失灵。
fn parse_probe_line(line: &str) -> Option<StreamInfo> {
    let parts: Vec<&str> = line.trim().split(',').map(str::trim).collect();
    let kind_idx = parts
        .iter()
        .position(|p| matches!(*p, "video" | "audio" | "subtitle"))?;
    let kind = parts[kind_idx].to_string();

    // 编码名：类型字段之外、第一个非空且非纯数字的字段
    let codec = parts
        .iter()
        .enumerate()
        .find(|(i, p)| *i != kind_idx && !p.is_empty() && p.parse::<u32>().is_err())
        .map(|(_, p)| p.to_string())?;

    // 宽高：其余字段里的纯数字（视频流才有两个，音频流一个都没有）
    let nums: Vec<u32> = parts
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != kind_idx)
        .filter_map(|(_, p)| p.parse::<u32>().ok())
        .collect();

    Some(StreamInfo {
        kind,
        codec,
        width: nums.first().copied(),
        height: nums.get(1).copied(),
    })
}

pub fn ffmpeg_available() -> bool {
    sidecar::ffmpeg_path().is_some()
}

/// 转码一个本地文件，产出到 `out`。
///
/// 与 `hls_to_mp4` 同一套模式（tokio 子进程 + `-progress pipe:1` 解析 `out_time_us`），
/// 差别在两点：
///
/// 1. 输入是本地文件而不是网络清单，因此进度用「源文件大小 × 已处理时长占比」折算——
///    转码后的体积事前无法预知，用源文件大小作分母能给出有意义的百分比，
///    完成时再用真实产物大小收尾。
/// 2. `should_stop` 返回 `Option<StopReason>` 而不是 `bool`：转码不支持断点续传，
///    暂停与取消要做的事不同（暂停要留个「可重新开始」的任务，取消要清干净）。
pub async fn transcode(
    input: &Path,
    out: &Path,
    body: &[String],
    duration_sec: Option<f64>,
    estimated_total: Option<u64>,
    on_progress: Box<dyn Fn(u64, Option<u64>) + Send + Sync + 'static>,
    should_stop: Box<dyn Fn() -> Option<StopReason> + Send + Sync + 'static>,
) -> AppResult<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    if !input.is_file() {
        return Err(AppError::new(
            "MEDIA_SOURCE_MISSING",
            "源文件不存在或已被移动",
            false,
        )
        .with_hint("请重新选择文件后再试"));
    }

    let exe = sidecar::ffmpeg_path().ok_or_else(AppError::media_tool_missing)?;

    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        "-y".into(),
        "-i".into(),
        input.to_string_lossy().to_string(),
    ];
    args.extend(body.iter().cloned());

    let mut cmd = tokio::process::Command::new(&exe);
    cmd.args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    #[cfg(windows)]
    {
        // 不给这个标志会弹出一个黑色控制台窗口
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::media_failed(format!("无法启动 FFmpeg：{e}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::media_failed("无法读取 FFmpeg 进度"))?;
    let mut lines = BufReader::new(stdout).lines();

    let mut tick = 0u32;
    loop {
        // 检查密度要够：转码是长任务，用户按暂停后不能等太久
        tick = tick.wrapping_add(1);
        if tick % 8 == 0 {
            if let Some(reason) = should_stop() {
                let _ = child.start_kill();
                let _ = child.wait().await;
                let _ = std::fs::remove_file(out);
                return match reason {
                    StopReason::Pause => Err(paused_error()),
                    StopReason::Cancel => Err(AppError::cancelled()),
                };
            }
        }

        match lines.next_line().await {
            Ok(Some(line)) => {
                let Some(value) = line.trim().strip_prefix("out_time_us=") else {
                    continue;
                };
                let Ok(micros) = value.trim().parse::<i64>() else {
                    continue;
                };
                if micros <= 0 {
                    continue;
                }
                // 分母换成源文件大小：转码后的体积事前无法预知，
                // 用已处理时长占比摊出「看起来合理」的进度
                let done = ratio_bytes(micros, duration_sec, estimated_total);
                on_progress(done, estimated_total);
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| AppError::media_failed(format!("FFmpeg 执行失败：{e}")))?;

    if !status.success() {
        let mut detail = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            use tokio::io::AsyncReadExt;
            let mut buf = String::new();
            let _ = stderr.read_to_string(&mut buf).await;
            detail = buf
                .lines()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
        }
        let _ = std::fs::remove_file(out);
        return Err(AppError::media_failed("转码失败").with_detail(detail));
    }

    let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        let _ = std::fs::remove_file(out);
        return Err(AppError::media_failed("转码产物为空"));
    }
    on_progress(size, Some(size));
    Ok(())
}

/// 按「已处理时长占比」折算进度字节（转码用，分母是源文件大小）
fn ratio_bytes(out_time_us: i64, duration_sec: Option<f64>, total_bytes: Option<u64>) -> u64 {
    let total = match total_bytes.filter(|t| *t > 0) {
        Some(t) => t,
        None => return 0,
    };
    let duration = match duration_sec.filter(|d| *d > 0.0) {
        Some(d) => d,
        None => return 0,
    };
    let ratio = (out_time_us as f64 / 1_000_000.0 / duration).clamp(0.0, 1.0);
    (ratio * total as f64) as u64
}

/// 暂停：转码不能断点续传，调用方据此走「停下来、恢复时从头再来」的分支
pub fn paused_error() -> AppError {
    AppError::new("TASK_PAUSED", "已暂停", false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 探测输出解析不受字段顺序影响() {
        // 实测的真实输出：编码在前、类型在后，音频流还少两个字段
        let v = parse_probe_line("h264,video,1280,720").unwrap();
        assert_eq!(v.kind, "video");
        assert_eq!(v.codec, "h264");
        assert_eq!(v.width, Some(1280));
        assert_eq!(v.height, Some(720));

        let a = parse_probe_line("aac,audio").unwrap();
        assert_eq!(a.kind, "audio");
        assert_eq!(a.codec, "aac");
        assert_eq!(a.width, None);
        assert_eq!(a.height, None);

        // 纯音频文件里只有一条 audio 流
        assert_eq!(parse_probe_line("flac,audio").unwrap().codec, "flac");
        // 字幕流
        assert_eq!(parse_probe_line("subrip,subtitle").unwrap().kind, "subtitle");
    }

    #[test]
    fn 探测解析忽略无类型行() {
        // ffprobe 可能输出空行或纯数据行，不能让它们变成假流
        assert!(parse_probe_line("").is_none());
        assert!(parse_probe_line("   ").is_none());
        assert!(parse_probe_line("1280,720").is_none());
    }

    #[test]
    fn 高分辨率视频读出正确尺寸() {
        let s = parse_probe_line("hevc,video,3840,2160").unwrap();
        assert_eq!(s.codec, "hevc");
        assert_eq!(s.width, Some(3840));
        assert_eq!(s.height, Some(2160));
    }

    #[test]
    fn 码率解析认得_na_与空值() {
        // 实测：mp3 流级能读到数字，flac 流级是 N/A，失败时是空串
        assert_eq!(parse_bitrate("192000\n"), Some(192000));
        assert_eq!(parse_bitrate("136193.000000"), Some(136193));
        assert_eq!(parse_bitrate("N/A"), None);
        assert_eq!(parse_bitrate(" n/a "), None);
        assert_eq!(parse_bitrate(""), None);
        // 0 也要当成读不到：显示「0 kbps → 192 kbps」比不显示更糟
        assert_eq!(parse_bitrate("0"), None);
        assert_eq!(parse_bitrate("abc"), None);
    }
}