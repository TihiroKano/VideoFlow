//! 平台能力：打开文件、定位目录、回收站删除、外部工具定位。
//!
//! 网络出口（代理探测等）已迁到 `crate::net`：那里是出口的唯一真相源。

pub mod browser;
pub mod sidecar;

use std::path::Path;
use std::process::Command;

use crate::error::{AppError, AppResult};

#[cfg(windows)]
pub(crate) fn no_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW)
}

#[cfg(not(windows))]
pub(crate) fn no_window(cmd: &mut Command) -> &mut Command {
    cmd
}

/// 用系统默认程序打开文件；只允许已完成且路径存在的文件（项目书 §3.4）
pub fn open_file(path: &str) -> AppResult<()> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(AppError::storage("文件不存在，可能在外部被移动或删除")
            .with_hint("可以在「下载记录」中移除该记录，或重新下载"));
    }

    #[cfg(windows)]
    {
        // 交给 shell 打开，避免自己解析文件关联
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "start", "", path]);
        no_window(&mut cmd);
        cmd.spawn()
            .map_err(|e| AppError::internal(format!("无法打开文件：{e}")))?;
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let mut cmd = Command::new(opener);
        cmd.arg(path);
        no_window(&mut cmd);
        cmd.spawn()
            .map_err(|e| AppError::internal(format!("无法打开文件：{e}")))?;
        Ok(())
    }
}

/// 在文件管理器中定位文件
pub fn reveal_file(path: &str) -> AppResult<()> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(AppError::storage("文件不存在，无法定位")
            .with_hint("可以在「下载记录」中移除该记录"));
    }

    #[cfg(windows)]
    {
        // explorer 要求 `/select,` 与路径处在同一个参数里，且路径必须自带引号：
        //   explorer /select,"C:\dir with space\a.mp4"
        // 不能用 Command::arg —— 它会在参数含空格时把整串包成
        //   "/select,C:\dir with space\a.mp4"
        // 引号跑到最前面，explorer 认不出 /select 开关，就退化成打开默认文件夹
        // （这正是「点了文件夹却跳到别的目录」的原因）。
        // raw_arg 原样透传，绕开 Rust 的参数转义。
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new("explorer");
        cmd.raw_arg(format!("/select,\"{path}\""));
        no_window(&mut cmd);
        cmd.spawn()
            .map_err(|e| AppError::internal(format!("无法定位文件：{e}")))?;
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        let dir = p.parent().unwrap_or(p);
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let mut cmd = Command::new(opener);
        cmd.arg(dir);
        cmd.spawn()
            .map_err(|e| AppError::internal(format!("无法定位文件：{e}")))?;
        Ok(())
    }
}

/// 删除文件：默认移入系统回收站，不直接抹除
pub fn delete_to_trash(path: &str) -> AppResult<()> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(AppError::storage("文件不存在，无需删除"));
    }
    trash::delete(p).map_err(|e| AppError::storage(format!("移入回收站失败：{e}")))
}