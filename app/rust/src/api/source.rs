//! 远程书源 API(WebDAV / SFTP 会话与浏览)。

use super::book::{register_book, BookInfo, CropRect, DirEntry, PageImage};
use crate::cache;
use crate::document;
use crate::document::remote_folder::{AdapterFolderReader, RemoteFolderBook};
use crate::remote_scan::adapter::{RemoteCapabilities, RemoteProviderAdapter, RemoteScanError};
use crate::remote_scan::model::{classify, normalize_path, RemoteEntry};
use crate::source::baidu::{self as baidu_source, BaiduClient};
use crate::source::cloud115::{self as cloud115_source, Cloud115Client, Cloud115WebClient};
use crate::source::quark::{self as quark_source, QuarkClient};
use crate::source::sftp::{self as sftp_source, SftpClient};
use crate::source::webdav::{self, DownloadProgress, WebDavClient, WebDavFile};
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, OnceLock};

static SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<WebDavClient>>>> = OnceLock::new();
static SFTP_SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<SftpClient>>>> = OnceLock::new();
static BAIDU_SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<BaiduClient>>>> = OnceLock::new();
static CLOUD115_SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<Cloud115Client>>>> = OnceLock::new();
static CLOUD115_COOKIE_SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<Cloud115WebClient>>>> =
    OnceLock::new();
static QUARK_SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<QuarkClient>>>> = OnceLock::new();
static NEXT: OnceLock<Mutex<u64>> = OnceLock::new();

/// 正在进行的下载进度追踪表(session_id -> DownloadProgress)。
static DOWNLOADS: OnceLock<Mutex<HashMap<u64, Arc<DownloadProgress>>>> = OnceLock::new();
static SFTP_DOWNLOADS: OnceLock<Mutex<HashMap<u64, Arc<DownloadProgress>>>> = OnceLock::new();
static BAIDU_DOWNLOADS: OnceLock<Mutex<HashMap<u64, Arc<DownloadProgress>>>> = OnceLock::new();
static CLOUD115_DOWNLOADS: OnceLock<Mutex<HashMap<u64, Arc<DownloadProgress>>>> = OnceLock::new();
static CLOUD115_COOKIE_DOWNLOADS: OnceLock<Mutex<HashMap<u64, Arc<DownloadProgress>>>> =
    OnceLock::new();
static QUARK_DOWNLOADS: OnceLock<Mutex<HashMap<u64, Arc<DownloadProgress>>>> = OnceLock::new();

fn downloads() -> &'static Mutex<HashMap<u64, Arc<DownloadProgress>>> {
    DOWNLOADS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn sessions() -> &'static Mutex<HashMap<u64, Arc<WebDavClient>>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn sftp_sessions() -> &'static Mutex<HashMap<u64, Arc<SftpClient>>> {
    SFTP_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn sftp_downloads() -> &'static Mutex<HashMap<u64, Arc<DownloadProgress>>> {
    SFTP_DOWNLOADS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn baidu_sessions() -> &'static Mutex<HashMap<u64, Arc<BaiduClient>>> {
    BAIDU_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn baidu_downloads() -> &'static Mutex<HashMap<u64, Arc<DownloadProgress>>> {
    BAIDU_DOWNLOADS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cloud115_sessions() -> &'static Mutex<HashMap<u64, Arc<Cloud115Client>>> {
    CLOUD115_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cloud115_downloads() -> &'static Mutex<HashMap<u64, Arc<DownloadProgress>>> {
    CLOUD115_DOWNLOADS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cloud115_cookie_sessions() -> &'static Mutex<HashMap<u64, Arc<Cloud115WebClient>>> {
    CLOUD115_COOKIE_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cloud115_cookie_downloads() -> &'static Mutex<HashMap<u64, Arc<DownloadProgress>>> {
    CLOUD115_COOKIE_DOWNLOADS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn quark_sessions() -> &'static Mutex<HashMap<u64, Arc<QuarkClient>>> {
    QUARK_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn quark_downloads() -> &'static Mutex<HashMap<u64, Arc<DownloadProgress>>> {
    QUARK_DOWNLOADS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_id() -> u64 {
    let m = NEXT.get_or_init(|| Mutex::new(0));
    let mut g = m.lock().unwrap();
    *g += 1;
    *g
}

pub(crate) fn get_session(id: u64) -> Result<Arc<WebDavClient>> {
    sessions()
        .lock()
        .unwrap()
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| anyhow::anyhow!("无效的 WebDAV 会话: {id}"))
}

pub(crate) fn get_sftp_session(id: u64) -> Result<Arc<SftpClient>> {
    sftp_sessions()
        .lock()
        .unwrap()
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| anyhow::anyhow!("无效的 SFTP 会话: {id}"))
}

pub(crate) fn get_baidu_session(id: u64) -> Result<Arc<BaiduClient>> {
    baidu_sessions()
        .lock()
        .unwrap()
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| anyhow::anyhow!("无效的百度网盘会话: {id}"))
}

pub(crate) fn get_cloud115_session(id: u64) -> Result<Arc<Cloud115Client>> {
    cloud115_sessions()
        .lock()
        .unwrap()
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| anyhow::anyhow!("无效的 115 网盘会话: {id}"))
}

pub(crate) fn get_quark_session(id: u64) -> Result<Arc<QuarkClient>> {
    quark_sessions()
        .lock()
        .unwrap()
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| anyhow::anyhow!("无效的夸克网盘会话: {id}"))
}

/// 打开策略（全局设置传入）：auto=先下载失败转流式，download=强制整本，stream=直接流式。
#[derive(Clone, Copy, PartialEq)]
enum OpenStrategy {
    Auto,
    Download,
    Stream,
}

fn parse_strategy(s: &str) -> OpenStrategy {
    match s {
        "download" => OpenStrategy::Download,
        "stream" => OpenStrategy::Stream,
        _ => OpenStrategy::Auto,
    }
}

/// WebDAV 会话信息。
pub struct WebDavSession {
    pub id: u64,
    pub root: String,
    /// 服务器能力报告摘要(Dart 侧用于显示状态标记)。
    pub capability_label: String, // "local" | "webdav_range" | "webdav_norange"
}

/// 连接 WebDAV 服务器并自动探测能力,返回会话句柄与初始浏览路径。
pub async fn webdav_connect(
    url: String,
    username: String,
    password: String,
) -> Result<WebDavSession> {
    let (client, root, label) =
        tokio::task::spawn_blocking(move || -> Result<(WebDavClient, String, String)> {
            let (mut client, root) = WebDavClient::new(&url, &username, &password)?;
            client.check_and_probe(&root)?;
            let cap = &client.capability;
            let label = if cap.avg_rtt_ms < 20.0 {
                "local".to_string()
            } else if cap.range_supported {
                "webdav_range".to_string()
            } else {
                "webdav_norange".to_string()
            };
            Ok((client, root, label))
        })
        .await??;
    let id = next_id();
    sessions().lock().unwrap().insert(id, Arc::new(client));
    Ok(WebDavSession {
        id,
        root,
        capability_label: label,
    })
}

/// 断开 WebDAV 会话。在 blocking 线程销毁客户端,避免异步上下文 drop 其内部 runtime。
pub async fn webdav_disconnect(id: u64) {
    let client = sessions().lock().unwrap().remove(&id);
    if let Some(client) = client {
        let _ = tokio::task::spawn_blocking(move || drop(client)).await;
    }
}

/// 列出 WebDAV 目录内容(目录在前,自然排序)。
pub async fn webdav_list(session: u64, path: String) -> Result<Vec<DirEntry>> {
    let client = get_session(session)?;
    let entries = tokio::task::spawn_blocking(move || client.list(&path)).await??;
    Ok(entries
        .into_iter()
        .map(|e| DirEntry {
            name: e.name,
            path: e.path,
            is_dir: e.is_dir,
            size: e.size,
            mtime: e.mtime,
        })
        .collect())
}

/// 打开 WebDAV 上的书籍。
/// 策略(strategy): "auto" **流式优先**（第 69 轮语义翻转）：命中 raw 缓存则本地打开，
/// 否则先按需 range 流式读，失败才整本下载到 raw/ 缓存
/// （实测：整本下载让打开时间 ∝ 文件大小，一本 49MB 的漫画要传 49MB）。
/// "download" 强制整本下载(失败报错); "stream" 直接流式(无 Range 服务器仍需整本)。
/// 若已有缓存则直接复用(秒开)。
/// 这是四层架构的关键: 阅读器只操作本地资源。
/// 下载进度可通过 webdav_download_progress(session) 轮询。
pub async fn open_webdav_book(session: u64, path: String, strategy: String) -> Result<BookInfo> {
    let client = get_session(session)?;
    let origin = client.origin().to_string();
    let cache_ns = format!("webdav|{}|{}", origin, path);
    let strat = parse_strategy(&strategy);

    // 记录本次下载进度(供轮询); stream 模式不下载不注册
    let progress = if strat != OpenStrategy::Stream {
        let file_size = {
            let client = Arc::clone(&client);
            let path = path.clone();
            tokio::task::spawn_blocking(move || client.file_size(&path)).await??
        };
        let p = Arc::new(DownloadProgress::new(file_size));
        downloads().lock().unwrap().insert(session, Arc::clone(&p));
        Some(p)
    } else {
        None
    };

    let book = {
        let client = Arc::clone(&client);
        let path = path.clone();
        tokio::task::spawn_blocking(move || -> Result<Box<dyn document::Document>> {
            match strat {
                OpenStrategy::Download => {
                    // 强制整本: 失败直接报错, 不静默转流式
                    let local_path = client.download_to_raw_cache(&path, progress)?;
                    tracing::info!("WebDAV 整本已缓存: {}", local_path.display());
                    let src = crate::source::local::LocalFile::open(&local_path)?;
                    document::open_document(src, &path)
                }
                OpenStrategy::Stream => {
                    // 缓存优先：已有 raw/ 本地缓存直接本地打开，不联网
                    if let Some(local_path) = webdav::raw_cache_path(client.origin(), &path) {
                        tracing::info!("WebDAV 命中缓存，直接本地打开: {}", local_path.display());
                        let src = crate::source::local::LocalFile::open(&local_path)?;
                        document::open_document(src, &path)
                    } else if client.range_supported(&path)? {
                        let len = client.file_size(&path)?;
                        let src = WebDavFile::new(client, path.clone(), len);
                        document::open_document(src, &path)
                    } else {
                        // 无 Range 服务器无法流式, 只能整本下载(raw/ 回退)
                        let src = client.download_full(&path)?;
                        document::open_document(src, &path)
                    }
                }
                OpenStrategy::Auto => {
                    // 优先尝试整本下载到 raw/ 缓存
                    match client.download_to_raw_cache(&path, progress) {
                        Ok(local_path) => {
                            tracing::info!("WebDAV 整本已缓存: {}", local_path.display());
                            let src = crate::source::local::LocalFile::open(&local_path)?;
                            document::open_document(src, &path)
                        }
                        Err(e) => {
                            tracing::warn!("WebDAV 整本下载失败, 回退到 Range 流式: {e}");
                            if client.range_supported(&path)? {
                                let len = client.file_size(&path)?;
                                let src = WebDavFile::new(client, path.clone(), len);
                                document::open_document(src, &path)
                            } else {
                                let src = client.download_full(&path)?;
                                document::open_document(src, &path)
                            }
                        }
                    }
                }
            }
        })
        .await??
    };

    // 下载完成后从跟踪表移除
    if strat != OpenStrategy::Stream {
        downloads().lock().unwrap().remove(&session);
    }

    Ok(register_book(book, &cache_ns))
}

pub(crate) fn get_cloud115_cookie_session(id: u64) -> Result<Arc<Cloud115WebClient>> {
    cloud115_cookie_sessions()
        .lock()
        .unwrap()
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| anyhow::anyhow!("115 Cookie 会话不存在，请重新连接"))
}

enum RemoteSessionClient {
    WebDav(Arc<WebDavClient>),
    Sftp(Arc<SftpClient>),
    Baidu(Arc<BaiduClient>),
    Cloud115(Arc<Cloud115Client>),
    Cloud115Cookie(Arc<Cloud115WebClient>),
    Quark(Arc<QuarkClient>),
}

struct SessionRemoteAdapter {
    client: RemoteSessionClient,
    root: String,
    opaque_paths: bool,
    path_ids: Mutex<HashMap<String, String>>,
}

impl SessionRemoteAdapter {
    fn provider_path(&self, canonical: &str) -> String {
        if !self.opaque_paths {
            return canonical.to_string();
        }
        self.path_ids
            .lock()
            .unwrap()
            .get(&normalize_path(canonical))
            .cloned()
            .unwrap_or_else(|| self.root.clone())
    }
}

fn canonical_child_path(parent: &str, name: &str) -> String {
    let name = name.trim_matches(['/', '\\']);
    normalize_path(&format!("{}/{}", normalize_path(parent), name))
}

