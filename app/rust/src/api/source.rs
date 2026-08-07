//! 远程书源 API(WebDAV / SFTP 会话与浏览)。

use super::book::{register_book, BookInfo, CropRect, DirEntry, PageImage};
use crate::document;
use crate::source::baidu::{self as baidu_source, BaiduClient};
use crate::source::cloud115::{self as cloud115_source, Cloud115Client};
use crate::source::quark::{self as quark_source, QuarkClient};
use crate::source::sftp::{self as sftp_source, SftpClient};
use crate::source::webdav::{self, DownloadProgress, WebDavClient, WebDavFile};
use crate::cache;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

static SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<WebDavClient>>>> = OnceLock::new();
static SFTP_SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<SftpClient>>>> = OnceLock::new();
static BAIDU_SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<BaiduClient>>>> = OnceLock::new();
static CLOUD115_SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<Cloud115Client>>>> = OnceLock::new();
static QUARK_SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<QuarkClient>>>> = OnceLock::new();
static NEXT: OnceLock<Mutex<u64>> = OnceLock::new();

/// 正在进行的下载进度追踪表(session_id -> DownloadProgress)。
static DOWNLOADS: OnceLock<Mutex<HashMap<u64, Arc<DownloadProgress>>>> = OnceLock::new();
static SFTP_DOWNLOADS: OnceLock<Mutex<HashMap<u64, Arc<DownloadProgress>>>> = OnceLock::new();
static BAIDU_DOWNLOADS: OnceLock<Mutex<HashMap<u64, Arc<DownloadProgress>>>> = OnceLock::new();
static CLOUD115_DOWNLOADS: OnceLock<Mutex<HashMap<u64, Arc<DownloadProgress>>>> = OnceLock::new();
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

fn get_session(id: u64) -> Result<Arc<WebDavClient>> {
    sessions()
        .lock()
        .unwrap()
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| anyhow::anyhow!("无效的 WebDAV 会话: {id}"))
}

fn get_sftp_session(id: u64) -> Result<Arc<SftpClient>> {
    sftp_sessions()
        .lock()
        .unwrap()
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| anyhow::anyhow!("无效的 SFTP 会话: {id}"))
}

fn get_baidu_session(id: u64) -> Result<Arc<BaiduClient>> {
    baidu_sessions()
        .lock()
        .unwrap()
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| anyhow::anyhow!("无效的百度网盘会话: {id}"))
}

fn get_cloud115_session(id: u64) -> Result<Arc<Cloud115Client>> {
    cloud115_sessions()
        .lock()
        .unwrap()
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| anyhow::anyhow!("无效的 115 网盘会话: {id}"))
}

