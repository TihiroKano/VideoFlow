//! 下载引擎。
//!
//! 双引擎（按产品决定）：
//! - `http`：VideoFlow 原生 HTTP 引擎，负责直链高速下载、分段并发、断点续传；
//! - `ytdlp`：站点授权流，交给 yt-dlp 下载并转发其进度。

pub mod http;
pub mod ytdlp;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

/// 用户对任务的即时控制意图
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlState {
    Run = 0,
    Pause = 1,
    Cancel = 2,
}

/// 检查点返回的停止原因：只会是暂停或取消，Run 不会出现在这里
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Pause,
    Cancel,
}

/// 跨任务的控制令牌：暂停 / 取消为即时乐观反馈，引擎在下一次检查点响应
#[derive(Debug, Default)]
pub struct ControlToken {
    state: AtomicU8,
    notify: Notify,
    /// 已写入磁盘的字节数（用于暂停时落盘检查点）
    pub written: AtomicU64,
}

impl ControlToken {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: AtomicU8::new(ControlState::Run as u8),
            notify: Notify::new(),
            written: AtomicU64::new(0),
        })
    }

    pub fn state(&self) -> ControlState {
        match self.state.load(Ordering::SeqCst) {
            1 => ControlState::Pause,
            2 => ControlState::Cancel,
            _ => ControlState::Run,
        }
    }

    pub fn request_pause(&self) {
        self.state.store(ControlState::Pause as u8, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn request_cancel(&self) {
        self.state.store(ControlState::Cancel as u8, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// 等待被允许继续；返回 Err 表示需要停下（暂停或取消）
    pub async fn checkpoint(&self) -> Result<(), StopReason> {
        match self.state() {
            ControlState::Run => Ok(()),
            ControlState::Pause => Err(StopReason::Pause),
            ControlState::Cancel => Err(StopReason::Cancel),
        }
    }
}

/// 每秒最多推送 4 次进度事件（项目书 §3.4）
pub const PROGRESS_MIN_INTERVAL_MS: u64 = 250;

/// 分片大小：8 MiB，符合项目书 §3.3 的 8–32 MiB 区间
pub const CHUNK_SIZE: u64 = 8 * 1024 * 1024;

/// 计算分片边界
pub fn plan_chunks(total: u64, chunk_size: u64) -> Vec<(u32, u64, u64)> {
    let mut chunks = Vec::new();
    if total == 0 {
        return chunks;
    }
    let mut index = 0u32;
    let mut start = 0u64;
    while start < total {
        let end = (start + chunk_size - 1).min(total - 1);
        chunks.push((index, start, end));
        index += 1;
        start = end + 1;
    }
    chunks
}

/// 文件写入辅助：多线程在偏移处并发写入
pub fn write_at(file: &std::fs::File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut written = 0usize;
        while written < buf.len() {
            let n = file.seek_write(&buf[written..], offset + written as u64)?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "写入返回 0 字节",
                ));
            }
            written += n;
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::FileExt;
        file.write_all_at(buf, offset)
    }
}

/// 把已完成的字节数映射为百分比（未知总量时为 None）
pub fn percent_of(downloaded: u64, total: Option<u64>) -> Option<u8> {
    let total = total?;
    if total == 0 {
        return None;
    }
    Some(((downloaded.min(total) * 100) / total) as u8)
}

/// 简易 5 秒 EWMA 速度估计（项目书 §3.4）
pub struct SpeedMeter {
    last_bytes: AtomicU64,
    ewma: AtomicU64, // f64 位模式存储
    last_tick_ms: AtomicU64,
}

impl Default for SpeedMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl SpeedMeter {
    pub fn new() -> Self {
        Self {
            last_bytes: AtomicU64::new(0),
            ewma: AtomicU64::new(0f64.to_bits()),
            last_tick_ms: AtomicU64::new(now_ms()),
        }
    }

    pub fn speed_bps(&self) -> f64 {
        f64::from_bits(self.ewma.load(Ordering::Relaxed))
    }

