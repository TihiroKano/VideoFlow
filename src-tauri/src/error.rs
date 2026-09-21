//! 错误分类与用户可执行文案（项目书 §3.5）。
//!
//! 主进程只返回结构化错误，UI 不做字符串匹配。

use serde::Serialize;

/// 错误码域：URL_* / PROVIDER_* / NETWORK_* / HTTP_* / STORAGE_* / MEDIA_* / INTEGRITY_*
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl AppError {
    pub fn new(code: &str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            retryable,
            hint: None,
            detail: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn url_malformed() -> Self {
        Self::new("URL_MALFORMED", "链接格式不正确", false).with_hint("请检查链接是否完整")
    }

    pub fn url_scheme() -> Self {
        Self::new("URL_SCHEME", "仅支持 https 链接", false)
    }

    /// 目标落在内网 / 本机地址上，被 SSRF 防护拦下。
    ///
    /// 除了用户直接填内网地址，还有一种常见情形：公网域名在本机被解析成
    /// 127.0.0.1（域名污染或分流）。文案必须说清判定发生在「本机解析」这一步，
    /// 否则用户看到的是「不解析内网或本机地址」，会以为自己填错了链接。
    pub fn url_private() -> Self {
        Self::new(
            "URL_PRIVATE",
            "该链接在本机被解析到内网或本机地址，出于安全考虑不予解析",
            false,
        )
        .with_hint(
            "若填的是公网链接，多半是本机 DNS 把它解析成了 127.0.0.1 这类地址（域名污染或分流）；请在代理软件里开启「系统代理」或 TUN 模式，再重启本应用重试",
        )
    }

    pub fn provider_unsupported() -> Self {
        Self::new("PROVIDER_UNSUPPORTED", "暂不支持该站点的解析", false)
            .with_hint("目前支持 Bilibili、YouTube 等 yt-dlp 可处理的站点，以及直链媒体文件")
    }

    pub fn provider_expired() -> Self {
        Self::new("PROVIDER_EXPIRED", "解析结果已过期", true)
            .with_hint("请重新解析该链接后再加入下载")
    }

    pub fn provider_restricted() -> Self {
        Self::new(
            "PROVIDER_RESTRICTED",
            "该资源需要登录、付费或受访问控制保护",
            false,
        )
        .with_hint("VideoFlow 不会绕过任何访问控制或 DRM，请选择你拥有下载权限的资源")
    }

    pub fn provider_tool_missing() -> Self {
        Self::new("PROVIDER_TOOL_MISSING", "站点解析器 yt-dlp 未就绪", false)
            .with_hint("请将 yt-dlp.exe 放入项目 bin 目录后重试")
    }

    /// 浏览器正在运行，Cookie 数据库被独占锁定，读不出登录状态。
    ///
    /// Chromium 系浏览器运行时会对 Cookies 库加独占锁，yt-dlp 复制不出来
    /// （上游 issue #7271）。这是 Windows 的文件锁，无法在应用侧绕过，只能让
    /// 用户先完全退出浏览器，或改用导出好的 cookies.txt。
    pub fn provider_cookie_locked(browser: &str) -> Self {
        let name = crate::platform::browser::display_name(browser);
        Self::new(
            "PROVIDER_COOKIE_LOCKED",
            format!("{name} 正在运行，无法读取它的登录状态"),
            true,
        )
        .with_hint(format!(
            "请完全退出{name}（含后台进程与托盘图标）后重试；不想关浏览器就改用「cookies.txt 文件」"
        ))
    }

    pub fn network(msg: impl Into<String>) -> Self {
        Self::new("NETWORK_ERROR", msg, true).with_hint("请检查网络后重试")
    }

    /// 域名在本机被解析到内网 / 回环地址。
    ///
    /// 与「用户自己填了内网地址」是两回事：这里链接本身是公网地址，是**本机 DNS**
    /// 给出了内网答案（域名污染或分流），所以文案必须把责任说清，并指向代理。
    pub fn network_dns_polluted(host: &str) -> Self {
        Self::new(
            "NETWORK_DNS_POLLUTED",
            format!("本机 DNS 把 {host} 解析到了内网或本机地址（域名污染）"),
            true,
        )
        .with_hint(
            "这是本机 DNS 的问题，不是链接的问题：请在代理软件里开启「系统代理」或 TUN，或在设置 → 网络与代理里配置自定义代理（SOCKS5H 由代理端解析域名）",
        )
    }

    /// 目标站点在当前出口下不可达（直连与代理都不通）
    pub fn network_unreachable(target: &str) -> Self {
        Self::new(
            "NETWORK_UNREACHABLE",
            format!("当前网络无法访问 {target}"),
            true,
        )
        .with_hint(
            "请启用系统代理/TUN，或在设置 → 网络与代理里配置自定义代理（HTTP / SOCKS5）后重试",
        )
    }

    /// 配置的代理不可用（连接被拒、超时、握手失败）
    pub fn proxy_unavailable(proxy: &str, detail: &str) -> Self {
        Self::new("PROXY_UNAVAILABLE", format!("代理 {proxy} 不可用"), true)
            .with_hint("请确认代理软件正在运行、端口与类型填写正确；可到设置 → 网络与代理点「重新检测」")
            .with_detail(detail.to_string())
    }

    /// X 链接的两条兜底链路都失败（yt-dlp 与 XDown），两条原因都要留下
    pub fn provider_xdown_failed(ytdlp: &str, xdown: &str) -> Self {
        Self::new("PROVIDER_XDOWN_FAILED", format!("X 链接解析失败：{xdown}"), false)
            .with_hint("已依次尝试 yt-dlp 与 XDown 兜底；若该链接确实有视频，请检查登录态或稍后重试")
            .with_detail(format!("yt-dlp：{ytdlp}；XDown：{xdown}"))
    }

    /// 设置项取值非法（例如代理地址写错）
    pub fn setting_invalid(msg: impl Into<String>) -> Self {
        Self::new("SETTING_INVALID", msg, false).with_hint("请修正后重试")
    }

    pub fn http_status(status: u16) -> Self {
        let retryable = status == 429 || status >= 500;
        Self::new(
            &format!("HTTP_{status}"),
            format!("服务器返回 {status}"),
            retryable,
        )
        .with_hint(if status == 403 || status == 404 {
            "链接可能已失效或需要重新解析"
        } else {
            "稍后重试，或重新解析链接"
        })
    }

    pub fn storage(msg: impl Into<String>) -> Self {
        Self::new("STORAGE_ERROR", msg, true).with_hint("请更换保存目录或释放磁盘空间")
    }

    pub fn media_tool_missing() -> Self {
        Self::new("MEDIA_TOOL_MISSING", "FFmpeg 未就绪，无法合并音视频", false)
            .with_hint("请将 ffmpeg.exe 放入项目 bin 目录，或选择无需合并的清晰度")
    }

    pub fn media_failed(msg: impl Into<String>) -> Self {
        Self::new("MEDIA_FAILED", msg, true).with_hint("源分片已保留，可以重试")
    }

    pub fn integrity(msg: impl Into<String>) -> Self {
        Self::new("INTEGRITY_ERROR", msg, true).with_hint("不可信的产物已被删除，请重新下载")
    }

    pub fn cancelled() -> Self {
        Self::new("TASK_CANCELLED", "任务已取消", false)
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new("INTERNAL_ERROR", msg, false)
    }

    /// 是否应该转「需重新解析」（项目书 §3.1 第 4 条与 §3.5）：
    /// 解析结果过期、签名地址失效，或下载时 403/404——这些不是靠重试能解决的，
    /// 必须让用户确认后重新解析拿新地址。
    pub fn requires_reparse(&self) -> bool {
        matches!(
            self.code.as_str(),
            "PROVIDER_EXPIRED" | "HTTP_403" | "HTTP_404"
        )
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(value: std::io::Error) -> Self {
        Self::storage(format!("文件操作失败：{value}"))
    }
}

impl From<reqwest::Error> for AppError {
    fn from(value: reqwest::Error) -> Self {
        if let Some(status) = value.status() {
            return Self::http_status(status.as_u16());
        }
        if value.is_timeout() {
            return Self::network("连接超时");
        }
        if value.is_connect() {
            return Self::network("无法连接到服务器");
        }
        Self::network(value.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(value: serde_json::Error) -> Self {
        Self::internal(format!("数据解析失败：{value}"))
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 失效地址归类为需重新解析() {
        // 项目书 §3.1 第 4 条与 §3.5：这几类错误必须走「重新解析」，不是无脑重试
        assert!(AppError::provider_expired().requires_reparse());
        assert!(AppError::http_status(403).requires_reparse());
        assert!(AppError::http_status(404).requires_reparse());
        assert!(!AppError::http_status(429).requires_reparse());
        assert!(!AppError::http_status(500).requires_reparse());
        assert!(!AppError::network("断网").requires_reparse());
    }

    #[test]
    fn 内网拦截文案说明是本机解析结果并给出代理提示() {
        let err = AppError::url_private();
        assert_eq!(err.code, "URL_PRIVATE");
        assert!(!err.retryable);
        // 判定发生在「本机解析」这一步，不能让人以为自己填了内网地址
        assert!(err.message.contains("本机"), "实际：{}", err.message);
        let hint = err.hint.expect("必须给出可操作提示");
        assert!(hint.contains("127.0.0.1"), "实际：{hint}");
        assert!(hint.contains("代理"), "实际：{hint}");
    }
}