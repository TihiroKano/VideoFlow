//! Browser Resolver 的 WebView2 驱动（需要 `gui` feature）。
//!
//! 与 `mod.rs` 的分工：那边是可脱离 GUI 单测的纯逻辑，这边只管建窗、导航、等待、清理。
//!
//! ## 通道设计
//!
//! 不用 CDP、不用 WebSocket——离线环境没有任何 WebSocket 客户端 crate，
//! 而且 CDP 还得自己实现握手与帧编解码。这里用注入脚本 + 哨兵导航：
//!
//! 1. 注入脚本（`hook.js`）改写 `fetch`/`XHR`，抓到详情响应后精简成小载荷，
//!    再 `location.href = "https://vf-capture.invalid/?d=<base64url>"`；
//! 2. `on_navigation` 认出这个哨兵地址，把载荷通过 channel 回传，并返回 `false`
//!    取消导航（wry 在 Windows 下映射为 `args.SetCancel(true)`），所以不会真的发请求。
//!
//! 载荷走 query 而不是 fragment：`Url::query_pairs` 能直接解码，base64 用
//! `URL_SAFE_NO_PAD` 避免 `+` `/` `=` 在 query 里被误处理。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::mpsc;

use crate::error::{AppError, AppResult};
use crate::resolver::browser::{self, CapturePayload};

/// 解析窗口的固定 label。
///
/// 固定而非每次新建：既省掉反复启动 WebView2 的开销，也避免
/// `WindowLabelAlreadyExists`（`tauri-2.11.5/src/manager/window.rs` 在 label 重复时直接报错）。
const WINDOW_LABEL: &str = "vf-browser-resolver";

/// 先隐藏等待的时长。抖音页面通常几秒内就会请求详情接口。
const HIDDEN_WAIT: Duration = Duration::from_secs(12);
/// 窗口显形后再等的时长，留给用户过验证码。
const VISIBLE_WAIT: Duration = Duration::from_secs(60);

/// 解析窗口的尺寸（显形后用）
const WINDOW_W: f64 = 960.0;
const WINDOW_H: f64 = 720.0;

/// 内嵌浏览器的 profile 目录：与 settings.json / tasks.json 一样落在 D 盘。
///
/// 账户登录窗口也用它，因此登录态对媒体抓取立即可见。
const PROFILE_DIR: &str = "D:\\VideoFlow\\browser-resolver";

/// 卸载旧文档用的空白页
const BLANK_URL: &str = "about:blank";
/// 两次导航之间留一点间隔，确保旧文档真的被卸载
const NAVIGATION_GAP: Duration = Duration::from_millis(150);

/// 抖音会判定无头/隐藏浏览器为机器人，因此不能用无头模式。
///
/// 这里不做伪装，只是把明显会暴露自动化的开关关掉（禁用通知、麦克风、摄像头权限询问），
/// 不涉及任何反检测手段——页面在真实 WebView2 里正常加载。
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI --disable-notifications";

/// 内嵌浏览器的启动参数（拼上当前出口的代理开关）。
///
/// WebView2 的启动参数在**环境创建时定型**，且环境按 `data_directory` 复用，
/// 所以两个窗口（解析窗口与登录窗口）必须传同一套参数，改代理后要重开窗口才生效。
pub(crate) fn browser_args(proxy_arg: Option<&str>) -> String {
    match proxy_arg.map(str::trim).filter(|a| !a.is_empty()) {
        Some(arg) => format!("{BROWSER_ARGS} {arg}"),
        None => BROWSER_ARGS.to_string(),
    }
}

/// Tick 间隔：在等 channel 的同时留出响应取消的粒度
const TICK: Duration = Duration::from_millis(120);

/// 内嵌浏览器窗口的 label。账户登录窗口用另一个 label，但共用同一个 profile。
pub fn window_label() -> &'static str {
    WINDOW_LABEL
}

