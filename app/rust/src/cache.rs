//! 磁盘缓存管理：五级缓存目录 + 大小计算 + 清理 + 封面缓存读写 + 自定义缓存根目录。

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

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

// ====== 应用根目录迁移（O1-A：复制 + 校验 + 成功后删源） ======

const MIGRATION_MARKER: &str = "migration.partial";

static MIGRATION_COPIED: OnceLock<AtomicU64> = OnceLock::new();
static MIGRATION_TOTAL: OnceLock<AtomicU64> = OnceLock::new();
static MIGRATION_TARGETS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();

fn migration_copied() -> &'static AtomicU64 {
    MIGRATION_COPIED.get_or_init(|| AtomicU64::new(0))
}
fn migration_total() -> &'static AtomicU64 {
    MIGRATION_TOTAL.get_or_init(|| AtomicU64::new(0))
}
fn migration_targets() -> &'static Mutex<Vec<PathBuf>> {
    MIGRATION_TARGETS.get_or_init(|| Mutex::new(Vec::new()))
}

/// 当前迁移进度（已复制字节, 总字节）。供 Dart 轮询。
pub fn migration_progress() -> (u64, u64) {
    (
        migration_copied().load(Ordering::Relaxed),
        migration_total().load(Ordering::Relaxed),
    )
}

fn file_count(p: &Path) -> u64 {
    let mut n = 0u64;
    if let Ok(entries) = std::fs::read_dir(p) {
        for e in entries.flatten() {
            if let Ok(meta) = e.metadata() {
                if meta.is_file() {
                    n += 1;
                } else if meta.is_dir() {
                    n += file_count(&e.path());
                }
            }
        }
    }
    n
}

fn copy_tree(src: &Path, dst: &Path) -> Result<u64> {
    if !src.exists() {
        return Ok(0);
    }
    std::fs::create_dir_all(dst)?;
    let mut total = 0u64;
    for entry in std::fs::read_dir(src)? {
        let e = entry?;
        let from = e.path();
        let to = dst.join(e.file_name());
        let meta = e.metadata()?;
        if meta.is_dir() {
            total += copy_tree(&from, &to)?;
        } else if meta.is_file() {
            std::fs::copy(&from, &to)?;
            total += meta.len();
            migration_copied().fetch_add(meta.len(), Ordering::Relaxed);
        }
    }
    Ok(total)
}

/// 目标盘可用空间（字节）。路径不存在返回 0。
pub fn available_space(path: &str) -> u64 {
    let p = PathBuf::from(path);
    if p.exists() {
        fs2::available_space(&p).unwrap_or(0)
    } else {
        0
    }
}

/// 迁移应用根目录：database.db + cache/ + download/ + 根级普通文件。
///
/// - 排除嵌套的应用支持目录（library.json 所在）与迁移标记本身；
/// - 开始写 `migration.partial` 标记（含 from/to，支持启动恢复），
///   成功或优雅失败时移除；
/// - 失败时清理目标已复制内容，源保持不变；
/// - 成功后由调用方删除源项目（delete_migrated_items）。
pub fn migrate_cache_root(from: &str, to: &str, support_dir: &str) -> Result<u64> {
    let from_p = PathBuf::from(from);
    let to_p = PathBuf::from(to);
    let support_p = PathBuf::from(support_dir);

    if from_p == to_p || from_p.starts_with(&to_p) || to_p.starts_with(&from_p) {
        bail!("目标目录不能与源目录相同或互为子目录");
    }
    if from_p.parent() == Some(&from_p) || to_p.parent() == Some(&to_p) {
        bail!("缓存目录不能是磁盘根目录");
    }

    // 收集待迁移项目：database.db、cache/、download/、根级普通文件。
    struct Item {
        name: String,
        is_dir: bool,
    }
    let mut items: Vec<Item> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&from_p) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == MIGRATION_MARKER {
                continue;
            }
            let p = e.path();
            if p == support_p || support_p.starts_with(&p) {
                continue; // 嵌套支持目录（或其内部）不迁移
            }
            let meta = match e.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                if name == "cache" || name == "download" {
                    items.push(Item { name, is_dir: true });
                }
                // 其他未知目录不迁移
            } else if meta.is_file() {
                items.push(Item { name, is_dir: false });
            }
        }
    }

    let mut grand_total = 0u64;
    for it in &items {
        let s = from_p.join(&it.name);
        grand_total += if it.is_dir {
            dir_size(&s)
        } else {
            std::fs::metadata(&s).map(|m| m.len()).unwrap_or(0)
        };
    }
    migration_copied().store(0, Ordering::Relaxed);
    migration_total().store(grand_total, Ordering::Relaxed);
    migration_targets().lock().unwrap().clear();

    let marker = from_p.join(MIGRATION_MARKER);
    let marker_json = serde_json::json!({ "from": from, "to": to });
    std::fs::write(&marker, serde_json::to_vec_pretty(&marker_json)?)?;

    let result = (|| -> Result<u64> {
        let mut copied_total = 0u64;
        for it in &items {
            let s = from_p.join(&it.name);
            let t = to_p.join(&it.name);
            if it.is_dir {
                copied_total += copy_tree(&s, &t)?;
            } else {
                std::fs::copy(&s, &t)?;
                let len = std::fs::metadata(&s).map(|m| m.len()).unwrap_or(0);
                copied_total += len;
                migration_copied().fetch_add(len, Ordering::Relaxed);
            }
            migration_targets().lock().unwrap().push(t);
        }
        // 校验：目录文件数量一致、文件大小一致。
        for it in &items {
            let s = from_p.join(&it.name);
            let t = to_p.join(&it.name);
            if it.is_dir {
                if file_count(&s) != file_count(&t) {
                    bail!("迁移校验失败：{} 文件数量不一致", it.name);
                }
            } else {
                let sl = std::fs::metadata(&s).map(|m| m.len()).unwrap_or(0);
                let tl = std::fs::metadata(&t).map(|m| m.len()).unwrap_or(0);
                if sl == 0 || sl != tl {
                    bail!("迁移校验失败：{} 大小不一致", it.name);
                }
            }
        }
        Ok(copied_total)
    })();

    match result {
        Ok(n) => {
            let _ = std::fs::remove_file(&marker);
            Ok(n)
        }
        Err(e) => {
            for t in migration_targets().lock().unwrap().iter() {
                let _ = std::fs::remove_dir_all(t);
            }
            let _ = std::fs::remove_file(&marker);
            Err(e)
        }
    }
}

