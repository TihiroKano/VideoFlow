//! 链接历史：记录用户解析成功过的链接，最多 100 条。
//!
//! 与 `tasks.json` / `settings.json` 一样落在 D 盘（`D:\VideoFlow\history.json`），
//! 不占系统盘。写入是原子的（先写 `.tmp` 再 rename），被杀进程不会留下半个 JSON。
//!
//! 只存**解析成功过**的链接：失败的、手滑粘错的链接不该污染这份列表，
//! 否则下拉框很快就没法用了。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// 历史条数上限
pub const MAX_ENTRIES: usize = 100;

/// 一条链接记录
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    /// 规范化后的链接（是去重键）
    pub url: String,
    /// 解析出的标题
    #[serde(default)]
    pub title: String,
    /// 站点名
    #[serde(default)]
    pub source_name: String,
    /// 最近一次使用时间（RFC 3339）
    pub at: String,
}

/// 历史文件路径
pub fn history_store_path() -> PathBuf {
    PathBuf::from("D:\\VideoFlow\\history.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryFile {
    version: u32,
    entries: Vec<HistoryEntry>,
}

/// 把一条记录并入列表（纯函数，便于单测）。
///
/// 三条规则：
/// - 同一个链接**去重**：已存在时提到最前并刷新标题与时间，而不是堆重复项；
/// - 最新的在最前；
/// - 超过上限时丢掉最旧的。
pub fn merge_entry(existing: &[HistoryEntry], entry: HistoryEntry) -> Vec<HistoryEntry> {
    let mut out: Vec<HistoryEntry> = Vec::with_capacity(MAX_ENTRIES);
    out.push(entry.clone());
    for e in existing {
        if out.len() >= MAX_ENTRIES {
            break;
        }
        // 已存在的同链接被上面那条取代；空链接直接丢弃
        if e.url == entry.url || e.url.trim().is_empty() {
            continue;
        }
        out.push(e.clone());
    }
    out.truncate(MAX_ENTRIES);
    out
}

/// 删除一条记录
pub fn remove_entry(existing: &[HistoryEntry], url: &str) -> Vec<HistoryEntry> {
    existing
        .iter()
        .filter(|e| e.url != url)
        .cloned()
        .collect()
}

/// 读取历史；文件不存在或损坏时返回空列表（不阻塞启动）
pub fn load_from(path: &std::path::Path) -> Vec<HistoryEntry> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    match serde_json::from_str::<HistoryFile>(&raw) {
        Ok(file) => file.entries.into_iter().take(MAX_ENTRIES).collect(),
        Err(e) => {
            eprintln!("[videoflow] 链接历史无法解析，已忽略：{e}");
            Vec::new()
        }
    }
}

/// 原子写入历史
pub fn save_to(path: &std::path::Path, entries: &[HistoryEntry]) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = HistoryFile {
        version: 1,
        entries: entries.to_vec(),
    };
    let data = serde_json::to_vec_pretty(&file)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 读当前历史
pub fn load() -> Vec<HistoryEntry> {
    load_from(&history_store_path())
}

/// 写当前历史
pub fn save(entries: &[HistoryEntry]) -> AppResult<()> {
    save_to(&history_store_path(), entries).map_err(|e| AppError::storage(format!("写入链接历史失败：{e}")))
}

/// 记一条（读-改-写）
pub fn add(entry: HistoryEntry) -> AppResult<Vec<HistoryEntry>> {
    let next = merge_entry(&load(), entry);
    save(&next)?;
    Ok(next)
}

/// 删一条
pub fn remove(url: &str) -> AppResult<Vec<HistoryEntry>> {
    let next = remove_entry(&load(), url);
    save(&next)?;
    Ok(next)
}

