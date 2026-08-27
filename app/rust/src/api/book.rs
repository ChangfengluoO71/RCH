//! 暴露给 Dart 的阅读相关 API。
//!
//! 会话模型:打开一本书得到 u64 句柄,后续按句柄读页、用完关闭。
//! 重 IO(解压/解码)经 `spawn_blocking` 隔离;翻页流畅性由
//! `reader::Reader` 的缓存 + 预取保证。

use crate::reader::Reader;
use crate::{cache, decode, document, source::local};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

struct BookSession {
    reader: Arc<Reader>,
}

static SESSIONS: OnceLock<Mutex<HashMap<u64, BookSession>>> = OnceLock::new();
static NEXT_ID: OnceLock<Mutex<u64>> = OnceLock::new();

fn sessions() -> &'static Mutex<HashMap<u64, BookSession>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_id() -> u64 {
    let m = NEXT_ID.get_or_init(|| Mutex::new(0));
    let mut g = m.lock().unwrap();
    *g += 1;
    *g
}

/// 书籍信息。
pub struct BookInfo {
    pub handle: u64,
    pub title: String,
    pub page_count: u32,
}

/// 一页解码后的位图(RGBA8888)。
pub struct PageImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// 封面裁剪区域(相对坐标 0-1)。
pub struct CropRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 注册一本书为阅读会话,返回句柄信息(供本地 / WebDAV 等来源复用)。
/// 首个可见页由 Dart 发起 `book_page()`；该前台页完成后 Reader 再围绕真实页码预取，
/// 避免注册阶段固定从第 0 页抢跑后台任务，挤占 PDF 等重渲染格式的首屏请求。
pub(crate) fn register_book(book: Box<dyn document::Document>, cache_ns: &str) -> BookInfo {
    let reader = Arc::new(Reader::new(book, cache_ns));
    let handle = next_id();
    let info = BookInfo {
        handle,
        title: reader.title(),
        page_count: reader.page_count(),
    };
    sessions().lock().unwrap().insert(
        handle,
        BookSession {
            reader: Arc::clone(&reader),
        },
    );
    info
}

/// 打开本地书籍(ZIP/CBZ/EPUB),返回会话句柄与信息。
/// 若 path 为目录,则走 Folder 格式(枚举目录下图片)。
pub async fn open_local_book(path: String) -> Result<BookInfo> {
    let book = tokio::task::spawn_blocking({
        let path = path.clone();
        move || -> Result<Box<dyn document::Document>> {
            if std::path::Path::new(&path).is_dir() {
                return document::open_folder_document(&path);
            }
            let src = local::LocalFile::open(&path)?;
            document::open_document(src, &path)
        }
    })
    .await??;
    let cache_ns = format!("local|{}", path);
    Ok(register_book(book, &cache_ns))
}

/// 读取一页的原始图片字节(优先命中缓存/预取,翻页秒出)。
/// 返回 ZIP 内该页的原始字节(JPEG/PNG 等);像素解码由 Flutter 侧完成(自带 image cache)。
pub async fn book_page(handle: u64, index: u32) -> Result<Vec<u8>> {
    let reader = {
        let g = sessions().lock().unwrap();
        g.get(&handle).map(|s| Arc::clone(&s.reader))
    };
    let reader = reader.ok_or_else(|| anyhow::anyhow!("无效的书句柄: {handle}"))?;
    let bytes = tokio::task::spawn_blocking(move || reader.get_page(index)).await??;
    Ok((*bytes).clone())
}

/// 生成书籍封面缩略图:取第 `page` 页,可按 `crop` 裁剪后缩放填充到 `w×h`。
/// 若 path 为目录,走 Folder 格式。
/// 生成本地书籍封面缩略图(取第 page 页,等比缩放 + 中心裁剪到 w×h)。
/// 封面结果写入磁盘缓存（cover/）供后续秒开。
pub async fn book_cover(
    path: String,
    page: u32,
    width: u32,
    height: u32,
    crop: Option<CropRect>,
) -> Result<PageImage> {
    let crop_tuple = crop.as_ref().map(|r| (r.x, r.y, r.w, r.h));
    // 先查磁盘缓存
    if let Some((rgba, w, h)) = cache::cover_cache_read(&path, page, width, height, crop_tuple) {
        return Ok(PageImage {
            rgba,
            width: w,
            height: h,
        });
    }
    let path_for_closure = path.clone();
    let img = tokio::task::spawn_blocking(move || -> Result<decode::DecodedImage> {
        let book: Box<dyn document::Document> = if std::path::Path::new(&path_for_closure).is_dir()
        {
            document::open_folder_document(&path_for_closure)?
        } else {
            let src = local::LocalFile::open(&path_for_closure)?;
            document::open_document(src, &path_for_closure)?
        };
        let bytes = book.page_bytes(page)?;
        let crop_f = crop.map(|r| (r.x, r.y, r.w, r.h));
        decode::decode_cover(&bytes, width, height, crop_f)
    })
    .await??;
    // 写入磁盘缓存
    let _ = cache::cover_cache_write(&path, page, width, height, crop_tuple, &img.rgba);
    Ok(PageImage {
        rgba: img.rgba,
        width: img.width,
        height: img.height,
    })
}

/// 关闭书籍会话,释放资源。
pub fn close_book(handle: u64) {
    sessions().lock().unwrap().remove(&handle);
}

/// 目录条目(书架/浏览用)。
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// 修改时间（unix 秒）；来源无此信息时为 0（如 WebDAV）。
    pub mtime: i64,
}

/// 列出本地目录内容(目录在前,自然排序)。
pub async fn list_local_dir(path: String) -> Result<Vec<DirEntry>> {
    let entries = tokio::task::spawn_blocking(move || local::list_dir(&path)).await??;
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

/// 检测目录是否为漫画文件夹（包含至少一张图片文件）。
/// 用于浏览页判断：是漫画文件夹 → 显示为海报卡片；否则 → 显示为普通文件夹。
pub fn is_comic_folder(dir_path: String) -> bool {
    crate::document::folder::is_comic_folder(dir_path)
}

/// 获取漫画文件夹的显式封面路径（cover.jpg / cover.png 等）。
/// 无显式封面时返回空字符串。
pub fn folder_cover_path(dir_path: String) -> String {
    crate::document::folder::FolderBook::cover_path(dir_path).unwrap_or_default()
}
