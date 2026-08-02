//! 远程书源 API(WebDAV 会话与浏览)。

use super::book::{register_book, BookInfo, CropRect, DirEntry, PageImage};
use crate::document;
use crate::source::webdav::{self, DownloadProgress, WebDavClient, WebDavFile};
use crate::cache;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

static SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<WebDavClient>>>> = OnceLock::new();
static NEXT: OnceLock<Mutex<u64>> = OnceLock::new();

/// 正在进行的下载进度追踪表(session_id -> DownloadProgress)。
static DOWNLOADS: OnceLock<Mutex<HashMap<u64, Arc<DownloadProgress>>>> = OnceLock::new();

fn downloads() -> &'static Mutex<HashMap<u64, Arc<DownloadProgress>>> {
    DOWNLOADS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn sessions() -> &'static Mutex<HashMap<u64, Arc<WebDavClient>>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
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
/// 策略: 先尝试整本下载到 raw/ 缓存, 后续基于本地缓存阅读。
/// 若已有缓存则直接复用(秒开)。
/// 这是四层架构的关键: 阅读器只操作本地资源。
/// 下载进度可通过 webdav_download_progress(session) 轮询。
pub async fn open_webdav_book(session: u64, path: String) -> Result<BookInfo> {
    let client = get_session(session)?;
    let origin = client.origin().to_string();
    let cache_ns = format!("webdav|{}|{}", origin, path);

    // 记录本次下载进度(供轮询)
    let file_size = {
        let client = Arc::clone(&client);
        let path = path.clone();
        tokio::task::spawn_blocking(move || client.file_size(&path)).await??
    };
    let progress = Arc::new(DownloadProgress::new(file_size));
    downloads().lock().unwrap().insert(session, Arc::clone(&progress));

    let book = {
        let client = Arc::clone(&client);
        let path = path.clone();
        let progress = Arc::clone(&progress);
        tokio::task::spawn_blocking(move || -> Result<Box<dyn document::Document>> {
            // 优先尝试整本下载到 raw/ 缓存
            match client.download_to_raw_cache(&path, Some(progress)) {
                Ok(local_path) => {
                    tracing::info!("WebDAV 整本已缓存: {}", local_path.display());
                    let src = crate::source::local::LocalFile::open(&local_path)?;
                    document::open_document(src, &path)
                }
                Err(e) => {
                    tracing::warn!("WebDAV 整本下载失败, 回退到 Range 流式: {e}");
                    // 回退: Range 流式(原有逻辑)
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
        })
        .await??
    };

    // 下载完成后从跟踪表移除
    downloads().lock().unwrap().remove(&session);

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
