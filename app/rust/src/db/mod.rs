//! SQLite 数据库：应用全量数据持久化（ADR-013/018）。
//!
//! ## 表结构（7张）
//!
//! | 表 | 用途 |
//! |---|---|
//! | `book_sources` | 书源（本地/WebDAV） |
//! | `read_records` | 阅读记录（最近/最多/进度） |
//! | `book_metas` | 漫画元数据（封面/标签/简介/感想） |
//! | `tags` | 标签实体（独立于漫画，补全列表来源） |
//! | `book_tags` | 漫画-标签关联（多对多） |
//! | `app_settings` | 应用设置（key-value） |
//! | `schema_version` | 迁移版本标记 |
//!
//! ## 迁移策略
//!
//! Dart 启动时检查 `is_migrated()`，若未迁移则调用 `migrate_from_library_json()`，
//! 将 library.json 全量导入 SQLite。迁移完成后存量数据走 SQLite，library.json 保留为备份。
//!
//! ## 线程安全
//!
//! 全局单例 `Mutex<Connection>`。所有 pub 函数持有锁期间完成整个操作（读或写事务），
//! 调用方无需关心锁。

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::PathBuf;

static DB: std::sync::OnceLock<std::sync::Mutex<Connection>> = std::sync::OnceLock::new();

fn db_path() -> PathBuf {
    crate::cache::cache_root().join("database.db")
}

pub fn get() -> &'static std::sync::Mutex<Connection> {
    DB.get_or_init(|| {
        let conn = Connection::open(db_path()).expect("无法打开 SQLite 数据库");
        init_tables(&conn).expect("无法初始化 SQLite 表");
        std::sync::Mutex::new(conn)
    })
}

/// 重开数据库连接（应用根目录切换后调用）。
///
/// 连接惰性打开且绑定旧根目录的文件，切换根后必须重开，
/// 否则后续写入仍落在旧文件，且旧文件被占用无法删除。
/// 打开/初始化新库失败时保持旧连接不变，返回错误。
pub fn reopen_data_db() -> Result<()> {
    let new_conn = Connection::open(db_path()).context("无法打开 SQLite 数据库")?;
    init_tables(&new_conn)?;
    let mut guard = get().lock().unwrap();
    *guard = new_conn;
    Ok(())
}