fn retry_after_ms(text: &str) -> Option<u64> {
    ["retry-after-ms=", "retry_after_ms="]
        .iter()
        .find_map(|prefix| {
            text.find(prefix).and_then(|index| {
                text[index + prefix.len()..]
                    .split(|c: char| !c.is_ascii_digit())
                    .next()?
                    .parse()
                    .ok()
            })
        })
}

fn scan_error(error: anyhow::Error) -> RemoteScanError {
    let text = error.to_string().to_ascii_lowercase();
    if text.contains("http 405") || text.contains("http: 405") {
        RemoteScanError::HttpStatus {
            stage: "downurl".into(),
            status: 405,
        }
    } else if text.contains("401")
        || text.contains("登录")
        || text.contains("token")
        || text.contains("认证")
    {
        RemoteScanError::Unauthorized
    } else if text.contains("403") || text.contains("权限") || text.contains("forbidden") {
        RemoteScanError::Forbidden
    } else if text.contains("404") || text.contains("不存在") || text.contains("not found") {
        RemoteScanError::NotFound
    } else if text.contains("429") || text.contains("频繁") || text.contains("rate") {
        RemoteScanError::RateLimited {
            retry_after_ms: retry_after_ms(&text),
        }
    } else if text.contains("timeout")
        || text.contains("timed out")
        || text.contains("连接")
        || text.contains("network")
    {
        RemoteScanError::TransientNetwork("provider_unavailable".into())
    } else {
        RemoteScanError::Provider("provider_error".into())
    }
}

/// Preserve protocol categories from provider Range reads.  The provider
/// clients expose `io::Result` for their streaming API, so only a sanitized
/// classification crosses the adapter boundary; response bodies and URLs
/// are never persisted or shown to the user.
fn scan_io_error(error: io::Error) -> RemoteScanError {
    let text = error.to_string().to_ascii_lowercase();
    if text.contains("http 405") || text.contains("http: 405") {
        RemoteScanError::HttpStatus {
            stage: "range_read".into(),
            status: 405,
        }
    } else if text.contains("401") || text.contains("unauthorized") {
        RemoteScanError::Unauthorized
    } else if text.contains("403") || text.contains("forbidden") {
        RemoteScanError::Forbidden
    } else if text.contains("404") || text.contains("not found") {
        RemoteScanError::NotFound
    } else if text.contains("429") || text.contains("rate") || text.contains("频繁") {
        RemoteScanError::RateLimited {
            retry_after_ms: retry_after_ms(&text),
        }
    } else if text.contains("200") && text.contains("range") {
        RemoteScanError::RangeUnavailable
    } else if text.contains("timeout")
        || text.contains("timed out")
        || text.contains("连接")
        || text.contains("network")
        || text.contains("请求失败")
    {
        RemoteScanError::TransientNetwork("range_read_failed".into())
    } else {
        RemoteScanError::Provider("range_read_failed".into())
    }
}

impl RemoteProviderAdapter for SessionRemoteAdapter {
    #[flutter_rust_bridge::frb(ignore)]
    fn list(
        &self,
        path: &str,
        cursor: Option<&str>,
    ) -> std::result::Result<(Vec<RemoteEntry>, Option<String>), RemoteScanError> {
        if cursor.is_some() {
            return Err(RemoteScanError::MalformedResponse(
                "unexpected_cursor".into(),
            ));
        }
        let provider_path = self.provider_path(path);
        let entries = match &self.client {
            RemoteSessionClient::WebDav(c) => c.list(&provider_path),
            RemoteSessionClient::Sftp(c) => c.list(&provider_path),
            RemoteSessionClient::Baidu(c) => c.list(&provider_path),
            RemoteSessionClient::Cloud115(c) => c.list(&provider_path),
            RemoteSessionClient::Cloud115Cookie(c) => c.list(&provider_path),
            RemoteSessionClient::Quark(c) => c.list(&provider_path),
        }
        .map_err(scan_error)?;
        let mapped = entries
            .into_iter()
            .map(|entry| {
                let logical_path = if self.opaque_paths {
                    canonical_child_path(path, &entry.name)
                } else {
                    normalize_path(&entry.path)
                };
                if self.opaque_paths {
                    self.path_ids
                        .lock()
                        .unwrap()
                        .insert(logical_path.clone(), entry.path.clone());
                }
                RemoteEntry {
                    asset_kind: classify(&entry.name, entry.is_dir),
                    name: entry.name,
                    logical_path,
                    provider_path: Some(entry.path.clone()),
                    is_dir: entry.is_dir,
                    size: (entry.size != 0).then_some(entry.size),
                    mtime: (entry.mtime != 0).then_some(entry.mtime),
                }
            })
            .collect();
        Ok((mapped, None))
    }

    #[flutter_rust_bridge::frb(ignore)]
    fn read_range(
        &self,
        path: &str,
        offset: u64,
        length: u64,
    ) -> std::result::Result<Vec<u8>, RemoteScanError> {
        let length = usize::try_from(length).map_err(|_| RemoteScanError::RangeUnavailable)?;
        let mut bytes = vec![0; length];
        let provider_path = self.provider_path(path);
        let read = match &self.client {
            RemoteSessionClient::WebDav(c) => c.read_range(&provider_path, offset, &mut bytes),
            RemoteSessionClient::Sftp(c) => c.read_at(&provider_path, offset, &mut bytes),
            RemoteSessionClient::Baidu(c) => {
                let (url, _) = c.dlink(&provider_path).map_err(scan_error)?;
                c.read_range_with_dlink(&url, &provider_path, offset, &mut bytes)
            }
            RemoteSessionClient::Cloud115(c) => {
                let (url, _) = c.downurl(&provider_path).map_err(scan_error)?;
                c.read_range_url(&url, offset, &mut bytes)
            }
            RemoteSessionClient::Cloud115Cookie(c) => {
                let info = c.downurl(&provider_path).map_err(scan_error)?;
                c.read_range_url(&info.url, offset, &mut bytes)
            }
            RemoteSessionClient::Quark(c) => {
                let info = c.downlink(&provider_path).map_err(scan_error)?;
                c.read_range_url(&info.url, offset, &mut bytes)
            }
        }
        .map_err(scan_io_error)?;
        bytes.truncate(read);
        Ok(bytes)
    }

    #[flutter_rust_bridge::frb(ignore)]
    fn read_file_limited(
        &self,
        _path: &str,
        _max_bytes: u64,
    ) -> std::result::Result<Vec<u8>, RemoteScanError> {
        // A provider-specific bounded streaming GET is not available here.
        // Return a typed capability failure instead of falling back to any
        // existing whole-file/archive download method.
        Err(RemoteScanError::RangeUnavailable)
    }

    #[flutter_rust_bridge::frb(ignore)]
    fn normalize_path(&self, path: &str) -> String {
        normalize_path(path)
    }

    #[flutter_rust_bridge::frb(ignore)]
    fn register_path(&self, logical_path: &str, provider_path: &str) {
        self.path_ids
            .lock()
            .unwrap()
            .insert(normalize_path(logical_path), provider_path.to_string());
    }

    #[flutter_rust_bridge::frb(ignore)]
    fn cache_path(&self, logical_path: &str) -> Option<String> {
        if !self.opaque_paths {
            return Some(normalize_path(logical_path));
        }
        self.path_ids
            .lock()
            .unwrap()
            .get(&normalize_path(logical_path))
            .cloned()
    }

    #[flutter_rust_bridge::frb(ignore)]
    fn capabilities(
        &self,
        _path: &str,
        _fingerprint: &str,
    ) -> std::result::Result<RemoteCapabilities, RemoteScanError> {
        let provider_path = self.provider_path(_path);
        let range_read = match &self.client {
            RemoteSessionClient::WebDav(c) => c.range_probe_checked(&provider_path)?.supported,
            RemoteSessionClient::Sftp(_) => true,
            RemoteSessionClient::Baidu(c) => {
                let (url, _) = c.dlink(&provider_path).map_err(scan_error)?;
                c.probe_range_checked(&url)?.supported
            }
            RemoteSessionClient::Cloud115(c) => {
                let (url, _) = c.downurl(&provider_path).map_err(scan_error)?;
                c.probe_checked(&url)?.supported
            }
            RemoteSessionClient::Cloud115Cookie(c) => {
                let info = c.downurl(&provider_path).map_err(scan_error)?;
                c.probe_checked(&info.url)?.supported
            }
            RemoteSessionClient::Quark(c) => {
                let info = c.downlink(&provider_path).map_err(scan_error)?;
                c.probe_checked(&info.url)?.supported
            }
        };
        Ok(RemoteCapabilities {
            range_read,
            pagination: false,
        })
    }
}

fn remote_folder_cache_ns(source_type: &str, session: u64, path: &str) -> Result<String> {
    let origin = match source_type {
        "webdav" => get_session(session)?.origin().to_string(),
        "sftp" => get_sftp_session(session)?.endpoint().to_string(),
        "baidu" => get_baidu_session(session)?.origin(),
        "115" => {
            if let Ok(client) = get_cloud115_cookie_session(session) {
                client.origin()
            } else {
                get_cloud115_session(session)?.origin()
            }
        }
        "quark" => get_quark_session(session)?.origin(),
        _ => return Err(anyhow!("unsupported remote folder source type")),
    };
    Ok(format!("{source_type}|{origin}|{}", normalize_path(path)))
}

fn prime_remote_folder_locator(
    adapter: &dyn RemoteProviderAdapter,
    logical_path: &str,
) -> std::result::Result<(), RemoteScanError> {
    let logical_path = normalize_path(logical_path);
    if logical_path != "/" {
        let mut parent = String::from("/");
        for segment in logical_path.trim_start_matches('/').split('/') {
            let _ = adapter.list(&parent, None)?;
            parent = canonical_child_path(&parent, segment);
        }
    }
    let _ = adapter.list(&logical_path, None)?;
    Ok(())
}

/// Whether a complete, successfully published image-folder manifest exists.
/// This is a local SQLite query and never creates a provider session.
pub fn remote_image_folder_manifest_complete(source_id: String, path: String) -> bool {
    crate::remote_scan::persistence::load_complete_image_folder_manifest(
        &crate::db::get().lock().unwrap(),
        &source_id,
        &path,
    )
    .map(|entries| !entries.is_empty())
    .unwrap_or(false)
}

/// Open a committed remote image folder without creating an archive raw cache.
pub async fn open_remote_folder_book(
    source_type: String,
    source_id: String,
    session: u64,
    path: String,
    title: String,
) -> Result<BookInfo> {
    let entries = crate::remote_scan::persistence::load_complete_image_folder_manifest(
        &crate::db::get().lock().unwrap(),
        &source_id,
        &path,
    )?;
    if entries.is_empty() {
        return Err(anyhow!("remote image-folder manifest is incomplete"));
    }
    let adapter = remote_provider_adapter(&source_type, session, "/")?;
    let path_for_prime = path.clone();
    let adapter_for_prime = Arc::clone(&adapter);
    tokio::task::spawn_blocking(move || {
        prime_remote_folder_locator(adapter_for_prime.as_ref(), &path_for_prime)
            .map_err(|error| anyhow!(error))
    })
    .await??;
    let cache_ns = remote_folder_cache_ns(&source_type, session, &path)?;
    let reader = Arc::new(AdapterFolderReader::new(adapter));
    let book = RemoteFolderBook::open(entries, reader, title)?;
    Ok(register_book(Box::new(book), &cache_ns))
}

pub(crate) fn remote_provider_adapter(
    source_type: &str,
    session: u64,
    root_path: &str,
) -> Result<Arc<dyn RemoteProviderAdapter>> {
    if matches!(source_type, "smb" | "local") {
        return Err(anyhow!("local-only source is not eligible for remote scan"));
    }
    if !supports_remote_scan(source_type) {
        return Err(anyhow!("unsupported remote source type"));
    }
    let (client, default_root, opaque_paths) = match source_type {
        "webdav" => (
            RemoteSessionClient::WebDav(get_session(session)?),
            root_path.to_string(),
            false,
        ),
        "sftp" => (
            RemoteSessionClient::Sftp(get_sftp_session(session)?),
            root_path.to_string(),
            false,
        ),
        "baidu" => {
            let client = get_baidu_session(session)?;
            let root = client.root().to_string();
            (RemoteSessionClient::Baidu(client), root, true)
        }
        "115" => {
            if let Ok(client) = get_cloud115_cookie_session(session) {
                let root = client.root().to_string();
                (RemoteSessionClient::Cloud115Cookie(client), root, true)
            } else {
                let client = get_cloud115_session(session)?;
                let root = client.root_id().to_string();
                (RemoteSessionClient::Cloud115(client), root, true)
            }
        }
        "quark" => {
            let client = get_quark_session(session)?;
            let root = client.root().to_string();
            (RemoteSessionClient::Quark(client), root, true)
        }
        _ => unreachable!("source type was validated above"),
    };
    let root = if root_path.trim().is_empty() || root_path == "/" {
        default_root
    } else {
        root_path.to_string()
    };
    let path_ids = Mutex::new(HashMap::from([("/".to_string(), root.clone())]));
    Ok(Arc::new(SessionRemoteAdapter {
        client,
        root,
        opaque_paths,
        path_ids,
    }))
}

