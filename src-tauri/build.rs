fn main() {
    // 只有启用 GUI 层时才需要 tauri-build：它会注入 tauri.conf.json 的资源与
    // `generate_context!` 所需的编译期变量。纯逻辑层（单元测试）不需要这些。
    if std::env::var_os("CARGO_FEATURE_GUI").is_some() {
        tauri_build::build();
    }
}