    pub fn eta_sec(&self, downloaded: u64, total: Option<u64>) -> Option<f64> {
        let total = total?;
        let speed = self.speed_bps();
        if speed <= 1.0 || total <= downloaded {
            return None;
        }
        Some((total - downloaded) as f64 / speed)
    }

    /// 采样一次；alpha 越小越平滑
    pub fn sample(&self, bytes: u64, alpha: f64) {
        let now = now_ms();
        let last_tick = self.last_tick_ms.swap(now, Ordering::Relaxed);
        let elapsed_ms = now.saturating_sub(last_tick);
        if elapsed_ms < 100 {
            return;
        }
        let last = self.last_bytes.swap(bytes, Ordering::Relaxed);
        let delta = bytes.saturating_sub(last) as f64;
        let instant = delta * 1000.0 / elapsed_ms as f64;
        let prev = self.speed_bps();
        let next = if prev <= 0.0 {
            instant
        } else {
            alpha * instant + (1.0 - alpha) * prev
        };
        self.ewma.store(next.to_bits(), Ordering::Relaxed);
    }

    pub fn reset(&self) {
        self.ewma.store(0f64.to_bits(), Ordering::Relaxed);
        self.last_bytes.store(0, Ordering::Relaxed);
        self.last_tick_ms.store(now_ms(), Ordering::Relaxed);
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 任务是否被用户暂停过（用于「用户暂停不自动恢复」的判定）
#[derive(Debug, Default)]
pub struct UserPauseFlag(pub AtomicBool);

impl UserPauseFlag {
    pub fn set(&self, v: bool) {
        self.0.store(v, Ordering::SeqCst);
    }
    pub fn get(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 分片规划覆盖完整区间且无缝无叠() {
        let chunks = plan_chunks(20, 8);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], (0, 0, 7));
        assert_eq!(chunks[1], (1, 8, 15));
        assert_eq!(chunks[2], (2, 16, 19));
        // 覆盖的总字节数等于文件大小
        let covered: u64 = chunks.iter().map(|(_, s, e)| e - s + 1).sum();
        assert_eq!(covered, 20);
    }

    #[test]
    fn 分片规划处理边界情况() {
        assert!(plan_chunks(0, 8).is_empty());
        let single = plan_chunks(8, 8);
        assert_eq!(single, vec![(0, 0, 7)]);
        let exact = plan_chunks(16, 8);
        assert_eq!(exact.len(), 2);
        assert_eq!(exact[1], (1, 8, 15));
    }

    #[test]
    fn 分片大小在项目书约定区间内() {
        let chunk = CHUNK_SIZE;
        assert!(chunk >= 8 * 1024 * 1024, "分片不应小于 8 MiB");
        assert!(chunk <= 32 * 1024 * 1024, "分片不应大于 32 MiB");
    }

    #[test]
    fn 百分比计算在未知总量时返回_none() {
        assert_eq!(percent_of(50, None), None);
        assert_eq!(percent_of(50, Some(0)), None);
        assert_eq!(percent_of(0, Some(100)), Some(0));
        assert_eq!(percent_of(50, Some(100)), Some(50));
        // 超出总量时截断到 100
        assert_eq!(percent_of(200, Some(100)), Some(100));
    }

    #[test]
    fn 控制令牌默认运行且可暂停取消() {
        let token = ControlToken::new();
        assert_eq!(token.state(), ControlState::Run);
        token.request_pause();
        assert_eq!(token.state(), ControlState::Pause);
        token.request_cancel();
        assert_eq!(token.state(), ControlState::Cancel);
    }

    #[test]
    fn 速度表在样本不足时不报错() {
        let meter = SpeedMeter::new();
        assert_eq!(meter.speed_bps(), 0.0);
        assert_eq!(meter.eta_sec(10, None), None);
        // 速度为 0 时不展示 ETA
        assert_eq!(meter.eta_sec(10, Some(100)), None);
    }
}