//! 浏览器登录态来源的识别与运行状态探测。
//!
//! 解析抖音这类站点时必须借用浏览器里已有的登录态（见 `resolver::ytdlp`），
//! 而 Chromium 系浏览器运行期间会对 Cookies 数据库加独占锁，yt-dlp 复制不出来
//! （上游 issue #7271）。这是 Windows 的文件锁，应用侧无法绕过，因此只能提前
//! 探测出「浏览器还开着」，让用户先退出，而不是等解析失败再报一句看不懂的错。

use serde::Serialize;

/// 登录态来源（与设置页一一对应）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CookieSource {
    /// 未配置：不携带任何登录态
    Off,
    /// `browser:<键名>`，如 `browser:edge`
    Browser(String),
    /// `file:<路径>`，用户从浏览器导出的 cookies.txt
    File(String),
    /// `account:`：应用内账户登录，Cookie 由内嵌浏览器导出到固定路径
    Account,
    /// 前缀无法识别（例如旧版本写入的值）
    Unknown,
}

/// 浏览器键名 → 显示名（用于错误文案与设置页）
pub fn display_name(key: &str) -> &'static str {
    match key {
        "edge" => "Microsoft Edge",
        "chrome" => "Google Chrome",
        "firefox" => "Firefox",
        // 未知键名给一个中性称呼，不臆造具体浏览器
        _ => "浏览器",
    }
}

/// 浏览器键名 → 进程映像名
pub fn process_name(key: &str) -> Option<&'static str> {
    match key {
        "edge" => Some("msedge.exe"),
        "chrome" => Some("chrome.exe"),
        "firefox" => Some("firefox.exe"),
        _ => None,
    }
}

/// 解析设置里存下来的登录态来源
pub fn parse_source(raw: &str) -> CookieSource {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return CookieSource::Off;
    }
    if let Some(key) = trimmed.strip_prefix("browser:") {
        let key = key.trim();
        return if key.is_empty() {
            CookieSource::Unknown
        } else {
            CookieSource::Browser(key.to_string())
        };
    }
    if let Some(path) = trimmed.strip_prefix("file:") {
        let path = path.trim();
        return if path.is_empty() {
            CookieSource::Unknown
        } else {
            CookieSource::File(path.to_string())
        };
    }
    if trimmed.starts_with(crate::resolver::ytdlp::COOKIES_ACCOUNT_PREFIX) {
        return CookieSource::Account;
    }
    CookieSource::Unknown
}

/// 设置页用来展示「当前登录态能不能用」的诊断结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieSourceStatus {
    /// 来源类型：none / browser / file / unknown
    pub kind: &'static str,
    /// 浏览器显示名（kind = browser 时有值）
    pub browser_label: Option<String>,
    /// 浏览器是否正在运行；None 表示无法判定
    pub browser_running: Option<bool>,
    /// cookies.txt 是否存在（kind = file 时有值）
    pub file_exists: Option<bool>,
    /// 已登录的站点名（kind = account 时有值）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_sites: Option<Vec<String>>,
    /// 账户 Cookie 文件是否存在（kind = account 时有值）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_file_exists: Option<bool>,
}

impl CookieSourceStatus {
    fn plain(kind: &'static str) -> Self {
        Self {
            kind,
            browser_label: None,
            browser_running: None,
            file_exists: None,
            account_sites: None,
            account_file_exists: None,
        }
    }

    /// 探测失败时的兜底：明确告诉界面「没查出来」，不要伪装成「未配置」
    pub fn unknown() -> Self {
        Self::plain("unknown")
    }