/// 清空
pub fn clear() -> AppResult<()> {
    save(&[])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(url: &str) -> HistoryEntry {
        HistoryEntry {
            url: url.to_string(),
            title: format!("标题 {url}"),
            source_name: "站点".to_string(),
            at: "2026-09-20T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn 最新记录排在最前() {
        let list = merge_entry(&[entry("a"), entry("b")], entry("c"));
        assert_eq!(
            list.iter().map(|e| e.url.as_str()).collect::<Vec<_>>(),
            vec!["c", "a", "b"]
        );
    }

    #[test]
    fn 同一链接去重并提到最前() {
        // 反复解析同一个链接不该把列表堆满重复项
        let list = merge_entry(&[entry("a"), entry("b"), entry("c")], entry("c"));
        assert_eq!(
            list.iter().map(|e| e.url.as_str()).collect::<Vec<_>>(),
            vec!["c", "a", "b"],
            "重复项应被提到最前而不是追加"
        );
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn 去重时刷新标题与时间() {
        let mut fresh = entry("a");
        fresh.title = "新标题".to_string();
        fresh.at = "2026-09-21T00:00:00Z".to_string();
        let list = merge_entry(&[entry("a")], fresh);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "新标题");
        assert_eq!(list[0].at, "2026-09-21T00:00:00Z");
    }

    #[test]
    fn 超过一百条时丢掉最旧的() {
        // 列表是「新 → 旧」顺序，所以构造出来的 u99 是最旧的那条
        let existing: Vec<HistoryEntry> = (0..MAX_ENTRIES)
            .map(|i| entry(&format!("u{i}")))
            .collect();
        let list = merge_entry(&existing, entry("new"));
        assert_eq!(list.len(), MAX_ENTRIES);
        assert_eq!(list[0].url, "new", "新记录在最前");
        assert_eq!(list[1].url, "u0", "原有的最新一条紧随其后");
        assert_eq!(
            list.last().unwrap().url,
            format!("u{}", MAX_ENTRIES - 2),
            "末尾应当是仅次于最旧的那条"
        );
        assert!(
            !list.iter().any(|e| e.url == format!("u{}", MAX_ENTRIES - 1)),
            "最旧的一条应被淘汰"
        );
    }

    #[test]
    fn 恰好一百条时不丢数据() {
        let existing: Vec<HistoryEntry> = (0..MAX_ENTRIES - 1)
            .map(|i| entry(&format!("u{i}")))
            .collect();
        let list = merge_entry(&existing, entry("new"));
        assert_eq!(list.len(), MAX_ENTRIES);
    }

    #[test]
    fn 空链接被丢弃() {
        let list = merge_entry(&[entry("a"), entry("  ")], entry("b"));
        assert_eq!(
            list.iter().map(|e| e.url.as_str()).collect::<Vec<_>>(),
            vec!["b", "a"]
        );
    }

    #[test]
    fn 删除只影响目标链接() {
        let list = remove_entry(&[entry("a"), entry("b"), entry("c")], "b");
        assert_eq!(
            list.iter().map(|e| e.url.as_str()).collect::<Vec<_>>(),
            vec!["a", "c"]
        );
    }

    #[test]
    fn 往返读写保持完整() {
        let dir = std::env::temp_dir().join(format!("vf-hist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");

        let entries = vec![entry("a"), entry("b")];
        save_to(&path, &entries).unwrap();
        assert_eq!(load_from(&path), entries);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn 文件缺失或损坏不阻塞启动() {
        let dir = std::env::temp_dir().join(format!("vf-hist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // 不存在
        assert!(load_from(&dir.join("nope.json")).is_empty());

        // 内容损坏
        let bad = dir.join("bad.json");
        std::fs::write(&bad, b"{ not json").unwrap();
        assert!(load_from(&bad).is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn 写入是原子的() {
        // 只应留下目标文件，不该残留 .tmp
        let dir = std::env::temp_dir().join(format!("vf-hist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");

        save_to(&path, &[entry("a")]).unwrap();
        assert!(path.exists());
        assert!(!dir.join("history.json.tmp").exists(), "临时文件应已被改名");

        std::fs::remove_dir_all(&dir).ok();
    }
}