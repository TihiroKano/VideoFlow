// Windows 发布构建下不弹出控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "gui")]
fn main() {
    videoflow_lib::run()
}

/// 关闭 desktop feature 时只编译纯逻辑层（用于单元测试）
#[cfg(not(feature = "gui"))]
fn main() {}