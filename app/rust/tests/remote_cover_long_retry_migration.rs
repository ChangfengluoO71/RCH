//! P1-C 步骤 1：长期补偿三列的幂等迁移契约（独立可跑，用于先取得真实 RED）。

use rusqlite::Connection;
use rust_lib_app::remote_scan::cover_model::CoverJobState;
use rust_lib_app::remote_scan::cover_state::CoverJobUpsertCause;
use rust_lib_app::remote_scan::{cover_store, persistence};

fn base_connection() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE book_sources(id TEXT PRIMARY KEY,type TEXT NOT NULL,fingerprint TEXT NOT NULL,path TEXT,root_id TEXT);
         CREATE TABLE library_index(
           id TEXT PRIMARY KEY,source_id TEXT,parent_id TEXT,name TEXT,path TEXT,entry_type TEXT,
           size INTEGER,modified_at INTEGER,asset_kind TEXT,content_fingerprint TEXT,
           scan_generation INTEGER,listing_complete INTEGER NOT NULL DEFAULT 0,
           deleted INTEGER NOT NULL DEFAULT 0,updated_at INTEGER);",
    )
    .unwrap();
    conn
}

fn job_key(source_id: &str, asset_id: &str) -> rust_lib_app::remote_scan::cover_model::CoverJobKey {
    rust_lib_app::remote_scan::cover_model::CoverJobKey {
        source_id: source_id.into(),
        asset_id: asset_id.into(),
        content_revision: "content".into(),
        selection_revision: "default".into(),
        profile: "340x480@1".into(),
    }
}

fn columns_of(conn: &Connection, table: &str) -> Vec<String> {
    conn.prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

// ---------------------------------------------------------------------------
// 1. migration
// ---------------------------------------------------------------------------



#[test]
fn migration_adds_the_long_compensation_columns_idempotently() {
    let conn = base_connection();
    persistence::migrate(&conn).unwrap();
    cover_store::migrate(&conn).unwrap();
    // 幂等：重复迁移不得失败，也不得丢列。
    cover_store::migrate(&conn).unwrap();
    persistence::migrate(&conn).unwrap();

    let columns = columns_of(&conn, "remote_cover_job");
    for expected in [
        "long_retry_not_before",
        "long_retry_consumed",
        "long_retry_pending",
    ] {
        assert!(
            columns.iter().any(|name| name == expected),
            "remote_cover_job must expose {expected}; got {columns:?}"
        );
    }

    // 默认值必须是"没有资格 / 未消耗 / 非补偿 pending"，避免老数据被误判为可补偿。
    let conn2 = base_connection();
    persistence::migrate(&conn2).unwrap();
    cover_store::migrate(&conn2).unwrap();
    let key = job_key("source", "asset");
    cover_store::upsert_job_on(
        &conn2,
        &key,
        CoverJobState::Failed,
        "background",
        10,
        1,
        "e1",
        5,
        CoverJobUpsertCause::Demand,
    )
    .unwrap();
    let (not_before, consumed, pending): (Option<i64>, i64, i64) = conn2
        .query_row(
            "SELECT long_retry_not_before,long_retry_consumed,long_retry_pending
               FROM remote_cover_job WHERE job_key=?1",
            [key.encode()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(not_before, None, "a plain failed job has no long-retry qualification");
    assert_eq!(consumed, 0);
    assert_eq!(pending, 0);
}
