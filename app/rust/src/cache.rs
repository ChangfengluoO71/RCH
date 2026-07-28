//! 磁盘缓存管理：五级缓存目录 + 大小计算 + 清理 + 封面缓存读写 + 自定义缓存根目录。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::RwLock;

/// 用户自定义缓存根目录（设置后可迁移缓存到其他磁盘）。
static CUSTOM_CACHE_ROOT: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();

fn custom_root() -> &'static RwLock<Option<PathBuf>> {
    CUSTOM_CACHE_ROOT.get_or_init(|| RwLock::new(None))
}

/// RCH 数据根目录（`<APPDATA>/RCH` 或 `<TEMP>/RCH`）。
/// 如果用户设置了自定义缓存根目录，优先使用自定义路径。
pub fn cache_root() -> PathBuf {
    if let Some(custom) = custom_root().read().ok().and_then(|g| g.clone()) {
        if !custom.as_os_str().is_empty() {
            return custom;
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        PathBuf::from(appdata).join("RCH")
    } else {
        std::env::temp_dir().join("RCH")
    }
}

/// 设置自定义缓存根目录（空字符串表示恢复默认）。
/// 调用方应确保迁移已完成后才调用此方法。
pub fn set_custom_cache_root(path: &str) {
    let p = if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    };
    if let Ok(mut w) = custom_root().write() {
        *w = p;
    }
}

/// 五级缓存子目录。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheDir {
    /// L2 磁盘页面缓存（读过的页写盘，避免重复下载）。
    Page,
    /// 整本漫画原始文件（WebDAV 下载后存储）。
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
            CacheDir::Page => "page",
            CacheDir::Raw => "raw",
            CacheDir::Cover => "cover",
            CacheDir::Thumb => "thumb",
            CacheDir::Ai => "ai",
            CacheDir::Temp => "temp",
        }
    }

    pub fn path(&self) -> PathBuf {
        if matches!(self, CacheDir::Temp) {
            // temp 放在系统临时目录，不占用用户数据目录空间
            std::env::temp_dir().join("RCH").join("temp")
        } else {
            cache_root().join("cache").join(self.as_str())
        }
    }

    /// 确保目录存在。
    pub fn ensure(&self) -> Result<PathBuf> {
        let p = self.path();
        std::fs::create_dir_all(&p)?;
        Ok(p)
    }
}

// ====== 封面磁盘缓存 ======

/// 计算封面缓存的磁盘键。
/// 格式: `{book_path_hash}_{page}_{width}_{height}_{crop_hash}.cover`
/// 使用路径 hash 避免路径中的非法文件名字符。
fn cover_cache_key(path: &str, page: u32, width: u32, height: u32, crop: Option<(f64, f64, f64, f64)>) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    let path_hash = hasher.finish();
    let crop_str = crop.map(|(x, y, w, h)| format!("_{x:.3}_{y:.3}_{w:.3}_{h:.3}")).unwrap_or_default();
    format!("{path_hash:x}_{page}_{width}_{height}{crop_str}.cover")
}

/// 从磁盘读取封面缓存（若存在）。
/// 返回完整的 RGBA 像素字节和宽高。
pub fn cover_cache_read(path: &str, page: u32, width: u32, height: u32, crop: Option<(f64, f64, f64, f64)>) -> Option<(Vec<u8>, u32, u32)> {
    let dir = CacheDir::Cover.path();
    let key = cover_cache_key(path, page, width, height, crop);
    let file_path = dir.join(key);
    let data = std::fs::read(&file_path).ok()?;
    if data.len() < 8 { return None; }
    let w = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let h = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let rgba = data[8..].to_vec();
    if rgba.len() as u32 != w * h * 4 { return None; }
    Some((rgba, w, h))
}

/// 将封面写入磁盘缓存。
pub fn cover_cache_write(path: &str, page: u32, width: u32, height: u32, crop: Option<(f64, f64, f64, f64)>, rgba: &[u8]) -> Result<()> {
    let dir = CacheDir::Cover.ensure()?;
    let key = cover_cache_key(path, page, width, height, crop);
    let file_path = dir.join(key);
    let mut data = Vec::with_capacity(8 + rgba.len());
    data.extend_from_slice(&width.to_le_bytes());
    data.extend_from_slice(&height.to_le_bytes());
    data.extend_from_slice(rgba);
    std::fs::write(&file_path, &data).context("写入封面缓存失败")?;
    Ok(())
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
    if !dir.exists() {
        return Ok(0);
    }
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

/// 清空 L2 页面缓存（page/）。
pub fn clear_page_cache() -> Result<u64> {
    let dir = CacheDir::Page.path();
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

/// 清空缩略图缓存（thumb/）。
pub fn clear_thumb_cache() -> Result<u64> {
    let dir = CacheDir::Thumb.path();
    if dir.exists() { remove_dir_contents(&dir) } else { Ok(0) }
}

/// 清空 AI 结果缓存（ai/）。
pub fn clear_ai_cache() -> Result<u64> {
    let dir = CacheDir::Ai.path();
    if dir.exists() { remove_dir_contents(&dir) } else { Ok(0) }
}

/// 清空临时文件（temp/）。
pub fn clear_temp_cache() -> Result<u64> {
    let dir = CacheDir::Temp.path();
    if dir.exists() { remove_dir_contents(&dir) } else { Ok(0) }
}

/// 清空下载缓存（旧路径兼容）。
pub fn clear_download_cache() -> Result<u64> {
    let dir = cache_root().join("download");
    if dir.exists() { remove_dir_contents(&dir) } else { Ok(0) }
}

/// 清空所有缓存（六级 + 旧 download 目录）。
pub fn clear_all_caches() -> Result<u64> {
    Ok(clear_page_cache()?
        + clear_raw_cache()?
        + clear_cover_cache()?
        + clear_thumb_cache()?
        + clear_ai_cache()?
        + clear_temp_cache()?
        + clear_download_cache()?)
}

/// 确保所有缓存目录存在。
pub fn ensure_all_cache_dirs() -> Result<()> {
    CacheDir::Page.ensure()?;
    CacheDir::Raw.ensure()?;
    CacheDir::Cover.ensure()?;
    CacheDir::Thumb.ensure()?;
    CacheDir::Ai.ensure()?;
    CacheDir::Temp.ensure()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_root_defaults_to_appdata() {
        // 在 Windows CI 环境 APPDATA 存在
        let root = cache_root();
        assert!(root.to_string_lossy().contains("RCH"));
    }

    #[test]
    fn custom_cache_root_works() {
        set_custom_cache_root("C:\\TestRCH");
        assert_eq!(cache_root(), PathBuf::from("C:\\TestRCH"));
        // 恢复默认
        set_custom_cache_root("");
        let root = cache_root();
        assert!(root.to_string_lossy().contains("RCH"));
    }

    #[test]
    fn cache_dir_paths_are_distinct() {
        let page = CacheDir::Page.path();
        let raw = CacheDir::Raw.path();
        let cover = CacheDir::Cover.path();
        assert_ne!(page, raw);
        assert_ne!(page, cover);
        assert_ne!(raw, cover);
    }

    #[test]
    fn dir_size_of_empty_dir_returns_zero() {
        let tmp = std::env::temp_dir().join("rch_test_empty_size");
        let _ = std::fs::create_dir_all(&tmp);
        assert_eq!(dir_size(&tmp), 0);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
