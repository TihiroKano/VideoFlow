//! 链接历史命令：列表、新增、删除、清空。
//!
//! 全部走同步 IO（文件很小，且都在 D 盘本地），但命令声明为 async
//! 以免阻塞主线程——与其它命令保持一致。

use crate::error::AppResult;
use crate::history::{self, HistoryEntry};

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryAddPayload {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub source_name: String,
}

#[tauri::command]
pub async fn history_list() -> Vec<HistoryEntry> {
    history::load()
}

/// 记一条解析成功的链接，返回更新后的完整列表
#[tauri::command]
pub async fn history_add(payload: HistoryAddPayload) -> AppResult<Vec<HistoryEntry>> {
    let url = payload.url.trim().to_string();
    if url.is_empty() {
        return Ok(history::load());
    }
    history::add(HistoryEntry {
        url,
        title: payload.title.trim().to_string(),
        source_name: payload.source_name.trim().to_string(),
        at: chrono::Utc::now().to_rfc3339(),
    })
}

#[tauri::command]
pub async fn history_remove(url: String) -> AppResult<Vec<HistoryEntry>> {
    history::remove(&url)
}

#[tauri::command]
pub async fn history_clear() -> AppResult<()> {
    history::clear()
}