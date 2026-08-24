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
        PathBuf::from(appdata)
            .join("RCH")
            .to_string_lossy()
            .into_owned()
    } else {
        std::env::temp_dir()
            .join("RCH")
            .to_string_lossy()
            .into_owned()
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

// ============================================================
// 清理失效漫画数据（设置 → 缓存管理 → 清理失效漫画数据）
// ============================================================

/// 解析 WebDAV 服务器 origin（`scheme://host[:port]`），与 WebDavClient::new 一致。
/// 仅解析身份，不建立任何连接。
fn webdav_origin(url: &str) -> Option<String> {
    let u = reqwest::Url::parse(url.trim()).ok()?;
    let scheme = u.scheme().to_string();
    let host = u.host_str()?.to_string();
    Some(match u.port() {
        Some(p) => format!("{scheme}://{host}:{p}"),
        None => format!("{scheme}://{host}"),
    })
}

/// 解析 SFTP endpoint（`host` 或 `host:port`，默认端口省略），
/// 与 Dart 侧 `sftp_session._parseHostPort` + Rust `SftpClient::new` 的 endpoint 规则一致。
fn sftp_endpoint(url: Option<&str>, port: Option<i64>) -> Option<String> {
    let addr = url?.trim().trim_end_matches('/').to_string();
    if addr.is_empty() {
        return None;
    }
    let (host, p) = if addr.contains(':') {
        let idx = addr.rfind(':')?;
        match addr[idx + 1..].parse::<i64>() {
            Ok(p) if p > 0 => (addr[..idx].to_string(), Some(p)),
            _ => (addr.clone(), port),
        }
    } else {
        (addr.clone(), port)
    };
    let p = p.unwrap_or(22);
    if p == 22 {
        Some(host)
    } else {
        Some(format!("{host}:{p}"))
    }
}

/// 清理单个失效漫画的磁盘缓存（page/ 页面 + raw/ 整本 + cover/ 封面），返回释放字节。
///
/// - `cache_ns` 按书源类型重建，与 `open_*_book` 时的命名空间完全一致，保证命中同一目录；
/// - 输入均为 BookSource 身份字段（Dart 原样传入），不联网、不建会话：
///   - `url`：webdav 的 base URL；sftp 的 `host` / `host:port` 地址
///   - `port`：sftp 端口（缺省 22）
///   - `root_path`：source.path（baidu 的 root 目录）
///   - `client_id`：baidu app_key / 115 app_id
///   - `root_id`：115 / quark 的根目录 id
///   - `cookie_mode`：115 是否为网页 Cookie 模式（origin 前缀不同）
///
/// 说明：quark / 115 的浏览路径本身即内部素材 id（fid / pick_code），
/// 与 `raw_cache_path(origin, path)` 的入参一致，raw/ 整本缓存可精确删除。
/// AI 超分缓存按页面内容哈希组织，需打开书本才能枚举，由「清空 AI 缓存」统一管理。
pub fn purge_stale_book_cache(
    source_type: String,
    path: String,
    url: Option<String>,
    port: Option<i64>,
    root_path: String,
    client_id: Option<String>,
    root_id: Option<String>,
    cookie_mode: bool,
) -> Result<u64, String> {
    use crate::cache::{
        delete_cover_cache_for_path, delete_page_cache_for_ns, delete_raw_cache_for_key,
    };
    if path.is_empty() {
        return Ok(0);
    }
    let mut freed = 0u64;

    match source_type.as_str() {
        "local" => {
            // 本地书源只写 page/ 与 cover/，无 raw/。
            freed +=
                delete_page_cache_for_ns(&format!("local|{path}")).map_err(|e| e.to_string())?;
            freed += delete_cover_cache_for_path(&path).map_err(|e| e.to_string())?;
        }
        "webdav" => {
            let origin = match url.as_deref().and_then(webdav_origin) {
                Some(o) => o,
                None => return Ok(0),
            };
            freed += delete_page_cache_for_ns(&format!("webdav|{origin}|{path}"))
                .map_err(|e| e.to_string())?;
            freed +=
                delete_raw_cache_for_key(&format!("{origin}{path}")).map_err(|e| e.to_string())?;
            freed += delete_cover_cache_for_path(&path).map_err(|e| e.to_string())?;
        }
        "sftp" => {
            let endpoint = match sftp_endpoint(url.as_deref(), port) {
                Some(e) => e,
                None => return Ok(0),
            };
            freed += delete_page_cache_for_ns(&format!("sftp|{endpoint}|{path}"))
                .map_err(|e| e.to_string())?;
            freed += delete_raw_cache_for_key(&format!("{endpoint}{path}"))
                .map_err(|e| e.to_string())?;
            freed += delete_cover_cache_for_path(&path).map_err(|e| e.to_string())?;
        }
        "baidu" => {
            // BaiduClient::new 中 root 为空时归一为 "/"，origin 必须一致才能命中缓存。
            let root = if root_path.trim().is_empty() {
                "/".to_string()
            } else {
                root_path
            };
            let origin = format!("baidu:{}:{}", client_id.unwrap_or_default(), root);
            freed += delete_page_cache_for_ns(&format!("baidu|{origin}|{path}"))
                .map_err(|e| e.to_string())?;
            freed +=
                delete_raw_cache_for_key(&format!("{origin}{path}")).map_err(|e| e.to_string())?;
            freed += delete_cover_cache_for_path(&path).map_err(|e| e.to_string())?;
        }
        "115" => {
            // Cloud115Client::new / Cloud115WebClient::new 中 root_id 为空时归一为 "0"。
            let root = root_id.unwrap_or_default();
            let root = if root.trim().is_empty() {
                "0".to_string()
            } else {
                root
            };
            let origin = if cookie_mode {
                format!("115web:{root}")
            } else {
                format!("115:{}:{root}", client_id.unwrap_or_default())
            };
            freed += delete_page_cache_for_ns(&format!("115|{origin}|{path}"))
                .map_err(|e| e.to_string())?;
            // Cookie 模式 raw 键也以浏览路径（pick_code）为入参，同样可精确删除。
            freed +=
                delete_raw_cache_for_key(&format!("{origin}{path}")).map_err(|e| e.to_string())?;
            freed += delete_cover_cache_for_path(&path).map_err(|e| e.to_string())?;
        }
        "quark" => {
            // QuarkClient::new 中 root 为空时归一为 "0"。
            let root = root_id.unwrap_or_default();
            let root = if root.trim().is_empty() {
                "0".to_string()
            } else {
                root
            };
            let origin = format!("quark:{root}");
            freed += delete_page_cache_for_ns(&format!("quark|{origin}|{path}"))
                .map_err(|e| e.to_string())?;
            // raw 键以素材 fid（浏览路径）为入参，可精确删除。
            freed +=
                delete_raw_cache_for_key(&format!("{origin}{path}")).map_err(|e| e.to_string())?;
            freed += delete_cover_cache_for_path(&path).map_err(|e| e.to_string())?;
        }
        _ => return Ok(0),
    }
    Ok(freed)
}

use std::path::PathBuf;
