//! 文件操作命令：打开、定位、移入回收站。

use crate::error::AppResult;
use crate::platform;

#[tauri::command]
pub fn open_file(path: String) -> AppResult<()> {
    platform::open_file(&path)
}

#[tauri::command]
pub fn reveal_file(path: String) -> AppResult<()> {
    platform::reveal_file(&path)
}

/// 删除文件：默认移入系统回收站，不直接抹除（项目书 §2.2）
#[tauri::command]
pub fn delete_file(path: String) -> AppResult<()> {
    platform::delete_to_trash(&path)
}