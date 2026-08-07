//! SQLite 数据库：应用全量数据持久化（ADR-013/018）。
//!
//! ## 表结构（10张）
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
//! | `devices` | 同步：设备注册表（含本机） |
//! | `sync_state` | 同步：device_id / 游标 / 传输配置 |
//! | `source_alias` | 同步：书源 fingerprint ↔ 本地 source_id 映射 |
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

/// 以指定路径打开数据库并替换全局连接（导出/导入工具用）。
pub fn open_at(path: &str) -> Result<()> {
    let new_conn = Connection::open(path).context("无法打开 SQLite 数据库")?;
    init_tables(&new_conn)?;
    let mut guard = get().lock().unwrap();
    *guard = new_conn;
    Ok(())
}

/// 书源凭据行（仅含敏感字段，用于加密导出/导入）。
#[derive(Debug, Clone)]
pub struct SourceCredentialRow {
    pub id: String,
    pub fingerprint: String,
    pub r#type: String,
    pub name: String,
    pub root_id: Option<String>,
    pub password: Option<String>,
    pub refresh_token: Option<String>,
    pub client_secret: Option<String>,
    pub cookie: Option<String>,
}

/// 读取所有含 fingerprint 且未删除书源的凭据（加密导出用）。
pub fn load_source_credentials(conn: &Connection) -> Result<Vec<SourceCredentialRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, fingerprint, type, name, root_id, password, refresh_token, client_secret, cookie
         FROM book_sources
         WHERE deleted = 0
           AND (fingerprint IS NOT NULL AND fingerprint != ''
                OR (password IS NOT NULL AND password != '')
                OR (refresh_token IS NOT NULL AND refresh_token != '')
                OR (client_secret IS NOT NULL AND client_secret != '')
                OR (cookie IS NOT NULL AND cookie != ''))",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(SourceCredentialRow {
            id: row.get(0)?,
            fingerprint: row.get(1)?,
            r#type: row.get(2)?,
            name: row.get(3)?,
            root_id: row.get(4)?,
            password: row.get(5)?,
            refresh_token: row.get(6)?,
            client_secret: row.get(7)?,
            cookie: row.get(8)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 按 fingerprint 更新书源凭据（加密导入用），返回更新的行数。
pub fn update_source_credentials_by_fingerprint(
    conn: &Connection,
    fingerprint: &str,
    password: Option<&str>,
    refresh_token: Option<&str>,
    client_secret: Option<&str>,
    cookie: Option<&str>,
) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE book_sources SET password = ?1, refresh_token = ?2, client_secret = ?3, cookie = ?4
         WHERE fingerprint = ?5",
        params![password, refresh_token, client_secret, cookie, fingerprint],
    )?)
}

/// 初始化所有表（幂等 CREATE TABLE IF NOT EXISTS）。
pub(crate) fn init_tables(conn: &Connection) -> Result<()> {
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
            port INTEGER,
            note TEXT NOT NULL DEFAULT '',
            capability_label TEXT NOT NULL DEFAULT '',
            fingerprint TEXT,
            remote_only INTEGER NOT NULL DEFAULT 0,
            origin_device_id TEXT,
            updated_at INTEGER NOT NULL DEFAULT 0,
            deleted INTEGER NOT NULL DEFAULT 0
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
            last_read_at INTEGER NOT NULL DEFAULT 0,
            stable_id TEXT,
            updated_at INTEGER NOT NULL DEFAULT 0,
            deleted INTEGER NOT NULL DEFAULT 0
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
            rotations TEXT NOT NULL DEFAULT '{}',
            stable_id TEXT,
            updated_at INTEGER NOT NULL DEFAULT 0,
            deleted INTEGER NOT NULL DEFAULT 0
        );

        -- 标签实体
        CREATE TABLE IF NOT EXISTS tags (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            created_at INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0,
            deleted INTEGER NOT NULL DEFAULT 0
        );

        -- 漫画-标签关联
        CREATE TABLE IF NOT EXISTS book_tags (
            book_key TEXT NOT NULL,
            tag_id TEXT NOT NULL,
            updated_at INTEGER NOT NULL DEFAULT 0,
            deleted INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (book_key, tag_id),
            FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE
        );

        -- 应用设置
        CREATE TABLE IF NOT EXISTS app_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at INTEGER NOT NULL DEFAULT 0,
            deleted INTEGER NOT NULL DEFAULT 0
        );

        -- 同步：设备注册表
        CREATE TABLE IF NOT EXISTS devices (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL
        );

        -- 同步：设备本地状态（device_id / 游标 / 传输配置）
        CREATE TABLE IF NOT EXISTS sync_state (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );

        -- 同步：书源 fingerprint ↔ 本地 source_id 映射
        CREATE TABLE IF NOT EXISTS source_alias (
            source_id TEXT PRIMARY KEY,
            fingerprint TEXT NOT NULL,
            device_id TEXT NOT NULL DEFAULT '',
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (source_id) REFERENCES book_sources(id) ON DELETE CASCADE
        );

        -- 同步：墓碑（本地删除传播用，P3）
        CREATE TABLE IF NOT EXISTS sync_tombstones (
            entity TEXT NOT NULL,
            key TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (entity, key)
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
    // 旧库升级：book_sources 补 port 列（SFTP 书源端口，默认 22）。
    let src_cols: Vec<String> = conn
        .prepare("PRAGMA table_info(book_sources)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|c| c.ok())
        .collect();
    if !src_cols.iter().any(|c| c == "port") {
        conn.execute("ALTER TABLE book_sources ADD COLUMN port INTEGER", [])?;
    }
    // 旧库升级：book_sources 补网盘书源列（refresh_token / client_id / client_secret / root_id）。
    for (col, ddl) in [
        ("refresh_token", "ALTER TABLE book_sources ADD COLUMN refresh_token TEXT"),
        ("client_id", "ALTER TABLE book_sources ADD COLUMN client_id TEXT"),
        ("client_secret", "ALTER TABLE book_sources ADD COLUMN client_secret TEXT"),
        ("root_id", "ALTER TABLE book_sources ADD COLUMN root_id TEXT"),
        ("cookie", "ALTER TABLE book_sources ADD COLUMN cookie TEXT"),
    ] {
        if !src_cols.iter().any(|c| c == col) {
            conn.execute(ddl, [])?;
        }
    }
    // 同步就绪：同步实体表补同步元数据列（幂等，老库自动升级）。
    ensure_columns(
        conn,
        "book_sources",
        &[
            ("fingerprint", "fingerprint TEXT"),
            ("remote_only", "remote_only INTEGER NOT NULL DEFAULT 0"),
            ("origin_device_id", "origin_device_id TEXT"),
            ("updated_at", "updated_at INTEGER NOT NULL DEFAULT 0"),
            ("deleted", "deleted INTEGER NOT NULL DEFAULT 0"),
        ],
    )?;
    ensure_columns(
        conn,
        "read_records",
        &[
            ("stable_id", "stable_id TEXT"),
            ("updated_at", "updated_at INTEGER NOT NULL DEFAULT 0"),
            ("deleted", "deleted INTEGER NOT NULL DEFAULT 0"),
        ],
    )?;
    ensure_columns(
        conn,
        "book_metas",
        &[
            ("stable_id", "stable_id TEXT"),
            ("updated_at", "updated_at INTEGER NOT NULL DEFAULT 0"),
            ("deleted", "deleted INTEGER NOT NULL DEFAULT 0"),
        ],
    )?;
    for t in ["tags", "book_tags", "app_settings"] {
        ensure_columns(
            conn,
            t,
            &[
                ("updated_at", "updated_at INTEGER NOT NULL DEFAULT 0"),
                ("deleted", "deleted INTEGER NOT NULL DEFAULT 0"),
            ],
        )?;
    }
    // 同步索引依赖新列，必须在补列之后创建（老库先补列再建索引）。
    conn.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_sources_fingerprint ON book_sources(fingerprint);
        CREATE INDEX IF NOT EXISTS idx_metas_stable_id ON book_metas(stable_id);
        CREATE INDEX IF NOT EXISTS idx_records_stable_id ON read_records(stable_id);
        CREATE INDEX IF NOT EXISTS idx_source_alias_fp ON source_alias(fingerprint);
        ",
    )?;
    // 自愈：旧版 hash ID 标签 → 名字 ID（幂等，每次打开都执行）。
    normalize_legacy_tag_ids(conn)?;
    Ok(())
}

