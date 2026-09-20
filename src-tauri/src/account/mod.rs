//! 账户登录：站点表、Cookie 记录与 Netscape cookies.txt 的序列化。
//!
//! 为什么需要这个模块：B 站的高清晰度（1080P 60帧 / 4K）由 Cookie 决定——
//! 无 Cookie 时 yt-dlp 会明确报 `Format(s) 4K 超高清, 1080P 60帧 are missing`。
//! 而 B 站的 `SESSDATA` 是 **httpOnly**，JS 读不到，必须用 WebView2 的
//! `cookies_for_url`（它明确包含 httpOnly Cookie）导出成 yt-dlp 认识的格式。
//!
//! 本模块**不依赖 Tauri**：站点表与 cookies.txt 的生成都能脱离 GUI 单测。

#[cfg(feature = "gui")]
pub mod window;

/// 一个可登录的站点
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountSite {
    /// 稳定标识，前端与设置里用它
    pub key: &'static str,
    /// 显示名
    pub label: &'static str,
    /// 登录入口（打开的页面）
    pub login_url: &'static str,
    /// 导出 Cookie 时使用的 URL（决定 WebView2 返回哪个域下的 Cookie）
    pub cookie_url: &'static str,
    /// 本站点的 Cookie 域（用于过滤与替换）
    pub cookie_domains: &'static [&'static str],
    /// 判定登录成功的 Cookie 名。
    ///
    /// `None` 表示不猜——抖音/快手的登录 Cookie 名不稳定，猜错会让用户
    /// 明明登录成功却被判为未登录，所以这两个站点走手动「我已完成登录」。
    pub login_cookie: Option<&'static str>,
}

/// 可登录站点表
pub const SITES: &[AccountSite] = &[
    AccountSite {
        key: "bilibili",
        label: "Bilibili",
        login_url: "https://passport.bilibili.com/login",
        cookie_url: "https://www.bilibili.com",
        cookie_domains: &["bilibili.com"],
        // SESSDATA 是 httpOnly 的登录令牌，名字稳定
        login_cookie: Some("SESSDATA"),
    },
    AccountSite {
        key: "douyin",
        label: "抖音",
        login_url: "https://www.douyin.com/",
        cookie_url: "https://www.douyin.com",
        cookie_domains: &["douyin.com"],
        login_cookie: None,
    },
    AccountSite {
        key: "kuaishou",
        label: "快手",
        login_url: "https://www.kuaishou.com/",
        cookie_url: "https://www.kuaishou.com",
        cookie_domains: &["kuaishou.com"],
        login_cookie: None,
    },
];

pub fn site_by_key(key: &str) -> Option<&'static AccountSite> {
    SITES.iter().find(|s| s.key == key)
}

/// 应用管理的 Cookie 文件路径。
///
/// 与 settings.json / tasks.json 同级，全部落在 D 盘，不写 C 盘用户目录。
pub fn cookie_store_path() -> std::path::PathBuf {
    std::path::PathBuf::from("D:\\VideoFlow\\cookies.txt")
}

/// cookies.txt 的文件头：yt-dlp 靠它识别格式
pub const COOKIE_FILE_HEADER: &str = "# Netscape HTTP Cookie File";

/// 一条 Cookie 的纯数据形式。
///
/// 不直接用 `cookie::Cookie` 是为了让本模块能脱离 Tauri 单测——
/// 从 WebView2 取到的 Cookie 在这里转成它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieRecord {
    /// 域，形如 `.bilibili.com` 或 `www.bilibili.com`
    pub domain: String,
    /// 是否对子域生效（对应 Netscape 的第二列）
    pub include_subdomains: bool,
    pub path: String,
    pub secure: bool,
    /// 过期时间（Unix 秒）；0 表示会话 Cookie
    pub expires: i64,
    pub name: String,
    pub value: String,
}

impl CookieRecord {
    /// 该 Cookie 是否属于某个站点
    pub fn belongs_to(&self, site: &AccountSite) -> bool {
        let domain = self.domain.trim_start_matches('.').to_ascii_lowercase();
        site.cookie_domains
            .iter()
            .any(|d| domain == *d || domain.ends_with(&format!(".{d}")))
    }

    /// 序列化成 Netscape 格式的一行（Tab 分隔，共 7 列）
    fn to_line(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.domain,
            if self.include_subdomains { "TRUE" } else { "FALSE" },
            self.path,
            if self.secure { "TRUE" } else { "FALSE" },
            self.expires,
            self.name,
            self.value
        )
    }
}

/// Cookie 值里混入 Tab/换行会破坏列结构（Netscape 是纯文本逐行解析），
/// 这类 Cookie 直接丢弃，而不是写出一份会被 yt-dlp 解析错的文件。
fn is_writable(c: &CookieRecord) -> bool {
    let bad = |s: &str| s.contains('\t') || s.contains('\n') || s.contains('\r');
    !bad(&c.domain) && !bad(&c.path) && !bad(&c.name) && !bad(&c.value) && !c.name.is_empty()
}

