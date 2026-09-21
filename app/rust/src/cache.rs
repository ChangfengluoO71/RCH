//! 磁盘缓存管理：五级缓存目录 + 大小计算 + 清理 + 封面缓存读写 + 自定义缓存根目录。

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::Write;
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
    // 测试构建：把数据根锚定到**进程专属临时目录**，第一次解析就生效。
    //
    // 为什么必须在 `cache_root()` 里锚定：单测会经**生产函数内部**的 `db::get()`
    // 打开数据库（如 wake/worker 路径），而 `db` 的连接是进程级 `OnceLock`——只认
    // 第一次解析出的根。若那次解析落在 `<APPDATA>/RCH`，测试数据就写进了用户默认库
    // （③-1/② 复测实测：残留 `wake-*` / `ready-*` 书源行与 cover job 行）。
    // 显式 `set_custom_cache_root(...)` 仍然优先（既有测试依赖这一优先级）；
    // 目录名保留 "RCH" 以满足 `cache_root_defaults_to_appdata` 等既有断言。
    #[cfg(test)]
    {
        static TEST_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        TEST_ROOT
            .get_or_init(|| {
                let dir = std::env::temp_dir().join(format!("RCH-test-{}", std::process::id()));
                let _ = std::fs::create_dir_all(&dir);
                dir
            })
            .clone()
    }
    #[cfg(not(test))]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            PathBuf::from(appdata).join("RCH")
        } else {
            std::env::temp_dir().join("RCH")
        }
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
    /// 整本漫画原始文件（远程书源整本下载后存储）。
    Raw,
    /// 封面缩略图缓存（按质量/裁剪分）。
    Cover,
    /// AI 超分结果缓存。
    Ai,
    /// AI 超分临时文件（输入/输出中间产物）。
    Temp,
    /// MOBI 页表缓存（首次打开的逐条魔数探测结果，见 `document/mobi.rs`）。
    MobiTable,
}

impl CacheDir {
    pub fn as_str(&self) -> &'static str {
        match self {
            CacheDir::Page => "page",
            CacheDir::Raw => "raw",
            CacheDir::Cover => "cover",
            CacheDir::Ai => "ai",
            CacheDir::Temp => "temp",
            CacheDir::MobiTable => "mobi_table",
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
fn cover_cache_key(
    path: &str,
    page: u32,
    width: u32,
    height: u32,
    crop: Option<(f64, f64, f64, f64)>,
) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    let path_hash = hasher.finish();
    let crop_str = crop
        .map(|(x, y, w, h)| format!("_{x:.3}_{y:.3}_{w:.3}_{h:.3}"))
        .unwrap_or_default();
    format!("{path_hash:x}_{page}_{width}_{height}{crop_str}.cover")
}

/// 从磁盘读取封面缓存（若存在）。
/// 返回完整的 RGBA 像素字节和宽高。
pub fn cover_cache_read(
    path: &str,
    page: u32,
    width: u32,
    height: u32,
    crop: Option<(f64, f64, f64, f64)>,
) -> Option<(Vec<u8>, u32, u32)> {
    let dir = CacheDir::Cover.path();
    let key = cover_cache_key(path, page, width, height, crop);
    let file_path = dir.join(key);
    let data = std::fs::read(&file_path).ok()?;
    if data.len() < 8 {
        return None;
    }
    let w = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let h = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let rgba = data[8..].to_vec();
    if rgba.len() as u32 != w * h * 4 {
        return None;
    }
    Some((rgba, w, h))
}

/// 将封面写入磁盘缓存。
pub fn cover_cache_write(
    path: &str,
    page: u32,
    width: u32,
    height: u32,
    crop: Option<(f64, f64, f64, f64)>,
    rgba: &[u8],
) -> Result<()> {
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

/// Versioned cloud cover cache keyed by source/asset/content/selection/profile.
/// Unlike the historical path-only cache this cannot alias two providers that
/// happen to expose the same logical path.  The on-disk payload keeps the
/// existing compact RGBA header so reads remain allocation-bounded.
fn remote_cover_cache_key(
    source_id: &str,
    asset_id: &str,
    content_revision: &str,
    selection_revision: &str,
    profile: &str,
) -> String {
    let mut digest = Sha256::new();
    for part in [
        source_id,
        asset_id,
        content_revision,
        selection_revision,
        profile,
    ] {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part.as_bytes());
    }
    format!("{:x}.cover-v2", digest.finalize())
}

/// Stable filename for a versioned remote-cover payload.  The cache service
/// uses this for the SQLite blob/ref projection as well as for file I/O; no
/// caller needs to reconstruct the hashing scheme independently.
pub fn remote_cover_cache_filename(
    source_id: &str,
    asset_id: &str,
    content_revision: &str,
    selection_revision: &str,
    profile: &str,
) -> String {
    remote_cover_cache_key(
        source_id,
        asset_id,
        content_revision,
        selection_revision,
        profile,
    )
}

pub fn remote_cover_cache_relative_path(
    source_id: &str,
    asset_id: &str,
    content_revision: &str,
    selection_revision: &str,
    profile: &str,
) -> String {
    format!(
        "cache/cover/{}",
        remote_cover_cache_filename(
            source_id,
            asset_id,
            content_revision,
            selection_revision,
            profile,
        )
    )
}

/// Remove one versioned cloud-cover payload.  Deleting an already missing
/// file is idempotent and returns zero, which keeps tombstone cleanup safe
/// after a manual cache purge.
pub fn remote_cover_cache_delete(
    source_id: &str,
    asset_id: &str,
    content_revision: &str,
    selection_revision: &str,
    profile: &str,
) -> Result<u64> {
    let path = CacheDir::Cover.path().join(remote_cover_cache_key(
        source_id,
        asset_id,
        content_revision,
        selection_revision,
        profile,
    ));
    match std::fs::remove_file(path) {
        Ok(()) => Ok(1),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error.into()),
    }
}

