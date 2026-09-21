//! VideoFlow 主进程。
//!
//! 模块划分对应 docs/VideoFlow-项目书.md §8：
//! core（领域模型）/ resolver（Provider）/ downloader（双引擎）/ media（FFmpeg）
//! / storage（文件与恢复点）/ platform（系统能力）/ commands（IPC）。
//!
//! 分层原则：**下载内核与 GUI 完全解耦**（项目书 §10 第 3 条）。
//! 只有 `state` 与 `commands` 依赖 Tauri；其余模块可在无 GUI 依赖下单独测试：
//!
//! ```text
//! cargo test --no-default-features --lib
//! ```

pub mod account;
pub mod core;
pub mod downloader;
pub mod error;
pub mod history;
pub mod media;
pub mod net;
pub mod platform;
pub mod resolver;
pub mod storage;

#[cfg(feature = "gui")]
pub mod commands;
#[cfg(feature = "gui")]
pub mod state;

#[cfg(feature = "gui")]
mod desktop {
    use std::sync::Arc;

    use tauri::{LogicalSize, Manager, Size};

    use crate::state::AppState;

    /// 设计基准上限（项目书 §11）
    const DESIGN_MAX_W: f64 = 1440.0;
    const DESIGN_MAX_H: f64 = 920.0;
    /// 最小可用尺寸（项目书 §7.2 桌面应用最小宽度 720px）
    const MIN_W: f64 = 900.0;
    const MIN_H: f64 = 600.0;

    /// 让窗口适配显示器工作区。
    ///
    /// 高 DPI 屏上 1280 逻辑像素可能超过物理分辨率（例如 150% 缩放 + 1440×960 屏幕），
    /// 此时按配置尺寸创建的窗口会被裁切，所以启动时按工作区重新计算一次。
    fn fit_window_to_work_area(window: &tauri::WebviewWindow) {
        // setup 阶段窗口可能还没落到具体显示器上，因此优先取窗口所在显示器，
        // 再退回主显示器与首个可用显示器。
        let monitor = window
            .current_monitor()
            .ok()
            .flatten()
            .or_else(|| window.primary_monitor().ok().flatten())
            .or_else(|| window.available_monitors().ok().and_then(|m| m.into_iter().next()));

        let Some(monitor) = monitor else {
            eprintln!("[videoflow] 未获取到显示器信息，保持配置尺寸");
            return;
        };

        let scale = monitor.scale_factor();
        if scale <= 0.0 {
            return;
        }

        let work = monitor.work_area();
        let avail_w = work.size.width as f64 / scale;
        let avail_h = work.size.height as f64 / scale;

        let current = window
            .outer_size()
            .map(|s| (s.width as f64 / scale, s.height as f64 / scale))
            .unwrap_or((0.0, 0.0));

        eprintln!(
            "[videoflow] 显示器 {:.0}x{:.0} 逻辑像素 (scale {scale})，窗口 {:.0}x{:.0}",
            avail_w, avail_h, current.0, current.1
        );

        if current.0 <= avail_w && current.1 <= avail_h {
            return;
        }

        // 留出边距，避免窗口紧贴屏幕边缘
        let target_w = (avail_w * 0.96).min(DESIGN_MAX_W).max(MIN_W);
        let target_h = (avail_h * 0.96).min(DESIGN_MAX_H).max(MIN_H);

        if let Err(e) = window.set_size(Size::Logical(LogicalSize::new(target_w, target_h))) {
            eprintln!("[videoflow] 调整窗口尺寸失败：{e}");
        }
        let _ = window.center();
    }

    /// setup 阶段显示器信息可能尚未就绪，延迟一小段时间再适配一次
    fn schedule_window_fit(window: tauri::WebviewWindow) {
        fit_window_to_work_area(&window);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            fit_window_to_work_area(&window);
        });
    }

    #[cfg_attr(mobile, tauri::mobile_entry_point)]
    pub fn run() {
        let app = tauri::Builder::default()
            .plugin(tauri_plugin_dialog::init())
            .setup(|app| {
                let state: Arc<AppState> = AppState::new(app.handle().clone());
                state.spawn_scheduler();
                app.manage(state);

                if let Some(window) = app.get_webview_window("main") {
                    schedule_window_fit(window);
                }

                // 预热硬件编码器探测：它要真编一帧（约 1~2 秒），
                // 放到后台线程先跑完，用户打开转换页时就不用等在「载入中…」上
                std::thread::spawn(|| {
                    let summary = crate::media::capabilities::hardware_summary();
                    eprintln!("[videoflow] 可用硬件编码：{summary:?}");
                });
                Ok(())
            })
            .invoke_handler(tauri::generate_handler![
                // 解析与任务
                crate::commands::resolve::resolve_urls,
                crate::commands::tasks::create_task,
                crate::commands::tasks::task_action,
                crate::commands::tasks::reparse_task,
                crate::commands::tasks::task_control_state,
                crate::commands::tasks::delete_task_file,
                crate::commands::get_snapshot,
                crate::commands::remove_task,
                // 文件
                crate::commands::files::open_file,
                crate::commands::files::reveal_file,
                crate::commands::files::delete_file,
                // 设置与系统
                crate::commands::settings::set_setting,
                crate::commands::settings::get_settings,
                crate::commands::settings::cookie_source_status,
                // 账户登录
                crate::commands::account::account_status,
                crate::commands::account::account_login,
                crate::commands::account::account_confirm_login,
                crate::commands::account::account_logout,
                crate::commands::account::account_refresh,
                // 格式转换
                crate::commands::convert::convert_capabilities,
                crate::commands::convert::probe_media_info,
                crate::commands::convert::create_convert_task,
                // 链接历史
                crate::commands::history::history_list,
                crate::commands::history::history_add,
                crate::commands::history::history_remove,
                crate::commands::history::history_clear,
                crate::commands::pick_directory,
                crate::commands::read_image_as_data_url,
                crate::commands::ensure_download_dir,
                crate::commands::sidecar_status,
                crate::commands::network_diagnostics,
            ])
            .build(tauri::generate_context!())
            .expect("VideoFlow 启动失败");

        // 退出前把队列落盘一次：运行中的任务在快照里记为 paused（项目书 §3.2）
        app.run(|handle, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                if let Some(state) = handle.try_state::<Arc<AppState>>() {
                    state.persist_snapshot();
                }
            }
        });
    }
}

#[cfg(feature = "gui")]
pub use desktop::run;