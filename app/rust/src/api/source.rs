//! 远程书源 API(WebDAV / SFTP 会话与浏览)。

use super::book::{register_book, BookInfo, CropRect, DirEntry, PageImage};
use crate::document;
use crate::source::sftp::{self as sftp_source, SftpClient};
use crate::source::webdav::{self, DownloadProgress, WebDavClient, WebDavFile};
use crate::cache;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

static SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<WebDavClient>>>> = OnceLock::new();
static SFTP_SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<SftpClient>>>> = OnceLock::new();
static NEXT: OnceLock<Mutex<u64>> = OnceLock::new();

/// 正在进行的下载进度追踪表(session_id -> DownloadProgress)。
static DOWNLOADS: OnceLock<Mutex<HashMap<u64, Arc<DownloadProgress>>>> = OnceLock::new();
static SFTP_DOWNLOADS: OnceLock<Mutex<HashMap<u64, Arc<DownloadProgress>>>> = OnceLock::new();

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
                        // 无 Range 服务器无法流式, 只能整本下载(download/ 回退)
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