/// RG-B 性能修复（方案 B）：封面素材的**廉价**存在性判据 —— `metadata` + `len > 0`。
///
/// 与 raw-cache 权威判据一致（"文件存在且非空"），但**不**做整文件读取与像素头校验
/// （`remote_cover_cache_read` 会 `std::fs::read` 整个文件，代价随封面尺寸增长）。
///
/// 用途：`available_books` 这类**统计**判定（可能在 500ms 轮询路径上被反复调用）。
/// 需要**字节级**有效性的场景（真正的读取/对账）仍应使用 `remote_cover_cache_read`。
pub fn remote_cover_cache_present(
    source_id: &str,
    asset_id: &str,
    content_revision: &str,
    selection_revision: &str,
    profile: &str,
) -> bool {
    let path = CacheDir::Cover.path().join(remote_cover_cache_key(
        source_id,
        asset_id,
        content_revision,
        selection_revision,
        profile,
    ));
    matches!(std::fs::metadata(&path), Ok(meta) if meta.len() > 0)
}

pub fn remote_cover_cache_read(
    source_id: &str,
    asset_id: &str,
    content_revision: &str,
    selection_revision: &str,
    profile: &str,
) -> Option<(Vec<u8>, u32, u32)> {
    let path = CacheDir::Cover.path().join(remote_cover_cache_key(
        source_id,
        asset_id,
        content_revision,
        selection_revision,
        profile,
    ));
    let length = std::fs::metadata(&path).ok()?.len();
    if length < 8 || length > (8 + 16 * 1024 * 1024) as u64 {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    if data.len() < 8 {
        return None;
    }
    let width = u32::from_le_bytes(data[0..4].try_into().ok()?);
    let height = u32::from_le_bytes(data[4..8].try_into().ok()?);
    let expected = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(4)?;
    let rgba = data.get(8..)?.to_vec();
    (rgba.len() == expected).then_some((rgba, width, height))
}

#[allow(clippy::too_many_arguments)]
pub fn remote_cover_cache_write(
    source_id: &str,
    asset_id: &str,
    content_revision: &str,
    selection_revision: &str,
    profile: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<()> {
    if width == 0 || height == 0 {
        bail!("封面尺寸无效");
    }
    let expected = usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("封面尺寸无效"))?;
    // Keep corrupt/malicious profile values from turning a cover request into
    // an unbounded allocation. The UI profiles are much smaller; this is a
    // generous hard ceiling for a persisted thumbnail.
    if expected > 16 * 1024 * 1024 {
        bail!("封面像素过大");
    }
    if rgba.len() != expected {
        bail!("封面像素长度与尺寸不一致");
    }
    let dir = CacheDir::Cover.ensure()?;
    let filename = remote_cover_cache_key(
        source_id,
        asset_id,
        content_revision,
        selection_revision,
        profile,
    );
    let target = dir.join(filename);
    let temp = dir.join(format!(
        ".{}.part-{}",
        target.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    let mut data = Vec::with_capacity(8 + rgba.len());
    data.extend_from_slice(&width.to_le_bytes());
    data.extend_from_slice(&height.to_le_bytes());
    data.extend_from_slice(rgba);
    let mut file = std::fs::File::create(&temp).context("创建封面临时文件失败")?;
    file.write_all(&data).context("写入封面缓存失败")?;
    file.sync_all().context("同步封面缓存失败")?;
    drop(file);
    std::fs::rename(&temp, &target).context("发布封面缓存失败")?;
    Ok(())
}

// ====== RG-A：原子发布缓存文件 ======

/// RG-A：「raw-cache 非原子写入」修复的共享实现。
///
/// 与既有封面缓存写法**共用同一临时名约定**（`.<name>.part-<pid>`，见上方封面写入），
/// 因此不引入第二套约定：
///
/// * 最终路径**只在完整写入并 rename 成功后**才出现 ⇒ 现有复用判据 `len() > 0`
///   依旧成立（不需要额外的完整性校验，也不需要新增持久化状态）；
/// * 中途出错、提前 Drop、或 rename 失败 ⇒ 删除临时文件，既不留半文件、
///   也不污染最终路径；
/// * 写入过程中任何读者都**看不到**部分内容（最终路径尚不存在）。
///
/// 调用方约定：`create` → 循环 `write_all` → `commit`。
/// 进程内单调序号（与 pid 组合生成唯一临时文件名）。
static ATOMIC_TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

pub struct AtomicCacheFile {
    target: PathBuf,
    temp: PathBuf,
    file: Option<std::fs::File>,
    committed: bool,
}

impl AtomicCacheFile {
    /// 在 `target` 的**同目录**创建临时文件（目录必须已存在——调用方负责创建）。
    pub fn create(target: &Path) -> Result<Self> {
        let dir = target
            .parent()
            .with_context(|| format!("缓存目标缺少父目录: {}", target.display()))?;
        let name = target
            .file_name()
            .with_context(|| format!("缓存目标缺少文件名: {}", target.display()))?
            .to_string_lossy()
            .to_string();
        // 唯一性：`.{name}.part-{pid}-{seq}`。
        // pid 保证**跨进程**唯一；进程内单调序号保证**同进程并发**写同一目标时
        // 两个 writer 不会共用同一个临时文件（否则会互相截断）。
        let temp = dir.join(format!(
            ".{}.part-{}-{}",
            name,
            std::process::id(),
            ATOMIC_TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let file = std::fs::File::create(&temp).context("创建缓存临时文件失败")?;
        Ok(Self {
            target: target.to_path_buf(),
            temp,
            file: Some(file),
            committed: false,
        })
    }

    /// 写入一块数据（语义同 `Write::write_all`）。
    pub fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.file
            .as_mut()
            .context("缓存临时文件已关闭")?
            .write_all(buf)
            .context("写入缓存临时文件失败")
    }

    /// 完成并**原子发布**到最终路径，返回该路径。
    pub fn commit(mut self) -> Result<PathBuf> {
        let mut file = self.file.take().context("缓存临时文件已关闭")?;
        file.flush().context("同步缓存临时文件失败")?;
        file.sync_all().context("同步缓存临时文件失败")?;
        drop(file);
        if self.target.exists() {
            // Windows 上 `rename` 不覆盖已存在目标。此处目标**不可能是**完整缓存
            // （完整缓存会走复用判据提前返回），因此先移除再 rename。
            std::fs::remove_file(&self.target).context("替换旧缓存失败")?;
        }
        std::fs::rename(&self.temp, &self.target).context("发布缓存文件失败")?;
        self.committed = true;
        Ok(self.target.clone())
    }
}

impl Drop for AtomicCacheFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.temp);
        }
    }
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
    if dir.exists() {
        remove_dir_contents(&dir)
    } else {
        Ok(0)
    }
}