fn supports_remote_scan(source_type: &str) -> bool {
    matches!(source_type, "webdav" | "sftp" | "baidu" | "115" | "quark")
}

#[cfg(test)]
mod remote_scan_adapter_tests {
    use super::*;

    #[test]
    fn opaque_child_ids_keep_canonical_nested_hierarchy() {
        assert_eq!(
            canonical_child_path("/series", "volume 1"),
            "/series/volume 1"
        );
        assert_eq!(
            canonical_child_path("/series/volume 1", "book.cbz"),
            "/series/volume 1/book.cbz"
        );
    }

    #[test]
    fn retry_after_metadata_is_preserved_without_exposing_the_error() {
        assert!(matches!(
            scan_error(anyhow!("HTTP 429 retry-after-ms=275 secret=hidden")),
            RemoteScanError::RateLimited {
                retry_after_ms: Some(275)
            }
        ));
        let sanitized = format!(
            "{:?}",
            scan_error(anyhow!(
                "network Authorization=Bearer-secret cookie=session-secret https://signed.example/x?token=secret"
            ))
        );
        for secret in [
            "Bearer-secret",
            "session-secret",
            "signed.example",
            "token=secret",
        ] {
            assert!(!sanitized.contains(secret));
        }
    }

    #[test]
    fn provider_factory_contract_covers_every_remote_source_and_excludes_local_only_sources() {
        for source_type in ["webdav", "sftp", "baidu", "115", "quark"] {
            assert!(supports_remote_scan(source_type), "{source_type}");
        }
        for source_type in ["local", "smb", "m8", "unknown"] {
            assert!(!supports_remote_scan(source_type), "{source_type}");
        }
    }
}

/// 查询当前下载进度(0.0 ~ 1.0),若 session 不在下载中则返回 1.0。
pub fn webdav_download_progress(session: u64) -> f64 {
    downloads()
        .lock()
        .unwrap()
        .get(&session)
        .map(|p| p.fraction())
        .unwrap_or(1.0)
}

/// 检查某 WebDAV 漫画是否已有 raw/ 本地缓存。
pub fn webdav_has_raw_cache(session: u64, path: String) -> bool {
    let client = match get_session(session) {
        Ok(c) => c,
        Err(_) => return false,
    };
    webdav::raw_cache_path(client.origin(), &path).is_some()
}

