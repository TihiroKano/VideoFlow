//! 外部工具（sidecar）定位与版本探测。
//!
//! 全部使用固定参数数组调用，禁止把 URL 或文件名拼接成 shell 命令（项目书 §7.3）。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 工具查找顺序：
/// 1. 可执行文件同级的 `bin/`（打包后的形态）
/// 2. 源码树中的 `bin/`（开发形态，编译期路径）
/// 3. 当前工作目录下的 `bin/`
pub fn bin_dir_candidates() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("bin"));
            // target/debug/videoflow.exe → 项目根
            dirs.push(dir.join("../../../bin"));
        }
    }

    // 开发形态：CARGO_MANIFEST_DIR 指向 src-tauri
    dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../bin"));

    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("bin"));
    }

    dirs
}

/// 定位某个工具，返回第一个存在的绝对路径
pub fn locate(name: &str) -> Option<PathBuf> {
    for dir in bin_dir_candidates() {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return std::fs::canonicalize(&candidate).ok().or(Some(candidate));
        }
    }
    None
}

/// 读取工具版本首行；失败时返回 None 而不是报错，硬件/环境不可得不是错误
pub fn version_of(path: &Path, args: &[&str]) -> Option<String> {
    // ffmpeg / yt-dlp 是控制台子系统程序：不加 CREATE_NO_WINDOW 会闪出终端窗口
    let mut cmd = Command::new(path);
    super::no_window(cmd.args(args));
    let output = cmd.output().ok()?;
    let text = if output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr).to_string()
    } else {
        String::from_utf8_lossy(&output.stdout).to_string()
    };
    text.lines().next().map(|s| s.trim().to_string())
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub available: bool,
    pub version: Option<String>,
    pub path: Option<String>,
}

impl ToolStatus {
    pub fn missing() -> Self {
        Self {
            available: false,
            version: None,
            path: None,
        }
    }

    fn found(path: PathBuf, version: Option<String>) -> Self {
        Self {
            available: true,
            version,
            path: Some(path.to_string_lossy().to_string()),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarStatus {
    pub yt_dlp: ToolStatus,
    pub ffmpeg: ToolStatus,
}

pub fn yt_dlp_path() -> Option<PathBuf> {
    locate("yt-dlp.exe").or_else(|| locate("yt-dlp"))
}

pub fn ffmpeg_path() -> Option<PathBuf> {
    locate("ffmpeg.exe").or_else(|| locate("ffmpeg"))
}

pub fn status() -> SidecarStatus {
    let yt_dlp = match yt_dlp_path() {
        Some(p) => ToolStatus::found(p.clone(), version_of(&p, &["--version"])),
        None => ToolStatus::missing(),
    };
    let ffmpeg = match ffmpeg_path() {
        Some(p) => {
            // ffmpeg -version 首行形如 "ffmpeg version 7.1 ..."
            let raw = version_of(&p, &["-version"]);
            ToolStatus::found(p, raw)
        }
        None => ToolStatus::missing(),
    };
    SidecarStatus { yt_dlp, ffmpeg }
}