/// 初始化所有表（幂等 CREATE TABLE IF NOT EXISTS）。
fn init_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        -- 缓存索引（已有，保留）
        CREATE TABLE IF NOT EXISTS cache_index (
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

        -- 书源能力（已有，保留）
        CREATE TABLE IF NOT EXISTS source_capability (
            source_id TEXT PRIMARY KEY,
            range_supported INTEGER DEFAULT 1,
            avg_rtt_ms REAL DEFAULT 0.0,
            max_concurrency INTEGER DEFAULT 2,
            updated_at INTEGER
        );

        -- 书源
        CREATE TABLE IF NOT EXISTS book_sources (
            id TEXT PRIMARY KEY,
            type TEXT NOT NULL,
            name TEXT NOT NULL,
            path TEXT NOT NULL DEFAULT '',
            url TEXT,
            username TEXT,
            password TEXT,
            note TEXT NOT NULL DEFAULT '',
            capability_label TEXT NOT NULL DEFAULT ''
        );

        -- 阅读记录
        CREATE TABLE IF NOT EXISTS read_records (
            key TEXT PRIMARY KEY,
            source_id TEXT NOT NULL,
            source_type TEXT NOT NULL,
            path TEXT NOT NULL,
            title TEXT NOT NULL,
            last_page INTEGER NOT NULL DEFAULT 0,
            read_count INTEGER NOT NULL DEFAULT 0,
            last_read_at INTEGER NOT NULL DEFAULT 0
        );

        -- 漫画元数据
        CREATE TABLE IF NOT EXISTS book_metas (
            key TEXT PRIMARY KEY,
            cover_page INTEGER NOT NULL DEFAULT 0,
            crop_x REAL,
            crop_y REAL,
            crop_w REAL,
            crop_h REAL,
            author TEXT NOT NULL DEFAULT '',
            genre TEXT NOT NULL DEFAULT '',
            series TEXT NOT NULL DEFAULT '',
            title TEXT NOT NULL DEFAULT '',
            chinese_title TEXT NOT NULL DEFAULT '',
            summary TEXT NOT NULL DEFAULT '',
            comment TEXT NOT NULL DEFAULT '',
            rotations TEXT NOT NULL DEFAULT '{}'
        );

        -- 标签实体
        CREATE TABLE IF NOT EXISTS tags (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            created_at INTEGER NOT NULL DEFAULT 0
        );

        -- 漫画-标签关联
        CREATE TABLE IF NOT EXISTS book_tags (
            book_key TEXT NOT NULL,
            tag_id TEXT NOT NULL,
            PRIMARY KEY (book_key, tag_id),
            FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE
        );

        -- 应用设置
        CREATE TABLE IF NOT EXISTS app_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        -- AI 超分后台任务队列（可跨重启续跑）
        CREATE TABLE IF NOT EXISTS ai_tasks (
            id TEXT PRIMARY KEY,
            book_key TEXT NOT NULL,
            source_type TEXT NOT NULL,
            source_id TEXT NOT NULL,
            path TEXT NOT NULL,
            title TEXT NOT NULL,
            scale INTEGER NOT NULL DEFAULT 2,
            total INTEGER NOT NULL DEFAULT 0,
            done INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL DEFAULT 'queued',
            sort_order INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        -- schema 版本
        CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            migrated_at INTEGER NOT NULL
        );

        -- 索引
        CREATE INDEX IF NOT EXISTS idx_cache_source ON cache_index(source_type, source_id);
        CREATE INDEX IF NOT EXISTS idx_book_tags_tag ON book_tags(tag_id);
        CREATE INDEX IF NOT EXISTS idx_book_tags_book ON book_tags(book_key);
        ",
    )?;
    // 旧库升级：补 rotations 列（每页旋转，JSON 文本，如 {"0":90}）。
    let meta_cols: Vec<String> = conn
        .prepare("PRAGMA table_info(book_metas)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|c| c.ok())
        .collect();
    if !meta_cols.iter().any(|c| c == "rotations") {
        conn.execute(
            "ALTER TABLE book_metas ADD COLUMN rotations TEXT NOT NULL DEFAULT '{}'",
            [],
        )?;
    }
    // 旧库升级：ai_tasks 补 sort_order 列（排队任务拖拽排序，重启后保持）。
    let task_cols: Vec<String> = conn
        .prepare("PRAGMA table_info(ai_tasks)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|c| c.ok())
        .collect();
    if !task_cols.iter().any(|c| c == "sort_order") {
        conn.execute(
            "ALTER TABLE ai_tasks ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    // 自愈：旧版 hash ID 标签 → 名字 ID（幂等，每次打开都执行）。
    normalize_legacy_tag_ids(conn)?;
    Ok(())
}

/// 归一化旧版 hash ID 标签。
///
/// 历史数据中 tags 行可能是 `id = 旧hash, name = 日漫`（id 与 tag_id(name) 不一致）。
/// 新代码按名字计算 ID（`name.trim().to_lowercase()`），按新 ID 查不到时会 INSERT，
/// 撞上 `tags.name UNIQUE` 导致 `UNIQUE constraint failed: tags.name`。
///
/// 这里把旧行迁移到名字 ID：确保新 ID 行存在 → 迁移 book_tags 关联 → 删除旧行。
/// 幂等，可重复执行。
fn normalize_legacy_tag_ids(conn: &Connection) -> Result<()> {
    let legacy: Vec<(String, String, i64)> = conn
        .prepare("SELECT id, name, created_at FROM tags")?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .filter(|(id, name, _)| {
            let t = name.trim();
            !t.is_empty() && *id != tag_id(t)
        })
        .collect();

    for (old_id, name, created_at) in legacy {
        let new_id = tag_id(&name);
        // 原地改 id（name 唯一约束下不可能存在同 name 的新 id 行，不会冲突）
        conn.execute(
            "UPDATE tags SET id = ?1, name = ?2, created_at = ?3 WHERE id = ?4",
            params![new_id, name, created_at, old_id],
        )?;
        conn.execute(
            "UPDATE OR IGNORE book_tags SET tag_id = ?1 WHERE tag_id = ?2",
            params![new_id, old_id],
        )?;
    }
    Ok(())
}

// ============================================================
// 迁移
// ============================================================

/// 当前 schema 版本号。
const CURRENT_SCHEMA_VERSION: i64 = 2;

/// 检查数据是否已从 library.json 迁移到 SQLite。
pub fn is_migrated() -> bool {
    let conn = get().lock().unwrap();
    conn.query_row(
        "SELECT version FROM schema_version WHERE version = ?1",
        params![CURRENT_SCHEMA_VERSION],
        |row| row.get::<_, i64>(0),
    )
    .is_ok()
}

/// 从 library.json 全量导入 SQLite。
///
/// `json_path` 由 Dart 侧通过 path_provider 获取并传入。
/// 迁移完成后写入 schema_version 标记。
/// 此函数幂等：已经迁移过的数据不会重复插入。
pub fn migrate_from_library_json(json_path: &str) -> Result<()> {
    if is_migrated() {
        return Ok(());
    }

    let content =
        std::fs::read_to_string(json_path).context("无法读取 library.json，可能尚未创建")?;
    let j: serde_json::Value = serde_json::from_str(&content).context("library.json 格式错误")?;

    let conn = get().lock().unwrap();

    // 使用事务确保原子性
    conn.execute("BEGIN", [])?;

    let migrate_result = (|| -> Result<()> {
        // ---- book_sources ----
        if let Some(sources) = j.get("sources").and_then(|v| v.as_array()) {
            for s in sources {
                let id = s["id"].as_str().unwrap_or("");
                if id.is_empty() {
                    continue;
                }
                conn.execute(
                    "INSERT OR IGNORE INTO book_sources
                     (id, type, name, path, url, username, password, note, capability_label)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        id,
                        s["type"].as_str().unwrap_or("local"),
                        s["name"].as_str().unwrap_or(""),
                        s["path"].as_str().unwrap_or(""),
                        s["url"].as_str(),
                        s["username"].as_str(),
                        s["password"].as_str(),
                        s["note"].as_str().unwrap_or(""),
                        s["capabilityLabel"].as_str().unwrap_or(""),
                    ],
                )?;
            }
        }

        // ---- read_records ----
        if let Some(records) = j.get("records").and_then(|v| v.as_object()) {
            for (key, r) in records {
                conn.execute(
                    "INSERT OR IGNORE INTO read_records
                     (key, source_id, source_type, path, title, last_page, read_count, last_read_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        key,
                        r["sourceId"].as_str().unwrap_or(""),
                        r["sourceType"].as_str().unwrap_or(""),
                        r["path"].as_str().unwrap_or(""),
                        r["title"].as_str().unwrap_or(""),
                        r["lastPage"].as_i64().unwrap_or(0),
                        r["readCount"].as_i64().unwrap_or(0),
                        r["lastReadAt"].as_i64().unwrap_or(0),
                    ],
                )?;
            }
        }

        // ---- book_metas ----
        if let Some(metas) = j.get("metas").and_then(|v| v.as_object()) {
            for (key, m) in metas {
                let rotations = m
                    .get("rotations")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "{}".to_string());
                conn.execute(
                    "INSERT OR IGNORE INTO book_metas
                     (key, cover_page, crop_x, crop_y, crop_w, crop_h,
                      author, genre, series, title, chinese_title, summary, comment, rotations)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                    params![
                        key,
                        m["coverPage"].as_i64().unwrap_or(0),
                        m["cropX"].as_f64(),
                        m["cropY"].as_f64(),
                        m["cropW"].as_f64(),
                        m["cropH"].as_f64(),
                        m["author"].as_str().unwrap_or(""),
                        m["genre"].as_str().unwrap_or(""),
                        m["series"].as_str().unwrap_or(""),
                        m["title"].as_str().unwrap_or(""),
                        m["chineseTitle"].as_str().unwrap_or(""),
                        m["summary"].as_str().unwrap_or(""),
                        m["comment"].as_str().unwrap_or(""),
                        rotations,
                    ],
                )?;
            }
        }

        // ---- tags ----
        if let Some(tags) = j.get("tags").and_then(|v| v.as_array()) {
            for t in tags {
                let id = t["id"].as_str().unwrap_or("");
                if id.is_empty() {
                    continue;
                }
                conn.execute(
                    "INSERT OR IGNORE INTO tags (id, name, created_at) VALUES (?1, ?2, ?3)",
                    params![
                        id,
                        t["name"].as_str().unwrap_or(""),
                        t["createdAt"].as_i64().unwrap_or(0),
                    ],
                )?;
            }
        }

        // ---- book_tags ----
        if let Some(book_tags) = j.get("book_tags").and_then(|v| v.as_array()) {
            for bt in book_tags {
                let book_key = bt["bookKey"].as_str().unwrap_or("");
                let tag_id = bt["tagId"].as_str().unwrap_or("");
                if book_key.is_empty() || tag_id.is_empty() {
                    continue;
                }
                conn.execute(
                    "INSERT OR IGNORE INTO book_tags (book_key, tag_id) VALUES (?1, ?2)",
                    params![book_key, tag_id],
                )?;
            }
        }

        // ---- app_settings ----
        if let Some(settings) = j.get("settings").and_then(|v| v.as_object()) {
            for (key, val) in settings {
                let v_str = match val {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                conn.execute(
                    "INSERT OR IGNORE INTO app_settings (key, value) VALUES (?1, ?2)",
                    params![key, v_str],
                )?;
            }
        }

        // ---- 标记迁移完成 ----
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        conn.execute(
            "INSERT OR REPLACE INTO schema_version (version, migrated_at) VALUES (?1, ?2)",
            params![CURRENT_SCHEMA_VERSION, now],
        )?;

        Ok(())
    })();

    match migrate_result {
        Ok(()) => {
            conn.execute("COMMIT", [])?;
            tracing::info!("library.json → SQLite 迁移完成");
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", []);
            Err(e)
        }
    }
}

// ============================================================
// book_sources CRUD
// ============================================================

/// 书源 DTO。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BookSourceRow {
    pub id: String,
    pub r#type: String,
    pub name: String,
    pub path: String,
    pub url: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub note: String,
    pub capability_label: String,
}

pub fn load_all_sources() -> Vec<BookSourceRow> {
    let conn = get().lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT id, type, name, path, url, username, password, note, capability_label FROM book_sources")
        .unwrap();
    stmt.query_map([], |row| {
        Ok(BookSourceRow {
            id: row.get(0)?,
            r#type: row.get(1)?,
            name: row.get(2)?,
            path: row.get(3)?,
            url: row.get(4)?,
            username: row.get(5)?,
            password: row.get(6)?,
            note: row.get(7)?,
            capability_label: row.get(8)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn upsert_source(s: &BookSourceRow) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO book_sources
         (id, type, name, path, url, username, password, note, capability_label)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            s.id,
            s.r#type,
            s.name,
            s.path,
            s.url,
            s.username,
            s.password,
            s.note,
            s.capability_label,
        ],
    )?;
    Ok(())
}

pub fn delete_source(id: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute("DELETE FROM book_sources WHERE id = ?1", params![id])?;
    Ok(())
}

// ============================================================
// read_records CRUD
// ============================================================

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReadRecordRow {
    pub key: String,
    pub source_id: String,
    pub source_type: String,
    pub path: String,
    pub title: String,
    pub last_page: i32,
    pub read_count: i32,
    pub last_read_at: i64,
}

pub fn load_all_records() -> Vec<ReadRecordRow> {
    let conn = get().lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT key, source_id, source_type, path, title, last_page, read_count, last_read_at
             FROM read_records",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok(ReadRecordRow {
            key: row.get(0)?,
            source_id: row.get(1)?,
            source_type: row.get(2)?,
            path: row.get(3)?,
            title: row.get(4)?,
            last_page: row.get(5)?,
            read_count: row.get(6)?,
            last_read_at: row.get(7)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn upsert_record(r: &ReadRecordRow) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO read_records
         (key, source_id, source_type, path, title, last_page, read_count, last_read_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            r.key,
            r.source_id,
            r.source_type,
            r.path,
            r.title,
            r.last_page,
            r.read_count,
            r.last_read_at,
        ],
    )?;
    Ok(())
}

pub fn delete_record(key: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute("DELETE FROM read_records WHERE key = ?1", params![key])?;
    Ok(())
}

pub fn delete_records_by_source_prefix(prefix: &str) -> Result<u32> {
    let conn = get().lock().unwrap();
    let n = conn.execute(
        "DELETE FROM read_records WHERE key LIKE ?1",
        params![format!("{prefix}%")],
    )?;
    Ok(n as u32)
}

// ============================================================
// book_metas CRUD
// ============================================================

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BookMetaRow {
    pub key: String,
    pub cover_page: i32,
    pub crop_x: Option<f64>,
    pub crop_y: Option<f64>,
    pub crop_w: Option<f64>,
    pub crop_h: Option<f64>,
    pub author: String,
    pub genre: String,
    pub series: String,
    pub title: String,
    pub chinese_title: String,
    pub summary: String,
    pub comment: String,
    pub rotations: String,
}

pub fn load_all_metas() -> Vec<BookMetaRow> {
    let conn = get().lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT key, cover_page, crop_x, crop_y, crop_w, crop_h,
                    author, genre, series, title, chinese_title, summary, comment,
                    rotations
             FROM book_metas",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok(BookMetaRow {
            key: row.get(0)?,
            cover_page: row.get(1)?,
            crop_x: row.get(2)?,
            crop_y: row.get(3)?,
            crop_w: row.get(4)?,
            crop_h: row.get(5)?,
            author: row.get(6)?,
            genre: row.get(7)?,
            series: row.get(8)?,
            title: row.get(9)?,
            chinese_title: row.get(10)?,
            summary: row.get(11)?,
            comment: row.get(12)?,
            rotations: row.get(13)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn upsert_meta(m: &BookMetaRow) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO book_metas
         (key, cover_page, crop_x, crop_y, crop_w, crop_h,
          author, genre, series, title, chinese_title, summary, comment, rotations)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            m.key,
            m.cover_page,
            m.crop_x,
            m.crop_y,
            m.crop_w,
            m.crop_h,
            m.author,
            m.genre,
            m.series,
            m.title,
            m.chinese_title,
            m.summary,
            m.comment,
            m.rotations,
        ],
    )?;
    Ok(())
}