/// 解析 cookies.txt，取出其中的数据行（忽略注释与空行）
fn parse_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .collect()
}

/// 把某站点的 Cookie 合并进 cookies.txt。
///
/// 先删掉文件里属于该站点的旧行再追加新行——多站点共用一个文件时互不覆盖，
/// 且重复登录不会留下互相矛盾的旧值。
pub fn merge_site_cookies(existing: &str, site: &AccountSite, cookies: &[CookieRecord]) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for line in parse_lines(existing) {
        // 只按第一列（域）判断归属；列数不足的行不是我们写的，一律保留
        let Some(domain) = line.split('\t').next() else {
            kept.push(line);
            continue;
        };
        let belongs = site.cookie_domains.iter().any(|d| {
            let domain = domain.trim_start_matches('.').to_ascii_lowercase();
            domain == *d || domain.ends_with(&format!(".{d}"))
        });
        if !belongs {
            kept.push(line);
        }
    }

    let mut out = String::from(COOKIE_FILE_HEADER);
    out.push('\n');
    for line in kept {
        out.push_str(line);
        out.push('\n');
    }
    for c in cookies.iter().filter(|c| is_writable(c) && c.belongs_to(site)) {
        out.push_str(&c.to_line());
        out.push('\n');
    }
    out
}

/// 从 cookies.txt 里移除某站点的全部 Cookie（退出登录）
pub fn remove_site_cookies(existing: &str, site: &AccountSite) -> String {
    merge_site_cookies(existing, site, &[])
}

/// 判断导出的 Cookie 里是否已经有登录令牌。
///
/// 只对配置了 `login_cookie` 的站点有意义——其余站点光看 Cookie 名判断不出来
/// （匿名访问也会写一堆 Cookie），一律返回 false，由用户手动确认。
pub fn has_login_cookie(site: &AccountSite, cookies: &[CookieRecord]) -> bool {
    let Some(name) = site.login_cookie else {
        return false;
    };
    cookies
        .iter()
        .any(|c| c.name == name && !c.value.trim().is_empty())
}

/// 综合判断某站点是否已登录。
///
/// 两条通路取或：识别到登录 Cookie，或用户明确点过「我已完成登录」。
/// 后者是给抖音/快手这类 Cookie 名不稳定的站点用的。
pub fn is_logged_in(site: &AccountSite, cookies: &[CookieRecord], confirmed: bool) -> bool {
    confirmed || has_login_cookie(site, cookies)
}

// ---- 手工确认标记 ----
//
// Cookie 文件本身不记录「用户确认过登录」，而抖音/快手的登录态无法靠 Cookie 名
// 判断，因此单独存一份标记。退出登录时连同 Cookie 一起清掉。

/// 手工确认过登录的站点标记文件
pub fn confirmed_store_path() -> std::path::PathBuf {
    std::path::PathBuf::from("D:\\VideoFlow\\accounts.json")
}

/// 读取手工确认过的站点列表；文件缺失或损坏时返回空
pub fn load_confirmed(text: &str) -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct File {
        #[serde(default)]
        confirmed: Vec<String>,
    }
    serde_json::from_str::<File>(text)
        .map(|f| f.confirmed)
        .unwrap_or_default()
}

/// 写入手工确认过的站点列表
pub fn save_confirmed(keys: &[String]) -> String {
    #[derive(serde::Serialize)]
    struct File<'a> {
        confirmed: &'a [String],
    }
    serde_json::to_string_pretty(&File { confirmed: keys })
        .unwrap_or_else(|_| "{\n  \"confirmed\": []\n}".to_string())
}

// ---- 文件读写 ----

/// 读取应用管理的 cookies.txt；不存在时返回空串
pub fn read_store() -> String {
    std::fs::read_to_string(cookie_store_path()).unwrap_or_default()
}

/// 写入应用管理的 cookies.txt（先建目录，失败时返回错误）
pub fn write_store(text: &str) -> std::io::Result<()> {
    let path = cookie_store_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)
}

/// 把 cookies.txt 的一行解析回 `CookieRecord`；格式不符返回 None
fn parse_cookie_line(line: &str) -> Option<CookieRecord> {
    let cols: Vec<&str> = line.split('\t').collect();
    if cols.len() != 7 {
        return None;
    }
    Some(CookieRecord {
        domain: cols[0].to_string(),
        include_subdomains: cols[1].eq_ignore_ascii_case("TRUE"),
        path: cols[2].to_string(),
        secure: cols[3].eq_ignore_ascii_case("TRUE"),
        expires: cols[4].parse().unwrap_or(0),
        name: cols[5].to_string(),
        value: cols[6].to_string(),
    })
}

