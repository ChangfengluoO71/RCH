//! 缓存管理 API（暴露给 Dart）。

use crate::cache;

/// 缓存分类大小信息。
pub struct CacheSize {
    /// 页面缓存(字节)，L2 磁盘页面缓存（page/）。
    pub page: u64,
    /// 整本下载缓存(字节)，远程书源整本下载（raw/）。
    pub raw: u64,
    /// 封面缓存(字节)，封面缩略图磁盘缓存（cover/）。
    pub cover: u64,
    /// AI 结果缓存(字节)（ai/）。
    pub ai: u64,
    /// 临时文件(字节)，AI 超分中间产物（temp/）。
    pub temp: u64,
    /// 所有缓存总和(字节)。
    pub total: u64,
}

/// 获取所有缓存分类大小。
pub fn cache_sizes() -> CacheSize {
    let page = cache::dir_size(&cache::CacheDir::Page.path());
    let raw = cache::dir_size(&cache::CacheDir::Raw.path());
    let cover = cache::dir_size(&cache::CacheDir::Cover.path());
    let ai = cache::dir_size(&cache::CacheDir::Ai.path());
    let temp = cache::dir_size(&cache::CacheDir::Temp.path());
    CacheSize {
        page,
        raw,
        cover,
        ai,
        temp,
        // 磁盘总占用 = 各缓存分类之和（不含数据库、日志、支持目录等非缓存数据）。
        total: page + raw + cover + ai + temp,
    }
}

/// 获取 L2 页面缓存大小（字节）。
pub fn page_cache_size() -> u64 {
    cache::dir_size(&cache::CacheDir::Page.path())
}

/// 获取所有缓存分类总占用（字节，不含数据库/日志等非缓存数据）。
pub fn total_cache_size() -> u64 {
    cache::dir_size(&cache::CacheDir::Page.path())
        + cache::dir_size(&cache::CacheDir::Raw.path())
        + cache::dir_size(&cache::CacheDir::Cover.path())
        + cache::dir_size(&cache::CacheDir::Ai.path())
        + cache::dir_size(&cache::CacheDir::Temp.path())
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

/// 清空 AI 结果缓存（ai/），返回释放的字节数。
pub fn clear_ai_cache() -> Result<u64, String> {
    cache::clear_ai_cache().map_err(|e| format!("{e}"))
}

/// 清空 AI 超分临时文件（temp/），返回释放的字节数。
pub fn clear_temp_cache() -> Result<u64, String> {
    cache::clear_temp_cache().map_err(|e| format!("{e}"))
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

/// 迁移应用根目录（database.db + cache/ + 根级文件），排除支持目录。
/// 成功返回复制的字节数；调用方随后 set_cache_root_path + delete_migrated_items。
pub fn migrate_cache_root(from: String, to: String, support_dir: String) -> Result<u64, String> {
    cache::migrate_cache_root(&from, &to, &support_dir).map_err(|e| format!("{e}"))
}

/// 迁移进度（已复制字节, 总字节），供 Dart 轮询。
pub fn migration_progress() -> (u64, u64) {
    cache::migration_progress()
}

/// 目标盘可用空间（字节）。路径不存在返回 0。
pub fn available_space(path: String) -> u64 {
    cache::available_space(&path)
}

/// 删除根目录下已迁移的项目（database.db、cache/），返回释放字节。
pub fn delete_migrated_items(root: String) -> Result<u64, String> {
    cache::delete_migrated_items(&root).map_err(|e| format!("{e}"))
}

/// 读取未完成迁移标记（from, to）；无标记返回 null。
pub fn pending_migration(root: String) -> Option<(String, String)> {
    cache::migration_pending(&root)
}

/// 清除迁移标记。
pub fn clear_migration_marker(root: String) {
    cache::clear_migration_marker(&root);
}

use std::path::PathBuf;