/// 缓存命中时直接本地打开远程书（封面编辑器等纯本地操作），全程不联网。
/// 未命中返回 `Ok(None)`，由调用方回退到策略打开（auto/download/stream）。
/// 命中时复用对应书源的 page/ 磁盘缓存命名空间，翻页与封面可直接命中本地。
pub async fn open_cached_remote_book(
    kind: String,
    session: u64,
    path: String,
) -> Result<Option<BookInfo>, String> {
    // 按书源类型解析 raw/ 缓存路径与缓存命名空间（与 open_*_book 保持一致）
    let (local_path, cache_ns) = match kind.as_str() {
        "webdav" => {
            let client = get_session(session).map_err(|e| e.to_string())?;
            let origin = client.origin().to_string();
            (
                webdav::raw_cache_path(&origin, &path),
                format!("webdav|{}|{}", origin, path),
            )
        }
        "sftp" => {
            let client = get_sftp_session(session).map_err(|e| e.to_string())?;
            let endpoint = client.endpoint().to_string();
            (
                sftp_source::raw_cache_path(&endpoint, &path),
                format!("sftp|{}|{}", endpoint, path),
            )
        }
        "baidu" => {
            let client = get_baidu_session(session).map_err(|e| e.to_string())?;
            let origin = client.origin();
            (
                baidu_source::raw_cache_path(&origin, &path),
                format!("baidu|{}|{}", origin, path),
            )
        }
        "115" => {
            // Cookie 模式与官方 APP ID 模式会话表不同：先查 Cookie 表，再查设备表
            if let Some(client) = cloud115_cookie_sessions()
                .lock()
                .unwrap()
                .get(&session)
                .cloned()
            {
                let origin = client.origin();
                (
                    cloud115_source::web_raw_cache_path(&origin, &path),
                    format!("115|{}|{}", origin, path),
                )
            } else {
                let client = get_cloud115_session(session).map_err(|e| e.to_string())?;
                let origin = client.origin();
                (
                    cloud115_source::raw_cache_path(&origin, &path),
                    format!("115|{}|{}", origin, path),
                )
            }
        }
        "quark" => {
            let client = get_quark_session(session).map_err(|e| e.to_string())?;
            let origin = client.origin();
            (
                quark_source::raw_cache_path(&origin, &path),
                format!("quark|{}|{}", origin, path),
            )
        }
        _ => return Err(format!("未知远程书源类型: {kind}")),
    };

    let local_path = match local_path {
        Some(p) => p,
        None => return Ok(None),
    };

    let cache_ns_open = cache_ns.clone();
    let book = tokio::task::spawn_blocking(move || -> Result<Box<dyn document::Document>> {
        // 缓存目录里的文件名即真实文件名（下载时按源文件名落盘）
        let name = local_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file.cbz".to_string());
        let src = crate::source::local::LocalFile::open(&local_path)?;
        document::open_document(src, &name)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    Ok(Some(register_book(book, &cache_ns_open)))
}

/// 上传文件到 WebDAV 路径（P2 同步包推送）。
pub async fn webdav_upload_file(session: u64, path: String, data: Vec<u8>) -> Result<(), String> {
    let client = get_session(session).map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || client.upload_file(&path, &data))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// 下载 WebDAV 文件到内存（P2 同步包拉取）。
pub async fn webdav_download_file(session: u64, path: String) -> Result<Vec<u8>, String> {
    let client = get_session(session).map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || client.download_file(&path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// 在 WebDAV 服务器幂等创建目录（P2 同步目录准备）。
pub async fn webdav_make_dir(session: u64, path: String) -> Result<(), String> {
    let client = get_session(session).map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || client.make_dir(&path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// 删除 WebDAV 文件（归档清理，404 视为已删除）。
pub async fn webdav_delete_file(session: u64, path: String) -> Result<(), String> {
    let client = get_session(session).map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || client.delete_file(&path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// SFTP 会话信息。
pub struct SftpSessionInfo {
    pub id: u64,
    pub root: String,
    /// 能力标记（Dart 侧显示用）。
    pub capability_label: String, // "sftp"
}

/// 连接 SFTP 服务器（密码认证），返回会话句柄；root 固定为 `/`。
pub async fn sftp_connect(
    host: String,
    port: u16,
    username: String,
    password: String,
) -> Result<SftpSessionInfo> {
    let client =
        tokio::task::spawn_blocking(move || SftpClient::connect(&host, port, &username, &password))
            .await??;
    let id = next_id();
    sftp_sessions().lock().unwrap().insert(id, Arc::new(client));
    Ok(SftpSessionInfo {
        id,
        root: "/".to_string(),
        capability_label: "sftp".to_string(),
    })
}

/// 断开 SFTP 会话（在 blocking 线程释放连接与 runtime）。
pub async fn sftp_disconnect(id: u64) {
    let client = sftp_sessions().lock().unwrap().remove(&id);
    if let Some(client) = client {
        let _ = tokio::task::spawn_blocking(move || {
            client.disconnect();
            drop(client);
        })
        .await;
    }
}

/// 列出 SFTP 目录内容（目录在前,自然排序）。
pub async fn sftp_list(session: u64, path: String) -> Result<Vec<DirEntry>> {
    let client = get_sftp_session(session)?;
    let entries = tokio::task::spawn_blocking(move || client.list(&path)).await??;
    Ok(entries
        .into_iter()
        .map(|e| DirEntry {
            name: e.name,
            path: e.path,
            is_dir: e.is_dir,
            size: e.size,
            mtime: e.mtime,
        })
        .collect())
}

/// 打开 SFTP 上的书籍，strategy 见 [`open_webdav_book`]。
/// 整本下载优先（进度经 sftp_download_progress 轮询）；失败回退 SftpFile 流式。
pub async fn open_sftp_book(session: u64, path: String, strategy: String) -> Result<BookInfo> {
    let client = get_sftp_session(session)?;
    let endpoint = client.endpoint().to_string();
    let cache_ns = format!("sftp|{}|{}", endpoint, path);
    let strat = parse_strategy(&strategy);

    let progress = if strat != OpenStrategy::Stream {
        let file_size = {
            let client = Arc::clone(&client);
            let path = path.clone();
            tokio::task::spawn_blocking(move || client.file_size(&path)).await??
        };
        let p = Arc::new(DownloadProgress::new(file_size));
        sftp_downloads()
            .lock()
            .unwrap()
            .insert(session, Arc::clone(&p));
        Some(p)
    } else {
        None
    };

    let book = {
        let client = Arc::clone(&client);
        let path = path.clone();
        tokio::task::spawn_blocking(move || -> Result<Box<dyn document::Document>> {
            let open_local =
                |local_path: std::path::PathBuf| -> Result<Box<dyn document::Document>> {
                    let src = crate::source::local::LocalFile::open(&local_path)?;
                    document::open_document(src, &path)
                };
            match strat {
                OpenStrategy::Download => {
                    let local_path = client.download_to_raw_cache(&path, progress)?;
                    tracing::info!("SFTP 整本已缓存: {}", local_path.display());
                    open_local(local_path)
                }
                OpenStrategy::Stream => {
                    // 缓存优先：已有 raw/ 本地缓存直接本地打开，不联网
                    if let Some(local_path) = sftp_source::raw_cache_path(client.endpoint(), &path)
                    {
                        tracing::info!("SFTP 命中缓存，直接本地打开: {}", local_path.display());
                        open_local(local_path)
                    } else {
                        let len = client.file_size(&path)?;
                        let src = sftp_source::SftpFile::new(client, path.clone(), len);
                        document::open_document(src, &path)
                    }
                }
                OpenStrategy::Auto => match client.download_to_raw_cache(&path, progress) {
                    Ok(local_path) => {
                        tracing::info!("SFTP 整本已缓存: {}", local_path.display());
                        open_local(local_path)
                    }
                    Err(e) => {
                        tracing::warn!("SFTP 整本下载失败, 回退流式: {e}");
                        let len = client.file_size(&path)?;
                        let src = sftp_source::SftpFile::new(client, path.clone(), len);
                        document::open_document(src, &path)
                    }
                },
            }
        })
        .await??
    };

    if strat != OpenStrategy::Stream {
        sftp_downloads().lock().unwrap().remove(&session);
    }

    Ok(register_book(book, &cache_ns))
}

/// 查询 SFTP 下载进度（0.0 ~ 1.0），非下载中返回 1.0。
pub fn sftp_download_progress(session: u64) -> f64 {
    sftp_downloads()
        .lock()
        .unwrap()
        .get(&session)
        .map(|p| p.fraction())
        .unwrap_or(1.0)
}

/// 检查某 SFTP 漫画是否已有 raw/ 本地缓存。
pub fn sftp_has_raw_cache(session: u64, path: String) -> bool {
    let client = match get_sftp_session(session) {
        Ok(c) => c,
        Err(_) => return false,
    };
    client.raw_cache_path(&path).is_some()
}

/// 生成 SFTP 书籍封面缩略图（优先 cover/ 磁盘缓存 → raw/ 本地缓存 → 流式解码）。
pub async fn sftp_cover(
    session: u64,
    path: String,
    page: u32,
    width: u32,
    height: u32,
    crop: Option<CropRect>,
) -> Result<PageImage> {
    let client = get_sftp_session(session)?;
    let endpoint = client.endpoint().to_string();
    let crop_tuple = crop.as_ref().map(|r| (r.x, r.y, r.w, r.h));
    let cache_lookup_path = client
        .raw_cache_path(&path)
        .or_else(|| Some(std::path::PathBuf::from(&path)));
    if let Some(ref lookup) = cache_lookup_path {
        let lookup_str = lookup.to_string_lossy();
        if let Some((rgba, w, h)) =
            cache::cover_cache_read(&lookup_str, page, width, height, crop_tuple)
        {
            return Ok(PageImage {
                rgba,
                width: w,
                height: h,
            });
        }
    }
    let endpoint_clone = endpoint.clone();
    let path_clone = path.clone();
    let client_clone = Arc::clone(&client);
    let img = tokio::task::spawn_blocking(move || -> Result<crate::decode::DecodedImage> {
        let governor = crate::reader::blocking_request_governor();
        let _permit = governor.acquire(crate::reader::RequestPriority::Cover)?;
        if let Some(local_path) = sftp_source::raw_cache_path(&endpoint_clone, &path_clone) {
            let src = crate::source::local::LocalFile::open(&local_path)?;
            let book = document::open_document(src, &path_clone)?;
            let bytes = book.page_bytes(page)?;
            let crop = crop.map(|r| (r.x, r.y, r.w, r.h));
            return crate::decode::decode_cover(&bytes, width, height, crop);
        }
        let len = client_clone.file_size(&path_clone)?;
        let src = sftp_source::SftpFile::new(client_clone, path_clone.clone(), len);
        let book = document::open_document(src, &path_clone)?;
        let bytes = book.page_bytes(page)?;
        let crop = crop.map(|r| (r.x, r.y, r.w, r.h));
        crate::decode::decode_cover(&bytes, width, height, crop)
    })
    .await??;
    let cache_write_path = client
        .raw_cache_path(&path)
        .or_else(|| Some(std::path::PathBuf::from(&path)));
    if let Some(ref wp) = cache_write_path {
        let _ = cache::cover_cache_write(
            &wp.to_string_lossy(),
            page,
            width,
            height,
            crop_tuple,
            &img.rgba,
        );
    }
    Ok(PageImage {
        rgba: img.rgba,
        width: img.width,
        height: img.height,
    })
}

// ============================================================
// 115 网页扫码 Cookie 模式（无需 APP ID，115 App 扫码即可）
// ============================================================

/// 115 网页扫码二维码载荷。
pub struct Cloud115CookieQrPayload {
    pub uid: String,
    pub time: i64,
    pub sign: String,
    pub qrcode: String,
}

/// 115 Cookie 模式会话信息。
pub struct Cloud115CookieSessionInfo {
    pub id: u64,
    pub root: String,
    pub capability_label: String,
    /// 当前会话 Cookie（与 DB 不一致时 Dart 回写）。
    pub cookie: String,
}

/// 第一步：获取 115 网页登录二维码（无需 APP ID）。
pub async fn cloud115_cookie_qr_start() -> Result<Cloud115CookieQrPayload> {
    let p = tokio::task::spawn_blocking(cloud115_source::web_qr_start).await??;
    Ok(Cloud115CookieQrPayload {
        uid: p.uid,
        time: p.time,
        sign: p.sign,
        qrcode: p.qrcode,
    })
}

/// 第二步：轮询扫码状态（0 等待 / 1 已扫 / 2 已登录 / -1 过期 / -2 取消）。
pub async fn cloud115_cookie_qr_poll(uid: String, time: i64, sign: String) -> Result<i32> {
    tokio::task::spawn_blocking(move || cloud115_source::web_qr_poll(&uid, time, &sign)).await?
}

/// 第三步：扫码成功后换取 Cookie（`k=v; k2=v2`，末尾不带 `;`）。
pub async fn cloud115_cookie_qr_result(uid: String, app: String) -> Result<String> {
    tokio::task::spawn_blocking(move || cloud115_source::web_qr_cookie(&uid, &app)).await?
}

// ============================================================
// 夸克网页扫码登录（Cookie 模式，免 F12）
// ============================================================

/// 夸克扫码载荷：`token`/`requestId` 用于轮询，`qrcode` 用于渲染二维码。
pub struct QuarkQrPayload {
    pub token: String,
    pub request_id: String,
    pub qrcode: String,
}

/// 第一步：获取夸克网页登录二维码（手机夸克 App 扫码）。
pub async fn quark_qr_start() -> Result<QuarkQrPayload> {
    let payload = tokio::task::spawn_blocking(quark_source::web_qr_start).await??;
    Ok(QuarkQrPayload {
        token: payload.token,
        request_id: payload.request_id,
        qrcode: payload.qrcode,
    })
}

/// 第二步：轮询扫码状态（0 等待 / 2 已登录 / -1 失败或过期）。
pub async fn quark_qr_poll(token: String, request_id: String) -> Result<i32> {
    tokio::task::spawn_blocking(move || quark_source::web_qr_poll(&token, &request_id)).await?
}

/// 第三步：扫码确认后换取 Cookie（`k=v; k2=v2`）。
pub async fn quark_qr_result(token: String, request_id: String) -> Result<String> {
    tokio::task::spawn_blocking(move || quark_source::web_qr_cookie(&token, &request_id)).await?
}

/// 连接 115（Cookie 模式）：列表根目录做连通性测试，返回会话。
pub async fn cloud115_cookie_connect(
    cookie: String,
    root_id: String,
) -> Result<Cloud115CookieSessionInfo> {
    let (client, root) =
        tokio::task::spawn_blocking(move || -> Result<(Cloud115WebClient, String)> {
            let client = Cloud115WebClient::new(&cookie, &root_id)?;
            client.check()?; // 列表根目录，登录失效会在这里暴露
            let root = client.root().to_string();
            Ok((client, root))
        })
        .await??;
    let id = next_id();
    let session_cookie = client.cookie();
    cloud115_cookie_sessions()
        .lock()
        .unwrap()
        .insert(id, Arc::new(client));
    Ok(Cloud115CookieSessionInfo {
        id,
        root,
        capability_label: "115".to_string(),
        cookie: session_cookie,
    })
}

/// 断开 115 Cookie 会话。
pub async fn cloud115_cookie_disconnect(id: u64) {
    let client = cloud115_cookie_sessions().lock().unwrap().remove(&id);
    if let Some(client) = client {
        let _ = tokio::task::spawn_blocking(move || drop(client)).await;
    }
}

/// 列出 115 目录（path 为文件夹 ID，根目录 `0`）。
pub async fn cloud115_cookie_list(session: u64, path: String) -> Result<Vec<DirEntry>> {
    let client = cloud115_cookie_sessions()
        .lock()
        .unwrap()
        .get(&session)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("115 Cookie 会话不存在，请重新连接"))?;
    let entries = tokio::task::spawn_blocking(move || client.list(&path)).await??;
    Ok(entries
        .into_iter()
        .map(|e| DirEntry {
            name: e.name,
            path: e.path,
            is_dir: e.is_dir,
            size: e.size,
            mtime: e.mtime,
        })
        .collect())
}

/// 打开 115 上的书籍（path 为 pickcode，三态策略）。
pub async fn open_cloud115_cookie_book(
    session: u64,
    path: String,
    strategy: String,
) -> Result<BookInfo> {
    let client = cloud115_cookie_sessions()
        .lock()
        .unwrap()
        .get(&session)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("115 Cookie 会话不存在，请重新连接"))?;
    let origin = client.origin();
    let cache_ns = format!("115|{}|{}", origin, path);
    let strat = parse_strategy(&strategy);

    let progress = if strat != OpenStrategy::Stream {
        let p = Arc::new(DownloadProgress::new(0));
        cloud115_cookie_downloads()
            .lock()
            .unwrap()
            .insert(session, Arc::clone(&p));
        Some(p)
    } else {
        None
    };

    let book = {
        let client = Arc::clone(&client);
        let path = path.clone();
        tokio::task::spawn_blocking(move || -> Result<Box<dyn document::Document>> {
            let open_local =
                |local_path: std::path::PathBuf| -> Result<Box<dyn document::Document>> {
                    // 缓存目录里的文件名就是真实文件名（下载时按 info.name 落盘）。
                    let name = local_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "file.cbz".to_string());
                    let src = crate::source::local::LocalFile::open(&local_path)?;
                    document::open_document(src, &name)
                };
            let open_stream =
                |client: Arc<Cloud115WebClient>| -> Result<Box<dyn document::Document>> {
                    let name = client.resolve_name(&path)?;
                    let info = client.downurl(&path)?;
                    let (supports, size) = client.probe(&info.url);
                    if !supports {
                        anyhow::bail!("115 直链不支持 Range，请改用整本下载策略");
                    }
                    let src =
                        cloud115_source::Cloud115WebFile::new(client, path.clone(), size, info.url);
                    document::open_document(src, &name)
                };
            match strat {
                OpenStrategy::Download => {
                    let local_path = client.download_to_raw_cache(&path, progress)?;
                    tracing::info!("115 整本已缓存: {}", local_path.display());
                    open_local(local_path)
                }
                OpenStrategy::Stream => {
                    // 已缓存时直接本地打开（pickcode 失效也能读），未缓存才走流式。
                    match cloud115_source::web_raw_cache_path(&client.origin(), &path) {
                        Some(local_path) => {
                            tracing::info!("115 命中缓存，直接本地打开: {}", local_path.display());
                            open_local(local_path)
                        }
                        None => open_stream(Arc::clone(&client)),
                    }
                }
                OpenStrategy::Auto => match cloud115_source::web_raw_cache_path(&client.origin(), &path) {
                    Some(local_path) => {
                        tracing::info!("115 命中缓存，直接本地打开: {}", local_path.display());
                        open_local(local_path)
                    }
                    None => match open_stream(Arc::clone(&client)) {
                        Ok(book) => {
                            tracing::info!("115 流式打开成功");
                            Ok(book)
                        }
                        Err(e) => {
                            tracing::warn!("115 流式失败，回退整本下载: {e}");
                            let local_path = client.download_to_raw_cache(&path, progress)?;
                            tracing::info!("115 整本已缓存: {}", local_path.display());
                            open_local(local_path)
                        }
                    }
                },
            }
        })
        .await??
    };

    if strat != OpenStrategy::Stream {
        cloud115_cookie_downloads().lock().unwrap().remove(&session);
    }
    Ok(register_book(book, &cache_ns))
}

/// 115 Cookie 下载进度（0.0~1.0，非下载中返回 1.0）。
pub fn cloud115_cookie_download_progress(session: u64) -> f64 {
    cloud115_cookie_downloads()
        .lock()
        .unwrap()
        .get(&session)
        .map(|p| p.fraction())
        .unwrap_or(1.0)
}

/// 115 Cookie 书籍是否已有 raw/ 本地缓存。
pub fn cloud115_cookie_has_raw_cache(session: u64, path: String) -> bool {
    let client = match cloud115_cookie_sessions()
        .lock()
        .unwrap()
        .get(&session)
        .cloned()
    {
        Some(c) => c,
        None => return false,
    };
    cloud115_source::web_raw_cache_path(&client.origin(), &path).is_some()
}

/// 115 Cookie 书籍封面（cover/ 磁盘缓存 → raw/ 本地缓存 → 流式解码）。
pub async fn cloud115_cookie_cover(
    session: u64,
    path: String,
    page: u32,
    width: u32,
    height: u32,
    crop: Option<CropRect>,
) -> Result<PageImage> {
    let client = cloud115_cookie_sessions()
        .lock()
        .unwrap()
        .get(&session)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("115 Cookie 会话不存在，请重新连接"))?;
    let origin = client.origin();
    let crop_tuple = crop.as_ref().map(|r| (r.x, r.y, r.w, r.h));
    let cache_lookup_path = cloud115_source::web_raw_cache_path(&origin, &path)
        .or_else(|| Some(std::path::PathBuf::from(&path)));
    if let Some(ref lookup) = cache_lookup_path {
        let lookup_str = lookup.to_string_lossy();
        if let Some((rgba, w, h)) =
            cache::cover_cache_read(&lookup_str, page, width, height, crop_tuple)
        {
            return Ok(PageImage {
                rgba,
                width: w,
                height: h,
            });
        }
    }
    let origin_clone = origin.clone();
    let path_clone = path.clone();
    let client_clone = Arc::clone(&client);
    let img = tokio::task::spawn_blocking(move || -> Result<crate::decode::DecodedImage> {
        let governor = crate::reader::blocking_request_governor();
        let _permit = governor.acquire(crate::reader::RequestPriority::Cover)?;
        let name = client_clone.resolve_name(&path_clone)?;
        if let Some(local_path) = cloud115_source::web_raw_cache_path(&origin_clone, &path_clone) {
            let src = crate::source::local::LocalFile::open(&local_path)?;
            let book = document::open_document(src, &name)?;
            let bytes = book.page_bytes(page)?;
            let crop = crop.map(|r| (r.x, r.y, r.w, r.h));
            return crate::decode::decode_cover(&bytes, width, height, crop);
        }
        let info = client_clone.downurl(&path_clone)?;
        let (supports, size) = client_clone.probe(&info.url);
        if !supports {
            anyhow::bail!("远程封面需要 Range 支持");
        }
        let src =
            cloud115_source::Cloud115WebFile::new(client_clone, path_clone.clone(), size, info.url);
        let book = document::open_document(src, &name)?;
        let bytes = book.page_bytes(page)?;
        let crop = crop.map(|r| (r.x, r.y, r.w, r.h));
        crate::decode::decode_cover(&bytes, width, height, crop)
    })
    .await??;
    let cache_write_path = cloud115_source::web_raw_cache_path(&origin, &path)
        .or_else(|| Some(std::path::PathBuf::from(&path)));
    if let Some(ref wp) = cache_write_path {
        let _ = cache::cover_cache_write(
            &wp.to_string_lossy(),
            page,
            width,
            height,
            crop_tuple,
            &img.rgba,
        );
    }
    Ok(PageImage {
        rgba: img.rgba,
        width: img.width,
        height: img.height,
    })
}

// ============================================================
// 夸克网盘书源（非官方 Web API，Cookie 认证）
// ============================================================

/// 夸克网盘会话信息。
pub struct QuarkSessionInfo {
    pub id: u64,
    pub root: String,
    /// "quark"
    pub capability_label: String,
    /// 会话内可能回写了 `__puus` 等续期 cookie；Dart 侧与 DB 不一致时回写。
    pub cookie: String,
}

/// 连接夸克网盘：`/config` + 根目录连通性测试，返回会话。
pub async fn quark_connect(cookie: String, root_id: String) -> Result<QuarkSessionInfo> {
    let (client, root) = tokio::task::spawn_blocking(move || -> Result<(QuarkClient, String)> {
        let client = QuarkClient::new(&cookie, &root_id)?;
        client.check()?; // /config + 首屏 list
        let root = client.root().to_string();
        Ok((client, root))
    })
    .await??;
    let id = next_id();
    let session_cookie = client.cookie();
    quark_sessions()
        .lock()
        .unwrap()
        .insert(id, Arc::new(client));
    Ok(QuarkSessionInfo {
        id,
        root,
        capability_label: "quark".to_string(),
        cookie: session_cookie,
    })
}

/// 断开夸克会话。
pub async fn quark_disconnect(id: u64) {
    let client = quark_sessions().lock().unwrap().remove(&id);
    if let Some(client) = client {
        let _ = tokio::task::spawn_blocking(move || drop(client)).await;
    }
}

/// 列出夸克目录（path 为文件夹 fid，根目录 `0`）。
pub async fn quark_list(session: u64, path: String) -> Result<Vec<DirEntry>> {
    let client = get_quark_session(session)?;
    let entries = tokio::task::spawn_blocking(move || client.list(&path)).await??;
    Ok(entries
        .into_iter()
        .map(|e| DirEntry {
            name: e.name,
            path: e.path,
            is_dir: e.is_dir,
            size: e.size,
            mtime: e.mtime,
        })
        .collect())
}

/// 打开夸克网盘上的书籍（path 为文件 fid，三态策略；格式探测走真实文件名）。
pub async fn open_quark_book(session: u64, path: String, strategy: String) -> Result<BookInfo> {
    let client = get_quark_session(session)?;
    let origin = client.origin();
    let cache_ns = format!("quark|{}|{}", origin, path);
    let strat = parse_strategy(&strategy);

    let progress = if strat != OpenStrategy::Stream {
        let p = Arc::new(DownloadProgress::new(0)); // 直链不带大小，下载响应后更新
        quark_downloads()
            .lock()
            .unwrap()
            .insert(session, Arc::clone(&p));
        Some(p)
    } else {
        None
    };

    let book = {
        let client = Arc::clone(&client);
        let path = path.clone();
        tokio::task::spawn_blocking(move || -> Result<Box<dyn document::Document>> {
            let open_local =
                |local_path: std::path::PathBuf| -> Result<Box<dyn document::Document>> {
                    // 缓存目录里的文件名即真实文件名（下载时按源文件名落盘）
                    let name = local_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "file.cbz".to_string());
                    let src = crate::source::local::LocalFile::open(&local_path)?;
                    document::open_document(src, &name)
                };
            let open_stream = |client: Arc<QuarkClient>| -> Result<Box<dyn document::Document>> {
                let name = client.resolve_name(&path)?;
                let info = client.downlink(&path)?;
                let (supports, size) = client.probe(&info.url);
                if !supports {
                    anyhow::bail!("夸克直链不支持 Range，请改用整本下载策略");
                }
                let src = quark_source::QuarkFile::new(client, path.clone(), size, info.url);
                document::open_document(src, &name)
            };
            match strat {
                OpenStrategy::Download => {
                    let local_path = client.download_to_raw_cache(&path, progress)?;
                    tracing::info!("夸克网盘整本已缓存: {}", local_path.display());
                    open_local(local_path)
                }
                OpenStrategy::Stream => {
                    // 缓存优先：已有 raw/ 本地缓存直接本地打开，不联网
                    match quark_source::raw_cache_path(&client.origin(), &path) {
                        Some(local_path) => {
                            tracing::info!("夸克命中缓存，直接本地打开: {}", local_path.display());
                            open_local(local_path)
                        }
                        None => open_stream(Arc::clone(&client)),
                    }
                }
                OpenStrategy::Auto => match quark_source::raw_cache_path(&client.origin(), &path) {
                    Some(local_path) => {
                        tracing::info!("夸克网盘命中缓存，直接本地打开: {}", local_path.display());
                        open_local(local_path)
                    }
                    None => match open_stream(Arc::clone(&client)) {
                        Ok(book) => {
                            tracing::info!("夸克网盘流式打开成功");
                            Ok(book)
                        }
                        Err(e) => {
                            tracing::warn!("夸克网盘流式失败，回退整本下载: {e}");
                            let local_path = client.download_to_raw_cache(&path, progress)?;
                            tracing::info!("夸克网盘整本已缓存: {}", local_path.display());
                            open_local(local_path)
                        }
                    }
                },
            }
        })
        .await??
    };

    if strat != OpenStrategy::Stream {
        quark_downloads().lock().unwrap().remove(&session);
    }
    Ok(register_book(book, &cache_ns))
}

/// 夸克下载进度（0.0~1.0，非下载中返回 1.0）。
pub fn quark_download_progress(session: u64) -> f64 {
    quark_downloads()
        .lock()
        .unwrap()
        .get(&session)
        .map(|p| p.fraction())
        .unwrap_or(1.0)
}

/// 夸克书籍是否已有 raw/ 本地缓存。
pub fn quark_has_raw_cache(session: u64, path: String) -> bool {
    let client = match get_quark_session(session) {
        Ok(c) => c,
        Err(_) => return false,
    };
    quark_source::raw_cache_path(&client.origin(), &path).is_some()
}

/// 夸克书籍封面（cover/ 磁盘缓存 → raw/ 本地缓存 → 流式解码）。
pub async fn quark_cover(
    session: u64,
    path: String,
    page: u32,
    width: u32,
    height: u32,
    crop: Option<CropRect>,
) -> Result<PageImage> {
    let client = get_quark_session(session)?;
    let origin = client.origin();
    let crop_tuple = crop.as_ref().map(|r| (r.x, r.y, r.w, r.h));
    let cache_lookup_path = quark_source::raw_cache_path(&origin, &path)
        .or_else(|| Some(std::path::PathBuf::from(&path)));
    if let Some(ref lookup) = cache_lookup_path {
        let lookup_str = lookup.to_string_lossy();
        if let Some((rgba, w, h)) =
            cache::cover_cache_read(&lookup_str, page, width, height, crop_tuple)
        {
            return Ok(PageImage {
                rgba,
                width: w,
                height: h,
            });
        }
    }
    let origin_clone = origin.clone();
    let path_clone = path.clone();
    let client_clone = Arc::clone(&client);
    let img = tokio::task::spawn_blocking(move || -> Result<crate::decode::DecodedImage> {
        let name = client_clone.resolve_name(&path_clone)?;
        let governor = crate::reader::blocking_request_governor();
        let _permit = governor.acquire(crate::reader::RequestPriority::Cover)?;
        if let Some(local_path) = quark_source::raw_cache_path(&origin_clone, &path_clone) {
            let src = crate::source::local::LocalFile::open(&local_path)?;
            let book = document::open_document(src, &name)?;
            let bytes = book.page_bytes(page)?;
            let crop = crop.map(|r| (r.x, r.y, r.w, r.h));
            return crate::decode::decode_cover(&bytes, width, height, crop);
        }
        let info = client_clone.downlink(&path_clone)?;
        let (supports, size) = client_clone.probe(&info.url);
        if !supports {
            anyhow::bail!("远程封面需要 Range 支持");
        }
        let src = quark_source::QuarkFile::new(client_clone, path_clone.clone(), size, info.url);
        let book = document::open_document(src, &name)?;
        let bytes = book.page_bytes(page)?;
        let crop = crop.map(|r| (r.x, r.y, r.w, r.h));
        crate::decode::decode_cover(&bytes, width, height, crop)
    })
    .await??;
    let cache_write_path = quark_source::raw_cache_path(&origin, &path)
        .or_else(|| Some(std::path::PathBuf::from(&path)));
    if let Some(ref wp) = cache_write_path {
        let _ = cache::cover_cache_write(
            &wp.to_string_lossy(),
            page,
            width,
            height,
            crop_tuple,
            &img.rgba,
        );
    }
    Ok(PageImage {
        rgba: img.rgba,
        width: img.width,
        height: img.height,
    })
}

/// 生成 WebDAV 书籍封面缩略图(取第 page 页,等比缩放 + 中心裁剪到 w×h)。
/// 封面结果写入磁盘缓存（cover/）供后续秒开。
/// 优先走磁盘缓存 → raw/ 本地缓存 → HTTP Range 流式。
/// P1-D-2：legacy 封面缓存的**纯本地**查找身份。
///
/// Dart 只传它本来就在用的 logical source fields —— **不得**传 origin / endpoint /
/// raw path / cache key / hash。`kind` 取值与仓库既有的
/// `open_cached_remote_book(kind, ..)` 保持一致：
/// `"webdav" | "sftp" | "baidu" | "115" | "115web" | "quark"`。
pub struct LegacyCoverLocalLookupDto {
    pub kind: String,
    /// WebDAV：书源 URL（authority 由 production 构造函数派生的 origin 决定）。
    pub url: String,
    /// SFTP：logical host / port。
    pub host: String,
    pub port: u32,
    /// Baidu：app_key（client_id）与 root。
    pub app_key: String,
    /// 115 app：app_id 与 root_id。
    pub app_id: String,
    pub root_id: String,
    /// Baidu / 115-web / Quark 的 root（语义随 kind 而定）。
    pub root: String,
    pub logical_path: String,
    pub page: u32,
    pub width: u32,
    pub height: u32,
    pub crop: Option<CropRect>,
}

/// 由 durable logical fields 派生 cache authority（**不触网、不建 session**）。
///
/// 每个分支都复用该 provider **现有的不触网构造函数 / 纯 helper**，
/// 不复制任何 authority 算法（SFTP 走共享 `endpoint_for`）。
fn legacy_cover_authority(lookup: &LegacyCoverLocalLookupDto) -> Option<String> {
    use crate::source::baidu::BaiduClient;
    use crate::source::cloud115::{Cloud115Client, Cloud115WebClient};
    use crate::source::quark::QuarkClient;
    use crate::source::webdav::WebDavClient;
    match lookup.kind.as_str() {
        "webdav" => WebDavClient::new(&lookup.url, "", "")
            .ok()
            .map(|(client, _)| client.origin().to_string()),
        "sftp" => Some(crate::source::sftp::endpoint_for(
            &lookup.host,
            u16::try_from(lookup.port).unwrap_or(22),
        )),
        "baidu" => BaiduClient::new(&lookup.app_key, "", "", &lookup.root)
            .ok()
            .map(|client| client.origin()),
        "115" => Cloud115Client::new(&lookup.app_id, "", &lookup.root_id)
            .ok()
            .map(|client| client.origin()),
        "115web" => Cloud115WebClient::new("", &lookup.root)
            .ok()
            .map(|client| client.origin()),
        "quark" => QuarkClient::new("", &lookup.root)
            .ok()
            .map(|client| client.origin()),
        _ => None,
    }
}

/// **纯本地 cover 缓存查找**（P1-D-2）。
///
/// 硬契约：不建 session、不连接、不刷新凭据、不访问 provider、不建 job、不 wake worker、
/// 不 scan、不改 retry、不改任何 durable cover state。只做：
/// `logical fields → authority → current key → (可恢复时) alternate historical key → cover_cache_read`。
///
/// 双历史 key 规则（冻结）：
/// * **Family 1**（webdav / sftp / baidu / 115）：raw 当前存在 → current = 实际 raw path，
///   alternate = logical path；raw 当前不存在 → current = logical path，
///   alternate = **确定性候选 raw path**（由 production 算法精确重建，故历史 raw-key cover 仍可读）。
/// * **Family 2**（115web / quark）：raw 当前存在 → current = 扫描出的实际 raw path，
///   alternate = logical path；raw 当前不存在 → current = logical path，**alternate = 无**，
///   直接 clean miss —— 因为文件名来自 provider 网络响应且从未持久化，
///   历史 raw-key cover 属**已证明不可恢复**状态（禁止猜测补齐）。
pub fn read_legacy_cover_local(lookup: LegacyCoverLocalLookupDto) -> Option<PageImage> {
    use std::path::PathBuf;
    let authority = legacy_cover_authority(&lookup)?;
    let logical = lookup.logical_path.as_str();
    let crop = lookup
        .crop
        .as_ref()
        .map(|rect| (rect.x, rect.y, rect.w, rect.h));

    let logical_path_buf = PathBuf::from(logical);
    let (current, alternate): (PathBuf, Option<PathBuf>) = match lookup.kind.as_str() {
        // Family 2：无文件级候选（目录虽确定，但文件名不可推导）。
        "115web" => match crate::source::cloud115::web_raw_cache_path(&authority, logical) {
            Some(raw) => (raw, Some(logical_path_buf)),
            None => (logical_path_buf, None),
        },
        "quark" => match crate::source::quark::raw_cache_path(&authority, logical) {
            Some(raw) => (raw, Some(logical_path_buf)),
            None => (logical_path_buf, None),
        },
        // Family 1：raw 缺失时可由 production 算法精确重建历史 raw path。
        "webdav" => {
            let raw = crate::source::webdav::raw_cache_path(&authority, logical);
            match raw {
                Some(raw) => (raw, Some(logical_path_buf)),
                None => (
                    logical_path_buf,
                    Some(crate::source::webdav::raw_cache_candidate_path(
                        &authority, logical,
                    )),
                ),
            }
        }
        "sftp" => {
            let raw = crate::source::sftp::raw_cache_path(&authority, logical);
            match raw {
                Some(raw) => (raw, Some(logical_path_buf)),
                None => (
                    logical_path_buf,
                    Some(crate::source::sftp::raw_cache_candidate_path(
                        &authority, logical,
                    )),
                ),
            }
        }
        "baidu" => {
            let raw = crate::source::baidu::raw_cache_path(&authority, logical);
            match raw {
                Some(raw) => (raw, Some(logical_path_buf)),
                None => (
                    logical_path_buf,
                    Some(crate::source::baidu::raw_cache_candidate_path(
                        &authority, logical,
                    )),
                ),
            }
        }
        "115" => {
            let raw = crate::source::cloud115::raw_cache_path(&authority, logical);
            match raw {
                Some(raw) => (raw, Some(logical_path_buf)),
                None => (
                    logical_path_buf,
                    Some(crate::source::cloud115::raw_cache_candidate_path(
                        &authority, logical,
                    )),
                ),
            }
        }
        _ => return None,
    };

    if let Some((rgba, width, height)) = crate::cache::cover_cache_read(
        &current.to_string_lossy(),
        lookup.page,
        lookup.width,
        lookup.height,
        crop,
    ) {
        return Some(PageImage {
            rgba,
            width,
            height,
        });
    }
    if let Some(alternate) = alternate {
        if let Some((rgba, width, height)) = crate::cache::cover_cache_read(
            &alternate.to_string_lossy(),
            lookup.page,
            lookup.width,
            lookup.height,
            crop,
        ) {
            return Some(PageImage {
                rgba,
                width,
                height,
            });
        }
    }
    // clean miss：**绝不** fallback 到 provider（网络回退属上层 Flutter control flow）。
    None
}

pub async fn webdav_cover(
    session: u64,
    path: String,
    page: u32,
    width: u32,
    height: u32,
    crop: Option<CropRect>,
) -> Result<PageImage> {
    let client = get_session(session)?;
    let origin = client.origin().to_string();
    let crop_tuple = crop.as_ref().map(|r| (r.x, r.y, r.w, r.h));
    // 先查磁盘缓存
    let cache_lookup_path =
        webdav::raw_cache_path(&origin, &path).or_else(|| Some(std::path::PathBuf::from(&path)));
    if let Some(ref lookup) = cache_lookup_path {
        let lookup_str = lookup.to_string_lossy();
        if let Some((rgba, w, h)) =
            cache::cover_cache_read(&lookup_str, page, width, height, crop_tuple)
        {
            return Ok(PageImage {
                rgba,
                width: w,
                height: h,
            });
        }
    }
    let origin_clone = origin.clone();
    let path_clone = path.clone();
    let client_clone = Arc::clone(&client);
    let img = tokio::task::spawn_blocking(move || -> Result<crate::decode::DecodedImage> {
        // 先尝试 raw/ 本地缓存(已下载过的漫画直接本地秒出)
        let governor = crate::reader::blocking_request_governor();
        let _permit = governor.acquire(crate::reader::RequestPriority::Cover)?;
        if let Some(local_path) = webdav::raw_cache_path(&origin_clone, &path_clone) {
            let src = crate::source::local::LocalFile::open(&local_path)?;
            let book = document::open_document(src, &path_clone)?;
            let bytes = book.page_bytes(page)?;
            let crop = crop.map(|r| (r.x, r.y, r.w, r.h));
            return crate::decode::decode_cover(&bytes, width, height, crop);
        }
        // 未下载: 走 HTTP Range 流式
        let len = client_clone.file_size(&path_clone)?;
        let src = WebDavFile::new(client_clone, path_clone.clone(), len);
        let book = document::open_document(src, &path_clone)?;
        let bytes = book.page_bytes(page)?;
        let crop = crop.map(|r| (r.x, r.y, r.w, r.h));
        crate::decode::decode_cover(&bytes, width, height, crop)
    })
    .await??;
    // 写入磁盘缓存
    let cache_write_path =
        webdav::raw_cache_path(&origin, &path).or_else(|| Some(std::path::PathBuf::from(&path)));
    if let Some(ref wp) = cache_write_path {
        let _ = cache::cover_cache_write(
            &wp.to_string_lossy(),
            page,
            width,
            height,
            crop_tuple,
            &img.rgba,
        );
    }
    Ok(PageImage {
        rgba: img.rgba,
        width: img.width,
        height: img.height,
    })
}

// ============================================================
// 百度网盘书源（官方开放平台 API）
// ============================================================

/// 百度 token 对（授权码换 token / 刷新结果）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct BaiduTokenPair {
    pub access_token: String,
    pub refresh_token: String,
}

/// 百度会话信息。
pub struct BaiduSessionInfo {
    pub id: u64,
    pub root: String,
    pub capability_label: String,
    /// 刷新后的 refresh_token（Dart 回写 DB）。
    pub refresh_token: String,
}

/// 构造百度 OAuth 授权链接（浏览器打开，redirect_uri=oob）。
pub fn baidu_auth_url(app_key: String) -> String {
    baidu_source::auth_url(&app_key)
}

/// 授权码换 token（不建会话）。
pub async fn baidu_exchange_code(
    app_key: String,
    client_secret: String,
    code: String,
) -> Result<BaiduTokenPair> {
    let pair = tokio::task::spawn_blocking(move || {
        baidu_source::exchange_code(&app_key, &client_secret, &code)
    })
    .await??;
    Ok(BaiduTokenPair {
        access_token: pair.access_token,
        refresh_token: pair.refresh_token,
    })
}

/// 连接百度网盘：刷新/校验 token + 连通性测试，返回会话与最新 refresh_token。
pub async fn baidu_connect(
    refresh_token: String,
    app_key: String,
    client_secret: String,
    root: String,
) -> Result<BaiduSessionInfo> {
    let (client, new_rt, root) =
        tokio::task::spawn_blocking(move || -> Result<(BaiduClient, String, String)> {
            let client = BaiduClient::new(&app_key, &client_secret, &refresh_token, &root)?;
            let pair = client.refresh()?;
            client.list(client.root())?; // 连通性测试
            let root = client.root().to_string();
            Ok((client, pair.refresh_token, root))
        })
        .await??;
    let id = next_id();
    baidu_sessions()
        .lock()
        .unwrap()
        .insert(id, Arc::new(client));
    Ok(BaiduSessionInfo {
        id,
        root,
        capability_label: "baidu".to_string(),
        refresh_token: new_rt,
    })
}

/// 断开百度会话。
pub async fn baidu_disconnect(id: u64) {
    let client = baidu_sessions().lock().unwrap().remove(&id);
    if let Some(client) = client {
        let _ = tokio::task::spawn_blocking(move || drop(client)).await;
    }
}

/// 列出百度网盘目录（按路径）。
pub async fn baidu_list(session: u64, path: String) -> Result<Vec<DirEntry>> {
    let client = get_baidu_session(session)?;
    let entries = tokio::task::spawn_blocking(move || client.list(&path)).await??;
    Ok(entries
        .into_iter()
        .map(|e| DirEntry {
            name: e.name,
            path: e.path,
            is_dir: e.is_dir,
            size: e.size,
            mtime: e.mtime,
        })
        .collect())
}

/// 打开百度网盘上的书籍（三态策略，镜像 open_webdav_book）。
pub async fn open_baidu_book(session: u64, path: String, strategy: String) -> Result<BookInfo> {
    let client = get_baidu_session(session)?;
    let origin = client.origin();
    let cache_ns = format!("baidu|{}|{}", origin, path);
    let strat = parse_strategy(&strategy);

    let progress = if strat != OpenStrategy::Stream {
        let (client, path) = (Arc::clone(&client), path.clone());
        let size =
            tokio::task::spawn_blocking(move || client.dlink(&path).map(|(_, s)| s)).await??;
        let p = Arc::new(DownloadProgress::new(size));
        baidu_downloads()
            .lock()
            .unwrap()
            .insert(session, Arc::clone(&p));
        Some(p)
    } else {
        None
    };

    let book = {
        let client = Arc::clone(&client);
        let path = path.clone();
        tokio::task::spawn_blocking(move || -> Result<Box<dyn document::Document>> {
            let open_local =
                |local_path: std::path::PathBuf| -> Result<Box<dyn document::Document>> {
                    let src = crate::source::local::LocalFile::open(&local_path)?;
                    document::open_document(src, &path)
                };
            let open_stream = |client: Arc<BaiduClient>| -> Result<Box<dyn document::Document>> {
                let (link, size) = client.dlink(&path)?;
                if client.probe_range(&link) {
                    let src = baidu_source::BaiduFile::new(client, path.clone(), size, link);
                    document::open_document(src, &path)
                } else {
                    // 不支持 Range：整本下载后本地读
                    let local_path = client.download_to_raw_cache(&path, None)?;
                    open_local(local_path)
                }
            };
            match strat {
                OpenStrategy::Download => {
                    let local_path = client.download_to_raw_cache(&path, progress)?;
                    tracing::info!("百度网盘整本已缓存: {}", local_path.display());
                    open_local(local_path)
                }
                OpenStrategy::Stream => {
                    // 缓存优先：已有 raw/ 本地缓存直接本地打开，不联网
                    match baidu_source::raw_cache_path(&client.origin(), &path) {
                        Some(local_path) => {
                            tracing::info!(
                                "百度网盘命中缓存，直接本地打开: {}",
                                local_path.display()
                            );
                            open_local(local_path)
                        }
                        None => open_stream(Arc::clone(&client)),
                    }
                }
                OpenStrategy::Auto => match baidu_source::raw_cache_path(&client.origin(), &path) {
                    Some(local_path) => {
                        tracing::info!("百度网盘命中缓存，直接本地打开: {}", local_path.display());
                        open_local(local_path)
                    }
                    None => match open_stream(Arc::clone(&client)) {
                        Ok(book) => {
                            tracing::info!("百度网盘流式打开成功");
                            Ok(book)
                        }
                        Err(e) => {
                            tracing::warn!("百度网盘流式失败，回退整本下载: {e}");
                            let local_path = client.download_to_raw_cache(&path, progress)?;
                            tracing::info!("百度网盘整本已缓存: {}", local_path.display());
                            open_local(local_path)
                        }
                    }
                },
            }
        })
        .await??
    };

    if strat != OpenStrategy::Stream {
        baidu_downloads().lock().unwrap().remove(&session);
    }
    Ok(register_book(book, &cache_ns))
}

/// 百度下载进度（0.0~1.0，非下载中返回 1.0）。
pub fn baidu_download_progress(session: u64) -> f64 {
    baidu_downloads()
        .lock()
        .unwrap()
        .get(&session)
        .map(|p| p.fraction())
        .unwrap_or(1.0)
}

/// 百度书籍是否已有 raw/ 本地缓存。
pub fn baidu_has_raw_cache(session: u64, path: String) -> bool {
    let client = match get_baidu_session(session) {
        Ok(c) => c,
        Err(_) => return false,
    };
    baidu_source::raw_cache_path(&client.origin(), &path).is_some()
}

/// 百度书籍封面（cover/ 磁盘缓存 → raw/ 本地缓存 → 流式解码）。
pub async fn baidu_cover(
    session: u64,
    path: String,
    page: u32,
    width: u32,
    height: u32,
    crop: Option<CropRect>,
) -> Result<PageImage> {
    let client = get_baidu_session(session)?;
    let origin = client.origin();
    let crop_tuple = crop.as_ref().map(|r| (r.x, r.y, r.w, r.h));
    let cache_lookup_path = baidu_source::raw_cache_path(&origin, &path)
        .or_else(|| Some(std::path::PathBuf::from(&path)));
    if let Some(ref lookup) = cache_lookup_path {
        let lookup_str = lookup.to_string_lossy();
        if let Some((rgba, w, h)) =
            cache::cover_cache_read(&lookup_str, page, width, height, crop_tuple)
        {
            return Ok(PageImage {
                rgba,
                width: w,
                height: h,
            });
        }
    }
    let origin_clone = origin.clone();
    let path_clone = path.clone();
    let client_clone = Arc::clone(&client);
    let img = tokio::task::spawn_blocking(move || -> Result<crate::decode::DecodedImage> {
        let governor = crate::reader::blocking_request_governor();
        let _permit = governor.acquire(crate::reader::RequestPriority::Cover)?;
        if let Some(local_path) = baidu_source::raw_cache_path(&origin_clone, &path_clone) {
            let src = crate::source::local::LocalFile::open(&local_path)?;
            let book = document::open_document(src, &path_clone)?;
            let bytes = book.page_bytes(page)?;
            let crop = crop.map(|r| (r.x, r.y, r.w, r.h));
            return crate::decode::decode_cover(&bytes, width, height, crop);
        }
        let (link, size) = client_clone.dlink(&path_clone)?;
        if !client_clone.probe_range(&link) {
            anyhow::bail!("远程封面需要 Range 支持");
        }
        let src = baidu_source::BaiduFile::new(client_clone, path_clone.clone(), size, link);
        let book = document::open_document(src, &path_clone)?;
        let bytes = book.page_bytes(page)?;
        let crop = crop.map(|r| (r.x, r.y, r.w, r.h));
        crate::decode::decode_cover(&bytes, width, height, crop)
    })
    .await??;
    let cache_write_path = baidu_source::raw_cache_path(&origin, &path)
        .or_else(|| Some(std::path::PathBuf::from(&path)));
    if let Some(ref wp) = cache_write_path {
        let _ = cache::cover_cache_write(
            &wp.to_string_lossy(),
            page,
            width,
            height,
            crop_tuple,
            &img.rgba,
        );
    }
    Ok(PageImage {
        rgba: img.rgba,
        width: img.width,
        height: img.height,
    })
}

// ============================================================
// 115 网盘书源（官方开放平台 API）
// ============================================================

/// 115 扫码授权二维码载荷。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Cloud115QrPayload {
    pub uid: String,
    pub time: i64,
    pub sign: String,
    pub qrcode: String,
}

/// 115 扫码轮询结果。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Cloud115QrPollResult {
    pub status: i32,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
}

