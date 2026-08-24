//! Sync Base（ADR-024 §3）：三方合并基线。
//!
//! 语义：上次成功同步时本机所见的远端状态；只有同步成功才推进
//! （下载/合并/上传任一失败，编排层不得调用 upsert/推进）。

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::db;

pub const ENTITY_SOURCES: &str = "sources";
pub const ENTITY_METAS: &str = "metas";
pub const ENTITY_TAGS: &str = "tags";
pub const ENTITY_BOOK_TAGS: &str = "book_tags";
pub const ENTITY_RECORDS: &str = "records";
pub const ENTITY_LIBRARY_INDEX: &str = "library_index";
pub const ENTITY_SETTINGS: &str = "settings";

pub const META_LIBRARY_ID: &str = "library_id";
pub const META_LAST_REVISION: &str = "last_revision";
pub const META_LAST_SYNC_AT: &str = "last_sync_at";
pub const META_LAST_ERROR: &str = "last_error";

#[derive(Debug, Clone)]
pub struct SyncBaseRow {
    pub entity_type: String,
    pub entity_key: String,
    pub state_hash: String,
    /// 字段级合并实体存完整 JSON（metas/tags/records/sources/settings）；library_index 仅 hash。
    pub state_json: Option<String>,
    pub revision: i64,
    pub updated_at: i64,
}

pub(crate) fn get_base_on(
    conn: &Connection,
    entity_type: &str,
    entity_key: &str,
) -> Option<SyncBaseRow> {
    conn.query_row(
        "SELECT entity_type, entity_key, state_hash, state_json, revision, updated_at
         FROM sync_base WHERE entity_type = ?1 AND entity_key = ?2",
        params![entity_type, entity_key],
        |r| {
            Ok(SyncBaseRow {
                entity_type: r.get(0)?,
                entity_key: r.get(1)?,
                state_hash: r.get(2)?,
                state_json: r.get(3)?,
                revision: r.get(4)?,
                updated_at: r.get(5)?,
            })
        },
    )
    .ok()
}

pub fn get_base(entity_type: &str, entity_key: &str) -> Option<SyncBaseRow> {
    let conn = db::get().lock().unwrap();
    get_base_on(&conn, entity_type, entity_key)
}

pub(crate) fn upsert_base_on(conn: &Connection, row: &SyncBaseRow) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_base (entity_type, entity_key, state_hash, state_json, revision, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(entity_type, entity_key) DO UPDATE SET
            state_hash=excluded.state_hash, state_json=excluded.state_json,
            revision=excluded.revision, updated_at=excluded.updated_at",
        params![
            row.entity_type,
            row.entity_key,
            row.state_hash,
            row.state_json,
            row.revision,
            row.updated_at,
        ],
    )?;
    Ok(())
}

pub fn upsert_base(row: &SyncBaseRow) -> Result<()> {
    let conn = db::get().lock().unwrap();
    upsert_base_on(&conn, row)
}

#[allow(dead_code)] // 保留：同步重置/清理维护 API（含测试覆盖）
pub(crate) fn delete_base_on(conn: &Connection, entity_type: &str, entity_key: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM sync_base WHERE entity_type = ?1 AND entity_key = ?2",
        params![entity_type, entity_key],
    )?;
    Ok(())
}

/// 删除某实体全部基线（如源删除级联清理该书源所有索引基线）。
#[allow(dead_code)] // 保留：同步重置/清理维护 API（含测试覆盖）
pub(crate) fn delete_entity_base_on(conn: &Connection, entity_type: &str) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM sync_base WHERE entity_type = ?1",
        params![entity_type],
    )?)
}

pub(crate) fn set_meta_on(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn set_meta(key: &str, value: &str) -> Result<()> {
    let conn = db::get().lock().unwrap();
    set_meta_on(&conn, key, value)
}

pub(crate) fn get_meta_on(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM sync_meta WHERE key = ?1",
        params![key],
        |r| r.get(0),
    )
    .ok()
}

pub fn get_meta(key: &str) -> Option<String> {
    let conn = db::get().lock().unwrap();
    get_meta_on(&conn, key)
}

// ============================================================
// sync_pending_apply（ADR-028 §12.3）：resolve 失败条目的待绑定缓冲。
// 参与本地快照（视为存在），防止三方合并不产生伪墓碑。
// ============================================================

#[derive(Debug, Clone)]
pub struct PendingApplyRow {
    pub entity_type: String,
    pub entity_key: String,
    pub reason: String,
    /// 完整 SyncEntry JSON。
    pub payload: String,
    pub created_at: i64,
    pub updated_at: i64,
}