/// 内嵌浏览器（解析窗口与登录窗口共用）的 profile 目录。
///
/// 两个窗口指定同一个 `data_directory`，Tauri 按它复用同一个 WebView2 环境
/// （`tauri-runtime-wry/src/lib.rs:4793-4800`），因此它们共享 Cookie——
/// 这是「在登录窗口登录一次，解析窗口就带着登录态」的依据。
pub fn profile_dir() -> &'static str {
    PROFILE_DIR
}

/// 确保 profile 里有一个窗口存在（懒创建解析窗口）。
///
/// 账户登录导出 Cookie 时用：Cookie 存在 profile 里，只要该 profile 下有任一
/// 窗口即可读出，不必关心它是解析窗口还是登录窗口。
pub fn ensure_profile_window(app: &AppHandle) -> AppResult<()> {
    BrowserResolver::new().ensure_window(app, None)
}

/// 驱动一个解析窗口，返回注入脚本抓到的媒体载荷。
pub struct BrowserResolver {
    /// 复用同一个窗口，解析串行化
    gate: tokio::sync::Mutex<()>,
    /// 当前这一轮解析的回传通道。
    ///
    /// 窗口是复用的，而 `on_navigation` 闭包在窗口创建时就固定下来了，
    /// 所以不能让闭包直接持有某一轮的 sender——那样第二次解析时新 receiver
    /// 会立刻看到「通道已断开」，表现为瞬间失败。这里改成每轮替换槽位里的 sender。
    pending: Arc<Mutex<Option<mpsc::UnboundedSender<String>>>>,
}

impl BrowserResolver {
    pub fn new() -> Self {
        Self {
            gate: tokio::sync::Mutex::new(()),
            pending: Arc::new(Mutex::new(None)),
        }
    }

    /// 打开（或复用）解析窗口，等待页面把媒体数据回传。
    ///
    /// 窗口先隐藏；`HIDDEN_WAIT` 内没拿到就显形让用户能看到验证码，再等 `VISIBLE_WAIT`。
    ///
    /// `proxy_arg` 是当前出口对应的 Chromium 代理参数（见 `net::proxy::browser_proxy_arg`）：
    /// 窗口已存在时不会改参数（WebView2 环境在创建时定型），由调用方在代理变化时关窗重建。
    pub async fn capture(
        &self,
        app: &AppHandle,
        page_url: &url::Url,
        token: Option<Arc<crate::downloader::ControlToken>>,
        proxy_arg: Option<&str>,
    ) -> AppResult<CapturePayload> {
        // 同一时刻只解析一个链接：窗口是单例，并发会互相串数据
        let _guard = self.gate.lock().await;

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        self.ensure_window(app, proxy_arg)?;
        // 每轮把回传通道换成这一轮的：窗口复用时循环体不会重新绑定闭包
        *self.pending.lock().expect("pending 锁不可中毒") = Some(tx);

        let window = app
            .get_webview_window(WINDOW_LABEL)
            .ok_or_else(|| {
                AppError::new("PROVIDER_BROWSER_UNAVAILABLE", "解析窗口不可用", true)
            })?;

        // 每轮先排空上一轮可能残留的消息，避免串数据
        while rx.try_recv().is_ok() {}

        // 先回到空白页再加载目标页。
        //
        // 复用窗口时直接导航到同一 URL 不会真正重新加载文档，抖音的 SPA 也不会
        // 重新请求详情接口，于是注入脚本拿不到任何数据（实测第二次解析会超时）。
        // 先卸载旧文档可以保证每次都是全新的一次页面启动。
        let _ = window.navigate(BLANK_URL.parse().expect("about:blank 一定合法"));
        tokio::time::sleep(NAVIGATION_GAP).await;

        window
            .navigate(page_url.clone())
            .map_err(|e| AppError::internal(format!("无法加载页面：{e}")))?;

        let outcome = self
            .wait_for_payload(&mut rx, token.as_deref(), HIDDEN_WAIT)
            .await;

        let raw = match outcome {
            WaitOutcome::Got(raw) => raw,
            // 隐藏状态没拿到：显形让用户处理验证码，再给一段时间
            WaitOutcome::Timeout => {
                let _ = window.show();
                let _ = window.set_focus();
                match self
                    .wait_for_payload(&mut rx, token.as_deref(), VISIBLE_WAIT)
                    .await
                {
                    WaitOutcome::Got(raw) => raw,
                    WaitOutcome::Timeout => {
                        let _ = window.hide();
                        return Err(AppError::new(
                            "PROVIDER_BROWSER_TIMEOUT",
                            "浏览器没能取到媒体数据",
                            true,
                        )
                        .with_hint(
                            "可能需要在弹出的窗口中完成验证码或登录。请在弹出的窗口里操作后重试",
                        ));
                    }
                    WaitOutcome::Cancelled => {
                        let _ = window.hide();
                        return Err(AppError::cancelled());
                    }
                }
            }
            WaitOutcome::Cancelled => return Err(AppError::cancelled()),
        };

        // 解析完成后收起来，下次复用
        let _ = window.hide();

        let parsed = browser::decode_captured(&raw)?;
        Ok(parsed)
    }

