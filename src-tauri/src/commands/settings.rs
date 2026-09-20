//! 设置命令：下载目录、并发数等由前端驱动，主进程持久化到 D 盘。

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;
use tauri::State;

use crate::error::{AppError, AppResult};
use crate::state::{AppState, SettingsPatch};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSettingPayload {
    pub key: String,
    pub value: Value,
}

#[tauri::command]
pub fn set_setting(state: State<'_, Arc<AppState>>, key: String, value: Value) -> AppResult<()> {
    let mut patch = SettingsPatch::default();

    match key.as_str() {
        "downloadDir" => {
            let dir = value
                .as_str()
                .ok_or_else(|| AppError::internal("downloadDir 必须是字符串"))?;
            patch.download_dir = Some(dir.to_string());
        }
        "maxConcurrent" => {
            let n = value
                .as_u64()
                .ok_or_else(|| AppError::internal("maxConcurrent 必须是整数"))?;
            patch.max_concurrent = Some(n as u32);
        }
        "chunkConcurrency" => {
            let n = value
                .as_u64()
                .ok_or_else(|| AppError::internal("chunkConcurrency 必须是整数"))?;
            patch.chunk_concurrency = Some(n as u32);
        }
        // 同一来源并发上限（项目书 §3.2）
        "perHostConcurrency" => {
            let n = value
                .as_u64()
                .ok_or_else(|| AppError::internal("perHostConcurrency 必须是整数"))?;
            patch.per_host_concurrency = Some(n as u32);
        }
        // 设备档位：低档时后端自动下调并发（项目书 §3.2）
        "deviceTier" => {
            let tier = value
                .as_str()
                .ok_or_else(|| AppError::internal("deviceTier 必须是字符串"))?;
            patch.device_tier = Some(tier.to_string());
        }
        "cookiesSource" => {
            let source = value
                .as_str()
                .ok_or_else(|| AppError::internal("cookiesSource 必须是字符串"))?;
            patch.cookies_source = Some(source.to_string());
        }
        other => {
            return Err(AppError::new(
                "SETTING_UNKNOWN",
                format!("未知的设置项：{other}"),
                false,
            ))
        }
    }

    state.update_settings(patch);
    Ok(())
}

/// 读取当前主进程侧生效的下载设置，用于设置页显示真实值
#[tauri::command]
pub fn get_settings(state: State<'_, Arc<AppState>>) -> crate::state::Settings {
    state.settings()
}

/// 诊断登录态来源是否可用：浏览器是否还开着、cookies.txt 是否还在。
///
/// 抖音这类站点必须借浏览器登录态，而浏览器运行时会锁住 Cookie 数据库，
/// 应用侧绕不过去。设置页提前把这件事显示出来，用户就不必对着解析失败反复试。
#[tauri::command]
pub async fn cookie_source_status(source: String) -> crate::platform::browser::CookieSourceStatus {
    // 探测要拉起 tasklist 并查磁盘，放到阻塞线程池，不占用 UI 相关线程
    tauri::async_runtime::spawn_blocking(move || {
        crate::platform::browser::CookieSourceStatus::detect(&source)
    })
    .await
    .unwrap_or_else(|e| {
        eprintln!("[videoflow] 登录态探测失败：{e}");
        crate::platform::browser::CookieSourceStatus::unknown()
    })
}