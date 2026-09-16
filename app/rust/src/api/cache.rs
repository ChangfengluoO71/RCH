//! 缓存管理 API（暴露给 Dart）。

use crate::cache;
use rusqlite::{params, Connection, OptionalExtension};

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

/// Completion cleanup for a live remote Reader session.
///
/// Generated covers and all database projections are deliberately outside
/// this function. Image folders never have a whole-book raw cache, so their
/// completion path deletes only the page-cache namespace.
pub fn purge_remote_book_content_cache(
    source_type: String,
    path: String,
    url: Option<String>,
    port: Option<i64>,
    root_path: String,
    client_id: Option<String>,
    root_id: Option<String>,
    cookie_mode: bool,
    image_folder: bool,
) -> Result<u64, String> {
    use crate::cache::{delete_page_cache_for_ns, delete_raw_cache_for_key};
    if path.is_empty() {
        return Ok(0);
    }
    let (cache_ns, raw_key) = match source_type.as_str() {
        "webdav" => {
            let Some(origin) = url.as_deref().and_then(webdav_origin) else {
                return Ok(0);
            };
            (format!("webdav|{origin}|{path}"), format!("{origin}{path}"))
        }
        "sftp" => {
            let Some(endpoint) = sftp_endpoint(url.as_deref(), port) else {
                return Ok(0);
            };
            (
                format!("sftp|{endpoint}|{path}"),
                format!("{endpoint}{path}"),
            )
        }
        "baidu" => {
            let root = if root_path.trim().is_empty() {
                "/".to_string()
            } else {
                root_path
            };
            let origin = format!("baidu:{}:{root}", client_id.unwrap_or_default());
            (format!("baidu|{origin}|{path}"), format!("{origin}{path}"))
        }
        "115" => {
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
            (format!("115|{origin}|{path}"), format!("{origin}{path}"))
        }
        "quark" => {
            let root = root_id.unwrap_or_default();
            let root = if root.trim().is_empty() {
                "0".to_string()
            } else {
                root
            };
            let origin = format!("quark:{root}");
            (format!("quark|{origin}|{path}"), format!("{origin}{path}"))
        }
        _ => return Ok(0),
    };
    let mut freed = delete_page_cache_for_ns(&cache_ns).map_err(|e| e.to_string())?;
    if !image_folder {
        freed += delete_raw_cache_for_key(&raw_key).map_err(|e| e.to_string())?;
    }
    Ok(freed)
}

fn path_is_within(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|tail| tail.starts_with('/'))
}

struct RemoteCacheSourceIdentity {
    source_type: String,
    url: Option<String>,
    port: Option<i64>,
    root_path: String,
    client_id: Option<String>,
    root_id: Option<String>,
    cookie_mode: bool,
}

