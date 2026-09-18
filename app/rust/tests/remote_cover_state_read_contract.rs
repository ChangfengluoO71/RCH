//! P1-E：`remote_cover_state(source_id, asset_id)` 只读契约（STATE-READ-1~3）。
//!
//! 硬契约：read only —— 不 enqueue / 不 wake / 不建 session / 不访问 provider /
//! 不修改 durable state / 不 bump revision。`None` = no job。

use rusqlite::{params, Connection};
use rust_lib_app::api::remote_cover::remote_cover_state;
use rust_lib_app::remote_scan::cover_model::{CoverJobKey, CoverJobState};
use rust_lib_app::remote_scan::cover_revision_stream::{reset_wake_decided_count, wake_decided_count};
use rust_lib_app::remote_scan::cover_state::CoverJobUpsertCause;
use rust_lib_app::remote_scan::{cover_store, persistence};
use rust_lib_app::db;

fn connection() -> Connection {
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
    persistence::migrate(&conn).unwrap();
    cover_store::migrate(&conn).unwrap();
    conn
}

fn job_key(source_id: &str, asset_id: &str) -> CoverJobKey {
    CoverJobKey {
        source_id: source_id.into(),
        asset_id: asset_id.into(),
        content_revision: "content".into(),
        selection_revision: "default".into(),
        profile: "340x480@1".into(),
    }
}

fn seed_running(source: &str, asset: &str) {
    let conn = db::get().lock().unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO remote_scan_epoch(
             source_id,generation,source_fingerprint,root_path,session_epoch,session_token)
         VALUES(?1,1,'fp','/','e1',42)",
        params![source],
    )
    .unwrap();
    let key = job_key(source, asset);
    cover_store::upsert_job_on(
        &conn,
        &key,
        CoverJobState::Pending,
        "background",
        10,
        1,
        "e1",
        0,
        CoverJobUpsertCause::Demand,
    )
    .unwrap();
    // pending → running（真实 claim）
    let claimed = cover_store::claim_next_job_for_source_session_on(
        &conn, source, "worker", 100, 60_000, 42,
    )
    .unwrap();
    assert!(claimed.is_some(), "the seeded job must be claimable");
}

// ------------------------------------------------------------- STATE-READ-1
#[test]
fn state_read_1_existing_job_returns_its_durable_state() {
    let source = "state-read-1";
    {
        let conn = connection();
        for table in ["remote_cover_job", "remote_cover_variant", "remote_scan_epoch"] {
            conn.execute(&format!("DELETE FROM {table} WHERE source_id=?1"), [source])
                .unwrap();
        }
    }
    // 用全局 DB（本 API 走 db::get()）
    {
        let conn = db::get().lock().unwrap();
        persistence::migrate(&conn).unwrap();
        cover_store::migrate(&conn).unwrap();
        for table in ["remote_cover_job", "remote_cover_variant", "remote_scan_epoch"] {
            conn.execute(&format!("DELETE FROM {table} WHERE source_id=?1"), [source])
                .unwrap();
        }
    }
    seed_running(source, "asset");

    let state = remote_cover_state(source.to_string(), "asset".to_string())
        .unwrap()
        .expect("an existing job must be reported");
    assert_eq!(state.state, "running");
    assert!(!state.ready, "running is not ready");
}

// ------------------------------------------------------------- STATE-READ-2
#[test]
fn state_read_2_missing_job_is_none() {
    let source = "state-read-2";
    {
        let conn = db::get().lock().unwrap();
        persistence::migrate(&conn).unwrap();
        cover_store::migrate(&conn).unwrap();
        for table in ["remote_cover_job", "remote_cover_variant", "remote_scan_epoch"] {
            conn.execute(&format!("DELETE FROM {table} WHERE source_id=?1"), [source])
                .unwrap();
        }
    }
    assert!(
        remote_cover_state(source.to_string(), "nope".to_string())
            .unwrap()
            .is_none(),
        "STATE-READ-2: no job => None"
    );
}

// ------------------------------------------------------------- STATE-READ-3
#[test]
fn state_read_3_is_pure_read_with_zero_side_effects() {
    let source = "state-read-3";
    {
        let conn = db::get().lock().unwrap();
        persistence::migrate(&conn).unwrap();
        cover_store::migrate(&conn).unwrap();
        for table in [
            "remote_cover_job",
            "remote_cover_variant",
            "remote_scan_epoch",
            "remote_view_revision",
        ] {
            conn.execute(&format!("DELETE FROM {table} WHERE source_id=?1"), [source])
                .unwrap();
        }
    }
    seed_running(source, "asset");

    let (jobs_before, revision_before) = {
        let conn = db::get().lock().unwrap();
        let jobs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM remote_cover_job WHERE source_id=?1",
                [source],
                |row| row.get(0),
            )
            .unwrap();
        (jobs, cover_store::view_revision(&conn, source).unwrap())
    };
    reset_wake_decided_count();
    let wake_before = wake_decided_count();

    // 读两次，确认幂等且无副作用。
    let _ = remote_cover_state(source.to_string(), "asset".to_string()).unwrap();
    let _ = remote_cover_state(source.to_string(), "asset".to_string()).unwrap();

    let (jobs_after, revision_after) = {
        let conn = db::get().lock().unwrap();
        let jobs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM remote_cover_job WHERE source_id=?1",
                [source],
                |row| row.get(0),
            )
            .unwrap();
        (jobs, cover_store::view_revision(&conn, source).unwrap())
    };
    assert_eq!(jobs_after, jobs_before, "STATE-READ-3: job rows must not change");
    assert_eq!(
        revision_after, revision_before,
        "STATE-READ-3: a pure read must NOT bump the revision"
    );
    assert_eq!(
        wake_decided_count(),
        wake_before,
        "STATE-READ-3: a pure read must NOT wake"
    );
}