/// 115 会话信息。
pub struct Cloud115SessionInfo {
    pub id: u64,
    pub root: String,
    pub capability_label: String,
    /// 刷新后的 refresh_token（Dart 回写 DB）。
    pub refresh_token: String,
}

/// 开始 115 设备码授权（Dart 渲染二维码）。
pub async fn cloud115_qr_start(app_id: String) -> Result<Cloud115QrPayload> {
    let p = tokio::task::spawn_blocking(move || cloud115_source::qr_start(&app_id)).await??;
    Ok(Cloud115QrPayload {
        uid: p.uid,
        time: p.time,
        sign: p.sign,
        qrcode: p.qrcode,
    })
}

/// 轮询 115 扫码状态；status=2 返回 token。
pub async fn cloud115_qr_poll(
    uid: String,
    time: i64,
    sign: String,
) -> Result<Cloud115QrPollResult> {
    let r =
        tokio::task::spawn_blocking(move || cloud115_source::qr_poll(&uid, time, &sign)).await??;
    Ok(Cloud115QrPollResult {
        status: r.status,
        access_token: r.access_token,
        refresh_token: r.refresh_token,
    })
}

/// 连接 115 网盘：刷新/校验 token + 连通性测试，返回会话与最新 refresh_token。
pub async fn cloud115_connect(
    refresh_token: String,
    app_id: String,
    root_id: String,
) -> Result<Cloud115SessionInfo> {
    let (client, new_rt, root) =
        tokio::task::spawn_blocking(move || -> Result<(Cloud115Client, String, String)> {
            let client = Cloud115Client::new(&app_id, &refresh_token, &root_id)?;
            let (_, rt) = client.refresh()?;
            client.user_info()?; // 连通性测试
            let root = client.root_id().to_string();
            Ok((client, rt, root))
        })
        .await??;
    let id = next_id();
    cloud115_sessions()
        .lock()
        .unwrap()
        .insert(id, Arc::new(client));
    Ok(Cloud115SessionInfo {
        id,
        root,
        capability_label: "115".to_string(),
        refresh_token: new_rt,
    })
}