/// `cache/raw`（整本下载包）的容量上限。
///
/// 2026-09-21（用户要求"把整本缓存清理机制拓展一下"）：raw 目录此前**只有**两条清理路径 ——
/// ①Dart 侧设置「阅读完成后自动删除整包」（关闭书本时删）；②缓存管理页手动"清空整本下载缓存"。
/// 若该设置被关掉、或包是为别的原因下载的（例如只为封面回退而下载一次就再没打开），
/// 包会**无限累积**。这里补一条兜底：超过上限就按修改时间**从旧到新**整包删除，
/// 直到降到上限以下（始终保留最新的那个包，避免刚下完就被自己删掉）。
pub const RAW_CACHE_LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// 包内最新的修改时间（递归）。目录自身的 mtime 在 Windows 上不可靠，
/// 所以用"包里最新那个文件"代表这个包最后一次被动过。
fn newest_mtime(path: &Path) -> std::time::SystemTime {
    let mut newest = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .unwrap_or(std::time::UNIX_EPOCH);
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let child = entry.path();
            let candidate = if child.is_dir() {
                newest_mtime(&child)
            } else {
                entry
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .unwrap_or(std::time::UNIX_EPOCH)
            };
            if candidate > newest {
                newest = candidate;
            }
        }
    }
    newest
}