pub(crate) fn purge_verified_remote_asset_on(
    conn: &Connection,
    source_id: &str,
    logical_path: &str,
    _dependency_paths: &[String],
) -> Result<u64, String> {
    let logical_path = crate::remote_scan::model::normalize_path(logical_path);
    let verified =
        crate::remote_scan::persistence::load_verified_remote_tombstones(conn, source_id)
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|tombstone| tombstone.logical_path == logical_path)
            .ok_or_else(|| {
                "remote deletion is not backed by current complete-listing proof".to_string()
            })?;
    // The proof is authoritative.  Callers may have loaded an older or
    // truncated DTO; never let that subset decide what dependent state stays.
    let dependencies = verified
        .dependency_paths
        .into_iter()
        .collect::<std::collections::HashSet<_>>();

    let source = conn
        .query_row(
            "SELECT type,url,port,path,client_id,root_id,COALESCE(cookie,'') <> '' \
             FROM book_sources WHERE id=?1",
            [source_id],
            |row| {
                Ok(RemoteCacheSourceIdentity {
                    source_type: row.get(0)?,
                    url: row.get(1)?,
                    port: row.get(2)?,
                    root_path: row.get(3)?,
                    client_id: row.get(4)?,
                    root_id: row.get(5)?,
                    cookie_mode: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(source) = source else {
        return Err("remote source no longer exists".to_string());
    };
    if !matches!(
        source.source_type.as_str(),
        "webdav" | "sftp" | "baidu" | "115" | "quark"
    ) {
        return Err("verified remote cleanup is unavailable for this source type".to_string());
    }

    let logical_prefix = format!("{logical_path}/");
    let mut physical_paths = conn
        .prepare("SELECT path FROM library_index WHERE source_id=?1")
        .map_err(|error| error.to_string())?
        .query_map([source_id], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .filter_map(|row| row.ok())
        .filter(|path| path == &logical_path || path.starts_with(&logical_prefix))
        .collect::<std::collections::HashSet<_>>();
    physical_paths.insert(logical_path.clone());

    let dependency_rows = conn
        .prepare(
            "SELECT book_key,dependency_path FROM remote_cover_dependency \
             WHERE substr(book_key,1,length(?1))=?1",
        )
        .map_err(|error| error.to_string())?
        .query_map([format!("{}|{source_id}|", source.source_type)], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?
        .filter_map(|row| row.ok())
        .collect::<Vec<_>>();
    let logical_book_key = crate::db::book_key_of(&source.source_type, source_id, &logical_path);
    let has_live_alias = conn
        .prepare("SELECT path FROM library_index WHERE source_id=?1 AND deleted=0")
        .map_err(|error| error.to_string())?
        .query_map([source_id], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .filter_map(|row| row.ok())
        .any(|path| {
            path != logical_path
                && crate::db::book_key_of(&source.source_type, source_id, &path) == logical_book_key
        });
    if !has_live_alias {
        physical_paths.extend(dependencies.iter().cloned());
    }
    let mut affected_book_keys = std::collections::HashSet::new();
    if !has_live_alias {
        affected_book_keys.insert(logical_book_key.clone());
    }
    for (book_key, dependency_path) in &dependency_rows {
        let dependency_path = crate::remote_scan::model::normalize_path(dependency_path);
        if !has_live_alias
            && (dependencies.contains(&dependency_path)
                || path_is_within(&dependency_path, &logical_path))
        {
            affected_book_keys.insert(book_key.clone());
            physical_paths.insert(dependency_path);
        }
    }

    for path in conn
        .prepare("SELECT path FROM library_index WHERE source_id=?1 AND deleted=0")
        .map_err(|error| error.to_string())?
        .query_map([source_id], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .filter_map(|row| row.ok())
    {
        let book_key = crate::db::book_key_of(&source.source_type, source_id, &path);
        if affected_book_keys.contains(&book_key)
            && !(has_live_alias && book_key == logical_book_key)
        {
            physical_paths.insert(path);
        }
    }

    let mut freed = 0;
    for path in physical_paths {
        freed += purge_stale_book_cache(
            source.source_type.clone(),
            path,
            source.url.clone(),
            source.port,
            source.root_path.clone(),
            source.client_id.clone(),
            source.root_id.clone(),
            source.cookie_mode,
        )?;
    }

    let tx = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    for (book_key, dependency_path) in dependency_rows {
        let dependency_path = crate::remote_scan::model::normalize_path(&dependency_path);
        if !has_live_alias
            && (dependencies.contains(&dependency_path)
                || path_is_within(&dependency_path, &logical_path))
        {
            tx.execute(
                "DELETE FROM remote_cover_dependency WHERE book_key=?1 AND dependency_path=?2",
                params![book_key, dependency_path],
            )
            .map_err(|error| error.to_string())?;
        }
    }
    for book_key in affected_book_keys {
        tx.execute(
            "DELETE FROM remote_cover_partial_cache WHERE book_key=?1",
            [book_key],
        )
        .map_err(|error| error.to_string())?;
    }
    tx.commit().map_err(|error| error.to_string())?;
    Ok(freed)
}

/// Delete cache state only for a tombstone proven by a complete listing from
/// the source's current successful scan generation.
pub fn purge_verified_remote_asset(
    source_id: String,
    logical_path: String,
    dependency_paths: Vec<String>,
) -> Result<u64, String> {
    let conn = crate::db::get().lock().map_err(|error| error.to_string())?;
    purge_verified_remote_asset_on(&conn, &source_id, &logical_path, &dependency_paths)
}

use std::path::PathBuf;

#[cfg(test)]
mod remote_book_completion_cleanup_tests {
    use super::*;
    use crate::cache::{self, CacheDir};

    #[test]
    fn remote_folder_completion_removes_pages_but_preserves_raw_and_cover() {
        let base = std::env::temp_dir().join(format!(
            "rch_remote_folder_completion_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        cache::set_custom_cache_root(base.to_str().unwrap());

        let path = "/Series/Book";
        let origin = "https://host";
        let page_dir = CacheDir::Page
            .ensure()
            .unwrap()
            .join(cache::stable_hash(&format!("webdav|{origin}|{path}")));
        std::fs::create_dir_all(&page_dir).unwrap();
        std::fs::write(page_dir.join("0.bin"), b"page").unwrap();
        let raw_dir = CacheDir::Raw
            .ensure()
            .unwrap()
            .join(cache::stable_hash(&format!("{origin}{path}")));
        std::fs::create_dir_all(&raw_dir).unwrap();
        std::fs::write(raw_dir.join("unexpected.bin"), b"raw").unwrap();
        cache::cover_cache_write(path, 0, 1, 1, None, &[1, 2, 3, 4]).unwrap();

        let freed = purge_remote_book_content_cache(
            "webdav".into(),
            path.into(),
            Some("https://host/dav".into()),
            None,
            "/".into(),
            None,
            None,
            false,
            true,
        )
        .unwrap();

        assert_eq!(freed, 4);
        assert!(!page_dir.exists());
        assert!(raw_dir.exists());
        assert!(cache::cover_cache_read(path, 0, 1, 1, None).is_some());

        cache::set_custom_cache_root("");
        let _ = std::fs::remove_dir_all(base);
    }
}

#[cfg(test)]
mod verified_remote_asset_cleanup_tests {
    use super::*;
    use crate::cache::{self, CacheDir};
    use rusqlite::{params, Connection};

    fn write_namespace(dir: CacheDir, key: &str, name: &str) -> std::path::PathBuf {
        let path = dir.ensure().unwrap().join(cache::stable_hash(key));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join(name), b"cache").unwrap();
        path
    }

    #[test]
    fn verified_folder_deletion_removes_alias_dependency_page_and_raw_caches_only() {
        let base = std::env::temp_dir().join(format!(
            "rch_verified_remote_asset_cleanup_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        cache::set_custom_cache_root(base.to_str().unwrap());

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE book_sources(\
               id TEXT PRIMARY KEY,type TEXT NOT NULL,fingerprint TEXT NOT NULL,url TEXT,port INTEGER,path TEXT,client_id TEXT,root_id TEXT,cookie TEXT);\
             CREATE TABLE library_index(\
               id TEXT PRIMARY KEY,source_id TEXT NOT NULL,parent_id TEXT,name TEXT,path TEXT,entry_type TEXT,\
               size INTEGER,modified_at INTEGER,asset_kind TEXT,content_fingerprint TEXT,\
               scan_generation INTEGER,listing_complete INTEGER NOT NULL DEFAULT 0,deleted INTEGER NOT NULL DEFAULT 0,updated_at INTEGER);",
        )
        .unwrap();
        crate::remote_scan::persistence::migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO book_sources VALUES('source','webdav','canonical-source','https://host/dav',NULL,'/',NULL,NULL,NULL)",
            [],
        )
        .unwrap();
        crate::remote_scan::persistence::bind_scan_epoch(&conn, "source", 2, "/", 2).unwrap();
        conn.execute(
            "INSERT INTO remote_scan_state(source_id,status,mode,generation) VALUES('source','Succeeded','Snapshot',2)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO remote_listing_state VALUES('source','/','root-v2',2,1)",
            [],
        )
        .unwrap();
        let root_id = crate::db::library_index_id("canonical-source", "/");
        for (path, deleted) in [
            ("/Series", 1),
            ("/Series/book.zip", 0),
            ("/Series/book.cbz", 0),
            ("/book.zip", 1),
            ("/book.cbz", 0),
            ("/Other/book.cbz", 0),
        ] {
            let parent = path
                .rsplit_once('/')
                .map(|(p, _)| if p.is_empty() { "/" } else { p })
                .unwrap();
            conn.execute(
                "INSERT INTO library_index(id,source_id,parent_id,name,path,entry_type,scan_generation,listing_complete,deleted,updated_at)\
                 VALUES(?1,'source',?2,?3,?4,?5,1,1,?6,1)",
                params![
                    crate::db::library_index_id("canonical-source", path),
                    if path == "/Series" { root_id.clone() } else { crate::db::library_index_id("canonical-source", parent) },
                    path.rsplit('/').next().unwrap(),
                    path,
                    if path.ends_with("Series") { "dir" } else { "file" },
                    deleted,
                ],
            )
            .unwrap();
        }
        let book_key = crate::db::book_key_of("webdav", "source", "/Series/book.cbz");
        conn.execute(
            "INSERT INTO remote_cover_dependency VALUES(?1,'/Series/001.jpg','image-v1','default','partial_ready')",
            [&book_key],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO remote_cover_dependency VALUES(?1,'/Series/new.jpg','image-v2','default','queued')",
            [&book_key],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO remote_cover_partial_cache VALUES(?1,'image-v1',X'0102',1)",
            [&book_key],
        )
        .unwrap();
        let unrelated_key = crate::db::book_key_of("webdav", "source", "/Other/book.cbz");
        conn.execute(
            "INSERT INTO remote_cover_dependency VALUES(?1,'/Other/001.jpg','other-v1','default','partial_ready')",
            [&unrelated_key],
        )
        .unwrap();
        let alias_key = crate::db::book_key_of("webdav", "source", "/book.zip");
        conn.execute(
            "INSERT INTO remote_cover_dependency VALUES(?1,'/outside/cover.jpg','alias-v1','default','partial_ready')",
            [&alias_key],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO remote_cover_partial_cache VALUES(?1,'alias-v1',X'0102',1)",
            [&alias_key],
        )
        .unwrap();

        let origin = "https://host";
        let alias_page = write_namespace(
            CacheDir::Page,
            &format!("webdav|{origin}|/Series/book.cbz"),
            "0.bin",
        );
        let alias_raw = write_namespace(
            CacheDir::Raw,
            &format!("{origin}/Series/book.zip"),
            "book.zip",
        );
        let unrelated_page = write_namespace(
            CacheDir::Page,
            &format!("webdav|{origin}|/Other/book.cbz"),
            "0.bin",
        );
        for path in [
            "/Series",
            "/Series/book.zip",
            "/Series/book.cbz",
            "/Series/001.jpg",
            "/book.zip",
            "/book.cbz",
            "/Other/book.cbz",
        ] {
            cache::cover_cache_write(path, 0, 1, 1, None, &[1, 2, 3, 4]).unwrap();
        }

        let freed = purge_verified_remote_asset_on(
            &conn,
            "source",
            "/Series",
            &["/Series/001.jpg".to_string()],
        )
        .unwrap();

        assert!(freed > 0);
        assert!(!alias_page.exists());
        assert!(!alias_raw.exists());
        for path in [
            "/Series",
            "/Series/book.zip",
            "/Series/book.cbz",
            "/Series/001.jpg",
        ] {
            assert!(
                cache::cover_cache_read(path, 0, 1, 1, None).is_none(),
                "{path}"
            );
        }
        assert!(unrelated_page.exists());
        assert!(cache::cover_cache_read("/Other/book.cbz", 0, 1, 1, None).is_some());
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM remote_cover_dependency WHERE book_key=?1",
                [&unrelated_key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 1);
        let queued: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM remote_cover_dependency WHERE book_key=?1",
                [&book_key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(queued, 0);
        let removed_old: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM remote_cover_dependency WHERE book_key=?1 AND dependency_path='/Series/001.jpg'",
                [&book_key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(removed_old, 0);

        purge_verified_remote_asset_on(&conn, "source", "/book.zip", &[]).unwrap();
        assert!(cache::cover_cache_read("/book.zip", 0, 1, 1, None).is_none());
        assert!(cache::cover_cache_read("/book.cbz", 0, 1, 1, None).is_some());
        let alias_state: (i64, i64) = conn
            .query_row(
                "SELECT
                   (SELECT COUNT(*) FROM remote_cover_dependency WHERE book_key=?1),
                   (SELECT COUNT(*) FROM remote_cover_partial_cache WHERE book_key=?1)",
                [&alias_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        // /book.zip and /book.cbz share the logical key. Removing the former
        // must not discard dependency/partial state still used by the live
        // alias.
        assert_eq!(alias_state, (1, 1));

        cache::set_custom_cache_root("");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn deleted_cover_dependency_preserves_newly_queued_replacement_for_live_book() {
        let base = std::env::temp_dir().join(format!(
            "rch_remote_cover_dependency_requeue_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        cache::set_custom_cache_root(base.to_str().unwrap());
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE book_sources(id TEXT PRIMARY KEY,type TEXT NOT NULL,fingerprint TEXT NOT NULL,url TEXT,port INTEGER,path TEXT,client_id TEXT,root_id TEXT,cookie TEXT);\
             CREATE TABLE library_index(id TEXT PRIMARY KEY,source_id TEXT,parent_id TEXT,name TEXT,path TEXT,entry_type TEXT,size INTEGER,modified_at INTEGER,asset_kind TEXT,content_fingerprint TEXT,scan_generation INTEGER,listing_complete INTEGER,deleted INTEGER,updated_at INTEGER);",
        )
        .unwrap();
        crate::remote_scan::persistence::migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO book_sources VALUES('source','webdav','canonical-source','https://host/dav',NULL,'/',NULL,NULL,NULL)",
            [],
        )
        .unwrap();
        crate::remote_scan::persistence::bind_scan_epoch(&conn, "source", 2, "/", 2).unwrap();
        conn.execute(
            "INSERT INTO remote_scan_state(source_id,status,mode,generation) VALUES('source','Succeeded','Snapshot',2)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO remote_listing_state VALUES('source','/Series','series-v2',2,1)",
            [],
        )
        .unwrap();
        let series_id = crate::db::library_index_id("canonical-source", "/Series");
        conn.execute(
            "INSERT INTO library_index(id,source_id,parent_id,name,path,entry_type,scan_generation,listing_complete,deleted,updated_at)\
             VALUES(?1,'source',?2,'Series','/Series','dir',2,1,0,1)",
            params![
                series_id,
                crate::db::library_index_id("canonical-source", "/"),
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO library_index(id,source_id,parent_id,name,path,entry_type,scan_generation,listing_complete,deleted,updated_at)\
             VALUES(?1,'source',?2,'001.jpg','/Series/001.jpg','file',1,1,1,1)",
            params![
                crate::db::library_index_id("canonical-source", "/Series/001.jpg"),
                series_id,
            ],
        )
        .unwrap();
        let book_key = crate::db::book_key_of("webdav", "source", "/Series");
        for (path, fingerprint, status) in [
            ("/Series/001.jpg", "image-v1", "partial_ready"),
            ("/Series/new.jpg", "image-v2", "queued"),
        ] {
            conn.execute(
                "INSERT INTO remote_cover_dependency VALUES(?1,?2,?3,'default',?4)",
                params![book_key, path, fingerprint, status],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO remote_cover_partial_cache VALUES(?1,'image-v1',X'0102',1)",
            [&book_key],
        )
        .unwrap();
        cache::cover_cache_write("/Series", 0, 1, 1, None, &[1, 2, 3, 4]).unwrap();

        purge_verified_remote_asset_on(
            &conn,
            "source",
            "/Series/001.jpg",
            &["/Series/001.jpg".into()],
        )
        .unwrap();

        let remaining: Vec<(String, String)> = conn
            .prepare(
                "SELECT dependency_path,status FROM remote_cover_dependency WHERE book_key=?1 ORDER BY dependency_path",
            )
            .unwrap()
            .query_map([&book_key], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(remaining, vec![("/Series/new.jpg".into(), "queued".into())]);
        assert!(cache::cover_cache_read("/Series", 0, 1, 1, None).is_none());
        let partial: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM remote_cover_partial_cache WHERE book_key=?1",
                [&book_key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(partial, 0);

        cache::set_custom_cache_root("");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn unverified_remote_deletion_does_not_remove_any_cache() {
        let base = std::env::temp_dir().join(format!(
            "rch_unverified_remote_asset_cleanup_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        cache::set_custom_cache_root(base.to_str().unwrap());
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE book_sources(id TEXT PRIMARY KEY,type TEXT NOT NULL,fingerprint TEXT NOT NULL,url TEXT,port INTEGER,path TEXT,client_id TEXT,root_id TEXT,cookie TEXT);\
             CREATE TABLE library_index(id TEXT PRIMARY KEY,source_id TEXT,parent_id TEXT,name TEXT,path TEXT,entry_type TEXT,size INTEGER,modified_at INTEGER,asset_kind TEXT,content_fingerprint TEXT,scan_generation INTEGER,listing_complete INTEGER,deleted INTEGER,updated_at INTEGER);",
        )
        .unwrap();
        crate::remote_scan::persistence::migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO book_sources VALUES('source','webdav','canonical-source','https://host/dav',NULL,'/',NULL,NULL,NULL)",
            [],
        )
        .unwrap();
        cache::cover_cache_write("/book.cbz", 0, 1, 1, None, &[1, 2, 3, 4]).unwrap();

        assert!(purge_verified_remote_asset_on(&conn, "source", "/book.cbz", &[]).is_err());
        assert!(cache::cover_cache_read("/book.cbz", 0, 1, 1, None).is_some());

        cache::set_custom_cache_root("");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn verified_cleanup_matches_source_id_literally() {
        let base = std::env::temp_dir().join(format!(
            "rch_verified_remote_asset_literal_source_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        cache::set_custom_cache_root(base.to_str().unwrap());
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE book_sources(id TEXT PRIMARY KEY,type TEXT NOT NULL,fingerprint TEXT NOT NULL,url TEXT,port INTEGER,path TEXT,client_id TEXT,root_id TEXT,cookie TEXT);\
             CREATE TABLE library_index(id TEXT PRIMARY KEY,source_id TEXT,parent_id TEXT,name TEXT,path TEXT,entry_type TEXT,size INTEGER,modified_at INTEGER,asset_kind TEXT,content_fingerprint TEXT,scan_generation INTEGER,listing_complete INTEGER,deleted INTEGER,updated_at INTEGER);",
        )
        .unwrap();
        crate::remote_scan::persistence::migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO book_sources VALUES('source%','webdav','canonical-source','https://host/dav',NULL,'/',NULL,NULL,NULL)",
            [],
        )
        .unwrap();
        crate::remote_scan::persistence::bind_scan_epoch(&conn, "source%", 2, "/", 2).unwrap();
        conn.execute(
            "INSERT INTO remote_scan_state(source_id,status,mode,generation) VALUES('source%','Succeeded','Snapshot',2)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO remote_listing_state VALUES('source%','/','root-v2',2,1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO library_index(id,source_id,parent_id,name,path,entry_type,scan_generation,listing_complete,deleted,updated_at)\
             VALUES(?1,'source%',?2,'gone.cbz','/gone.cbz','file',1,1,1,1)",
            params![
                crate::db::library_index_id("canonical-source", "/gone.cbz"),
                crate::db::library_index_id("canonical-source", "/"),
            ],
        )
        .unwrap();
        let unrelated_book_key = crate::db::book_key_of("webdav", "sourceX", "/other.cbz");
        conn.execute(
            "INSERT INTO remote_cover_dependency VALUES(?1,'/gone.cbz','other-v1','default','partial_ready')",
            [&unrelated_book_key],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO remote_cover_partial_cache VALUES(?1,'other-v1',X'0102',1)",
            [&unrelated_book_key],
        )
        .unwrap();

        purge_verified_remote_asset_on(&conn, "source%", "/gone.cbz", &[]).unwrap();

        let dependency_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM remote_cover_dependency WHERE book_key=?1",
                [&unrelated_book_key],
                |row| row.get(0),
            )
            .unwrap();
        let partial_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM remote_cover_partial_cache WHERE book_key=?1",
                [&unrelated_book_key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((dependency_count, partial_count), (1, 1));

        cache::set_custom_cache_root("");
        let _ = std::fs::remove_dir_all(base);
    }
}