/// 断开 115 会话。
pub async fn cloud115_disconnect(id: u64) {
    let client = cloud115_sessions().lock().unwrap().remove(&id);
    if let Some(client) = client {
        let _ = tokio::task::spawn_blocking(move || drop(client)).await;
    }
}

/// 列出 115 目录（path 为文件夹 ID）。
pub async fn cloud115_list(session: u64, path: String) -> Result<Vec<DirEntry>> {
    let client = get_cloud115_session(session)?;
    let entries = tokio::task::spawn_blocking(move || client.list(&path)).await??;
    Ok(entries
        .into_iter()
        .map(|e| DirEntry {
            name: e.name,
            path: e.path,
            is_dir: e.is_dir,
            size: e.size,
            mtime: e.mtime,
        })
        .collect())
}

/// 打开 115 上的书籍（path 为文件提取码，三态策略）。
pub async fn open_cloud115_book(session: u64, path: String, strategy: String) -> Result<BookInfo> {
    let client = get_cloud115_session(session)?;
    let origin = client.origin();
    let cache_ns = format!("115|{}|{}", origin, path);
    let strat = parse_strategy(&strategy);

    let progress = if strat != OpenStrategy::Stream {
        let p = Arc::new(DownloadProgress::new(0)); // 115 直链不带大小，下载响应后更新
        cloud115_downloads()
            .lock()
            .unwrap()
            .insert(session, Arc::clone(&p));
        Some(p)
    } else {
        None
    };

    let book = {
        let client = Arc::clone(&client);
        let path = path.clone();
        tokio::task::spawn_blocking(move || -> Result<Box<dyn document::Document>> {
            let open_local =
                |local_path: std::path::PathBuf| -> Result<Box<dyn document::Document>> {
                    let src = crate::source::local::LocalFile::open(&local_path)?;
                    document::open_document(src, &path)
                };
            let open_stream = |client: Arc<Cloud115Client>| -> Result<Box<dyn document::Document>> {
                let (url, _) = client.downurl(&path)?;
                let size = client
                    .probe_size(&url)
                    .ok_or_else(|| anyhow::anyhow!("115 直链不支持 Range，请改用整本下载策略"))?;
                let src = cloud115_source::Cloud115File::new(client, path.clone(), size, url);
                document::open_document(src, &path)
            };
            match strat {
                OpenStrategy::Download => {
                    let local_path = client.download_to_raw_cache(&path, &path, progress)?;
                    tracing::info!("115 整本已缓存: {}", local_path.display());
                    open_local(local_path)
                }
                OpenStrategy::Stream => {
                    // 缓存优先：已有 raw/ 本地缓存直接本地打开，不联网
                    match cloud115_source::raw_cache_path(&client.origin(), &path) {
                        Some(local_path) => {
                            tracing::info!("115 命中缓存，直接本地打开: {}", local_path.display());
                            open_local(local_path)
                        }
                        None => open_stream(Arc::clone(&client)),
                    }
                }
                OpenStrategy::Auto => match client.download_to_raw_cache(&path, &path, progress) {
                    Ok(local_path) => {
                        tracing::info!("115 整本已缓存: {}", local_path.display());
                        open_local(local_path)
                    }
                    Err(e) => {
                        tracing::warn!("115 整本下载失败，回退流式: {e}");
                        open_stream(Arc::clone(&client))
                    }
                },
            }
        })
        .await??
    };

    if strat != OpenStrategy::Stream {
        cloud115_downloads().lock().unwrap().remove(&session);
    }
    Ok(register_book(book, &cache_ns))
}