pub(crate) fn upsert_pending_on(
    conn: &Connection,
    entity_type: &str,
    entity_key: &str,
    reason: &str,
    payload: &str,
) -> Result<()> {
    let now = db::now_ms();
    conn.execute(
        "INSERT INTO sync_pending_apply
            (entity_type, entity_key, reason, payload, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(entity_type, entity_key) DO UPDATE SET
            reason=excluded.reason, payload=excluded.payload, updated_at=excluded.updated_at",
        params![entity_type, entity_key, reason, payload, now],
    )?;
    Ok(())
}

pub(crate) fn delete_pending_on(
    conn: &Connection,
    entity_type: &str,
    entity_key: &str,
) -> Result<()> {
    conn.execute(
        "DELETE FROM sync_pending_apply WHERE entity_type = ?1 AND entity_key = ?2",
        params![entity_type, entity_key],
    )?;
    Ok(())
}

pub(crate) fn load_pending_on(conn: &Connection) -> Result<Vec<PendingApplyRow>> {
    let mut stmt = conn.prepare(
        "SELECT entity_type, entity_key, reason, payload, created_at, updated_at
         FROM sync_pending_apply",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(PendingApplyRow {
                entity_type: r.get(0)?,
                entity_key: r.get(1)?,
                reason: r.get(2)?,
                payload: r.get(3)?,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// 生成同步库身份（时间戳 + 进程 + 随机数；库级唯一，防不同同步库误合并）。
pub fn new_library_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    format!("lib_{}_{:08x}", db::now_ms(), rng.gen::<u32>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn schema_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init_tables(&conn).unwrap();
        conn
    }

    #[test]
    fn base_upsert_get_update_delete() {
        let conn = schema_conn();
        assert!(get_base_on(&conn, ENTITY_METAS, "k1").is_none());
        upsert_base_on(
            &conn,
            &SyncBaseRow {
                entity_type: ENTITY_METAS.into(),
                entity_key: "k1".into(),
                state_hash: "h1".into(),
                state_json: Some("{}".into()),
                revision: 1,
                updated_at: 100,
            },
        )
        .unwrap();
        let b = get_base_on(&conn, ENTITY_METAS, "k1").unwrap();
        assert_eq!(b.state_hash, "h1");
        assert_eq!(b.state_json.as_deref(), Some("{}"));
        assert_eq!(b.revision, 1);

        // 覆盖更新（推进 = 新 revision）
        upsert_base_on(
            &conn,
            &SyncBaseRow {
                entity_type: ENTITY_METAS.into(),
                entity_key: "k1".into(),
                state_hash: "h2".into(),
                state_json: None,
                revision: 2,
                updated_at: 200,
            },
        )
        .unwrap();
        let b2 = get_base_on(&conn, ENTITY_METAS, "k1").unwrap();
        assert_eq!(b2.state_hash, "h2");
        assert_eq!(b2.revision, 2);

        delete_base_on(&conn, ENTITY_METAS, "k1").unwrap();
        assert!(get_base_on(&conn, ENTITY_METAS, "k1").is_none());
    }

    #[test]
    fn meta_round_trip_and_library_id_unique() {
        let conn = schema_conn();
        assert!(get_meta_on(&conn, META_LAST_REVISION).is_none());
        set_meta_on(&conn, META_LAST_REVISION, "7").unwrap();
        assert_eq!(get_meta_on(&conn, META_LAST_REVISION).as_deref(), Some("7"));
        let lid = new_library_id();
        assert!(!lid.is_empty());
        assert_ne!(lid, new_library_id());
    }

    #[test]
    fn entity_base_cleanup_is_scoped() {
        let conn = schema_conn();
        for (et, k) in [
            (ENTITY_METAS, "a"),
            (ENTITY_METAS, "b"),
            (ENTITY_RECORDS, "c"),
        ] {
            upsert_base_on(
                &conn,
                &SyncBaseRow {
                    entity_type: et.into(),
                    entity_key: k.into(),
                    state_hash: "h".into(),
                    state_json: None,
                    revision: 1,
                    updated_at: 1,
                },
            )
            .unwrap();
        }
        assert_eq!(delete_entity_base_on(&conn, ENTITY_METAS).unwrap(), 2);
        assert!(get_base_on(&conn, ENTITY_METAS, "a").is_none());
        assert!(get_base_on(&conn, ENTITY_RECORDS, "c").is_some());
    }
}
