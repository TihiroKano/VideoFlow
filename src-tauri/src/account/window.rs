//! 账户登录窗口与 Cookie 导出（需要 `gui` feature）。
//!
//! ## 为什么是独立窗口
//!
//! 解析窗口（`resolver/browser/driver.rs`）注入了 `hook.js`，会在页面里跑媒体嗅探。
//! 若让登录页也跑嗅探，它会误发哨兵导航、可能打断登录流程。所以登录用独立窗口，
//! 职责清晰、互不干扰。
//!
//! ## 为什么能与解析窗口共享登录态
//!
//! 两个窗口指定**同一个 `data_directory`**，而 Tauri 的 WebView2 环境是按
//! `data_directory` 作 key 复用同一个 `WryWebContext` 的
//! （`tauri-runtime-wry-2.11.4/src/lib.rs:4793-4800`），因此 Cookie 天然共享：
//! 用户在登录窗口登录一次，解析窗口抓媒体时就带着这个登录态。

use std::time::Duration;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::account::{self, AccountSite};
use crate::error::{AppError, AppResult};

/// 登录窗口 label（与解析窗口不同，但共用同一个 profile 目录）
const LOGIN_LABEL: &str = "vf-account-login";

/// 登录窗口的 label（命令层收起窗口时用）
pub fn login_label() -> &'static str {
    LOGIN_LABEL
}

/// 登录窗口尺寸：要放得下二维码
const WINDOW_W: f64 = 520.0;
const WINDOW_H: f64 = 700.0;

/// 轮询登录 Cookie 的间隔
const POLL: Duration = Duration::from_millis(800);

/// 等用户完成登录的最长时间。扫码 + 手机确认通常 30 秒内完成，
/// 给到 5 分钟足够从容，又不会让窗口无限期挂着。
const LOGIN_BUDGET: Duration = Duration::from_secs(300);

/// 打开（或复用）登录窗口并导航到站点登录页
///
/// `proxy_arg` 与解析窗口用同一个值（见 `net::proxy::browser_proxy_arg`）：
/// 两个窗口共用一个 `data_directory`，WebView2 环境在创建时定型，参数必须一致。
fn open_login_window(
    app: &AppHandle,
    site: &AccountSite,
    proxy_arg: Option<&str>,
) -> AppResult<tauri::WebviewWindow> {
    if let Some(existing) = app.get_webview_window(LOGIN_LABEL) {
        let _ = existing.set_title(&format!("登录 {} — 完成后可关闭本窗口", site.label));
        let _ = existing.navigate(parse(site.login_url)?);
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(existing);
    }

    // 与解析窗口共用一个 profile，因此共享 Cookie（见模块文档）
    let data_dir = std::path::PathBuf::from(crate::resolver::browser::driver::profile_dir());
    let _ = std::fs::create_dir_all(&data_dir);

    let window = WebviewWindowBuilder::new(app, LOGIN_LABEL, WebviewUrl::External(parse(site.login_url)?))
        .title(format!("登录 {} — 完成后可关闭本窗口", site.label))
        .inner_size(WINDOW_W, WINDOW_H)
        .center()
        .data_directory(data_dir)
        // 登录窗口不注入 hook.js：这里不需要媒体嗅探
        .additional_browser_args(&crate::resolver::browser::driver::browser_args(proxy_arg))
        .build()
        .map_err(|e| {
            AppError::new("ACCOUNT_WINDOW_FAILED", "无法打开登录窗口", true)
                .with_detail(format!("{e}"))
        })?;

    Ok(window)
}

fn parse(url: &str) -> AppResult<url::Url> {
    url::Url::parse(url).map_err(|_| AppError::url_malformed())
}