fn get_quark_session(id: u64) -> Result<Arc<QuarkClient>> {
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
pub async fn webdav_connect(url: String, username: String, password: String) -> Result<WebDavSession> {
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
    Ok(WebDavSession { id, root, capability_label: label })
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
/// 策略(strategy): "auto" 先尝试整本下载到 raw/ 缓存, 失败回退流式;
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
                    if client.range_supported(&path)? {
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
    let client = tokio::task::spawn_blocking(move || {
        SftpClient::connect(&host, port, &username, &password)
    })
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
pub async fn open_sftp_book(
    session: u64,
    path: String,
    strategy: String,
) -> Result<BookInfo> {
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
        sftp_downloads().lock().unwrap().insert(session, Arc::clone(&p));
        Some(p)
    } else {
        None
    };

    let book = {
        let client = Arc::clone(&client);
        let path = path.clone();
        tokio::task::spawn_blocking(move || -> Result<Box<dyn document::Document>> {
            let open_local = |local_path: std::path::PathBuf| -> Result<Box<dyn document::Document>> {
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
                    let len = client.file_size(&path)?;
                    let src = sftp_source::SftpFile::new(client, path.clone(), len);
                    document::open_document(src, &path)
                }
                OpenStrategy::Auto => {
                    match client.download_to_raw_cache(&path, progress) {
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
                    }
                }
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
    let (client, root) =
        tokio::task::spawn_blocking(move || -> Result<(QuarkClient, String)> {
            let client = QuarkClient::new(&cookie, &root_id)?;
            client.check()?; // /config + 首屏 list
            let root = client.root().to_string();
            Ok((client, root))
        })
        .await??;
    let id = next_id();
    let session_cookie = client.cookie();
    quark_sessions().lock().unwrap().insert(id, Arc::new(client));
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
pub async fn open_quark_book(
    session: u64,
    path: String,
    strategy: String,
) -> Result<BookInfo> {
    let client = get_quark_session(session)?;
    let origin = client.origin();
    let cache_ns = format!("quark|{}|{}", origin, path);
    let strat = parse_strategy(&strategy);

    let progress = if strat != OpenStrategy::Stream {
        let p = Arc::new(DownloadProgress::new(0)); // 直链不带大小，下载响应后更新
        quark_downloads().lock().unwrap().insert(session, Arc::clone(&p));
        Some(p)
    } else {
        None
    };

    let book = {
        let client = Arc::clone(&client);
        let path = path.clone();
        tokio::task::spawn_blocking(move || -> Result<Box<dyn document::Document>> {
            let name = client.resolve_name(&path)?;
            let open_local = |local_path: std::path::PathBuf| -> Result<Box<dyn document::Document>> {
                let src = crate::source::local::LocalFile::open(&local_path)?;
                document::open_document(src, &name)
            };
            let open_stream = |client: Arc<QuarkClient>| -> Result<Box<dyn document::Document>> {
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
                OpenStrategy::Stream => open_stream(Arc::clone(&client)),
                OpenStrategy::Auto => {
                    match client.download_to_raw_cache(&path, progress) {
                        Ok(local_path) => {
                            tracing::info!("夸克网盘整本已缓存: {}", local_path.display());
                            open_local(local_path)
                        }
                        Err(e) => {
                            tracing::warn!("夸克网盘整本下载失败，回退流式: {e}");
                            open_stream(Arc::clone(&client))
                        }
                    }
                }
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
            return Ok(PageImage { rgba, width: w, height: h });
        }
    }
    let origin_clone = origin.clone();
    let path_clone = path.clone();
    let client_clone = Arc::clone(&client);
    let img = tokio::task::spawn_blocking(move || -> Result<crate::decode::DecodedImage> {
        let name = client_clone.resolve_name(&path_clone)?;
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
            let local_path = client_clone.download_to_raw_cache(&path_clone, None)?;
            let src = crate::source::local::LocalFile::open(&local_path)?;
            let book = document::open_document(src, &name)?;
            let bytes = book.page_bytes(page)?;
            let crop = crop.map(|r| (r.x, r.y, r.w, r.h));
            return crate::decode::decode_cover(&bytes, width, height, crop);
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
    let cache_lookup_path = webdav::raw_cache_path(&origin, &path)
        .or_else(|| Some(std::path::PathBuf::from(&path)));
    if let Some(ref lookup) = cache_lookup_path {
        let lookup_str = lookup.to_string_lossy();
        if let Some((rgba, w, h)) = cache::cover_cache_read(&lookup_str, page, width, height, crop_tuple) {
            return Ok(PageImage { rgba, width: w, height: h });
        }
    }
    let origin_clone = origin.clone();
    let path_clone = path.clone();
    let client_clone = Arc::clone(&client);
    let img = tokio::task::spawn_blocking(move || -> Result<crate::decode::DecodedImage> {
        // 先尝试 raw/ 本地缓存(已下载过的漫画直接本地秒出)
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
    let cache_write_path = webdav::raw_cache_path(&origin, &path)
        .or_else(|| Some(std::path::PathBuf::from(&path)));
    if let Some(ref wp) = cache_write_path {
        let _ = cache::cover_cache_write(&wp.to_string_lossy(), page, width, height, crop_tuple, &img.rgba);
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
    baidu_sessions().lock().unwrap().insert(id, Arc::new(client));
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
pub async fn open_baidu_book(
    session: u64,
    path: String,
    strategy: String,
) -> Result<BookInfo> {
    let client = get_baidu_session(session)?;
    let origin = client.origin();
    let cache_ns = format!("baidu|{}|{}", origin, path);
    let strat = parse_strategy(&strategy);

    let progress = if strat != OpenStrategy::Stream {
        let (client, path) = (Arc::clone(&client), path.clone());
        let size = tokio::task::spawn_blocking(move || client.dlink(&path).map(|(_, s)| s))
            .await??;
        let p = Arc::new(DownloadProgress::new(size));
        baidu_downloads().lock().unwrap().insert(session, Arc::clone(&p));
        Some(p)
    } else {
        None
    };

    let book = {
        let client = Arc::clone(&client);
        let path = path.clone();
        tokio::task::spawn_blocking(move || -> Result<Box<dyn document::Document>> {
            let open_local = |local_path: std::path::PathBuf| -> Result<Box<dyn document::Document>> {
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
                OpenStrategy::Stream => open_stream(Arc::clone(&client)),
                OpenStrategy::Auto => match client.download_to_raw_cache(&path, progress) {
                    Ok(local_path) => {
                        tracing::info!("百度网盘整本已缓存: {}", local_path.display());
                        open_local(local_path)
                    }
                    Err(e) => {
                        tracing::warn!("百度网盘整本下载失败，回退流式: {e}");
                        open_stream(Arc::clone(&client))
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
            return Ok(PageImage { rgba, width: w, height: h });
        }
    }
    let origin_clone = origin.clone();
    let path_clone = path.clone();
    let client_clone = Arc::clone(&client);
    let img = tokio::task::spawn_blocking(move || -> Result<crate::decode::DecodedImage> {
        if let Some(local_path) = baidu_source::raw_cache_path(&origin_clone, &path_clone) {
            let src = crate::source::local::LocalFile::open(&local_path)?;
            let book = document::open_document(src, &path_clone)?;
            let bytes = book.page_bytes(page)?;
            let crop = crop.map(|r| (r.x, r.y, r.w, r.h));
            return crate::decode::decode_cover(&bytes, width, height, crop);
        }
        let (link, size) = client_clone.dlink(&path_clone)?;
        if !client_clone.probe_range(&link) {
            let local_path = client_clone.download_to_raw_cache(&path_clone, None)?;
            let src = crate::source::local::LocalFile::open(&local_path)?;
            let book = document::open_document(src, &path_clone)?;
            let bytes = book.page_bytes(page)?;
            let crop = crop.map(|r| (r.x, r.y, r.w, r.h));
            return crate::decode::decode_cover(&bytes, width, height, crop);
        }
        let src = baidu_source::BaiduFile::new(
            client_clone,
            path_clone.clone(),
            size,
            link,
        );
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
    let r = tokio::task::spawn_blocking(move || cloud115_source::qr_poll(&uid, time, &sign))
        .await??;
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
    cloud115_sessions().lock().unwrap().insert(id, Arc::new(client));
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
pub async fn open_cloud115_book(
    session: u64,
    path: String,
    strategy: String,
) -> Result<BookInfo> {
    let client = get_cloud115_session(session)?;
    let origin = client.origin();
    let cache_ns = format!("115|{}|{}", origin, path);
    let strat = parse_strategy(&strategy);

    let progress = if strat != OpenStrategy::Stream {
        let p = Arc::new(DownloadProgress::new(0)); // 115 直链不带大小，下载响应后更新
        cloud115_downloads().lock().unwrap().insert(session, Arc::clone(&p));
        Some(p)
    } else {
        None
    };

    let book = {
        let client = Arc::clone(&client);
        let path = path.clone();
        tokio::task::spawn_blocking(move || -> Result<Box<dyn document::Document>> {
            let open_local = |local_path: std::path::PathBuf| -> Result<Box<dyn document::Document>> {
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
                OpenStrategy::Stream => open_stream(Arc::clone(&client)),
                OpenStrategy::Auto => {
                    match client.download_to_raw_cache(&path, &path, progress) {
                        Ok(local_path) => {
                            tracing::info!("115 整本已缓存: {}", local_path.display());
                            open_local(local_path)
                        }
                        Err(e) => {
                            tracing::warn!("115 整本下载失败，回退流式: {e}");
                            open_stream(Arc::clone(&client))
                        }
                    }
                }
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
            return Ok(PageImage { rgba, width: w, height: h });
        }
    }
    let origin_clone = origin.clone();
    let path_clone = path.clone();
    let client_clone = Arc::clone(&client);
    let img = tokio::task::spawn_blocking(move || -> Result<crate::decode::DecodedImage> {
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
            None => {
                let local_path =
                    client_clone.download_to_raw_cache(&path_clone, &path_clone, None)?;
                let src = crate::source::local::LocalFile::open(&local_path)?;
                let book = document::open_document(src, &path_clone)?;
                let bytes = book.page_bytes(page)?;
                let crop = crop.map(|r| (r.x, r.y, r.w, r.h));
                return crate::decode::decode_cover(&bytes, width, height, crop);
            }
        };
        let src =
            cloud115_source::Cloud115File::new(client_clone, path_clone.clone(), size, url);
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
