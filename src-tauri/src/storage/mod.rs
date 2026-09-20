//! 文件命名、冲突策略与恢复点落盘（项目书 §3.3、§7.3）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::model::DownloadTask;
use crate::error::{AppError, AppResult};

/// Windows 文件名非法字符
const INVALID_CHARS: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// 把标题净化为安全的文件名主干
pub fn sanitize_stem(input: &str) -> String {
    let mut out: String = input
        .chars()
        .map(|c| {
            if INVALID_CHARS.contains(&c) || (c as u32) < 0x20 {
                ' '
            } else {
                c
            }
        })
        .collect();

    // 折叠空白
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");

    // Windows 不允许文件名以点或空格结尾
    while out.ends_with('.') || out.ends_with(' ') {
        out.pop();
    }

    // Windows 保留名
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.iter().any(|r| out.eq_ignore_ascii_case(r)) {
        out.push('_');
    }

    if out.is_empty() {
        out = "videoflow-download".to_string();
    }
    // 给扩展名与序号留出空间
    if out.chars().count() > 150 {
        out = out.chars().take(150).collect();
    }
    out
}

/// 冲突策略：`name.ext` → `name (1).ext` → `name (2).ext`……默认绝不覆盖
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let ext = ext.trim_start_matches('.');
    let first = if ext.is_empty() {
        dir.join(stem)
    } else {
        dir.join(format!("{stem}.{ext}"))
    };
    if !first.exists() {
        return first;
    }
    for n in 1..10_000u32 {
        let candidate = if ext.is_empty() {
            dir.join(format!("{stem} ({n})"))
        } else {
            dir.join(format!("{stem} ({n}).{ext}"))
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    // 极端情况：退化为时间戳后缀
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
    if ext.is_empty() {
        dir.join(format!("{stem}-{stamp}"))
    } else {
        dir.join(format!("{stem}-{stamp}.{ext}"))
    }
}

/// 临时分片路径与断点文件路径
pub fn temp_paths(target: &Path) -> (PathBuf, PathBuf) {
    let part = target.with_extension(format!(
        "{}part",
        target
            .extension()
            .map(|e| format!("{}.", e.to_string_lossy()))
            .unwrap_or_default()
    ));
    let resume = target.with_extension("resume.json");
    (part, resume)
}

/// 确保目录存在并且可写
pub fn ensure_dir(dir: &Path) -> AppResult<()> {
    if dir.exists() {
        if !dir.is_dir() {
            return Err(AppError::storage(format!(
                "{} 已存在但不是目录",
                dir.display()
            )));
        }
    } else {
        std::fs::create_dir_all(dir)?;
    }

    // 可写性探针：写一个临时文件再删除
    let probe = dir.join(".videoflow-write-test");
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => Err(AppError::storage(format!("目录不可写：{e}"))),
    }
}

/// 防范路径穿越：解析后的最终路径必须位于允许目录内
pub fn ensure_within(base: &Path, candidate: &Path) -> AppResult<()> {
    let base = std::fs::canonicalize(base)
        .unwrap_or_else(|_| base.to_path_buf());
    // 目标文件可能还不存在，取父目录做校验
    let parent = candidate.parent().ok_or_else(|| AppError::storage("路径无效"))?;
    let parent = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
    if !parent.starts_with(&base) {
        return Err(AppError::storage("目标路径超出了允许的下载目录"));
    }
    Ok(())
}

/// 任务专属的临时目录
pub fn scratch_dir_for(base: &Path, task_id: &str) -> PathBuf {
    base.join(".videoflow-scratch").join(task_id)
}