/// 等用户完成登录。
///
/// 两条通路取或：
/// - 站点配置了 `login_cookie` 时，轮询到该 Cookie 出现即算成功；
/// - `force` 为 true 表示用户点了「我已完成登录」，直接收工
///   （抖音/快手的登录 Cookie 名不稳定，只能靠用户确认）。
///
/// 返回 true 表示应继续导出 Cookie；false 表示用户中途取消。
pub async fn wait_for_login(
    app: &AppHandle,
    site: &AccountSite,
    proxy_arg: Option<&str>,
    mut force: impl FnMut() -> bool,
) -> AppResult<bool> {
    let window = open_login_window(app, site, proxy_arg)?;
    let cookie_url = parse(site.cookie_url)?;
    let deadline = tokio::time::Instant::now() + LOGIN_BUDGET;

    loop {
        // 用户点了「我已完成登录」：不再等 Cookie 名
        if force() {
            return Ok(true);
        }
        // 用户把窗口关了：视为取消，不留半截登录态
        if app.get_webview_window(LOGIN_LABEL).is_none() {
            return Ok(false);
        }

        if site.login_cookie.is_some() {
            let cookies = read_cookies(&window, cookie_url.clone()).await?;
            if account::has_login_cookie(site, &cookies) {
                return Ok(true);
            }
        }

        if tokio::time::Instant::now() >= deadline {
            let _ = window.hide();
            return Err(AppError::new(
                "ACCOUNT_LOGIN_TIMEOUT",
                format!("等待 {} 登录超时", site.label),
                true,
            )
            .with_hint("可以重新点击「登录」再试；若页面一直无法加载，请到「其他方式」里改用浏览器登录态"));
        }

        tokio::time::sleep(POLL).await;
    }
}

/// 从 WebView2 读出某个 URL 下的全部 Cookie 并转成 `CookieRecord`。
///
/// **必须在非主线程调用**：Windows 上 `cookies_for_url` 在同步命令或事件处理器里
/// 会死锁（wry#583）。这里统一走 `spawn_blocking`。
pub async fn read_cookies(
    window: &tauri::WebviewWindow,
    url: url::Url,
) -> AppResult<Vec<account::CookieRecord>> {
    let window = window.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let cookies = window.cookies_for_url(url).map_err(|e| {
            AppError::new("ACCOUNT_COOKIE_FAILED", "读取登录状态失败", true)
                .with_detail(format!("{e}"))
        })?;
        Ok(cookies.iter().map(to_record).collect())
    })
    .await
    .map_err(|e| AppError::internal(format!("读取 Cookie 任务失败：{e}")))?
}

/// 把 `cookie::Cookie` 转成纯数据形式。
///
/// 过期时间：会话 Cookie 写 0（Netscape 里表示「会话结束失效」）——
/// yt-dlp 认这个值，写成未来时间反而会让一份临时会话看起来永久有效。
fn to_record(c: &cookie::Cookie<'static>) -> account::CookieRecord {
    let domain = c.domain().unwrap_or_default();
    account::CookieRecord {
        domain: domain.to_string(),
        include_subdomains: domain.starts_with('.'),
        path: c.path().unwrap_or("/").to_string(),
        // secure() 返回 Option：没显式设置时按不安全处理
        secure: c.secure().unwrap_or(false),
        expires: match c.expires() {
            Some(cookie::Expiration::DateTime(dt)) => dt.unix_timestamp(),
            _ => 0,
        },
        name: c.name().to_string(),
        value: c.value().to_string(),
    }
}

/// 供命令层调用：读 Cookie → 合并进 cookies.txt → 返回本次导出的条数
pub async fn export_site_cookies(
    app: &AppHandle,
    site: &AccountSite,
) -> AppResult<usize> {
    // 确保 profile 里有窗口：解析窗口与登录窗口共用一个 profile，
    // 任一存在即可读到全部 Cookie（包括用户在解析窗口里产生的）
    let window = match app.get_webview_window(LOGIN_LABEL) {
        Some(w) => w,
        None => {
            crate::resolver::browser::driver::ensure_profile_window(app)?;
            app.get_webview_window(crate::resolver::browser::driver::window_label())
                .ok_or_else(|| {
                    AppError::new("ACCOUNT_WINDOW_FAILED", "无法打开内嵌浏览器", true)
                })?
        }
    };

    let cookies = read_cookies(&window, parse(site.cookie_url)?).await?;
    let merged = account::merge_site_cookies(&account::read_store(), site, &cookies);
    account::write_store(&merged)
        .map_err(|e| AppError::storage(format!("写入 Cookie 文件失败：{e}")))?;

    Ok(cookies.iter().filter(|c| c.belongs_to(site)).count())
}