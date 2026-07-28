//! 统一下载调度器。
//!
//! 所有网络下载请求必须经过此模块。负责：
//! - 队列管理（FIFO + 优先级插队）
//! - 请求去重（同一 URL 只下载一次）
//! - 并发限制
//! - 重试策略（429 退避 / 401 停止 / 超时重试 3 次）

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

/// 下载优先级：数值越小越优先。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// 当前阅读页（最高）。
    Current = 0,
    /// 下一页预取。
    Next = 1,
    /// 封面生成。
    Cover = 2,
    /// 后台缓存。
    Background = 3,
}

/// 下载任务。
#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,
    pub dest: std::path::PathBuf,
    pub priority: Priority,
    /// Range 请求头（可选）。
    pub range: Option<String>,
    /// 用户名（WebDAV auth）。
    pub username: Option<String>,
    /// 密码。
    pub password: Option<String>,
}

/// 下载结果。
#[derive(Debug)]
pub enum DownloadResult {
    /// 下载成功，返回写入的文件路径。
    Success(std::path::PathBuf),
    /// 认证失败（401），需要更新密码。
    AuthFailed,
    /// 限流（429），可稍后重试。
    RateLimited,
    /// 其他错误。
    Error(String),
}

/// 统一下载器状态。
pub struct Downloader {
    /// 并发上限。保留供后续使用。
    #[allow(dead_code)]
    max_concurrency: usize,
    /// 正在进行的下载（URL → 取消标志）。
    _active: HashMap<String, ()>,
    /// 已完成缓存（URL → 本地路径），避免重复下载。
    cache: HashMap<String, std::path::PathBuf>,
}

impl Downloader {
    pub fn new(max_concurrency: usize) -> Self {
        Downloader {
            max_concurrency,
            _active: HashMap::new(),
            cache: HashMap::new(),
        }
    }

    /// 检查缓存是否已存在。
    pub fn is_cached(&self, url: &str) -> Option<&std::path::PathBuf> {
        self.cache.get(url)
    }

    /// 注册缓存（下载完成后调用）。
    pub fn register_cache(&mut self, url: String, path: std::path::PathBuf) {
        self.cache.insert(url, path);
    }

    /// 推荐的 WebDAV 并发数（保守）。
    pub fn default_webdav_concurrency() -> usize {
        2
    }
}

/// 全局下载器单例。
static DOWNLOADER: std::sync::OnceLock<Mutex<Downloader>> = std::sync::OnceLock::new();

pub fn global() -> &'static Mutex<Downloader> {
    DOWNLOADER.get_or_init(|| Mutex::new(Downloader::new(Downloader::default_webdav_concurrency())))
}

/// 执行一次 HTTP GET（带 Range + Basic Auth），写入 dest 文件，返回结果。
/// 内置重试逻辑：429 退避 5s 重试最多 3 次，401 立即返回 AuthFailed。
pub fn blocking_download(task: &DownloadTask) -> DownloadResult {
    use reqwest::blocking::Client;
    use reqwest::header::RANGE;
    use std::io::Write;

    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(10))
        .build();

    let client = match client {
        Ok(c) => c,
        Err(e) => return DownloadResult::Error(format!("创建 HTTP 客户端失败: {e}")),
    };

    let mut last_err = String::new();

    for attempt in 0..3 {
        let mut req = client.get(&task.url);

        if let Some(ref range) = task.range {
            req = req.header(RANGE, range.as_str());
        }
        if let (Some(u), Some(p)) = (&task.username, &task.password) {
            req = req.basic_auth(u, Some(p));
        }

        let resp = match req.send() {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("请求失败: {e}");
                if attempt < 2 {
                    std::thread::sleep(Duration::from_secs(2));
                }
                continue;
            }
        };

        let status = resp.status();

        if status.is_success() || status == reqwest::StatusCode::PARTIAL_CONTENT {
            // 写入文件
            let bytes = match resp.bytes() {
                Ok(b) => b,
                Err(e) => {
                    last_err = format!("读取响应失败: {e}");
                    continue;
                }
            };

            if let Some(parent) = task.dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            let mut file = match std::fs::File::create(&task.dest) {
                Ok(f) => f,
                Err(e) => return DownloadResult::Error(format!("创建文件失败: {e}")),
            };

            if let Err(e) = file.write_all(&bytes) {
                return DownloadResult::Error(format!("写入文件失败: {e}"));
            }

            return DownloadResult::Success(task.dest.clone());
        }

        match status.as_u16() {
            401 => return DownloadResult::AuthFailed,
            403 => return DownloadResult::Error("403 Forbidden".to_string()),
            429 => {
                // 退避重试
                let wait = 5u64 * (attempt + 1) as u64;
                std::thread::sleep(Duration::from_secs(wait));
                last_err = format!("429 Too Many Requests (重试 {}/{})", attempt + 1, 3);
            }
            404 => return DownloadResult::Error("404 Not Found".to_string()),
            _ => {
                last_err = format!("HTTP {}", status.as_u16());
                if attempt < 2 {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }

    DownloadResult::Error(last_err)
}