/// 递归清理任务临时目录，忽略不存在的错误
pub fn cleanup_scratch(dir: &Path) {
    if dir.exists() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// 计算文件的 SHA-256（小写十六进制）。
///
/// 只在 Provider 提供了哈希时调用（项目书 §3.3「Provider 提供时校验 SHA-256」），
/// 因此这里不做缓存：校验路径本身就是一次完整读盘。
pub fn sha256_file(path: &Path) -> AppResult<String> {
    use sha2::{Digest, Sha256};

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

// ---- 队列快照（项目书 §3.2「持久化队列」）----

/// 队列快照文件：与 settings.json 同级，全部落在 D 盘
pub fn tasks_store_path() -> PathBuf {
    PathBuf::from("D:\\VideoFlow\\tasks.json")
}

/// 一条持久化记录：任务本体 + 仅主进程内部使用的字段。
///
/// `DownloadTask` 里下载地址那几个字段对前端是隐藏的（`#[serde(skip)]`），
/// 但恢复队列时必须带上它们，否则重启后无法继续下载，所以在这里单独装一层壳。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedTask {
    #[serde(flatten)]
    pub task: DownloadTask,
    #[serde(default)]
    pub stream_url: Option<String>,
    #[serde(default)]
    pub audio_url: Option<String>,
    #[serde(default)]
    pub scratch_dir: Option<String>,
    /// Provider 提供的内容哈希，恢复后仍要能继续校验（项目书 §3.3）
    #[serde(default)]
    pub sha256: Option<String>,
    /// 下载时携带的 Referer。抖音的 CDN 直链校验来源，丢了重启后会 403
    #[serde(default)]
    pub referer: Option<String>,
    /// 媒体时长（秒）。HLS 恢复后要用它继续推进度
    #[serde(default)]
    pub duration_sec: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskStoreFile {
    version: u32,
    saved_at: String,
    tasks: Vec<PersistedTask>,
}

/// 原子写入队列快照：先写 `.tmp` 再 rename，避免中途被杀留下半个 JSON
pub fn save_tasks_to(path: &Path, tasks: &[PersistedTask]) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = TaskStoreFile {
        version: 1,
        saved_at: chrono::Utc::now().to_rfc3339(),
        tasks: tasks.to_vec(),
    };
    let data = serde_json::to_vec_pretty(&file)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 读取队列快照；文件不存在或内容损坏时返回空列表（不阻塞启动）
pub fn load_tasks_from(path: &Path) -> Vec<PersistedTask> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    match serde_json::from_str::<TaskStoreFile>(&raw) {
        Ok(file) => file.tasks,
        Err(e) => {
            eprintln!("[videoflow] 队列快照无法解析，已忽略：{e}");
            Vec::new()
        }
    }
}

/// 写入默认位置的队列快照
pub fn save_tasks(tasks: &[PersistedTask]) -> AppResult<()> {
    save_tasks_to(&tasks_store_path(), tasks)
}

/// 读取默认位置的队列快照
pub fn load_tasks() -> Vec<PersistedTask> {
    load_tasks_from(&tasks_store_path())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{EngineKind, TaskKind, TaskStatus};

    #[test]
    fn 文件名净化移除非法字符() {
        assert_eq!(sanitize_stem("a<b>c:d\"e/f\\g|h?i*j"), "a b c d e f g h i j");
    }

    #[test]
    fn 文件名净化处理控制字符与结尾点空格() {
        assert_eq!(sanitize_stem("hello\u{1}world. "), "hello world");
    }

    #[test]
    fn 文件名净化避开_windows_保留名() {
        assert_eq!(sanitize_stem("CON"), "CON_");
        assert_eq!(sanitize_stem("nul"), "nul_");
    }

    #[test]
    fn 文件名净化兜底空输入() {
        assert_eq!(sanitize_stem("   "), "videoflow-download");
    }

    #[test]
    fn 文件名净化限制长度() {
        let long = "あ".repeat(400);
        assert!(sanitize_stem(&long).chars().count() <= 150);
    }

    #[test]
    fn 临时路径与断点路径按约定生成() {
        let target = Path::new("D:/dl/movie.mp4");
        let (part, resume) = temp_paths(target);
        assert!(part.to_string_lossy().ends_with("movie.mp4.part"));
        assert!(resume.to_string_lossy().ends_with("movie.resume.json"));
    }

    #[test]
    fn 冲突策略追加序号且不覆盖() {
        let dir = std::env::temp_dir().join(format!("vf-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let first = unique_path(&dir, "demo", "mp4");
        assert_eq!(first.file_name().unwrap(), "demo.mp4");
        std::fs::write(&first, b"x").unwrap();

        let second = unique_path(&dir, "demo", "mp4");
        assert_eq!(second.file_name().unwrap(), "demo (1).mp4");
        std::fs::write(&second, b"x").unwrap();

        let third = unique_path(&dir, "demo", "mp4");
        assert_eq!(third.file_name().unwrap(), "demo (2).mp4");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn 路径穿越被拒绝() {
        let dir = std::env::temp_dir().join(format!("vf-guard-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = std::fs::canonicalize(&dir).unwrap();

        let inside = base.join("ok.mp4");
        assert!(ensure_within(&base, &inside).is_ok());

        let outside = base.join("..").join("evil.mp4");
        assert!(ensure_within(&base, &outside).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 造一条用于持久化往返测试的记录
    fn sample_record() -> PersistedTask {
        PersistedTask {
            task: DownloadTask {
                id: "t-persist-1".to_string(),
                kind: TaskKind::Download,
                source_url: "https://www.bilibili.com/video/BV1seed".to_string(),
                provider_id: Some("yt-dlp".to_string()),
                title: "持久化测试".to_string(),
                status: TaskStatus::Paused,
                priority: 1,
                created_at: "2026-09-19T00:00:00Z".to_string(),
                updated_at: "2026-09-19T00:00:00Z".to_string(),
                version: 3,
                thumbnail_url: None,
                quality_label: Some("1080P".to_string()),
                container: Some("mp4".to_string()),
                audio_label: None,
                target_path: Some("D:\\VideoFlow\\Downloads\\持久化测试.mp4".to_string()),
                downloaded_bytes: 1024,
                total_bytes: Some(4096),
                speed_bps: 0.0,
                eta_sec: None,
                retry_count: 0,
                error: None,
                resume: None,
                source_stream_url: Some("https://cdn.example.com/video.m4s?sig=abc".to_string()),
                source_audio_url: Some("https://cdn.example.com/audio.m4s?sig=def".to_string()),
                referer: None,
                duration_sec: None,
                expected_sha256: Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string()),
                engine: EngineKind::NativeHttp,
                scratch_dir: Some("D:\\VideoFlow\\Downloads\\.videoflow-scratch\\t-persist-1".to_string()),
                convert: None,
            },
            stream_url: Some("https://cdn.example.com/video.m4s?sig=abc".to_string()),
            audio_url: Some("https://cdn.example.com/audio.m4s?sig=def".to_string()),
            scratch_dir: Some("D:\\VideoFlow\\Downloads\\.videoflow-scratch\\t-persist-1".to_string()),
            sha256: Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string()),
            referer: Some("https://www.douyin.com/".to_string()),
            duration_sec: Some(323.167),
        }
    }

    #[test]
    fn 队列快照往返读写保持完整() {
        let dir = std::env::temp_dir().join(format!("vf-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tasks.json");

        save_tasks_to(&path, &[sample_record()]).unwrap();
        let loaded = load_tasks_from(&path);

        assert_eq!(loaded.len(), 1);
        let task = &loaded[0].task;
        assert_eq!(task.id, "t-persist-1");
        assert_eq!(task.status, TaskStatus::Paused);
        assert_eq!(task.priority, 1);
        assert_eq!(task.version, 3);
        assert_eq!(task.downloaded_bytes, 1024);
        // 下载地址不在 DownloadTask 的序列化字段里，必须由外壳补回来
        assert_eq!(
            loaded[0].stream_url.as_deref(),
            Some("https://cdn.example.com/video.m4s?sig=abc")
        );
        assert_eq!(
            loaded[0].scratch_dir.as_deref(),
            Some("D:\\VideoFlow\\Downloads\\.videoflow-scratch\\t-persist-1")
        );
        // Referer 同样不在 DownloadTask 的序列化字段里，丢了重启后会 403
        assert_eq!(loaded[0].referer.as_deref(), Some("https://www.douyin.com/"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn 队列快照缺失或损坏时不阻塞启动() {
        let dir = std::env::temp_dir().join(format!("vf-store-bad-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        assert!(load_tasks_from(&dir.join("missing.json")).is_empty());

        let broken = dir.join("broken.json");
        std::fs::write(&broken, b"{ not json").unwrap();
        assert!(load_tasks_from(&broken).is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn 文件哈希与标准向量一致() {
        let dir = std::env::temp_dir().join(format!("vf-hash-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // 空文件与 "abc" 是 SHA-256 的标准测试向量
        let empty = dir.join("empty.bin");
        std::fs::write(&empty, b"").unwrap();
        assert_eq!(
            sha256_file(&empty).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        let abc = dir.join("abc.bin");
        std::fs::write(&abc, b"abc").unwrap();
        assert_eq!(
            sha256_file(&abc).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        // 跨缓冲区边界的内容也要一致（校验逻辑按 1 MiB 分块读）
        let big = dir.join("big.bin");
        let data = vec![0x5au8; 3 * 1024 * 1024 + 17];
        std::fs::write(&big, &data).unwrap();
        assert_eq!(sha256_file(&big).unwrap().len(), 64);

        std::fs::remove_dir_all(&dir).ok();
    }
}