/// 按容量上限清理 `cache/raw`，返回释放的字节数（best-effort，绝不因清理失败而报错）。
pub fn enforce_raw_cache_limit(limit: u64) -> Result<u64> {
    let dir = CacheDir::Raw.path();
    if !dir.exists() {
        return Ok(0);
    }
    // 一个包 = raw/ 下的一个子目录（也可能有零散文件，按同样规则一并计入）。
    let mut packages: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    let mut total = 0u64;
    for entry in std::fs::read_dir(&dir)?.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        let size = if meta.is_dir() {
            dir_size(&path)
        } else {
            meta.len()
        };
        let modified = newest_mtime(&path);
        total = total.saturating_add(size);
        packages.push((modified, size, path));
    }
    if total <= limit {
        return Ok(0);
    }
    // 最旧优先；**至少保留一个**（最新的），避免"刚下载完就被清理"。
    packages.sort_by_key(|(modified, _, _)| *modified);
    let mut freed = 0u64;
    while total > limit && packages.len() > 1 {
        let (_, size, path) = packages.remove(0);
        let removed = if path.is_dir() {
            let inner = remove_dir_contents(&path).unwrap_or(0);
            let _ = std::fs::remove_dir_all(&path);
            inner
        } else {
            let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if std::fs::remove_file(&path).is_ok() {
                len
            } else {
                0
            }
        };
        let removed = if removed == 0 { size } else { removed };
        total = total.saturating_sub(removed);
        freed = freed.saturating_add(removed);
    }
    Ok(freed)
}

/// 清空原始文件缓存（raw/）。
pub fn clear_raw_cache() -> Result<u64> {
    let dir = CacheDir::Raw.path();
    if dir.exists() {
        remove_dir_contents(&dir)
    } else {
        Ok(0)
    }
}

/// 清空封面缓存（cover/）。
pub fn clear_cover_cache() -> Result<u64> {
    let dir = CacheDir::Cover.path();
    if dir.exists() {
        remove_dir_contents(&dir)
    } else {
        Ok(0)
    }
}

/// 清空 AI 结果缓存（ai/）。
pub fn clear_ai_cache() -> Result<u64> {
    let dir = CacheDir::Ai.path();
    if dir.exists() {
        remove_dir_contents(&dir)
    } else {
        Ok(0)
    }
}

/// 清空 MOBI 页表缓存（mobi_table/）。
///
/// 页表只影响"首次打开要不要重新探测"，删掉只会让下次打开慢一点，**不影响正确性**。
pub fn clear_mobi_table_cache() -> Result<u64> {
    let dir = CacheDir::MobiTable.path();
    if dir.exists() {
        remove_dir_contents(&dir)
    } else {
        Ok(0)
    }
}

/// 清空 AI 超分临时文件（temp/）。
pub fn clear_temp_cache() -> Result<u64> {
    let dir = CacheDir::Temp.path();
    if dir.exists() {
        remove_dir_contents(&dir)
    } else {
        Ok(0)
    }
}

// ====== 按书清理（清理失效漫画数据用） ======