/// 115 下载进度（0.0~1.0，非下载中返回 1.0）。
pub fn cloud115_download_progress(session: u64) -> f64 {
    cloud115_downloads()
        .lock()
        .unwrap()
        .get(&session)
        .map(|p| p.fraction())
        .unwrap_or(1.0)
}

/// 115 书籍是否已有 raw/ 本地缓存。
pub fn cloud115_has_raw_cache(session: u64, path: String) -> bool {
    let client = match get_cloud115_session(session) {
        Ok(c) => c,
        Err(_) => return false,
    };
    cloud115_source::raw_cache_path(&client.origin(), &path).is_some()
}

/// 115 书籍封面（cover/ 磁盘缓存 → raw/ 本地缓存 → 流式解码）。
pub async fn cloud115_cover(
    session: u64,
    path: String,
    page: u32,
    width: u32,
    height: u32,
    crop: Option<CropRect>,
) -> Result<PageImage> {
    let client = get_cloud115_session(session)?;
    let origin = client.origin();
    let crop_tuple = crop.as_ref().map(|r| (r.x, r.y, r.w, r.h));
    let cache_lookup_path = cloud115_source::raw_cache_path(&origin, &path)
        .or_else(|| Some(std::path::PathBuf::from(&path)));
    if let Some(ref lookup) = cache_lookup_path {
        let lookup_str = lookup.to_string_lossy();
        if let Some((rgba, w, h)) =
            cache::cover_cache_read(&lookup_str, page, width, height, crop_tuple)
        {
            return Ok(PageImage {
                rgba,
                width: w,
                height: h,
            });
        }
    }
    let origin_clone = origin.clone();
    let path_clone = path.clone();
    let client_clone = Arc::clone(&client);
    let img = tokio::task::spawn_blocking(move || -> Result<crate::decode::DecodedImage> {
        let governor = crate::reader::blocking_request_governor();
        let _permit = governor.acquire(crate::reader::RequestPriority::Cover)?;
        if let Some(local_path) = cloud115_source::raw_cache_path(&origin_clone, &path_clone) {
            let src = crate::source::local::LocalFile::open(&local_path)?;
            let book = document::open_document(src, &path_clone)?;
            let bytes = book.page_bytes(page)?;
            let crop = crop.map(|r| (r.x, r.y, r.w, r.h));
            return crate::decode::decode_cover(&bytes, width, height, crop);
        }
        let (url, _) = client_clone.downurl(&path_clone)?;
        let size = match client_clone.probe_size(&url) {
            Some(s) => s,
            None => anyhow::bail!("远程封面需要 Range 支持"),
        };
        let src = cloud115_source::Cloud115File::new(client_clone, path_clone.clone(), size, url);
        let book = document::open_document(src, &path_clone)?;
        let bytes = book.page_bytes(page)?;
        let crop = crop.map(|r| (r.x, r.y, r.w, r.h));
        crate::decode::decode_cover(&bytes, width, height, crop)
    })
    .await??;
    let cache_write_path = cloud115_source::raw_cache_path(&origin, &path)
        .or_else(|| Some(std::path::PathBuf::from(&path)));
    if let Some(ref wp) = cache_write_path {
        let _ = cache::cover_cache_write(
            &wp.to_string_lossy(),
            page,
            width,
            height,
            crop_tuple,
            &img.rgba,
        );
    }
    Ok(PageImage {
        rgba: img.rgba,
        width: img.width,
        height: img.height,
    })
}


#[cfg(test)]
mod d2_cache_authority_tests {
    //! P1-D-2：legacy cover 的**双历史 key** 兼容契约（CA 套件）。
    //!
    //! 冻结的 per-family 能力（审阅通过）：
    //! * **Family 1**（webdav / sftp / baidu / 115）：raw 文件删除后，历史 raw path 可由
    //!   production 算法**精确重建** → `present→absent` 必须 HIT。
    //! * **Family 2**（115web / quark）：raw 文件名来自 provider 网络响应 `info.name`
    //!   且**从未持久化** → `present→absent` 属**已证明不可恢复的历史状态**，
    //!   必须 clean MISS，**禁止猜测补齐**。
    //!
    //! 注意：需要 `--test-threads=1`（`cache::set_custom_cache_root` 是进程级全局）。

    use super::*;
    use std::path::{Path, PathBuf};

    const PAGE: u32 = 0;
    const W: u32 = 4;
    const H: u32 = 4;

    struct Fixture {
        kind: &'static str,
        /// Family 2：raw 文件名由 provider 决定，测试用任意"远端文件名"模拟 writer。
        family2: bool,
        expected_authority: &'static str,
        dto: LegacyCoverLocalLookupDto,
    }

    fn base_dto(kind: &str) -> LegacyCoverLocalLookupDto {
        LegacyCoverLocalLookupDto {
            kind: kind.to_string(),
            url: String::new(),
            host: String::new(),
            port: 22,
            app_key: String::new(),
            app_id: String::new(),
            root_id: String::new(),
            root: String::new(),
            logical_path: "/books/demo.cbz".to_string(),
            page: PAGE,
            width: W,
            height: H,
            crop: None,
        }
    }