pub fn delete_meta(key: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute("DELETE FROM book_metas WHERE key = ?1", params![key])?;
    Ok(())
}

pub fn delete_metas_by_source_prefix(prefix: &str) -> Result<u32> {
    let conn = get().lock().unwrap();
    let n = conn.execute(
        "DELETE FROM book_metas WHERE key LIKE ?1",
        params![format!("{prefix}%")],
    )?;
    Ok(n as u32)
}

// ============================================================
// tags CRUD（ADR-017：独立标签实体）
// ============================================================

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TagRow {
    pub id: String,
    pub name: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BookTagRow {
    pub book_key: String,
    pub tag_id: String,
}

pub fn load_all_tags() -> Vec<TagRow> {
    let conn = get().lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT id, name, created_at FROM tags ORDER BY name")
        .unwrap();
    stmt.query_map([], |row| {
        Ok(TagRow {
            id: row.get(0)?,
            name: row.get(1)?,
            created_at: row.get(2)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn load_all_book_tags() -> Vec<BookTagRow> {
    let conn = get().lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT book_key, tag_id FROM book_tags")
        .unwrap();
    stmt.query_map([], |row| {
        Ok(BookTagRow {
            book_key: row.get(0)?,
            tag_id: row.get(1)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

/// 确保标签存在（幂等），返回标签行。
/// 若标签已存在则直接返回；否则创建并返回。
pub fn ensure_tag(name: &str) -> Result<TagRow> {
    let conn = get().lock().unwrap();
    let id = tag_id(name);

    // 先查是否存在
    let existing: Option<TagRow> = conn
        .query_row(
            "SELECT id, name, created_at FROM tags WHERE id = ?1",
            params![id],
            |row| {
                Ok(TagRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                })
            },
        )
        .ok();

    if let Some(tag) = existing {
        return Ok(tag);
    }

    // 兼容旧数据：同 name 的旧 hash ID 行存在时，原地迁移到新 ID 再返回。
    let by_name: Option<TagRow> = conn
        .query_row(
            "SELECT id, name, created_at FROM tags WHERE name = ?1",
            params![name],
            |row| {
                Ok(TagRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                })
            },
        )
        .ok();
    if let Some(tag) = by_name {
        if tag.id != id {
            conn.execute(
                "UPDATE tags SET id = ?1, name = ?2, created_at = ?3 WHERE id = ?4",
                params![id, name, tag.created_at, tag.id],
            )?;
            conn.execute(
                "UPDATE OR IGNORE book_tags SET tag_id = ?1 WHERE tag_id = ?2",
                params![id, tag.id],
            )?;
            return Ok(TagRow {
                id,
                name: name.to_string(),
                created_at: tag.created_at,
            });
        }
        return Ok(tag);
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    conn.execute(
        "INSERT INTO tags (id, name, created_at) VALUES (?1, ?2, ?3)",
        params![id, name, now],
    )?;
    Ok(TagRow {
        id,
        name: name.to_string(),
        created_at: now,
    })
}

/// 重命名标签（同时更新所有关联 — 通过 UPDATE tag_id 实现）。
/// 旧名不存在时静默返回。
pub fn rename_tag(old_name: &str, new_name: &str) -> Result<()> {
    if old_name == new_name || new_name.is_empty() {
        return Ok(());
    }
    let conn = get().lock().unwrap();
    let old_id = tag_id(old_name);
    let new_id = tag_id(new_name);

    // 检查旧标签是否存在
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM tags WHERE id = ?1",
            params![old_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);

    if !exists {
        return Ok(());
    }

    // 事务内完成：创建新标签 → 迁移关联 → 删除旧标签
    conn.execute("BEGIN", [])?;
    let result = (|| -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        // 创建新标签（或忽略已存在）
        conn.execute(
            "INSERT OR IGNORE INTO tags (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![new_id, new_name, now],
        )?;
        // 迁移所有 book_tags 关联到新 id
        conn.execute(
            "UPDATE OR IGNORE book_tags SET tag_id = ?1 WHERE tag_id = ?2",
            params![new_id, old_id],
        )?;
        // 清理可能残留的旧关联
        conn.execute(
            "DELETE FROM book_tags WHERE tag_id = ?1",
            params![old_id],
        )?;
        // 删除旧标签
        conn.execute("DELETE FROM tags WHERE id = ?1", params![old_id])?;
        Ok(())
    })();

    match result {
        Ok(()) => {
            conn.execute("COMMIT", [])?;
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", []);
            return Err(e);
        }
    }
    Ok(())
}

/// 删除标签及其所有关联。
pub fn delete_tag(name: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    let id = tag_id(name);
    conn.execute("DELETE FROM book_tags WHERE tag_id = ?1", params![id])?;
    conn.execute("DELETE FROM tags WHERE id = ?1", params![id])?;
    Ok(())
}

/// 将标签关联到一本书（幂等）。
pub fn link_tag(book_key: &str, tag_name: &str) -> Result<()> {
    if tag_name.is_empty() {
        return Ok(());
    }
    let tag_id = ensure_tag(tag_name)?.id;
    let conn = get().lock().unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO book_tags (book_key, tag_id) VALUES (?1, ?2)",
        params![book_key, tag_id],
    )?;
    Ok(())
}

/// 将标签从一本书移除。
pub fn unlink_tag(book_key: &str, tag_name: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    let id = tag_id(tag_name);
    conn.execute(
        "DELETE FROM book_tags WHERE book_key = ?1 AND tag_id = ?2",
        params![book_key, id],
    )?;
    Ok(())
}

/// 设置一本书的标签集（全量替换：先删后插）。
pub fn set_book_tags(book_key: &str, tag_names: &[String]) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute("BEGIN", [])?;
    let result = (|| -> Result<()> {
        conn.execute(
            "DELETE FROM book_tags WHERE book_key = ?1",
            params![book_key],
        )?;
        for name in tag_names {
            if name.is_empty() {
                continue;
            }
            // 确保标签存在
            let id = tag_id(name);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            conn.execute(
                "INSERT OR IGNORE INTO tags (id, name, created_at) VALUES (?1, ?2, ?3)",
                params![id, name, now],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO book_tags (book_key, tag_id) VALUES (?1, ?2)",
                params![book_key, id],
            )?;
        }
        Ok(())
    })();

    match result {
        Ok(()) => {
            conn.execute("COMMIT", [])?;
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", []);
            return Err(e);
        }
    }
    Ok(())
}

// ============================================================
// app_settings CRUD
// ============================================================

/// 设置条目 DTO。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SettingEntry {
    pub key: String,
    pub value: String,
}

pub fn load_all_settings() -> Vec<SettingEntry> {
    let conn = get().lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT key, value FROM app_settings")
        .unwrap();
    stmt.query_map([], |row| {
        Ok(SettingEntry {
            key: row.get(0)?,
            value: row.get(1)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn save_setting(key: &str, value: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO app_settings (key, value) VALUES (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

pub fn delete_setting(key: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute("DELETE FROM app_settings WHERE key = ?1", params![key])?;
    Ok(())
}

// ============================================================
// cache_index（已有 API，保留）
// ============================================================

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub remote_path: String,
    pub local_path: String,
    pub file_hash: Option<String>,
    pub file_size: Option<i64>,
}

pub fn find_cache(source_type: &str, source_id: &str, remote_path: &str) -> Option<CacheEntry> {
    let conn = get().lock().unwrap();
    conn.query_row(
        "SELECT remote_path, local_path, file_hash, file_size FROM cache_index
         WHERE source_type = ?1 AND source_id = ?2 AND remote_path = ?3",
        params![source_type, source_id, remote_path],
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
        params![source_type, source_id, remote_path, local_path, file_size, now],
    )?;
    Ok(())
}

pub fn remove_source_caches(source_type: &str, source_id: &str) -> Result<usize> {
    let conn = get().lock().unwrap();
    let n = conn.execute(
        "DELETE FROM cache_index WHERE source_type = ?1 AND source_id = ?2",
        params![source_type, source_id],
    )?;
    Ok(n)
}

pub fn total_cached_size() -> i64 {
    let conn = get().lock().unwrap();
    conn.query_row(
        "SELECT COALESCE(SUM(file_size), 0) FROM cache_index",
        [],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

// ============================================================
// 内部工具函数
// ============================================================

/// 标签名即 ID — 用标签名的小写 trim 作为主键。
///
/// 简单可靠，消除跨语言 Hash 不一致问题。
/// 标签名区分大小写显示，但 ID 统一用小写（方便去重）。
fn tag_id(name: &str) -> String {
    name.trim().to_lowercase()
}

#[derive(Debug, Clone)]
pub struct AiTaskRow {
    pub id: String,
    pub book_key: String,
    pub source_type: String,
    pub source_id: String,
    pub path: String,
    pub title: String,
    pub scale: i64,
    pub total: i64,
    pub done: i64,
    pub status: String,
    pub sort_order: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 写入/更新一条 AI 超分任务（INSERT OR REPLACE）。
pub fn upsert_ai_task(t: &AiTaskRow) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO ai_tasks
         (id, book_key, source_type, source_id, path, title, scale, total, done, status, sort_order, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            t.id, t.book_key, t.source_type, t.source_id, t.path, t.title,
            t.scale, t.total, t.done, t.status, t.sort_order, t.created_at, t.updated_at
        ],
    )?;
    Ok(())
}

/// 加载全部 AI 超分任务（进行中在前，排队按 sort_order、创建时间排序）。
pub fn load_all_ai_tasks() -> Vec<AiTaskRow> {
    let conn = get().lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT id, book_key, source_type, source_id, path, title, scale, total, done, status, sort_order, created_at, updated_at FROM ai_tasks ORDER BY (status = 'running') DESC, sort_order, created_at")
        .unwrap();
    stmt.query_map([], |row| {
        Ok(AiTaskRow {
            id: row.get(0)?,
            book_key: row.get(1)?,
            source_type: row.get(2)?,
            source_id: row.get(3)?,
            path: row.get(4)?,
            title: row.get(5)?,
            scale: row.get(6)?,
            total: row.get(7)?,
            done: row.get(8)?,
            status: row.get(9)?,
            sort_order: row.get(10)?,
            created_at: row.get(11)?,
            updated_at: row.get(12)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

/// 按给定顺序重排排队任务的 sort_order（1..N）。调用方应只传排队中任务的 id，
/// 进行中任务不受影响（保持在顶部）。
pub fn reorder_ai_tasks(ids: &[String]) -> Result<()> {
    let mut conn = get().lock().unwrap();
    reorder_ai_tasks_on(&mut conn, ids)
}

fn reorder_ai_tasks_on(conn: &mut Connection, ids: &[String]) -> Result<()> {
    let tx = conn.transaction()?;
    for (i, id) in ids.iter().enumerate() {
        tx.execute(
            "UPDATE ai_tasks SET sort_order = ?1 WHERE id = ?2",
            params![(i + 1) as i64, id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// 删除一条 AI 超分任务。
pub fn delete_ai_task(id: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute("DELETE FROM ai_tasks WHERE id = ?1", params![id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn memory_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE tags (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                created_at INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE book_tags (
                book_key TEXT NOT NULL,
                tag_id TEXT NOT NULL,
                PRIMARY KEY (book_key, tag_id)
            );
            ",
        )
        .unwrap();
        conn
    }

    #[test]
    fn normalize_legacy_tag_ids_migrates_hash_rows() {
        let conn = memory_conn();
        conn.execute(
            "INSERT INTO tags (id, name, created_at) VALUES ('2b6c5f54', '日漫', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO book_tags (book_key, tag_id) VALUES ('bk1', '2b6c5f54')",
            [],
        )
        .unwrap();

        normalize_legacy_tag_ids(&conn).unwrap();

        let id: String = conn.query_row("SELECT id FROM tags", [], |r| r.get(0)).unwrap();
        assert_eq!(id, "日漫");
        let links: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM book_tags WHERE tag_id = '日漫'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(links, 1);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM tags", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );

        // 幂等：再次执行不报错、不变化
        normalize_legacy_tag_ids(&conn).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM tags", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn init_tables_creates_ai_tasks() {
        let conn = Connection::open_in_memory().unwrap();
        init_tables(&conn).unwrap();
        conn.execute(
            "INSERT INTO ai_tasks (id, book_key, source_type, source_id, path, title, scale, total, done, status, created_at, updated_at)
             VALUES ('t1', 'bk', 'local', 's1', '/p', 'T', 2, 10, 3, 'queued', 1, 1)",
            [],
        )
        .unwrap();
        let row: (String, i64) = conn
            .query_row("SELECT id, done FROM ai_tasks", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(row, ("t1".to_string(), 3));
    }

    #[test]
    fn init_tables_creates_rotations_column() {
        let conn = Connection::open_in_memory().unwrap();
        init_tables(&conn).unwrap();
        conn.execute("INSERT INTO book_metas (key) VALUES ('k1')", [])
            .unwrap();
        let rotations: String = conn
            .query_row("SELECT rotations FROM book_metas WHERE key = 'k1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(rotations, "{}");
    }

    #[test]
    fn ai_tasks_sort_order_migration_reorder_and_ordering() -> Result<(), Box<dyn std::error::Error>> {
        let mut conn = Connection::open_in_memory().unwrap();
        // 模拟旧库：ai_tasks 无 sort_order 列
        conn.execute_batch(
            "CREATE TABLE ai_tasks (
                id TEXT PRIMARY KEY,
                book_key TEXT NOT NULL,
                source_type TEXT NOT NULL,
                source_id TEXT NOT NULL,
                path TEXT NOT NULL,
                title TEXT NOT NULL,
                scale INTEGER NOT NULL DEFAULT 2,
                total INTEGER NOT NULL DEFAULT 0,
                done INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL DEFAULT 'queued',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();
        init_tables(&conn).unwrap(); // 触发迁移补列
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(ai_tasks)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(|c| c.ok())
            .collect();
        assert!(cols.contains(&"sort_order".to_string()));

        // 旧行默认 sort_order = 0
        conn.execute(
            "INSERT INTO ai_tasks (id, book_key, source_type, source_id, path, title, created_at, updated_at)
             VALUES ('a', 'k', 'local', 's', 'p', 't', 1, 1)",
            [],
        )
        .unwrap();
        let so: i64 = conn
            .query_row("SELECT sort_order FROM ai_tasks WHERE id = 'a'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(so, 0);

        // reorder：只重排传入的排队任务
        for (id, status) in [("b", "queued"), ("c", "queued"), ("d", "running")] {
            conn.execute(
                "INSERT INTO ai_tasks (id, book_key, source_type, source_id, path, title, status, sort_order, created_at, updated_at)
                 VALUES (?1, 'k', 'local', 's', 'p', 't', ?2, 0, 1, 1)",
                rusqlite::params![id, status],
            )
            .unwrap();
        }
        reorder_ai_tasks_on(&mut conn, &["c".into(), "b".into()])?;
        let order: Vec<String> = conn
            .prepare("SELECT id FROM ai_tasks ORDER BY (status = 'running') DESC, sort_order, created_at")?
            .query_map([], |r| r.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        // running 固定在顶部；排队任务按 sort_order 升序；旧数据（sort_order=0）按创建时间兜底排前
        assert_eq!(order, vec!["d", "a", "c", "b"]);
        Ok(())
    }

}