    /// 按来源生成诊断结果（会去查进程与文件，属于阻塞操作）
    pub fn detect(raw: &str) -> Self {
        match parse_source(raw) {
            CookieSource::Off => Self::plain("none"),
            CookieSource::Unknown => Self::plain("unknown"),
            CookieSource::Browser(key) => Self {
                kind: "browser",
                browser_label: Some(display_name(&key).to_string()),
                browser_running: is_running(&key),
                file_exists: None,
                account_sites: None,
                account_file_exists: None,
            },
            CookieSource::File(path) => Self {
                kind: "file",
                browser_label: None,
                browser_running: None,
                file_exists: Some(std::path::Path::new(&path).is_file()),
                account_sites: None,
                account_file_exists: None,
            },
            CookieSource::Account => {
                // 账户登录：报出已登录站点，让设置页能直接显示「能用」
                let confirmed = crate::account::read_confirmed();
                let cookies = crate::account::read_cookie_records();
                let sites: Vec<String> = crate::account::SITES
                    .iter()
                    .filter(|s| {
                        let is_confirmed = confirmed.iter().any(|k| k == s.key);
                        crate::account::is_logged_in(s, &cookies, is_confirmed)
                    })
                    .map(|s| s.label.to_string())
                    .collect();
                Self {
                    kind: "account",
                    browser_label: None,
                    browser_running: None,
                    file_exists: None,
                    account_file_exists: Some(crate::account::cookie_store_path().is_file()),
                    account_sites: Some(sites),
                }
            }
        }
    }
}

/// 探测浏览器是否正在运行。
///
/// 返回 `None` 表示无法判定（非 Windows 平台，或系统命令不可用），
/// 调用方应把它当成「未知」而不是「没运行」。
pub fn is_running(key: &str) -> Option<bool> {
    let image = process_name(key)?;

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let mut cmd = std::process::Command::new("tasklist");
        cmd.args(["/FI", &format!("IMAGENAME eq {image}"), "/NH"]);
        cmd.creation_flags(CREATE_NO_WINDOW);

        let output = cmd.output().ok()?;
        // tasklist 在无匹配时输出一句本地化提示，因此以映像名是否出现为准
        let text = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
        Some(text.contains(&image.to_ascii_lowercase()))
    }

    #[cfg(not(windows))]
    {
        let _ = image;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 已知浏览器有显示名与进程名() {
        for key in ["edge", "chrome", "firefox"] {
            assert_ne!(display_name(key), "浏览器", "{key} 应有专属显示名");
            assert!(process_name(key).is_some(), "{key} 应有进程映像名");
        }
    }

    #[test]
    fn 未知键名不猜测进程名() {
        // 猜进程名会让设置页误报「浏览器正在运行」
        assert!(process_name("safari").is_none());
        assert_eq!(is_running("safari"), None);
    }

    #[test]
    fn 来源解析覆盖三种前缀() {
        assert_eq!(parse_source(""), CookieSource::Off);
        assert_eq!(parse_source("   "), CookieSource::Off);
        assert_eq!(
            parse_source("browser:edge"),
            CookieSource::Browser("edge".into())
        );
        assert_eq!(
            parse_source(" file:D:\\cookies.txt "),
            CookieSource::File("D:\\cookies.txt".into())
        );
    }

    #[test]
    fn 只有前缀没有内容时归为无法识别() {
        assert_eq!(parse_source("browser:"), CookieSource::Unknown);
        assert_eq!(parse_source("file:"), CookieSource::Unknown);
        assert_eq!(parse_source("edge"), CookieSource::Unknown);
    }

    #[test]
    fn 账户登录前缀被识别() {
        assert_eq!(parse_source("account:"), CookieSource::Account);
        // 前缀后的内容由应用管理，任何值都归入账户登录
        assert_eq!(parse_source(" account:whatever "), CookieSource::Account);
    }

    #[test]
    fn 未配置来源不探测进程() {
        let status = CookieSourceStatus::detect("");
        assert_eq!(status.kind, "none");
        assert!(status.browser_running.is_none());
    }

    #[test]
    fn 浏览器来源给出显示名并探测运行状态() {
        let status = CookieSourceStatus::detect("browser:edge");
        assert_eq!(status.kind, "browser");
        assert_eq!(status.browser_label.as_deref(), Some("Microsoft Edge"));
        // 运行与否取决于测试机上有没有开着 Edge，这里只要求探测有结论
        assert!(status.browser_running.is_some());
    }

    #[test]
    fn 文件来源检查文件是否存在() {
        let missing = CookieSourceStatus::detect("file:D:\\__videoflow_not_exist__.txt");
        assert_eq!(missing.kind, "file");
        assert_eq!(missing.file_exists, Some(false));

        let here = CookieSourceStatus::detect(&format!("file:{}", file!()));
        assert_eq!(here.file_exists, Some(true));
    }
}