/// 解析 cookies.txt 文本为记录列表（忽略注释、空行与格式不符的行）
pub fn parse_cookie_records(text: &str) -> Vec<CookieRecord> {
    parse_lines(text).into_iter().filter_map(parse_cookie_line).collect()
}

/// 读取当前 Cookie 文件里的全部记录
pub fn read_cookie_records() -> Vec<CookieRecord> {
    parse_cookie_records(&read_store())
}

/// 读取手工确认过的站点列表
pub fn read_confirmed() -> Vec<String> {
    load_confirmed(&std::fs::read_to_string(confirmed_store_path()).unwrap_or_default())
}

/// 写入手工确认过的站点列表
pub fn write_confirmed(keys: &[String]) -> std::io::Result<()> {
    let path = confirmed_store_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, save_confirmed(keys))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(key: &str) -> &'static AccountSite {
        site_by_key(key).expect("站点应存在")
    }

    fn cookie(domain: &str, name: &str, value: &str) -> CookieRecord {
        CookieRecord {
            domain: domain.to_string(),
            include_subdomains: true,
            path: "/".to_string(),
            secure: true,
            expires: 1790000000,
            name: name.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn 站点表包含三种站点() {
        for key in ["bilibili", "douyin", "kuaishou"] {
            let s = site(key);
            assert!(!s.label.is_empty());
            assert!(s.login_url.starts_with("https://"));
            assert!(!s.cookie_domains.is_empty());
        }
        assert!(site_by_key("nope").is_none());
    }

    #[test]
    fn cookie_文件路径在_d_盘() {
        let p = cookie_store_path();
        assert_eq!(p.to_string_lossy(), "D:\\VideoFlow\\cookies.txt");
    }

    #[test]
    fn 序列化是七列制表符分隔() {
        let text = merge_site_cookies(
            "",
            site("bilibili"),
            &[cookie(".bilibili.com", "SESSDATA", "abc123")],
        );
        assert!(text.starts_with(COOKIE_FILE_HEADER));
        let line = parse_lines(&text)[0];
        assert_eq!(line, ".bilibili.com\tTRUE\t/\tTRUE\t1790000000\tSESSDATA\tabc123");
        assert_eq!(line.split('\t').count(), 7);
    }

    #[test]
    fn 会话_cookie_过期时间写_0() {
        let mut c = cookie(".bilibili.com", "buvid3", "x");
        c.expires = 0;
        let text = merge_site_cookies("", site("bilibili"), &[c]);
        assert!(parse_lines(&text)[0].contains("\t0\tbuvid3\t"));
    }

    #[test]
    fn 只保留属于该站点的_cookie() {
        let text = merge_site_cookies(
            "",
            site("bilibili"),
            &[
                cookie(".bilibili.com", "SESSDATA", "keep"),
                // 别的站的不能被写进来
                cookie(".douyin.com", "ttwid", "drop"),
                cookie(".example.com", "other", "drop"),
            ],
        );
        let lines = parse_lines(&text);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("SESSDATA"));
    }

    #[test]
    fn 子域_cookie_也归属该站点() {
        let text = merge_site_cookies(
            "",
            site("bilibili"),
            &[cookie(".passport.bilibili.com", "token", "v")],
        );
        assert_eq!(parse_lines(&text).len(), 1);
    }

    #[test]
    fn 再次合并同一站点会替换旧值而不是叠加() {
        let first = merge_site_cookies(
            "",
            site("bilibili"),
            &[cookie(".bilibili.com", "SESSDATA", "old")],
        );
        let second = merge_site_cookies(
            &first,
            site("bilibili"),
            &[cookie(".bilibili.com", "SESSDATA", "new")],
        );
        let lines = parse_lines(&second);
        assert_eq!(lines.len(), 1, "旧值应被替换掉");
        assert!(lines[0].ends_with("\tnew"));
    }

    #[test]
    fn 多站点共用一个文件互不覆盖() {
        let bili = merge_site_cookies(
            "",
            site("bilibili"),
            &[cookie(".bilibili.com", "SESSDATA", "b")],
        );
        let both = merge_site_cookies(
            &bili,
            site("douyin"),
            &[cookie(".douyin.com", "ttwid", "d")],
        );
        let lines = parse_lines(&both);
        assert_eq!(lines.len(), 2, "两个站点的 Cookie 都应保留");
        assert!(lines.iter().any(|l| l.contains("SESSDATA")));
        assert!(lines.iter().any(|l| l.contains("ttwid")));

        // 再更新 B 站，不应影响抖音那条
        let updated = merge_site_cookies(
            &both,
            site("bilibili"),
            &[cookie(".bilibili.com", "SESSDATA", "b2")],
        );
        let lines = parse_lines(&updated);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().any(|l| l.contains("ttwid")), "抖音的不能被删掉");
        assert!(lines.iter().any(|l| l.ends_with("\tb2")));
    }

    #[test]
    fn 退出登录只清掉该站点() {
        let bili = merge_site_cookies(
            "",
            site("bilibili"),
            &[cookie(".bilibili.com", "SESSDATA", "b")],
        );
        let both = merge_site_cookies(
            &bili,
            site("douyin"),
            &[cookie(".douyin.com", "ttwid", "d")],
        );
        let after = remove_site_cookies(&both, site("bilibili"));
        let lines = parse_lines(&after);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("ttwid"));
    }

    #[test]
    fn 含制表符或换行的_cookie_被丢弃() {
        // 写出这种值会让 yt-dlp 解析错列，宁可不写
        let text = merge_site_cookies(
            "",
            site("bilibili"),
            &[
                cookie(".bilibili.com", "good", "ok"),
                cookie(".bilibili.com", "bad\tname", "v"),
                cookie(".bilibili.com", "badnl", "a\nb"),
            ],
        );
        let lines = parse_lines(&text);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("\tgood\t"));
    }

    #[test]
    fn 登录判定按_cookie_名() {
        let s = site("bilibili");
        assert!(has_login_cookie(
            s,
            &[cookie(".bilibili.com", "SESSDATA", "x")]
        ));
        // 空值不算登录
        assert!(!has_login_cookie(
            s,
            &[cookie(".bilibili.com", "SESSDATA", "  ")]
        ));
        // 别的 Cookie 不算
        assert!(!has_login_cookie(
            s,
            &[cookie(".bilibili.com", "buvid3", "x")]
        ));
    }

    #[test]
    fn 未配置_cookie_名的站点不靠_cookie_判定登录() {
        // 匿名访问也会写 Cookie，光看「有没有 Cookie」会把未登录误判成已登录
        let s = site("kuaishou");
        assert!(!has_login_cookie(s, &[cookie(".kuaishou.com", "did", "x")]));
        assert!(!is_logged_in(s, &[cookie(".kuaishou.com", "did", "x")], false));
        // 只能靠用户手动确认
        assert!(is_logged_in(s, &[], true));
    }

    #[test]
    fn 登录判定_识别到令牌或手动确认都算() {
        let s = site("bilibili");
        let with_token = [cookie(".bilibili.com", "SESSDATA", "x")];
        assert!(is_logged_in(s, &with_token, false));
        assert!(is_logged_in(s, &[], true));
        assert!(!is_logged_in(s, &[cookie(".bilibili.com", "buvid3", "x")], false));
    }

    #[test]
    fn 手动确认标记可往返读写() {
        let text = save_confirmed(&["kuaishou".to_string(), "douyin".to_string()]);
        assert_eq!(load_confirmed(&text), vec!["kuaishou", "douyin"]);
        // 空标记与损坏内容都不能让启动失败
        assert!(load_confirmed(&save_confirmed(&[])).is_empty());
        assert!(load_confirmed("not json").is_empty());
        assert!(load_confirmed("").is_empty());
    }

    #[test]
    fn 写出的文件能被解析回来() {
        // 序列化与解析必须闭环，否则登录状态检测会读不到自己写的 Cookie
        let text = merge_site_cookies(
            "",
            site("bilibili"),
            &[
                cookie(".bilibili.com", "SESSDATA", "tok"),
                cookie(".bilibili.com", "buvid3", "dev"),
            ],
        );
        let records = parse_cookie_records(&text);
        assert_eq!(records.len(), 2);
        assert!(has_login_cookie(site("bilibili"), &records));
        assert_eq!(records[0].name, "SESSDATA");
        assert_eq!(records[0].value, "tok");
        assert!(records[0].include_subdomains);
        assert!(records[0].secure);
        assert_eq!(records[0].expires, 1790000000);
        assert_eq!(records[0].path, "/");
    }

    #[test]
    fn 格式不符的行在解析时被忽略() {
        let text = "# 注释\n坏行没有制表符\n.cn\tTRUE\t/\tFALSE\t0\tok\tv\n";
        let records = parse_cookie_records(text);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "ok");
    }

    #[test]
    fn 已有文件不是我们写的行也保留() {
        // 用户手写的注释与额外行不能被我们吃掉
        let existing = "# Netscape HTTP Cookie File\n# 我自己的注释\n.cn\tTRUE\t/\tFALSE\t0\tfoo\tbar\n";
        let text = merge_site_cookies(
            existing,
            site("bilibili"),
            &[cookie(".bilibili.com", "SESSDATA", "b")],
        );
        let lines = parse_lines(&text);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().any(|l| l.contains("foo")));
    }
}