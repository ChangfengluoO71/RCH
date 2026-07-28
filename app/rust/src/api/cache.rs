//! 缓存管理 API（暴露给 Dart）。

use crate::cache;

/// 缓存分类大小信息。
pub struct CacheSize {
    /// 页面缓存(字节)，L2 磁盘页面缓存（page/）。
    pub page: u64,
    /// 整本下载缓存(字节)，WebDAV raw/ 整本下载。
    pub raw: u64,
    /// 封面缓存(字节)，封面缩略图磁盘缓存（cover/）。
    pub cover: u64,
    /// 缩略图缓存(字节)（thumb/）。
    pub thumb: u64,
    /// AI 结果缓存(字节)（ai/）。
    pub ai: u64,
    /// 旧下载目录(字节)。
    pub download: u64,
    /// 临时文件(字节)（temp/）。
    pub temp: u64,
    /// 所有缓存总和(字节)。
    pub total: u64,
}

/// 获取所有缓存分类大小。
pub fn cache_sizes() -> CacheSize {
    CacheSize {
        page: cache::dir_size(&cache::CacheDir::Page.path()),
        raw: cache::dir_size(&cache::CacheDir::Raw.path()),
        cover: cache::dir_size(&cache::CacheDir::Cover.path()),
        thumb: cache::dir_size(&cache::CacheDir::Thumb.path()),
        ai: cache::dir_size(&cache::CacheDir::Ai.path()),
        download: cache::dir_size(&cache::cache_root().join("download")),
        temp: cache::dir_size(&cache::CacheDir::Temp.path()),
        total: cache::dir_size(&cache::cache_root()),
    }
}

/// 获取 L2 页面缓存大小（字节）。
pub fn page_cache_size() -> u64 {
    cache::dir_size(&cache::CacheDir::Page.path())
}

/// 获取下载缓存大小（字节）。
pub fn download_cache_size() -> u64 {
    cache::dir_size(&cache::cache_root().join("download"))
}

/// 获取所有缓存磁盘总占用（字节）。
pub fn total_cache_size() -> u64 {
    cache::dir_size(&cache::cache_root())
}

/// 清空 L2 页面缓存，返回释放的字节数。
pub fn clear_page_cache() -> Result<u64, String> {
    cache::clear_page_cache().map_err(|e| format!("{e}"))
}

/// 清空原始文件缓存（raw/），返回释放的字节数。
pub fn clear_raw_cache() -> Result<u64, String> {
    cache::clear_raw_cache().map_err(|e| format!("{e}"))
}

/// 清空封面缓存（cover/），返回释放的字节数。
pub fn clear_cover_cache() -> Result<u64, String> {
    cache::clear_cover_cache().map_err(|e| format!("{e}"))
}

/// 清空缩略图缓存（thumb/），返回释放的字节数。
pub fn clear_thumb_cache() -> Result<u64, String> {
    cache::clear_thumb_cache().map_err(|e| format!("{e}"))
}

/// 清空 AI 结果缓存（ai/），返回释放的字节数。
pub fn clear_ai_cache() -> Result<u64, String> {
    cache::clear_ai_cache().map_err(|e| format!("{e}"))
}

/// 清空临时文件（temp/），返回释放的字节数。
pub fn clear_temp_cache() -> Result<u64, String> {
    cache::clear_temp_cache().map_err(|e| format!("{e}"))
}

/// 清空下载缓存，返回释放的字节数。
pub fn clear_download_cache() -> Result<u64, String> {
    cache::clear_download_cache().map_err(|e| format!("{e}"))
}

/// 清空全部缓存，返回释放的字节数。
pub fn clear_all_caches() -> Result<u64, String> {
    cache::clear_all_caches().map_err(|e| format!("{e}"))
}

/// 获取缓存根目录路径。
pub fn cache_root_path() -> String {
    cache::cache_root().to_string_lossy().into_owned()
}

/// 设置自定义缓存根目录（空字符串恢复默认）。
/// 调用方应确保已迁移旧数据后再调用。
pub fn set_cache_root_path(path: String) {
    cache::set_custom_cache_root(&path);
}

/// 获取默认缓存根目录（APPDATA/RCH），不受自定义路径影响。
pub fn default_cache_root_path() -> String {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        PathBuf::from(appdata).join("RCH").to_string_lossy().into_owned()
    } else {
        std::env::temp_dir().join("RCH").to_string_lossy().into_owned()
    }
}

use std::path::PathBuf;
