//! SQLite 数据库：漫画索引、缓存状态、书源能力。

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Mutex;

static DB: std::sync::OnceLock<Mutex<Connection>> = std::sync::OnceLock::new();

fn db_path() -> PathBuf {
    crate::cache::cache_root().join("database.db")
}

pub fn get() -> &'static Mutex<Connection> {
    DB.get_or_init(|| {
        let conn = Connection::open(db_path()).expect("无法打开 SQLite 数据库");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cache_index (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                source_type TEXT NOT NULL,
                source_id TEXT NOT NULL,
                remote_path TEXT NOT NULL,
                local_path TEXT NOT NULL,
                file_hash TEXT,
                file_size INTEGER,
                etag TEXT,
                downloaded_at INTEGER,
                UNIQUE(source_type, source_id, remote_path)
            );
            CREATE TABLE IF NOT EXISTS source_capability (
                source_id TEXT PRIMARY KEY,
                range_supported INTEGER DEFAULT 1,
                avg_rtt_ms REAL DEFAULT 0.0,
                max_concurrency INTEGER DEFAULT 2,
                updated_at INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_cache_source ON cache_index(source_type, source_id);",
        )
        .expect("无法初始化 SQLite 表");
        Mutex::new(conn)
    })
}

/// 缓存条目。
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub remote_path: String,
    pub local_path: String,
    pub file_hash: Option<String>,
    pub file_size: Option<i64>,
}

/// 查询缓存：根据来源和远程路径查找本地缓存。
pub fn find_cache(source_type: &str, source_id: &str, remote_path: &str) -> Option<CacheEntry> {
    let conn = get().lock().unwrap();
    conn.query_row(
        "SELECT remote_path, local_path, file_hash, file_size FROM cache_index
         WHERE source_type = ?1 AND source_id = ?2 AND remote_path = ?3",
        rusqlite::params![source_type, source_id, remote_path],
        |row| {
            Ok(CacheEntry {
                remote_path: row.get(0)?,
                local_path: row.get(1)?,
                file_hash: row.get(2)?,
                file_size: row.get(3)?,
            })
        },
    )
    .ok()
}

/// 注册缓存条目（下载完成后调用）。
pub fn register_cache(
    source_type: &str,
    source_id: &str,
    remote_path: &str,
    local_path: &str,
    file_size: i64,
) -> Result<()> {
    let conn = get().lock().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    conn.execute(
        "INSERT OR REPLACE INTO cache_index
         (source_type, source_id, remote_path, local_path, file_size, downloaded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![source_type, source_id, remote_path, local_path, file_size, now],
    )?;
    Ok(())
}

/// 删除来源相关的所有缓存记录。
pub fn remove_source_caches(source_type: &str, source_id: &str) -> Result<usize> {
    let conn = get().lock().unwrap();
    let n = conn.execute(
        "DELETE FROM cache_index WHERE source_type = ?1 AND source_id = ?2",
        rusqlite::params![source_type, source_id],
    )?;
    Ok(n)
}

/// 获取缓存总文件大小（字节）。
pub fn total_cached_size() -> i64 {
    let conn = get().lock().unwrap();
    conn.query_row(
        "SELECT COALESCE(SUM(file_size), 0) FROM cache_index",
        [],
        |row| row.get(0),
    )
    .unwrap_or(0)
}
