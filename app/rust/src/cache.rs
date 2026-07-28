//! 磁盘缓存管理：五级缓存目录 + 大小计算 + 清理。

use anyhow::Result;
use std::path::{Path, PathBuf};

/// RCH 数据根目录（`<APPDATA>/RCH` 或 `<TEMP>/RCH`）。
pub fn cache_root() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        PathBuf::from(appdata).join("RCH")
    } else {
        std::env::temp_dir().join("RCH")
    }
}

/// 五级缓存子目录。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheDir {
    /// 整本漫画原始文件（下载后存储）。
    Raw,
    /// 封面缩略图缓存（按质量/裁剪分）。
    Cover,
    /// 缩略图缓存。
    Thumb,
    /// AI 超分结果缓存。
    Ai,
    /// 临时文件（CB7/CBR 解压中间产物）。
    Temp,
}

impl CacheDir {
    pub fn as_str(&self) -> &'static str {
        match self {
            CacheDir::Raw => "raw",
            CacheDir::Cover => "cover",
            CacheDir::Thumb => "thumb",
            CacheDir::Ai => "ai",
            CacheDir::Temp => "temp",
        }
    }

    pub fn path(&self) -> PathBuf {
        cache_root().join("cache").join(self.as_str())
    }

    /// 确保目录存在。
    pub fn ensure(&self) -> Result<PathBuf> {
        let p = self.path();
        std::fs::create_dir_all(&p)?;
        Ok(p)
    }
}

/// 获取页面缓存目录（L2 兼容旧路径）。
pub fn page_cache_dir() -> PathBuf {
    cache_root().join("cache")
}

// ====== 大小计算与清理 ======

/// 递归计算目录大小（字节）。
pub fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    total += meta.len();
                } else if meta.is_dir() {
                    total += dir_size(&p);
                }
            }
        }
    }
    total
}

/// 递归清空目录内容（保留根目录），返回释放的字节数。
fn remove_dir_contents(dir: &Path) -> Result<u64> {
    let mut freed = 0u64;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries {
            let entry = entry?;
            let p = entry.path();
            let meta = entry.metadata()?;
            if meta.is_file() {
                freed += meta.len();
                std::fs::remove_file(&p)?;
            } else if meta.is_dir() {
                freed += remove_dir_contents(&p)?;
                std::fs::remove_dir_all(&p)?;
            }
        }
    }
    Ok(freed)
}

/// 清空 L2 页面缓存（兼容旧路径）。
pub fn clear_page_cache() -> Result<u64> {
    let dir = page_cache_dir();
    if dir.exists() { remove_dir_contents(&dir) } else { Ok(0) }
}

/// 清空原始文件缓存（raw/）。
pub fn clear_raw_cache() -> Result<u64> {
    let dir = CacheDir::Raw.path();
    if dir.exists() { remove_dir_contents(&dir) } else { Ok(0) }
}

/// 清空封面缓存（cover/）。
pub fn clear_cover_cache() -> Result<u64> {
    let dir = CacheDir::Cover.path();
    if dir.exists() { remove_dir_contents(&dir) } else { Ok(0) }
}

/// 清空 AI 结果缓存（ai/）。
pub fn clear_ai_cache() -> Result<u64> {
    let dir = CacheDir::Ai.path();
    if dir.exists() { remove_dir_contents(&dir) } else { Ok(0) }
}

/// 清空下载缓存（整包回退，兼容旧路径）。
pub fn clear_download_cache() -> Result<u64> {
    let dir = cache_root().join("download");
    if dir.exists() { remove_dir_contents(&dir) } else { Ok(0) }
}

/// 清空所有缓存（五级 + 旧目录）。
pub fn clear_all_caches() -> Result<u64> {
    Ok(clear_page_cache()?
        + clear_raw_cache()?
        + clear_cover_cache()?
        + clear_ai_cache()?
        + clear_download_cache()?)
}

/// 确保所有缓存目录存在。
pub fn ensure_all_cache_dirs() -> Result<()> {
    CacheDir::Raw.ensure()?;
    CacheDir::Cover.ensure()?;
    CacheDir::Thumb.ensure()?;
    CacheDir::Ai.ensure()?;
    CacheDir::Temp.ensure()?;
    Ok(())
}