    /// 六条 authority 路径的 fixture（115 app / web 必须分别覆盖）。
    fn fixtures() -> Vec<Fixture> {
        let mut webdav = base_dto("webdav");
        webdav.url = "https://dav.example.com/dav".to_string();
        let mut sftp22 = base_dto("sftp");
        sftp22.host = "nas.local".to_string();
        sftp22.port = 22;
        let mut sftp_custom = base_dto("sftp");
        sftp_custom.host = "nas.local".to_string();
        sftp_custom.port = 2222;
        let mut baidu = base_dto("baidu");
        baidu.app_key = "appkey".to_string();
        let mut c115 = base_dto("115");
        c115.app_id = "appid".to_string();
        let c115web = base_dto("115web");
        let quark = base_dto("quark");

        vec![
            Fixture {
                kind: "webdav",
                family2: false,
                expected_authority: "https://dav.example.com",
                dto: webdav,
            },
            Fixture {
                kind: "sftp(22)",
                family2: false,
                expected_authority: "nas.local",
                dto: sftp22,
            },
            Fixture {
                kind: "sftp(2222)",
                family2: false,
                expected_authority: "nas.local:2222",
                dto: sftp_custom,
            },
            Fixture {
                kind: "baidu",
                family2: false,
                expected_authority: "baidu:appkey:/",
                dto: baidu,
            },
            Fixture {
                kind: "115",
                family2: false,
                expected_authority: "115:appid:0",
                dto: c115,
            },
            Fixture {
                kind: "115web",
                family2: true,
                expected_authority: "115web:0",
                dto: c115web,
            },
            Fixture {
                kind: "quark",
                family2: true,
                expected_authority: "quark:0",
                dto: quark,
            },
        ]
    }

    fn dto_copy(dto: &LegacyCoverLocalLookupDto) -> LegacyCoverLocalLookupDto {
        LegacyCoverLocalLookupDto {
            kind: dto.kind.clone(),
            url: dto.url.clone(),
            host: dto.host.clone(),
            port: dto.port,
            app_key: dto.app_key.clone(),
            app_id: dto.app_id.clone(),
            root_id: dto.root_id.clone(),
            root: dto.root.clone(),
            logical_path: dto.logical_path.clone(),
            page: dto.page,
            width: dto.width,
            height: dto.height,
            crop: None,
        }
    }

    fn logical(dto: &LegacyCoverLocalLookupDto) -> &str {
        dto.logical_path.as_str()
    }

    /// 确定性 raw 路径：Family 1 用 candidate；Family 2 只有目录（文件名不可推导）。
    fn raw_path_for(fixture: &Fixture, authority: &str) -> Option<PathBuf> {
        let dto = &fixture.dto;
        match dto.kind.as_str() {
            "webdav" => Some(crate::source::webdav::raw_cache_candidate_path(
                authority,
                logical(dto),
            )),
            "sftp" => Some(crate::source::sftp::raw_cache_candidate_path(
                authority,
                logical(dto),
            )),
            "baidu" => Some(crate::source::baidu::raw_cache_candidate_path(
                authority,
                logical(dto),
            )),
            "115" => Some(crate::source::cloud115::raw_cache_candidate_path(
                authority,
                logical(dto),
            )),
            _ => None,
        }
    }

    fn raw_dir_for(fixture: &Fixture, authority: &str) -> PathBuf {
        let dto = &fixture.dto;
        match dto.kind.as_str() {
            "115web" => crate::source::cloud115::web_raw_cache_dir(authority, logical(dto)),
            "quark" => crate::source::quark::raw_cache_dir(authority, logical(dto)),
            _ => raw_path_for(fixture, authority)
                .and_then(|p| p.parent().map(Path::to_path_buf))
                .expect("family 1 candidate must have a parent"),
        }
    }

    /// 按"当前 raw 文件是否存在"复刻 production 的 current key 表达式。
    fn current_key(fixture: &Fixture, authority: &str) -> PathBuf {
        let dto = &fixture.dto;
        let logical_buf = PathBuf::from(logical(dto));
        match dto.kind.as_str() {
            "115web" => crate::source::cloud115::web_raw_cache_path(authority, logical(dto))
                .unwrap_or(logical_buf),
            "quark" => {
                crate::source::quark::raw_cache_path(authority, logical(dto)).unwrap_or(logical_buf)
            }
            "webdav" => crate::source::webdav::raw_cache_path(authority, logical(dto))
                .unwrap_or(logical_buf),
            "sftp" => crate::source::sftp::raw_cache_path(authority, logical(dto))
                .unwrap_or(logical_buf),
            "baidu" => crate::source::baidu::raw_cache_path(authority, logical(dto))
                .unwrap_or(logical_buf),
            "115" => crate::source::cloud115::raw_cache_path(authority, logical(dto))
                .unwrap_or(logical_buf),
            other => panic!("unknown kind {other}"),
        }
    }

    /// 模拟 production writer 写出 raw 文件（内容非空），返回其路径。
    fn create_raw(fixture: &Fixture, authority: &str) -> PathBuf {
        let dir = raw_dir_for(fixture, authority);
        std::fs::create_dir_all(&dir).expect("raw dir");
        // Family 2 的文件名来自 provider（任意值）；Family 1 用确定性名。
        let path = if fixture.family2 {
            dir.join("provider-returned-name.cbz")
        } else {
            raw_path_for(fixture, authority).expect("family 1 raw path")
        };
        std::fs::write(&path, b"raw-bytes").expect("raw write");
        path
    }

    fn remove_raw(path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    fn fresh_root(name: &str) {
        let root = std::env::temp_dir().join(format!("rch_p1d2_ca_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        crate::cache::set_custom_cache_root(root.to_str().unwrap());
    }

    fn write_cover(key: &Path) {
        let rgba = vec![7u8; (W * H * 4) as usize];
        crate::cache::cover_cache_write(&key.to_string_lossy(), PAGE, W, H, None, &rgba).unwrap();
    }

    /// 保证测试结束时恢复默认 cache root（即使 panic），
    /// 否则会污染同进程内其它测试（曾导致 cache::tests::cache_root_defaults_to_appdata 失败）。
    struct CacheRootGuard;
    impl Drop for CacheRootGuard {
        fn drop(&mut self) {
            crate::cache::set_custom_cache_root("");
        }
    }

    /// CA-4：冻结六条 authority 的现有字面规则（**不定义任何新 normalization**）。
    #[test]
    fn d2_ca4_authority_literals_are_frozen() {
        let _guard = CacheRootGuard;
        fresh_root("ca4");
        for fixture in fixtures() {
            let authority = legacy_cover_authority(&fixture.dto)
                .unwrap_or_else(|| panic!("authority must derive for {}", fixture.kind));
            assert_eq!(
                authority, fixture.expected_authority,
                "authority literal changed for {}",
                fixture.kind
            );
        }
    }

    /// CA-2 / CA-1：**完全没有 runtime session** 时，六条路径当前可恢复的旧缓存都能被读到。
    ///
    /// 这里没有任何 session/registry/epoch 参与 —— reader 只用 logical fields。
    #[test]
    fn d2_ca1_ca2_sessionless_lookup_reads_existing_caches() {
        let _guard = CacheRootGuard;
        fresh_root("ca12");
        for fixture in fixtures() {
            let authority = legacy_cover_authority(&fixture.dto).unwrap();
            // 当前状态：raw 存在（production writer 会以 raw path 为键写 cover）
            let raw = create_raw(&fixture, &authority);
            let key = current_key(&fixture, &authority);
            assert_eq!(key, raw, "current key must be the probed raw path");
            write_cover(&key);

            let hit = read_legacy_cover_local(dto_copy(&fixture.dto)).expect("sessionless hit");
            assert_eq!(hit.width, W);
            assert_eq!(hit.height, H);
            assert_eq!(hit.rgba.len(), (W * H * 4) as usize);
        }
    }

    /// CA-STATE-1 / 2 / 3 / 4：raw 状态转换矩阵。
    ///
    /// Family 1 四种全 HIT；Family 2 的 `present→absent` 为已证明不可恢复 → MISS。
    #[test]
    fn d2_ca_state_transition_matrix() {
        let _guard = CacheRootGuard;
        let mut failures: Vec<String> = Vec::new();
        for (index, fixture) in fixtures().into_iter().enumerate() {
            fresh_root(&format!("state{index}"));
            let authority = legacy_cover_authority(&fixture.dto).unwrap();
            for (transition, (write_present, read_present)) in
                [(true, true), (false, false), (true, false), (false, true)]
                    .into_iter()
                    .enumerate()
            {
                // 每个状态转换都必须从**干净的 cover 缓存**开始，否则上一轮写在
                // 另一个 key 下的 .cover 会造成假命中（测试自身的状态泄漏）。
                fresh_root(&format!("state{index}_{transition}"));
                // 每次转换都从干净的 raw 状态开始。
                let raw = create_raw(&fixture, &authority);
                if !write_present {
                    remove_raw(&raw);
                }
                let write_key = current_key(&fixture, &authority);
                write_cover(&write_key);

                // 调整到"读取时"的 raw 状态。
                match read_present {
                    true => {
                        let _ = create_raw(&fixture, &authority);
                    }
                    false => remove_raw(&raw),
                }

                let hit = read_legacy_cover_local(dto_copy(&fixture.dto)).is_some();
                let unrecoverable = fixture.family2 && write_present && !read_present;
                let expected = !unrecoverable;
                if hit != expected {
                    failures.push(format!(
                        "{} write_raw_present={write_present} read_raw_present={read_present} => got hit={hit} want hit={expected}",
                        fixture.kind
                    ));
                }
                // 清理，保证下一次转换从确定状态开始。
                remove_raw(&raw);
            }
        }
        assert!(
            failures.is_empty(),
            "CA state matrix violated in {} case(s):\n  {}",
            failures.len(),
            failures.join("\n  ")
        );
    }

    /// CA-STATE-3-UNRECOVERABLE（冻结已证明的局限，**不是失败**）：
    ///
    /// 115-web / Quark：raw 存在时以 raw-key 写入 cover → 删除 raw → 保留 `.cover`
    /// → sessionless lookup 必须 **clean MISS**。
    ///
    /// 原因：`remote filename was not durably persisted; historical raw key cannot be
    /// reconstructed`（文件名来自 provider 网络响应的 `info.name`）。
    /// 禁止任何猜测补齐（不扫 `.cover` 目录、不枚举 hash、不调 provider、不建 session）。
    #[test]
    fn d2_ca_state3_unrecoverable_for_family2_is_a_clean_miss() {
        let _guard = CacheRootGuard;
        fresh_root("state3");
        for fixture in fixtures().into_iter().filter(|f| f.family2) {
            let authority = legacy_cover_authority(&fixture.dto).unwrap();
            let raw = create_raw(&fixture, &authority);
            let write_key = current_key(&fixture, &authority);
            write_cover(&write_key);
            // 该 `.cover` 确实存在（证明是"不可达"而不是"没写过"）。
            assert!(
                crate::cache::cover_cache_read(&write_key.to_string_lossy(), PAGE, W, H, None)
                    .is_some(),
                "the historical raw-key cover must really exist on disk"
            );
            remove_raw(&raw);

            assert!(
                read_legacy_cover_local(dto_copy(&fixture.dto)).is_none(),
                "{} must be a clean MISS when the historical raw filename is gone",
                fixture.kind
            );
        }
    }

    /// CA-3：miss 时完全无副作用（无 session / 无 provider / 不产生额外文件）。
    #[test]
    fn d2_ca3_miss_is_side_effect_free() {
        let _guard = CacheRootGuard;
        fresh_root("ca3");
        for (index, fixture) in fixtures().into_iter().enumerate() {
            let before: Option<usize> = None;
            let _ = before;
            let cover_dir = crate::cache::CacheDir::Cover.path();
            let count_files = |dir: &Path| -> usize {
                std::fs::read_dir(dir)
                    .map(|it| it.flatten().count())
                    .unwrap_or(0)
            };
            let before_cover = count_files(&cover_dir);
            // 无任何 raw、无任何 cover。
            assert!(
                read_legacy_cover_local(dto_copy(&fixture.dto)).is_none(),
                "fixture {index} must miss"
            );
            assert_eq!(
                count_files(&cover_dir),
                before_cover,
                "a miss must not create cache files (fixture {index})"
            );
        }
    }

    /// R5：local-only lookup **永不** fallback 到 provider —— 它不接受 session、
    /// 也不返回任何"需要联网"的信号；miss 就是 `None`。
    #[test]
    fn d2_r5_local_lookup_never_falls_back_to_provider() {
        let _guard = CacheRootGuard;
        fresh_root("r5");
        // 即便传进来的 kind 完全未知，也只是 None，不会尝试任何网络路径。
        let mut unknown = base_dto("unknown-provider");
        unknown.url = "https://dav.example.com/dav".to_string();
        assert!(read_legacy_cover_local(unknown).is_none());
    }
}