/// 幂等补列：查询现有列名，缺失列执行 `ALTER TABLE ADD COLUMN`。
///
/// 表名与 DDL 均为硬编码常量，无注入面。沿用既有 rotations/port 升级模式，
/// 使老库在打开时自动补齐同步元数据列。
fn ensure_columns(conn: &Connection, table: &str, cols: &[(&str, &str)]) -> Result<()> {
    let existing: Vec<String> = conn
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|c| c.ok())
        .collect();
    for (col, ddl) in cols {
        if !existing.iter().any(|c| c == *col) {
            conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {ddl}"), [])?;
        }
    }
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
                     (id, type, name, path, url, username, password, port, note, capability_label)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        id,
                        s["type"].as_str().unwrap_or("local"),
                        s["name"].as_str().unwrap_or(""),
                        s["path"].as_str().unwrap_or(""),
                        s["url"].as_str(),
                        s["username"].as_str(),
                        s["password"].as_str(),
                        s["port"].as_i64(),
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
    pub port: Option<i64>,
    pub refresh_token: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub root_id: Option<String>,
    pub cookie: Option<String>,
    pub note: String,
    pub capability_label: String,
    pub remote_only: bool,
    pub origin_device_id: Option<String>,
}

pub fn load_all_sources() -> Vec<BookSourceRow> {
    let conn = get().lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT id, type, name, path, url, username, password, port, refresh_token, client_id, client_secret, root_id, cookie, note, capability_label, remote_only, origin_device_id FROM book_sources")
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
            port: row.get(7)?,
            refresh_token: row.get(8)?,
            client_id: row.get(9)?,
            client_secret: row.get(10)?,
            root_id: row.get(11)?,
            cookie: row.get(12)?,
            note: row.get(13)?,
            capability_label: row.get(14)?,
            remote_only: row.get::<_, i64>(15)? != 0,
            origin_device_id: row.get(16)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

fn upsert_source_on(conn: &Connection, s: &BookSourceRow) -> Result<()> {
    conn.execute(
        "INSERT INTO book_sources
         (id, type, name, path, url, username, password, port, refresh_token, client_id, client_secret, root_id, cookie, note, capability_label, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT(id) DO UPDATE SET
            type=excluded.type, name=excluded.name, path=excluded.path, url=excluded.url,
            username=excluded.username, password=excluded.password, port=excluded.port,
            refresh_token=excluded.refresh_token, client_id=excluded.client_id,
            client_secret=excluded.client_secret, root_id=excluded.root_id, cookie=excluded.cookie,
            note=excluded.note, capability_label=excluded.capability_label, updated_at=excluded.updated_at",
        params![
            s.id,
            s.r#type,
            s.name,
            s.path,
            s.url,
            s.username,
            s.password,
            s.port,
            s.refresh_token,
            s.client_id,
            s.client_secret,
            s.root_id,
            s.cookie,
            s.note,
            s.capability_label,
            now_ms(),
        ],
    )?;
    Ok(())
}

pub fn upsert_source(s: &BookSourceRow) -> Result<()> {
    let conn = get().lock().unwrap();
    upsert_source_on(&conn, s)
}

pub fn delete_source(id: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute("DELETE FROM source_alias WHERE source_id = ?1", params![id])?;
    conn.execute("DELETE FROM book_sources WHERE id = ?1", params![id])?;
    upsert_tombstone_on(&conn, "sources", id)?;
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

fn upsert_record_on(conn: &Connection, r: &ReadRecordRow) -> Result<()> {
    conn.execute(
        "INSERT INTO read_records
         (key, source_id, source_type, path, title, last_page, read_count, last_read_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(key) DO UPDATE SET
            source_id=excluded.source_id, source_type=excluded.source_type,
            path=excluded.path, title=excluded.title, last_page=excluded.last_page,
            read_count=excluded.read_count, last_read_at=excluded.last_read_at,
            updated_at=excluded.updated_at",
        params![
            r.key,
            r.source_id,
            r.source_type,
            r.path,
            r.title,
            r.last_page,
            r.read_count,
            r.last_read_at,
            now_ms(),
        ],
    )?;
    Ok(())
}

pub fn upsert_record(r: &ReadRecordRow) -> Result<()> {
    let conn = get().lock().unwrap();
    upsert_record_on(&conn, r)
}

pub fn delete_record(key: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute("DELETE FROM read_records WHERE key = ?1", params![key])?;
    upsert_tombstone_on(&conn, "records", key)?;
    Ok(())
}

pub fn delete_records_by_source_prefix(prefix: &str) -> Result<u32> {
    let conn = get().lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT key FROM read_records WHERE key LIKE ?1")
        .unwrap();
    let keys: Vec<String> = stmt
        .query_map([format!("{prefix}%")], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    let n = conn.execute(
        "DELETE FROM read_records WHERE key LIKE ?1",
        params![format!("{prefix}%")],
    )?;
    for k in &keys {
        upsert_tombstone_on(&conn, "records", k)?;
    }
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

fn upsert_meta_on(conn: &Connection, m: &BookMetaRow) -> Result<()> {
    conn.execute(
        "INSERT INTO book_metas
         (key, cover_page, crop_x, crop_y, crop_w, crop_h,
          author, genre, series, title, chinese_title, summary, comment, rotations, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT(key) DO UPDATE SET
            cover_page=excluded.cover_page, crop_x=excluded.crop_x, crop_y=excluded.crop_y,
            crop_w=excluded.crop_w, crop_h=excluded.crop_h, author=excluded.author,
            genre=excluded.genre, series=excluded.series, title=excluded.title,
            chinese_title=excluded.chinese_title, summary=excluded.summary,
            comment=excluded.comment, rotations=excluded.rotations, updated_at=excluded.updated_at",
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
            now_ms(),
        ],
    )?;
    Ok(())
}

pub fn upsert_meta(m: &BookMetaRow) -> Result<()> {
    let conn = get().lock().unwrap();
    upsert_meta_on(&conn, m)
}

pub fn delete_meta(key: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute("DELETE FROM book_metas WHERE key = ?1", params![key])?;
    upsert_tombstone_on(&conn, "metas", key)?;
    Ok(())
}

pub fn delete_metas_by_source_prefix(prefix: &str) -> Result<u32> {
    let conn = get().lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT key FROM book_metas WHERE key LIKE ?1")
        .unwrap();
    let keys: Vec<String> = stmt
        .query_map([format!("{prefix}%")], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    let n = conn.execute(
        "DELETE FROM book_metas WHERE key LIKE ?1",
        params![format!("{prefix}%")],
    )?;
    for k in &keys {
        upsert_tombstone_on(&conn, "metas", k)?;
    }
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

fn load_all_book_tags_on(conn: &Connection) -> Vec<BookTagRow> {
    let mut stmt = conn
        .prepare("SELECT book_key, tag_id FROM book_tags WHERE deleted = 0")
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
        "INSERT INTO tags (id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
        params![id, name, now, now],
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
            "INSERT OR IGNORE INTO tags (id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
            params![new_id, new_name, now, now],
        )?;
        // 墓碑：旧标签与其关联（先于迁移捕获，传播删除）
        let mut stmt = conn
            .prepare("SELECT book_key FROM book_tags WHERE tag_id = ?1")
            .unwrap();
        let old_links: Vec<String> = stmt
            .query_map([&old_id], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for bk in &old_links {
            upsert_tombstone_on(&conn, "book_tags", &format!("{bk}|{old_id}"))?;
        }
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
        upsert_tombstone_on(&conn, "tags", &old_id)?;
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
    let mut stmt = conn
        .prepare("SELECT book_key FROM book_tags WHERE tag_id = ?1")
        .unwrap();
    let links: Vec<String> = stmt
        .query_map([&id], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    conn.execute("DELETE FROM book_tags WHERE tag_id = ?1", params![id])?;
    conn.execute("DELETE FROM tags WHERE id = ?1", params![id])?;
    for bk in &links {
        upsert_tombstone_on(&conn, "book_tags", &format!("{bk}|{id}"))?;
    }
    upsert_tombstone_on(&conn, "tags", &id)?;
    Ok(())
}

/// 将标签关联到一本书（幂等）。
fn link_tag_on(conn: &Connection, book_key: &str, tag_id: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO book_tags (book_key, tag_id, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(book_key, tag_id) DO UPDATE SET deleted=0, updated_at=excluded.updated_at",
        params![book_key, tag_id, now_ms()],
    )?;
    Ok(())
}

pub fn link_tag(book_key: &str, tag_name: &str) -> Result<()> {
    if tag_name.is_empty() {
        return Ok(());
    }
    let tag_id = ensure_tag(tag_name)?.id;
    let conn = get().lock().unwrap();
    link_tag_on(&conn, book_key, &tag_id)
}

/// 将标签从一本书移除。
pub fn unlink_tag(book_key: &str, tag_name: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    let id = tag_id(tag_name);
    conn.execute(
        "DELETE FROM book_tags WHERE book_key = ?1 AND tag_id = ?2",
        params![book_key, id],
    )?;
    upsert_tombstone_on(&conn, "book_tags", &format!("{book_key}|{id}"))?;
    Ok(())
}

/// 设置一本书的标签集（全量替换：先删后插）。
fn set_book_tags_on(conn: &Connection, book_key: &str, tag_names: &[String]) -> Result<()> {
    conn.execute("BEGIN", [])?;
    let result = (|| -> Result<()> {
        // 记录将被移除的关联（墓碑传播）
        let mut stmt = conn
            .prepare("SELECT tag_id FROM book_tags WHERE book_key = ?1")
            .unwrap();
        let removed: Vec<String> = stmt
            .query_map([book_key], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .filter(|tid| !tag_names.iter().any(|n| tag_id(n) == *tid))
            .collect();
        conn.execute(
            "DELETE FROM book_tags WHERE book_key = ?1",
            params![book_key],
        )?;
        for tid in &removed {
            upsert_tombstone_on(conn, "book_tags", &format!("{book_key}|{tid}"))?;
        }
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
                "INSERT OR IGNORE INTO tags (id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
                params![id, name, now, now],
            )?;
            link_tag_on(conn, book_key, &id)?;
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

pub fn set_book_tags(book_key: &str, tag_names: &[String]) -> Result<()> {
    let conn = get().lock().unwrap();
    set_book_tags_on(&conn, book_key, tag_names)
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

fn save_setting_on(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
        params![key, value, now_ms()],
    )?;
    Ok(())
}

pub fn save_setting(key: &str, value: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    save_setting_on(&conn, key, value)
}

pub fn delete_setting(key: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute("DELETE FROM app_settings WHERE key = ?1", params![key])?;
    upsert_tombstone_on(&conn, "settings", key)?;
    Ok(())
}

// ============================================================
// 同步支撑（P0：devices / sync_state / source_alias / stable_id）
// ============================================================

#[derive(Debug, Clone)]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub last_seen_at: i64,
}

#[derive(Debug, Clone)]
pub struct SourceAliasRow {
    pub source_id: String,
    pub fingerprint: String,
    pub device_id: String,
    pub updated_at: i64,
}

pub(crate) fn get_or_create_device_id_on(conn: &Connection) -> Result<String> {
    if let Some(id) = get_sync_state_on(conn, "device_id") {
        return Ok(id);
    }
    let id = format!("dev_{}_{}", now_ms(), std::process::id());
    set_sync_state_on(conn, "device_id", &id)?;
    Ok(id)
}

/// 获取（或生成并持久化）本机设备 ID，幂等。
pub fn get_or_create_device_id() -> Result<String> {
    let conn = get().lock().unwrap();
    get_or_create_device_id_on(&conn)
}

/// 注册/刷新一台设备（含本机）。
pub fn register_device(id: &str, name: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    register_device_on(&conn, id, name)
}

pub(crate) fn register_device_on(conn: &Connection, id: &str, name: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO devices (id, name, created_at, last_seen_at) VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(id) DO UPDATE SET name=excluded.name, last_seen_at=excluded.last_seen_at",
        params![id, name, now_ms()],
    )?;
    Ok(())
}

pub fn list_devices() -> Vec<DeviceRow> {
    let conn = get().lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT id, name, created_at, last_seen_at FROM devices ORDER BY created_at")
        .unwrap();
    stmt.query_map([], |row| {
        Ok(DeviceRow {
            id: row.get(0)?,
            name: row.get(1)?,
            created_at: row.get(2)?,
            last_seen_at: row.get(3)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub(crate) fn get_sync_state_on(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM sync_state WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

pub(crate) fn set_sync_state_on(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_state (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
        params![key, value, now_ms()],
    )?;
    Ok(())
}

pub fn get_sync_state(key: &str) -> Option<String> {
    let conn = get().lock().unwrap();
    get_sync_state_on(&conn, key)
}

pub fn set_sync_state(key: &str, value: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    set_sync_state_on(&conn, key, value)
}

fn set_source_alias_on(
    conn: &Connection,
    source_id: &str,
    fingerprint: &str,
    device_id: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO source_alias (source_id, fingerprint, device_id, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(source_id) DO UPDATE SET fingerprint=excluded.fingerprint, device_id=excluded.device_id, updated_at=excluded.updated_at",
        params![source_id, fingerprint, device_id, now_ms()],
    )?;
    Ok(())
}

pub fn set_source_alias(source_id: &str, fingerprint: &str, device_id: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    set_source_alias_on(&conn, source_id, fingerprint, device_id)
}

fn get_source_alias_on(conn: &Connection, source_id: &str) -> Option<SourceAliasRow> {
    conn.query_row(
        "SELECT source_id, fingerprint, device_id, updated_at FROM source_alias WHERE source_id = ?1",
        params![source_id],
        |row| {
            Ok(SourceAliasRow {
                source_id: row.get(0)?,
                fingerprint: row.get(1)?,
                device_id: row.get(2)?,
                updated_at: row.get(3)?,
            })
        },
    )
    .ok()
}

pub fn get_source_alias(source_id: &str) -> Option<SourceAliasRow> {
    let conn = get().lock().unwrap();
    get_source_alias_on(&conn, source_id)
}

pub fn load_source_aliases() -> Vec<SourceAliasRow> {
    let conn = get().lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT source_id, fingerprint, device_id, updated_at FROM source_alias")
        .unwrap();
    stmt.query_map([], |row| {
        Ok(SourceAliasRow {
            source_id: row.get(0)?,
            fingerprint: row.get(1)?,
            device_id: row.get(2)?,
            updated_at: row.get(3)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

/// 写书源 fingerprint（跨端稳定标识，由 P1 计算后调用）。
pub fn set_source_fingerprint(id: &str, fingerprint: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute(
        "UPDATE book_sources SET fingerprint = ?1, updated_at = ?2 WHERE id = ?3",
        params![fingerprint, now_ms(), id],
    )?;
    Ok(())
}

/// 写漫画元数据的稳定书 ID。
pub fn set_meta_stable_id(key: &str, stable_id: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute(
        "UPDATE book_metas SET stable_id = ?1, updated_at = ?2 WHERE key = ?3",
        params![stable_id, now_ms(), key],
    )?;
    Ok(())
}

/// 写阅读记录的稳定书 ID。
pub fn set_record_stable_id(key: &str, stable_id: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    conn.execute(
        "UPDATE read_records SET stable_id = ?1, updated_at = ?2 WHERE key = ?3",
        params![stable_id, now_ms(), key],
    )?;
    Ok(())
}

// ============================================================
// P3：墓碑 / fingerprint 匹配
// ============================================================

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TombstoneSyncRow {
    pub entity: String,
    pub key: String,
    pub updated_at: i64,
}

pub(crate) fn upsert_tombstone_on(conn: &Connection, entity: &str, key: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_tombstones (entity, key, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(entity, key) DO UPDATE SET updated_at=excluded.updated_at",
        params![entity, key, now_ms()],
    )?;
    Ok(())
}

pub fn upsert_tombstone(entity: &str, key: &str) -> Result<()> {
    let conn = get().lock().unwrap();
    upsert_tombstone_on(&conn, entity, key)
}

pub(crate) fn load_tombstones_for_sync_on(conn: &Connection, since: i64) -> Vec<TombstoneSyncRow> {
    let mut stmt = conn
        .prepare(
            "SELECT entity, key, updated_at FROM sync_tombstones
             WHERE updated_at > ?1 ORDER BY updated_at",
        )
        .unwrap();
    stmt.query_map([since], |row| {
        Ok(TombstoneSyncRow {
            entity: row.get(0)?,
            key: row.get(1)?,
            updated_at: row.get(2)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn load_tombstones_for_sync(since: i64) -> Vec<TombstoneSyncRow> {
    let conn = get().lock().unwrap();
    load_tombstones_for_sync_on(&conn, since)
}

pub(crate) fn find_source_id_by_fingerprint_on(
    conn: &Connection,
    fingerprint: &str,
) -> Option<String> {
    conn.query_row(
        "SELECT id FROM book_sources WHERE fingerprint = ?1 AND deleted = 0 ORDER BY updated_at LIMIT 1",
        params![fingerprint],
        |r| r.get(0),
    )
    .ok()
}

pub(crate) fn source_exists_on(conn: &Connection, id: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM book_sources WHERE id = ?1",
        params![id],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

// ============================================================
// 同步导出/导入（P1：标准包格式数据存取层）
// ============================================================

/// 书源同步行（不含 password / refresh_token / client_secret / cookie 敏感字段）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSyncRow {
    pub id: String,
    pub r#type: String,
    pub name: String,
    pub path: String,
    pub url: Option<String>,
    pub username: Option<String>,
    pub port: Option<i64>,
    pub note: String,
    pub capability_label: String,
    pub fingerprint: Option<String>,
    pub remote_only: bool,
    pub origin_device_id: Option<String>,
    pub root_id: Option<String>,
    pub client_id: Option<String>,
    pub updated_at: i64,
    pub deleted: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordSyncRow {
    pub key: String,
    pub stable_id: Option<String>,
    pub source_id: String,
    pub source_type: String,
    pub path: String,
    pub title: String,
    pub last_page: i32,
    pub read_count: i32,
    pub last_read_at: i64,
    pub updated_at: i64,
    pub deleted: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaSyncRow {
    pub key: String,
    pub stable_id: Option<String>,
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
    pub updated_at: i64,
    pub deleted: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagSyncRow {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookTagSyncRow {
    pub book_key: String,
    pub tag_id: String,
    pub updated_at: i64,
    pub deleted: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingSyncRow {
    pub key: String,
    pub value: String,
    pub updated_at: i64,
    pub deleted: bool,
}

pub(crate) fn load_sources_for_sync_on(conn: &Connection, since: i64) -> Vec<SourceSyncRow> {
    let mut stmt = conn
        .prepare(
            "SELECT id, type, name, path, url, username, port, note, capability_label,
                    fingerprint, remote_only, origin_device_id, root_id, client_id,
                    updated_at, deleted
             FROM book_sources WHERE updated_at > ?1 ORDER BY updated_at",
        )
        .unwrap();
    stmt.query_map([since], |row| {
        Ok(SourceSyncRow {
            id: row.get(0)?,
            r#type: row.get(1)?,
            name: row.get(2)?,
            path: row.get(3)?,
            url: row.get(4)?,
            username: row.get(5)?,
            port: row.get(6)?,
            note: row.get(7)?,
            capability_label: row.get(8)?,
            fingerprint: row.get(9)?,
            remote_only: row.get::<_, i64>(10)? != 0,
            origin_device_id: row.get(11)?,
            root_id: row.get(12)?,
            client_id: row.get(13)?,
            updated_at: row.get(14)?,
            deleted: row.get::<_, i64>(15)? != 0,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn load_sources_for_sync(since: i64) -> Vec<SourceSyncRow> {
    let conn = get().lock().unwrap();
    load_sources_for_sync_on(&conn, since)
}

pub(crate) fn load_records_for_sync_on(conn: &Connection, since: i64) -> Vec<RecordSyncRow> {
    let mut stmt = conn
        .prepare(
            "SELECT key, stable_id, source_id, source_type, path, title,
                    last_page, read_count, last_read_at, updated_at, deleted
             FROM read_records WHERE updated_at > ?1 ORDER BY updated_at",
        )
        .unwrap();
    stmt.query_map([since], |row| {
        Ok(RecordSyncRow {
            key: row.get(0)?,
            stable_id: row.get(1)?,
            source_id: row.get(2)?,
            source_type: row.get(3)?,
            path: row.get(4)?,
            title: row.get(5)?,
            last_page: row.get(6)?,
            read_count: row.get(7)?,
            last_read_at: row.get(8)?,
            updated_at: row.get(9)?,
            deleted: row.get::<_, i64>(10)? != 0,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn load_records_for_sync(since: i64) -> Vec<RecordSyncRow> {
    let conn = get().lock().unwrap();
    load_records_for_sync_on(&conn, since)
}

pub(crate) fn load_metas_for_sync_on(conn: &Connection, since: i64) -> Vec<MetaSyncRow> {
    let mut stmt = conn
        .prepare(
            "SELECT key, stable_id, cover_page, crop_x, crop_y, crop_w, crop_h,
                    author, genre, series, title, chinese_title, summary, comment,
                    rotations, updated_at, deleted
             FROM book_metas WHERE updated_at > ?1 ORDER BY updated_at",
        )
        .unwrap();
    stmt.query_map([since], |row| {
        Ok(MetaSyncRow {
            key: row.get(0)?,
            stable_id: row.get(1)?,
            cover_page: row.get(2)?,
            crop_x: row.get(3)?,
            crop_y: row.get(4)?,
            crop_w: row.get(5)?,
            crop_h: row.get(6)?,
            author: row.get(7)?,
            genre: row.get(8)?,
            series: row.get(9)?,
            title: row.get(10)?,
            chinese_title: row.get(11)?,
            summary: row.get(12)?,
            comment: row.get(13)?,
            rotations: row.get(14)?,
            updated_at: row.get(15)?,
            deleted: row.get::<_, i64>(16)? != 0,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn load_metas_for_sync(since: i64) -> Vec<MetaSyncRow> {
    let conn = get().lock().unwrap();
    load_metas_for_sync_on(&conn, since)
}

pub(crate) fn load_tags_for_sync_on(conn: &Connection, since: i64) -> Vec<TagSyncRow> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, created_at, updated_at, deleted
             FROM tags WHERE updated_at > ?1 ORDER BY updated_at",
        )
        .unwrap();
    stmt.query_map([since], |row| {
        Ok(TagSyncRow {
            id: row.get(0)?,
            name: row.get(1)?,
            created_at: row.get(2)?,
            updated_at: row.get(3)?,
            deleted: row.get::<_, i64>(4)? != 0,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn load_tags_for_sync(since: i64) -> Vec<TagSyncRow> {
    let conn = get().lock().unwrap();
    load_tags_for_sync_on(&conn, since)
}

pub(crate) fn load_book_tags_for_sync_on(conn: &Connection, since: i64) -> Vec<BookTagSyncRow> {
    let mut stmt = conn
        .prepare(
            "SELECT book_key, tag_id, updated_at, deleted
             FROM book_tags WHERE updated_at > ?1 ORDER BY updated_at",
        )
        .unwrap();
    stmt.query_map([since], |row| {
        Ok(BookTagSyncRow {
            book_key: row.get(0)?,
            tag_id: row.get(1)?,
            updated_at: row.get(2)?,
            deleted: row.get::<_, i64>(3)? != 0,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn load_book_tags_for_sync(since: i64) -> Vec<BookTagSyncRow> {
    let conn = get().lock().unwrap();
    load_book_tags_for_sync_on(&conn, since)
}

pub(crate) fn load_settings_for_sync_on(conn: &Connection, since: i64) -> Vec<SettingSyncRow> {
    let mut stmt = conn
        .prepare(
            "SELECT key, value, updated_at, deleted
             FROM app_settings WHERE updated_at > ?1 ORDER BY updated_at",
        )
        .unwrap();
    stmt.query_map([since], |row| {
        Ok(SettingSyncRow {
            key: row.get(0)?,
            value: row.get(1)?,
            updated_at: row.get(2)?,
            deleted: row.get::<_, i64>(3)? != 0,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn load_settings_for_sync(since: i64) -> Vec<SettingSyncRow> {
    let conn = get().lock().unwrap();
    load_settings_for_sync_on(&conn, since)
}

/// 应用同步行（保留行内 updated_at/deleted；凭据字段 COALESCE 保留本地值）。
pub(crate) fn apply_source_sync_on(conn: &Connection, r: &SourceSyncRow) -> Result<()> {
    conn.execute(
        "INSERT INTO book_sources
         (id, type, name, path, url, username, port, note, capability_label,
          fingerprint, remote_only, origin_device_id, root_id, client_id, updated_at, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT(id) DO UPDATE SET
            type=excluded.type, name=excluded.name, path=excluded.path, url=excluded.url,
            username=excluded.username, port=excluded.port, note=excluded.note,
            capability_label=excluded.capability_label, fingerprint=excluded.fingerprint,
            remote_only=excluded.remote_only, origin_device_id=excluded.origin_device_id,
            root_id=excluded.root_id, client_id=excluded.client_id,
            password=COALESCE(book_sources.password, excluded.password),
            refresh_token=COALESCE(book_sources.refresh_token, excluded.refresh_token),
            client_secret=COALESCE(book_sources.client_secret, excluded.client_secret),
            cookie=COALESCE(book_sources.cookie, excluded.cookie),
            updated_at=excluded.updated_at, deleted=excluded.deleted",
        params![
            r.id,
            r.r#type,
            r.name,
            r.path,
            r.url,
            r.username,
            r.port,
            r.note,
            r.capability_label,
            r.fingerprint,
            r.remote_only,
            r.origin_device_id,
            r.root_id,
            r.client_id,
            r.updated_at,
            r.deleted,
        ],
    )?;
    Ok(())
}

pub fn apply_source_sync(r: &SourceSyncRow) -> Result<()> {
    let conn = get().lock().unwrap();
    apply_source_sync_on(&conn, r)
}

pub(crate) fn apply_record_sync_on(conn: &Connection, r: &RecordSyncRow) -> Result<()> {
    conn.execute(
        "INSERT INTO read_records
         (key, stable_id, source_id, source_type, path, title, last_page, read_count, last_read_at, updated_at, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(key) DO UPDATE SET
            stable_id=excluded.stable_id, source_id=excluded.source_id,
            source_type=excluded.source_type, path=excluded.path, title=excluded.title,
            last_page=excluded.last_page, read_count=excluded.read_count,
            last_read_at=excluded.last_read_at,
            updated_at=excluded.updated_at, deleted=excluded.deleted",
        params![
            r.key,
            r.stable_id,
            r.source_id,
            r.source_type,
            r.path,
            r.title,
            r.last_page,
            r.read_count,
            r.last_read_at,
            r.updated_at,
            r.deleted,
        ],
    )?;
    Ok(())
}

pub fn apply_record_sync(r: &RecordSyncRow) -> Result<()> {
    let conn = get().lock().unwrap();
    apply_record_sync_on(&conn, r)
}

pub(crate) fn apply_meta_sync_on(conn: &Connection, r: &MetaSyncRow) -> Result<()> {
    conn.execute(
        "INSERT INTO book_metas
         (key, stable_id, cover_page, crop_x, crop_y, crop_w, crop_h,
          author, genre, series, title, chinese_title, summary, comment,
          rotations, updated_at, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
         ON CONFLICT(key) DO UPDATE SET
            stable_id=excluded.stable_id, cover_page=excluded.cover_page,
            crop_x=excluded.crop_x, crop_y=excluded.crop_y,
            crop_w=excluded.crop_w, crop_h=excluded.crop_h,
            author=excluded.author, genre=excluded.genre, series=excluded.series,
            title=excluded.title, chinese_title=excluded.chinese_title,
            summary=excluded.summary, comment=excluded.comment, rotations=excluded.rotations,
            updated_at=excluded.updated_at, deleted=excluded.deleted",
        params![
            r.key,
            r.stable_id,
            r.cover_page,
            r.crop_x,
            r.crop_y,
            r.crop_w,
            r.crop_h,
            r.author,
            r.genre,
            r.series,
            r.title,
            r.chinese_title,
            r.summary,
            r.comment,
            r.rotations,
            r.updated_at,
            r.deleted,
        ],
    )?;
    Ok(())
}

pub fn apply_meta_sync(r: &MetaSyncRow) -> Result<()> {
    let conn = get().lock().unwrap();
    apply_meta_sync_on(&conn, r)
}

pub(crate) fn apply_tag_sync_on(conn: &Connection, r: &TagSyncRow) -> Result<()> {
    conn.execute(
        "INSERT INTO tags (id, name, created_at, updated_at, deleted) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET
            name=excluded.name, created_at=excluded.created_at,
            updated_at=excluded.updated_at, deleted=excluded.deleted",
        params![r.id, r.name, r.created_at, r.updated_at, r.deleted],
    )?;
    Ok(())
}

pub fn apply_tag_sync(r: &TagSyncRow) -> Result<()> {
    let conn = get().lock().unwrap();
    apply_tag_sync_on(&conn, r)
}

pub(crate) fn apply_book_tag_sync_on(conn: &Connection, r: &BookTagSyncRow) -> Result<()> {
    conn.execute(
        "INSERT INTO book_tags (book_key, tag_id, updated_at, deleted) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(book_key, tag_id) DO UPDATE SET
            updated_at=excluded.updated_at, deleted=excluded.deleted",
        params![r.book_key, r.tag_id, r.updated_at, r.deleted],
    )?;
    Ok(())
}

pub fn apply_book_tag_sync(r: &BookTagSyncRow) -> Result<()> {
    let conn = get().lock().unwrap();
    apply_book_tag_sync_on(&conn, r)
}

pub(crate) fn apply_setting_sync_on(conn: &Connection, r: &SettingSyncRow) -> Result<()> {
    conn.execute(
        "INSERT INTO app_settings (key, value, updated_at, deleted) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(key) DO UPDATE SET
            value=excluded.value, updated_at=excluded.updated_at, deleted=excluded.deleted",
        params![r.key, r.value, r.updated_at, r.deleted],
    )?;
    Ok(())
}

pub fn apply_setting_sync(r: &SettingSyncRow) -> Result<()> {
    let conn = get().lock().unwrap();
    apply_setting_sync_on(&conn, r)
}

// ============================================================
// P3：LWW 合并（拉取=merge，恢复=force）
// ============================================================

fn existing_updated_at(conn: &Connection, table: &str, key_col: &str, key: &str) -> Option<i64> {
    conn.query_row(
        &format!("SELECT updated_at FROM {table} WHERE {key_col} = ?1"),
        params![key],
        |r| r.get::<_, i64>(0),
    )
    .ok()
}

fn merge_row_on(
    conn: &Connection,
    table: &str,
    key_col: &str,
    entity: &str,
    key: &str,
    incoming_updated_at: i64,
    deleted: bool,
    force: bool,
    apply: impl FnOnce(&Connection) -> Result<()>,
) -> Result<bool> {
    if deleted {
        let should = force
            || existing_updated_at(conn, table, key_col, key)
                .map_or(false, |t| incoming_updated_at > t);
        if should {
            conn.execute(
                &format!("DELETE FROM {table} WHERE {key_col} = ?1"),
                params![key],
            )?;
            upsert_tombstone_on(conn, entity, key)?;
            return Ok(true);
        }
        return Ok(false);
    }
    let should = force
        || existing_updated_at(conn, table, key_col, key).map_or(true, |t| incoming_updated_at > t);
    if should {
        apply(conn)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn existing_book_tag_updated_at(conn: &Connection, book_key: &str, tag_id: &str) -> Option<i64> {
    conn.query_row(
        "SELECT updated_at FROM book_tags WHERE book_key = ?1 AND tag_id = ?2",
        params![book_key, tag_id],
        |r| r.get::<_, i64>(0),
    )
    .ok()
}

pub(crate) fn merge_source_sync_on(conn: &Connection, r: &SourceSyncRow, force: bool) -> Result<bool> {
    if r.deleted {
        let should = force
            || existing_updated_at(conn, "book_sources", "id", &r.id)
                .map_or(false, |t| r.updated_at > t);
        if should {
            conn.execute("DELETE FROM source_alias WHERE source_id = ?1", params![r.id])?;
            conn.execute("DELETE FROM book_sources WHERE id = ?1", params![r.id])?;
            upsert_tombstone_on(conn, "sources", &r.id)?;
            return Ok(true);
        }
        return Ok(false);
    }
    let should = force
        || existing_updated_at(conn, "book_sources", "id", &r.id)
            .map_or(true, |t| r.updated_at > t);
    if should {
        apply_source_sync_on(conn, r)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn merge_source_sync(r: &SourceSyncRow, force: bool) -> Result<bool> {
    let conn = get().lock().unwrap();
    merge_source_sync_on(&conn, r, force)
}

pub(crate) fn merge_record_sync_on(conn: &Connection, r: &RecordSyncRow, force: bool) -> Result<bool> {
    merge_row_on(conn, "read_records", "key", "records", &r.key, r.updated_at, r.deleted, force, |c| {
        apply_record_sync_on(c, r)
    })
}

pub fn merge_record_sync(r: &RecordSyncRow, force: bool) -> Result<bool> {
    let conn = get().lock().unwrap();
    merge_record_sync_on(&conn, r, force)
}

pub(crate) fn merge_meta_sync_on(conn: &Connection, r: &MetaSyncRow, force: bool) -> Result<bool> {
    merge_row_on(conn, "book_metas", "key", "metas", &r.key, r.updated_at, r.deleted, force, |c| {
        apply_meta_sync_on(c, r)
    })
}

pub fn merge_meta_sync(r: &MetaSyncRow, force: bool) -> Result<bool> {
    let conn = get().lock().unwrap();
    merge_meta_sync_on(&conn, r, force)
}

pub(crate) fn merge_tag_sync_on(conn: &Connection, r: &TagSyncRow, force: bool) -> Result<bool> {
    if r.deleted {
        let should = force
            || existing_updated_at(conn, "tags", "id", &r.id)
                .map_or(false, |t| r.updated_at > t);
        if should {
            conn.execute("DELETE FROM book_tags WHERE tag_id = ?1", params![r.id])?;
            conn.execute("DELETE FROM tags WHERE id = ?1", params![r.id])?;
            upsert_tombstone_on(conn, "tags", &r.id)?;
            return Ok(true);
        }
        return Ok(false);
    }
    let should = force
        || existing_updated_at(conn, "tags", "id", &r.id).map_or(true, |t| r.updated_at > t);
    if should {
        apply_tag_sync_on(conn, r)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn merge_tag_sync(r: &TagSyncRow, force: bool) -> Result<bool> {
    let conn = get().lock().unwrap();
    merge_tag_sync_on(&conn, r, force)
}

pub(crate) fn merge_book_tag_sync_on(
    conn: &Connection,
    r: &BookTagSyncRow,
    force: bool,
) -> Result<bool> {
    let key = format!("{}|{}", r.book_key, r.tag_id);
    if r.deleted {
        let should = force
            || existing_book_tag_updated_at(conn, &r.book_key, &r.tag_id)
                .map_or(false, |t| r.updated_at > t);
        if should {
            conn.execute(
                "DELETE FROM book_tags WHERE book_key = ?1 AND tag_id = ?2",
                params![r.book_key, r.tag_id],
            )?;
            upsert_tombstone_on(conn, "book_tags", &key)?;
            return Ok(true);
        }
        return Ok(false);
    }
    let should = force
        || existing_book_tag_updated_at(conn, &r.book_key, &r.tag_id)
            .map_or(true, |t| r.updated_at > t);
    if should {
        apply_book_tag_sync_on(conn, r)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn merge_book_tag_sync(r: &BookTagSyncRow, force: bool) -> Result<bool> {
    let conn = get().lock().unwrap();
    merge_book_tag_sync_on(&conn, r, force)
}

pub(crate) fn merge_setting_sync_on(
    conn: &Connection,
    r: &SettingSyncRow,
    force: bool,
) -> Result<bool> {
    merge_row_on(
        conn,
        "app_settings",
        "key",
        "settings",
        &r.key,
        r.updated_at,
        r.deleted,
        force,
        |c| apply_setting_sync_on(c, r),
    )
}

pub fn merge_setting_sync(r: &SettingSyncRow, force: bool) -> Result<bool> {
    let conn = get().lock().unwrap();
    merge_setting_sync_on(&conn, r, force)
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

/// 当前毫秒时间戳（同步 LWW 与墓碑统一使用）。
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
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

pub fn load_all_book_tags() -> Vec<BookTagRow> {
    let conn = get().lock().unwrap();
    load_all_book_tags_on(&conn)
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

    fn schema_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_tables(&conn).unwrap();
        conn
    }

    fn table_cols(conn: &Connection, table: &str) -> Vec<String> {
        conn.prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(|c| c.ok())
            .collect()
    }

    #[test]
    fn fresh_schema_has_sync_columns_and_tables() {
        let conn = schema_conn();
        for t in [
            "book_sources",
            "read_records",
            "book_metas",
            "tags",
            "book_tags",
            "app_settings",
        ] {
            let c = table_cols(&conn, t);
            assert!(c.contains(&"updated_at".to_string()), "{t} 缺 updated_at");
            assert!(c.contains(&"deleted".to_string()), "{t} 缺 deleted");
        }
        let src = table_cols(&conn, "book_sources");
        for col in ["fingerprint", "remote_only", "origin_device_id"] {
            assert!(src.contains(&col.to_string()), "book_sources 缺 {col}");
        }
        for t in ["read_records", "book_metas"] {
            assert!(
                table_cols(&conn, t).contains(&"stable_id".to_string()),
                "{t} 缺 stable_id"
            );
        }
        for t in ["devices", "sync_state", "source_alias"] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    params![t],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "缺表 {t}");
        }
    }

    #[test]
    fn legacy_schema_migrates_sync_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE book_sources (
                id TEXT PRIMARY KEY, type TEXT NOT NULL, name TEXT NOT NULL,
                path TEXT NOT NULL DEFAULT '', url TEXT, username TEXT, password TEXT,
                port INTEGER, note TEXT NOT NULL DEFAULT '', capability_label TEXT NOT NULL DEFAULT ''
            );
            INSERT INTO book_sources (id, type, name, path) VALUES ('s1', 'local', '书库', 'D:/Comics');",
        )
        .unwrap();
        init_tables(&conn).unwrap();
        let c = table_cols(&conn, "book_sources");
        for col in ["fingerprint", "remote_only", "origin_device_id", "updated_at", "deleted"] {
            assert!(c.contains(&col.to_string()), "缺列 {col}");
        }
        let name: String = conn
            .query_row("SELECT name FROM book_sources WHERE id='s1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "书库");
    }

    #[test]
    fn upsert_preserves_sync_columns_and_keeps_single_row() {
        let conn = schema_conn();
        let src = BookSourceRow {
            id: "s1".into(),
            r#type: "local".into(),
            name: "书库".into(),
            path: "D:/Comics".into(),
            url: None,
            username: None,
            password: None,
            port: None,
            refresh_token: None,
            client_id: None,
            client_secret: None,
            root_id: None,
            cookie: None,
            note: String::new(),
            capability_label: "local".into(),
            remote_only: false,
            origin_device_id: None,
        };
        upsert_source_on(&conn, &src).unwrap();
        conn.execute("UPDATE book_sources SET fingerprint='fp1' WHERE id='s1'", [])
            .unwrap();
        upsert_source_on(&conn, &src).unwrap();
        let fp: Option<String> = conn
            .query_row(
                "SELECT fingerprint FROM book_sources WHERE id='s1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fp.as_deref(), Some("fp1"));
        let cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM book_sources WHERE id='s1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cnt, 1);

        let meta = BookMetaRow {
            key: "local|s1|b.cbz".into(),
            cover_page: 0,
            crop_x: None,
            crop_y: None,
            crop_w: None,
            crop_h: None,
            author: String::new(),
            genre: String::new(),
            series: String::new(),
            title: "B".into(),
            chinese_title: String::new(),
            summary: String::new(),
            comment: String::new(),
            rotations: "{}".into(),
        };
        upsert_meta_on(&conn, &meta).unwrap();
        conn.execute(
            "UPDATE book_metas SET stable_id='sid1' WHERE key=?1",
            params![meta.key],
        )
        .unwrap();
        upsert_meta_on(&conn, &meta).unwrap();
        let sid: Option<String> = conn
            .query_row(
                "SELECT stable_id FROM book_metas WHERE key=?1",
                params![meta.key],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sid.as_deref(), Some("sid1"));
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM book_metas WHERE key=?1",
                params![meta.key],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn sync_state_and_source_alias_crud() {
        let conn = schema_conn();
        set_sync_state_on(&conn, "cursor_x", "42").unwrap();
        assert_eq!(get_sync_state_on(&conn, "cursor_x").as_deref(), Some("42"));
        // source_alias 外键指向 book_sources，先建书源再建别名。
        let src = BookSourceRow {
            id: "s1".into(),
            r#type: "local".into(),
            name: "书库".into(),
            path: "D:/Comics".into(),
            url: None,
            username: None,
            password: None,
            port: None,
            refresh_token: None,
            client_id: None,
            client_secret: None,
            root_id: None,
            cookie: None,
            note: String::new(),
            capability_label: "local".into(),
            remote_only: false,
            origin_device_id: None,
        };
        upsert_source_on(&conn, &src).unwrap();
        set_source_alias_on(&conn, "s1", "fp1", "dev1").unwrap();
        let row = get_source_alias_on(&conn, "s1").unwrap();
        assert_eq!(row.fingerprint, "fp1");
        assert_eq!(row.device_id, "dev1");
    }

    #[test]
    fn book_tags_load_filters_deleted() {
        let conn = schema_conn();
        conn.execute(
            "INSERT INTO tags (id, name, created_at, updated_at) VALUES ('t1', 'tag', 1, 1)",
            [],
        )
        .unwrap();
        link_tag_on(&conn, "k1", "t1").unwrap();
        conn.execute("UPDATE book_tags SET deleted=1 WHERE book_key='k1' AND tag_id='t1'", [])
            .unwrap();
        assert!(load_all_book_tags_on(&conn).is_empty());
    }

}