/// 对缓存命名空间做稳定哈希,作为 page/ 下的目录名。
/// 与 reader.rs 打开书籍时的命名空间哈希完全一致，保证按书删除命中同一目录。
pub fn stable_hash(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// 删除某本书的 L2 页面缓存目录（page/<ns-hash>/），返回释放的字节数。
/// `cache_ns` 必须与打开该书时的命名空间完全一致（如 `webdav|{origin}|{path}`）。
pub fn delete_page_cache_for_ns(cache_ns: &str) -> Result<u64> {
    let dir = CacheDir::Page.path().join(stable_hash(cache_ns));
    if !dir.exists() {
        return Ok(0);
    }
    let freed = dir_size(&dir);
    std::fs::remove_dir_all(&dir)?;
    Ok(freed)
}

/// 删除某本书的整本下载缓存目录（raw/<key-hash>/），返回释放的字节数。
/// `key` 必须与对应书源 `raw_cache_path` 的 hash 输入一致（通常为 `{origin}{path}`）。
/// 目录级删除（不关心缓存文件的实际文件名），命中即整目录移除。
pub fn delete_raw_cache_for_key(key: &str) -> Result<u64> {
    let hash = stable_hash(key);
    let dir = CacheDir::Raw.path().join(&hash);
    if !dir.exists() {
        return Ok(0);
    }
    let freed = dir_size(&dir);
    std::fs::remove_dir_all(&dir)?;
    Ok(freed)
}

/// 删除某本书的封面缓存文件（cover/ 下以 path 哈希为前缀的 .cover 文件），返回释放字节。
/// 封面 key 为 `{path_hash}_{page}_{w}_{h}[_crop].cover`，按 path 哈希前缀匹配删除，
/// 无需知道具体页码/尺寸/裁剪组合。
pub fn delete_cover_cache_for_path(path: &str) -> Result<u64> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    let prefix = format!("{:x}_", hasher.finish());

    let dir = CacheDir::Cover.path();
    let mut freed = 0u64;
    if !dir.exists() {
        return Ok(0);
    }
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let meta = entry.metadata()?;
        if meta.is_file() && name.starts_with(&prefix) {
            freed += meta.len();
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(freed)
}

/// 清空旧版遗留缓存：
/// - cache/ 根目录下的 16 位哈希目录（v0.2 之前的页面缓存布局）；
/// - 根级 download/ 目录（旧版 WebDAV 整本下载回退，自本版本起不再创建）。
/// v0.2 之前页面缓存写为 cache/<hash>/N.bin，升级后改为 cache/page/<hash>/N.bin；
/// 历史遗留目录不归任何当前缓存层管理，清空全部缓存时一并移除。
fn clear_legacy_artifacts() -> Result<u64> {
    let mut freed = 0u64;
    // 旧版 download/ 目录
    let legacy_download = cache_root().join("download");
    if legacy_download.exists() {
        freed += dir_size(&legacy_download);
        std::fs::remove_dir_all(&legacy_download)?;
    }
    // 旧版页面缓存（cache/<16位hash>/）
    let dir = cache_root().join("cache");
    if dir.exists() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let meta = entry.metadata()?;
            if !meta.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_legacy_hash = name.len() == 16 && name.chars().all(|c| c.is_ascii_hexdigit());
            if is_legacy_hash {
                let p = entry.path();
                freed += dir_size(&p);
                std::fs::remove_dir_all(&p)?;
            }
        }
    }
    Ok(freed)
}

