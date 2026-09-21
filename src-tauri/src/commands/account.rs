//! 账户登录命令：登录、退出、状态查询、刷新。
//!
//! B 站的高清晰度由 Cookie 决定（无 Cookie 时 yt-dlp 会报
//! `Format(s) 4K 超高清, 1080P 60帧 are missing`），所以这里做的事是：
//! 让用户在内嵌浏览器里登录一次 → 把 WebView2 里的 Cookie（含 httpOnly 的
//! `SESSDATA`）导出成 cookies.txt → yt-dlp 用 `--cookies` 消费它。

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::account::{self, window};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// 前端展示用的站点状态
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSiteStatus {
    pub key: String,
    pub label: String,
    /// 是否已登录（识别到登录 Cookie，或用户手动确认过）
    pub logged_in: bool,
    /// 是否需要用户手动确认完成（Cookie 名不稳定的站点）
    pub needs_manual_confirm: bool,
    /// 账号名（能查到才有）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    /// 是否大会员。B 站的 4K 与 1080P 高码率需要大会员，登录本身不够
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_vip: Option<bool>,
}

/// 校验结果：用导出的 Cookie 调站点自己的「当前用户」接口
#[derive(Debug, Default)]
struct VerifyResult {
    user_name: Option<String>,
    is_vip: Option<bool>,
}

/// 用导出的 Cookie 调 B 站的 nav 接口，确认登录态真的有效。
///
/// 这是「功能是否真的有效」的第一级客观证据：接口自己说 isLogin=true，
/// 就说明导出的 Cookie 确实被服务端认可，而不是我们自说自话。
async fn verify_bilibili(client: &reqwest::Client, cookie_header: &str) -> Option<VerifyResult> {
    let resp = client
        .get("https://api.bilibili.com/x/web-interface/nav")
        .header(reqwest::header::COOKIE, cookie_header)
        .header(reqwest::header::REFERER, "https://www.bilibili.com/")
        .send()
        .await
        .ok()?;
    let json: serde_json::Value = resp.json().await.ok()?;
    let data = json.get("data")?;
    if data.get("isLogin").and_then(|v| v.as_bool()) != Some(true) {
        return None;
    }
    Some(VerifyResult {
        user_name: data
            .get("uname")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        // vipStatus 1 表示大会员
        is_vip: data
            .get("vipStatus")
            .and_then(|v| v.as_i64())
            .map(|v| v == 1),
    })
}

/// 从导出的 Cookie 记录拼出 Cookie 请求头（只用于校验）
fn cookie_header(cookies: &[account::CookieRecord]) -> String {
    cookies
        .iter()
        .map(|c| format!("{}={}", c.name, c.value))
        .collect::<Vec<_>>()
        .join("; ")
}

/// 汇总各站点登录状态
async fn collect_status(state: &AppState) -> Vec<AccountSiteStatus> {
    let confirmed = account::read_confirmed();
    let cookies = account::read_cookie_records();

    let mut out = Vec::new();
    for site in account::SITES {
        let is_confirmed = confirmed.iter().any(|k| k == site.key);
        let logged_in = account::is_logged_in(site, &cookies, is_confirmed);

        // 只有 B 站有公开的「当前用户」接口，用它做真伪校验
        let verified = if logged_in && site.key == "bilibili" {
            let own: Vec<account::CookieRecord> = cookies
                .iter()
                .filter(|c| c.belongs_to(site))
                .cloned()
                .collect();
            verify_bilibili(&state.client(), &cookie_header(&own)).await
        } else {
            None
        };

        out.push(AccountSiteStatus {
            key: site.key.to_string(),
            label: site.label.to_string(),
            // B 站：接口说没登录就是没登录，不显示成已登录
            logged_in: if site.key == "bilibili" && logged_in {
                verified.is_some()
            } else {
                logged_in
            },
            needs_manual_confirm: site.login_cookie.is_none(),
            user_name: verified.as_ref().and_then(|v| v.user_name.clone()),
            is_vip: verified.as_ref().and_then(|v| v.is_vip),
        });
    }
    out
}

#[tauri::command]
pub async fn account_status(
    state: State<'_, Arc<AppState>>,
) -> AppResult<Vec<AccountSiteStatus>> {
    Ok(collect_status(&state).await)
}