/// 读取未完成的迁移标记（from, to）。
pub fn migration_pending(root: &str) -> Option<(String, String)> {
    let p = PathBuf::from(root).join(MIGRATION_MARKER);
    if !p.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&p).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    let from = v["from"].as_str()?.to_string();
    let to = v["to"].as_str()?.to_string();
    Some((from, to))
}

/// 清除迁移标记。
pub fn clear_migration_marker(root: &str) {
    let _ = std::fs::remove_file(PathBuf::from(root).join(MIGRATION_MARKER));
}

/// 删除根目录下已迁移的项目（database.db、cache/、download/），返回释放字节。
pub fn delete_migrated_items(root: &str) -> Result<u64> {
    let root_p = PathBuf::from(root);
    if root_p.parent() == Some(&root_p) {
        bail!("缓存目录不能是磁盘根目录");
    }
    let mut freed = 0u64;
    for name in ["database.db", "cache", "download"] {
        let p = root_p.join(name);
        if !p.starts_with(&root_p) || !p.exists() {
            continue;
        }
        let meta = std::fs::metadata(&p)?;
        freed += if meta.is_dir() { dir_size(&p) } else { meta.len() };
        if meta.is_dir() {
            std::fs::remove_dir_all(&p)?;
        } else {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(freed)
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

    #[test]
    fn migrate_copies_db_cache_download_skips_support() {
        let base = std::env::temp_dir().join("rch_test_migrate_v2");
        let from = base.join("from");
        let to = base.join("to");
        let support = from.join("RCH");
        let _ = std::fs::remove_dir_all(&base);

        std::fs::create_dir_all(from.join("cache/page")).unwrap();
        std::fs::create_dir_all(from.join("download")).unwrap();
        std::fs::create_dir_all(&support).unwrap();
        std::fs::write(from.join("database.db"), vec![1u8; 500]).unwrap();
        std::fs::write(from.join("cache/page/a.bin"), vec![2u8; 100]).unwrap();
        std::fs::write(from.join("download/b.zip"), vec![3u8; 200]).unwrap();
        std::fs::write(from.join("note.txt"), b"root file").unwrap();
        std::fs::write(support.join("library.json"), b"{}").unwrap();

        let n = migrate_cache_root(
            from.to_str().unwrap(),
            to.to_str().unwrap(),
            support.to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(n, 500 + 100 + 200 + 9);
        assert!(to.join("database.db").exists());
        assert!(to.join("cache/page/a.bin").exists());
        assert!(to.join("download/b.zip").exists());
        assert!(to.join("note.txt").exists());
        // 支持目录不迁移；成功迁移后标记被清除
        assert!(!to.join("RCH").exists());
        assert!(!from.join(MIGRATION_MARKER).exists());

        let freed = delete_migrated_items(from.to_str().unwrap()).unwrap();
        assert!(freed >= 800);
        assert!(!from.join("database.db").exists());
        assert!(!from.join("cache").exists());
        assert!(!from.join("download").exists());
        assert!(from.join("RCH").exists()); // 支持目录保留
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn migrate_rejects_unsafe_paths() {
        let base = std::env::temp_dir().join("rch_test_migrate_safe_v2");
        let a = base.join("a");
        let support = base.join("sup");
        let _ = std::fs::create_dir_all(&a);
        let _ = std::fs::create_dir_all(&support);
        assert!(
            migrate_cache_root(a.to_str().unwrap(), a.to_str().unwrap(), support.to_str().unwrap())
                .is_err()
        );
        assert!(
            migrate_cache_root(base.to_str().unwrap(), a.to_str().unwrap(), support.to_str().unwrap())
                .is_err()
        );
        assert!(
            migrate_cache_root(a.to_str().unwrap(), base.to_str().unwrap(), support.to_str().unwrap())
                .is_err()
        );
        assert!(migrate_cache_root("C:\\", "D:\\tmp_x2", support.to_str().unwrap()).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