    /// 取（或懒创建）解析窗口
    pub(crate) fn ensure_window(&self, app: &AppHandle, proxy_arg: Option<&str>) -> AppResult<()> {
        if app.get_webview_window(WINDOW_LABEL).is_some() {
            return Ok(());
        }

        // 独立 data_directory：抖音的 Cookie 必须与主界面隔离，
        // 否则会写进主应用 profile，并与主窗口共享同一个 WebView2 环境。
        // 与 settings.json / tasks.json 一样落在 D 盘，不写 C 盘用户目录。
        let data_dir = PathBuf::from(PROFILE_DIR);
        let _ = std::fs::create_dir_all(&data_dir);

        let slot = Arc::clone(&self.pending);
        let builder = WebviewWindowBuilder::new(app, WINDOW_LABEL, WebviewUrl::External(
            "about:blank".parse().expect("about:blank 一定合法"),
        ))
        .title("VideoFlow 正在读取页面")
        .inner_size(WINDOW_W, WINDOW_H)
        .visible(false)
        .center()
        .data_directory(data_dir)
        .additional_browser_args(&browser_args(proxy_arg))
        .initialization_script(include_str!("hook.js"))
        .on_navigation(move |url| {
            // 只有哨兵地址才拦：其余一律放行，否则会挡掉抖音自身的重定向
            // （v.douyin.com → iesdouyin.com → www.douyin.com）
            match url.host_str() {
                Some(h) if h == browser::SENTINEL_HOST => {
                    if let Some((_, value)) = url
                        .query_pairs()
                        .find(|(k, _)| k == browser::SENTINEL_PARAM)
                    {
                        // 从槽位取当前这一轮的 sender（窗口复用时闭包是固定的）
                        if let Ok(guard) = slot.lock() {
                            if let Some(tx) = guard.as_ref() {
                                let _ = tx.send(value.to_string());
                            }
                        }
                    }
                    // 取消导航：不会真的去请求这个地址
                    false
                }
                _ => true,
            }
        });

        builder
            .build()
            .map_err(|e| AppError::new(
                "PROVIDER_BROWSER_UNAVAILABLE",
                "无法创建解析窗口",
                true,
            )
            .with_detail(format!("{e}")))?;
        Ok(())
    }

    /// 等待注入脚本回传载荷
    async fn wait_for_payload(
        &self,
        rx: &mut mpsc::UnboundedReceiver<String>,
        token: Option<&crate::downloader::ControlToken>,
        budget: Duration,
    ) -> WaitOutcome {
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            if let Some(t) = token {
                if t.state() == crate::downloader::ControlState::Cancel {
                    return WaitOutcome::Cancelled;
                }
            }
            match rx.try_recv() {
                Ok(raw) => return WaitOutcome::Got(raw),
                Err(mpsc::error::TryRecvError::Disconnected) => return WaitOutcome::Timeout,
                Err(mpsc::error::TryRecvError::Empty) => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return WaitOutcome::Timeout;
            }
            tokio::time::sleep(TICK).await;
        }
    }
}

impl Default for BrowserResolver {
    fn default() -> Self {
        Self::new()
    }
}

enum WaitOutcome {
    Got(String),
    Timeout,
    Cancelled,
}