/// 打开登录窗口，等完成后导出 Cookie 并把登录态来源切到「账户登录」。
#[tauri::command]
pub async fn account_login(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    site_key: String,
) -> AppResult<Vec<AccountSiteStatus>> {
    let site = account::site_by_key(&site_key)
        .ok_or_else(|| AppError::new("ACCOUNT_UNKNOWN_SITE", "未知的登录站点", false))?;

    // 用户点「我已完成登录」会写这个标记，login_confirm 命令负责置位。
    // 先清掉上一轮遗留的确认，否则会立刻收工导致没等用户操作。
    let flag = state.login_confirm_flag(&site_key);
    flag.store(false, std::sync::atomic::Ordering::SeqCst);
    let force = move || flag.load(std::sync::atomic::Ordering::SeqCst);

    let proxy_arg = state.browser_proxy_arg();
    let proceed = window::wait_for_login(&app, site, proxy_arg.as_deref(), force).await?;
    if !proceed {
        return Err(AppError::new("ACCOUNT_LOGIN_CANCELLED", "已取消登录", false));
    }

    let count = window::export_site_cookies(&app, site).await?;
    if count == 0 {
        return Err(AppError::new(
            "ACCOUNT_NO_COOKIES",
            format!("没有从 {} 读到任何 Cookie", site.label),
            true,
        )
        .with_hint("请确认已经在弹出的窗口里完成登录，然后重试"));
    }

    // 站点能自动判定登录态（有明确的登录 Cookie）时，先用服务端接口验证，
    // **验证通过才写确认标记**——否则一次失败的登录会留下「已登录」的假象。
    if site.login_cookie.is_some() {
        let cookies = account::read_cookie_records();
        let own: Vec<account::CookieRecord> = cookies
            .iter()
            .filter(|c| c.belongs_to(site))
            .cloned()
            .collect();
        let verified = if site.key == "bilibili" {
            verify_bilibili(&state.client(), &cookie_header(&own)).await
        } else {
            // 其它站点没有公开的校验接口，能读到登录 Cookie 即视为有效
            Some(VerifyResult::default())
        };
        if verified.is_none() {
            return Err(AppError::new(
                "ACCOUNT_VERIFY_FAILED",
                format!("{} 的登录状态未通过校验", site.label),
                true,
            )
            .with_hint("Cookie 已导出但服务端未认可，通常是登录还没完成。请在登录窗口里走完流程后重试"));
        }
    }

    // 到这里再落确认标记
    let mut confirmed = account::read_confirmed();
    if !confirmed.iter().any(|k| k == site.key) {
        confirmed.push(site.key.to_string());
        let _ = account::write_confirmed(&confirmed);
    }

    // 把登录态来源切到账户登录，后续解析才会带上这份 Cookie
    state.set_cookies_source("account:");

    // 收起登录窗口：登录完了就别再占着屏幕
    if let Some(w) = app.get_webview_window(crate::account::window::login_label()) {
        let _ = w.hide();
    }

    Ok(collect_status(&state).await)
}

/// 用户在登录窗口里点「我已完成登录」时调用，让等待循环立刻收工。
///
/// 抖音/快手的登录 Cookie 名不稳定，无法自动判定，只能由用户确认。
#[tauri::command]
pub fn account_confirm_login(state: State<'_, Arc<AppState>>, site_key: String) {
    state.mark_login_confirm(&site_key);
}

/// 退出登录：清掉该站点的 Cookie 与确认标记
#[tauri::command]
pub async fn account_logout(
    state: State<'_, Arc<AppState>>,
    site_key: String,
) -> AppResult<Vec<AccountSiteStatus>> {
    let site = account::site_by_key(&site_key)
        .ok_or_else(|| AppError::new("ACCOUNT_UNKNOWN_SITE", "未知的登录站点", false))?;

    let cleaned = account::remove_site_cookies(&account::read_store(), site);
    account::write_store(&cleaned)
        .map_err(|e| AppError::storage(format!("写入 Cookie 文件失败：{e}")))?;

    let confirmed: Vec<String> = account::read_confirmed()
        .into_iter()
        .filter(|k| k != site.key)
        .collect();
    let _ = account::write_confirmed(&confirmed);

    // 一个站点都没了就把来源切回「不使用」，免得继续拿着空文件
    let remaining = collect_status(&state).await;
    if remaining.iter().all(|s| !s.logged_in) {
        state.set_cookies_source("");
    }

    Ok(remaining)
}

/// 重新从内嵌浏览器导出 Cookie 并复校（Cookie 会自然刷新，登录久了需要更新）
#[tauri::command]
pub async fn account_refresh(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> AppResult<Vec<AccountSiteStatus>> {
    let confirmed = account::read_confirmed();
    if confirmed.is_empty() {
        return Ok(collect_status(&state).await);
    }

    let sites: Vec<&'static account::AccountSite> = account::SITES
        .iter()
        .filter(|s| confirmed.iter().any(|k| k == s.key))
        .collect();

    for site in sites {
        // 单个站点失败不影响其它站点，最后统一用状态反馈
        let _ = window::export_site_cookies(&app, site).await;
    }

    Ok(collect_status(&state).await)
}