/// 清空所有缓存（page/raw/cover/ai/temp + 旧版遗留目录）。
pub fn clear_all_caches() -> Result<u64> {
    Ok(clear_page_cache()?
        + clear_raw_cache()?
        + clear_cover_cache()?
        + clear_ai_cache()?
        + clear_temp_cache()?
        + clear_legacy_artifacts()?)
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

/// 迁移应用根目录：database.db + cache/ + 根级普通文件。
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

    // 收集待迁移项目：database.db、cache/、根级普通文件。
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
                if name == "cache" {
                    items.push(Item { name, is_dir: true });
                }
                // 其他未知目录不迁移
            } else if meta.is_file() {
                items.push(Item {
                    name,
                    is_dir: false,
                });
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
        // Windows 下文件刚复制完可能被杀软/索引服务瞬时锁定（metadata 读取失败
        // 会被 file_count 跳过），用短重试容忍瞬时抖动，避免误报迁移失败并触发回滚清理。
        for it in &items {
            let s = from_p.join(&it.name);
            let t = to_p.join(&it.name);
            let mut verified = false;
            for attempt in 0..5 {
                if it.is_dir {
                    if file_count(&s) == file_count(&t) {
                        verified = true;
                        break;
                    }
                } else {
                    let sl = std::fs::metadata(&s).map(|m| m.len()).unwrap_or(0);
                    let tl = std::fs::metadata(&t).map(|m| m.len()).unwrap_or(0);
                    if sl != 0 && sl == tl {
                        verified = true;
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(50 * (attempt + 1)));
            }
            if !verified {
                if it.is_dir {
                    bail!("迁移校验失败：{} 文件数量不一致", it.name);
                } else {
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

/// 删除根目录下已迁移的项目（database.db、cache/），返回释放字节。
pub fn delete_migrated_items(root: &str) -> Result<u64> {
    let root_p = PathBuf::from(root);
    if root_p.parent() == Some(&root_p) {
        bail!("缓存目录不能是磁盘根目录");
    }
    let mut freed = 0u64;
    for name in ["database.db", "cache"] {
        let p = root_p.join(name);
        if !p.starts_with(&root_p) || !p.exists() {
            continue;
        }
        let meta = std::fs::metadata(&p)?;
        freed += if meta.is_dir() {
            dir_size(&p)
        } else {
            meta.len()
        };
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

    /// 按书清理三件套（page/raw/cover）只命中目标书，其他缓存不受影响。
    /// 三者共享全局自定义缓存根，必须同一测试顺序执行（测试默认并发会互相覆盖该状态）。
    #[test]
    fn delete_by_book_helpers_only_remove_matching() {
        use std::hash::{Hash, Hasher};
        let base = std::env::temp_dir().join(format!("rch_test_purge_book_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        set_custom_cache_root(base.to_str().unwrap());

        // ---- page/ ----
        let ns_a = "webdav|https://host:443|/a.cbz";
        let ns_b = "webdav|https://host:443|/b.cbz";
        let _ = CacheDir::Page.ensure().unwrap();
        let dir_a = CacheDir::Page.path().join(stable_hash(ns_a));
        let dir_b = CacheDir::Page.path().join(stable_hash(ns_b));
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        std::fs::write(dir_a.join("data.bin"), vec![9u8; 32]).unwrap();
        let freed = delete_page_cache_for_ns(ns_a).unwrap();
        assert_eq!(freed, 32);
        assert!(!dir_a.exists());
        assert!(dir_b.exists());
        assert_eq!(
            delete_page_cache_for_ns("webdav|https://x|/n.cbz").unwrap(),
            0
        );

        // ---- raw/ ----
        let key_a = "https://host:443/dav/漫画.cbz";
        let key_b = "https://host:443/dav/另一本.cbz";
        let _ = CacheDir::Raw.ensure().unwrap();
        let raw_a = CacheDir::Raw.path().join(stable_hash(key_a));
        let raw_b = CacheDir::Raw.path().join(stable_hash(key_b));
        std::fs::create_dir_all(&raw_a).unwrap();
        std::fs::create_dir_all(&raw_b).unwrap();
        std::fs::write(raw_a.join("本.cbz"), vec![1u8; 64]).unwrap();
        let freed = delete_raw_cache_for_key(key_a).unwrap();
        assert_eq!(freed, 64);
        assert!(!raw_a.exists());
        assert!(raw_b.exists());

        // ---- cover/ ----
        let hash_of = |s: &str| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            s.hash(&mut h);
            format!("{:x}", h.finish())
        };
        let path_a = "/dav/漫画A.cbz";
        let path_b = "/dav/漫画B.cbz";
        let cover = CacheDir::Cover.ensure().unwrap();
        let f_a1 = cover.join(format!("{}_0_100_100.cover", hash_of(path_a)));
        let f_a2 = cover.join(format!("{}_3_200_200.cover", hash_of(path_a)));
        let f_b = cover.join(format!("{}_0_100_100.cover", hash_of(path_b)));
        std::fs::write(&f_a1, vec![2u8; 16]).unwrap();
        std::fs::write(&f_a2, vec![3u8; 16]).unwrap();
        std::fs::write(&f_b, vec![4u8; 16]).unwrap();
        let freed = delete_cover_cache_for_path(path_a).unwrap();
        assert_eq!(freed, 32);
        assert!(!f_a1.exists());
        assert!(!f_a2.exists());
        assert!(f_b.exists());

        set_custom_cache_root("");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn migrate_copies_db_cache_skips_support() {
        // 进程唯一临时目录，避免上次运行残留（删除被锁/失败时遗留）干扰本次断言。
        let base = std::env::temp_dir().join(format!("rch_test_migrate_v2_{}", std::process::id()));
        let from = base.join("from");
        let to = base.join("to");
        let support = from.join("RCH");
        let _ = std::fs::remove_dir_all(&base);

        std::fs::create_dir_all(from.join("cache/page")).unwrap();
        std::fs::create_dir_all(&support).unwrap();
        std::fs::write(from.join("database.db"), vec![1u8; 500]).unwrap();
        std::fs::write(from.join("cache/page/a.bin"), vec![2u8; 100]).unwrap();
        std::fs::write(from.join("note.txt"), b"root file").unwrap();
        std::fs::write(support.join("library.json"), b"{}").unwrap();

        let n = migrate_cache_root(
            from.to_str().unwrap(),
            to.to_str().unwrap(),
            support.to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(n, 500 + 100 + 9);
        assert!(to.join("database.db").exists());
        assert!(to.join("cache/page/a.bin").exists());
        assert!(to.join("note.txt").exists());
        // 支持目录不迁移；成功迁移后标记被清除
        assert!(!to.join("RCH").exists());
        assert!(!from.join(MIGRATION_MARKER).exists());

        let freed = delete_migrated_items(from.to_str().unwrap()).unwrap();
        assert!(freed >= 600);
        assert!(!from.join("database.db").exists());
        assert!(!from.join("cache").exists());
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
        assert!(migrate_cache_root(
            a.to_str().unwrap(),
            a.to_str().unwrap(),
            support.to_str().unwrap()
        )
        .is_err());
        assert!(migrate_cache_root(
            base.to_str().unwrap(),
            a.to_str().unwrap(),
            support.to_str().unwrap()
        )
        .is_err());
        assert!(migrate_cache_root(
            a.to_str().unwrap(),
            base.to_str().unwrap(),
            support.to_str().unwrap()
        )
        .is_err());
        assert!(migrate_cache_root("C:\\", "D:\\tmp_x2", support.to_str().unwrap()).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}


#[cfg(test)]
mod rg_a_atomic_cache_file_tests {
    //! RG-A：raw-cache 写入的**原子可见性**与失败清理契约。
    use super::AtomicCacheFile;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rch_rga_atomic_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn part_files(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".part-"))
            .collect()
    }

    /// 核心不变量：**未 commit 时最终路径不可见**（读者看不到部分内容）。
    #[test]
    fn rg_a_atomic_target_is_invisible_until_commit() {
        let dir = temp_dir("invisible");
        let target = dir.join("book.cbz");

        let mut writer = AtomicCacheFile::create(&target).unwrap();
        writer.write_all(b"partial").unwrap();

        assert!(
            !target.exists(),
            "final cache path must NOT exist while the write is in flight"
        );
        assert_eq!(part_files(&dir).len(), 1, "exactly one .part-<pid> temp exists");

        let published = writer.commit().unwrap();
        assert_eq!(published, target);
        assert_eq!(std::fs::read(&target).unwrap(), b"partial");
        assert!(
            part_files(&dir).is_empty(),
            "commit must leave no temp file behind"
        );
    }

    /// 2026-09-21（用户要求拓展"整本缓存清理机制"）：`cache/raw` 超限时必须
    /// **最旧优先**整包删除，且**始终保留最新的那个包**（避免刚下完就被自己删掉）。
    #[test]
    fn raw_cache_limit_evicts_oldest_packages_and_keeps_the_newest() {
        use super::{
            enforce_raw_cache_limit, set_custom_cache_root, CacheDir, RAW_CACHE_LIMIT_BYTES,
        };
        use std::time::{Duration, SystemTime, UNIX_EPOCH};

        struct RootGuard;
        impl Drop for RootGuard {
            fn drop(&mut self) {
                set_custom_cache_root("");
            }
        }

        let root = std::env::temp_dir().join(format!(
            "rch_raw_limit_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        set_custom_cache_root(root.to_str().unwrap());
        let _guard = RootGuard;

        let raw = CacheDir::Raw.path();
        std::fs::create_dir_all(&raw).unwrap();
        // 三个 1 MiB 的包，文件 mtime 依次递增（old < mid < new）。
        let base = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        for (index, name) in ["old", "mid", "new"].iter().enumerate() {
            let dir = raw.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("book.bin");
            std::fs::write(&file, vec![0u8; 1024 * 1024]).unwrap();
            let stamp = base + Duration::from_secs(index as u64 * 60);
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(stamp)
                .unwrap();
        }

        // 未超限：不动任何东西。
        assert_eq!(enforce_raw_cache_limit(RAW_CACHE_LIMIT_BYTES).unwrap(), 0);
        assert!(raw.join("old").exists());

        // 上限 2.5 MiB（三个包共 3 MiB）⇒ 必须删掉最旧的 old，保留 mid/new。
        let freed = enforce_raw_cache_limit(1024 * 1024 * 5 / 2).unwrap();
        assert!(freed >= 1024 * 1024, "应释放约 1 MiB，实际 {freed}");
        assert!(!raw.join("old").exists(), "最旧的包必须先被删");
        assert!(raw.join("mid").exists() && raw.join("new").exists());

        // 上限极小：仍**至少保留一个**（最新的），绝不把 raw 清空。
        let _ = enforce_raw_cache_limit(1).unwrap();
        assert!(
            raw.join("new").exists(),
            "无论上限多小，都必须保留最新的那个包"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 失败/提前 Drop ⇒ 目标不存在且临时文件被清理（不留半文件）。
    #[test]
    fn rg_a_atomic_abort_removes_temp_and_leaves_no_target() {
        let dir = temp_dir("abort");
        let target = dir.join("book.cbz");

        {
            let mut writer = AtomicCacheFile::create(&target).unwrap();
            writer.write_all(b"half").unwrap();
            // 模拟写入中途出错：直接 drop（不 commit）。
        }

        assert!(!target.exists(), "aborted write must not publish a target");
        assert!(
            part_files(&dir).is_empty(),
            "aborted write must remove its temp file"
        );
    }

    /// 已存在的**非完整**目标（例如旧实现的 0 字节残留）可被安全替换。
    #[test]
    fn rg_a_atomic_commit_replaces_existing_stale_target() {
        let dir = temp_dir("replace");
        let target = dir.join("book.cbz");
        std::fs::write(&target, b"").unwrap();
        assert_eq!(std::fs::metadata(&target).unwrap().len(), 0);

        let mut writer = AtomicCacheFile::create(&target).unwrap();
        writer.write_all(b"complete").unwrap();
        writer.commit().unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"complete");
        assert!(part_files(&dir).is_empty());
    }

    /// 多次写入按顺序拼接（调用方用 64KB 循环写入）。
    #[test]
    fn rg_a_atomic_supports_chunked_writes() {
        let dir = temp_dir("chunked");
        let target = dir.join("book.cbz");
        let mut writer = AtomicCacheFile::create(&target).unwrap();
        for _ in 0..3 {
            writer.write_all(&[7u8; 64 * 1024]).unwrap();
        }
        writer.commit().unwrap();
        assert_eq!(
            std::fs::metadata(&target).unwrap().len(),
            3 * 64 * 1024,
            "all chunks must be present after commit"
        );
    }
}


#[cfg(test)]
mod rg_a_atomic_temp_uniqueness_tests {
    //! RG-A：临时文件**唯一性**契约（同进程并发写同一目标不得互相截断）。
    use super::AtomicCacheFile;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rch_rga_uniq_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn part_files(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".part-"))
            .collect()
    }

    /// 两个 writer 写**同一目标** ⇒ 两个不同临时文件；依次 commit 后无残留，
    /// 且不会出现"一个 writer 截断另一个 writer 的临时文件"。
    #[test]
    fn rg_a_atomic_temp_names_are_unique_per_writer() {
        let dir = temp_dir("writers");
        let target = dir.join("book.cbz");

        let mut first = AtomicCacheFile::create(&target).unwrap();
        first.write_all(b"AAAA").unwrap();
        let mut second = AtomicCacheFile::create(&target).unwrap();
        second.write_all(b"BBBBBB").unwrap();

        assert_eq!(
            part_files(&dir).len(),
            2,
            "two concurrent writers must hold two distinct temp files"
        );

        first.commit().unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"AAAA");
        assert!(target.exists(), "first commit publishes the target");

        // 第二个 writer 的临时文件**未被**第一个 writer 影响，仍可正常发布。
        second.commit().unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"BBBBBB");
        assert!(
            part_files(&dir).is_empty(),
            "no temp file may survive the two commits"
        );
    }

    /// 一个 writer 中止、另一个成功 ⇒ 只清理自己的临时文件，不留任何残留。
    #[test]
    fn rg_a_atomic_abort_does_not_disturb_another_writer() {
        let dir = temp_dir("mixed");
        let target = dir.join("book.cbz");

        let mut aborted = AtomicCacheFile::create(&target).unwrap();
        aborted.write_all(b"stale").unwrap();
        let mut good = AtomicCacheFile::create(&target).unwrap();
        good.write_all(b"final").unwrap();

        drop(aborted); // 模拟失败/取消
        good.commit().unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"final");
        assert!(
            part_files(&dir).is_empty(),
            "aborted writer must clean only its own temp file"
        );
    }
}
