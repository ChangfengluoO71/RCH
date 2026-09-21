//! P1-E：`remote_cover_state(source_id, asset_id, selection, profile)` 只读契约
//!（STATE-READ-1~4）。
//!
//! 硬契约：read only —— 不 enqueue / 不 wake / 不建 session / 不访问 provider /
//! 不修改 durable state / 不 bump revision。`None` = no job。
//!
//! 第 79 轮续（真机 bug）：读取**必须按传入的 selection + profile** 作用域 ——
//! 卡片按 `coverQuality` 取图（low = `170x240@1`），若读状态写死 `340x480@1`，就会
//! 出现"详情页有图、海报墙获取失败"（见 STATE-READ-4）。

use rusqlite::{params, Connection};
use rust_lib_app::api::remote_cover::{
    remote_cover_state, CoverProfileDto, CoverSelectionDto,
};
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

/// 读取键：与 `job_key` 同源（`default` 选择 + 指定 profile）。
fn state_keys_for(width: u32, height: u32) -> (CoverSelectionDto, CoverProfileDto) {
    (
        CoverSelectionDto {
            page: 0,
            crop: None,
            explicit_asset_id: None,
            revision: "default".into(),
        },
        CoverProfileDto {
            width,
            height,
            decoder_version: 1,
        },
    )
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

    let (selection, profile) = state_keys_for(340, 480);
    let state = remote_cover_state(source.to_string(), "asset".to_string(), selection, profile)
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
    let (selection, profile) = state_keys_for(340, 480);
    assert!(
        remote_cover_state(source.to_string(), "nope".to_string(), selection, profile)
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
    let (selection, profile) = state_keys_for(340, 480);
    let _ = remote_cover_state(
        source.to_string(),
        "asset".to_string(),
        selection,
        profile,
    )
    .unwrap();
    let (selection, profile) = state_keys_for(340, 480);
    let _ = remote_cover_state(
        source.to_string(),
        "asset".to_string(),
        selection,
        profile,
    )
    .unwrap();

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

// ------------------------------------------------------------- STATE-READ-4

/// 第 79 轮续（真机 bug 回归）：state 读取必须**按传入的 selection + profile** 作用域。
///
/// 真机现象：卡片按设置 `coverQuality`（low = `170x240@1`）请求并渲染封面，而状态读取
/// 写死 `default` + `340x480@1` ⇒ 同一 asset 上 170 已 ready、340 停在旧 failed 时，
/// 墙面显示"获取失败"，而详情页（同一 profile 读图）却能看到图。
#[test]
fn state_read_4_is_scoped_by_selection_and_profile() {
    let source = "state-read-4";
    let asset = "asset";
    {
        let conn = db::get().lock().unwrap();
        persistence::migrate(&conn).unwrap();
        cover_store::migrate(&conn).unwrap();
        for table in ["remote_cover_job", "remote_cover_variant", "remote_scan_epoch"] {
            conn.execute(&format!("DELETE FROM {table} WHERE source_id=?1"), [source])
                .unwrap();
        }
        conn.execute(
            "INSERT OR REPLACE INTO remote_scan_epoch(
                 source_id,generation,source_fingerprint,root_path,session_epoch,session_token)
             VALUES(?1,1,'fp','/','e1',42)",
            params![source],
        )
        .unwrap();
        // 同一 asset、同一 selection，两个 profile 的状态故意相反。
        for (profile, state) in [("170x240@1", "ready"), ("340x480@1", "failed")] {
            conn.execute(
                "INSERT OR REPLACE INTO remote_cover_variant(
                     source_id,asset_id,content_revision,selection_revision,profile,
                     blob_key,state,revision,is_previous_revision,updated_at)
                 VALUES(?1,?2,'content','default',?3,NULL,?4,1,0,1000)",
                params![source, asset, profile, state],
            )
            .unwrap();
            cover_store::upsert_job_on(
                &conn,
                &CoverJobKey {
                    source_id: source.into(),
                    asset_id: asset.into(),
                    content_revision: "content".into(),
                    selection_revision: "default".into(),
                    profile: profile.into(),
                },
                if state == "ready" {
                    CoverJobState::Ready
                } else {
                    CoverJobState::Failed
                },
                "visible",
                300,
                1,
                "e1",
                2000,
                CoverJobUpsertCause::Demand,
            )
            .unwrap();
        }
    }

    let (selection, profile) = state_keys_for(170, 240);
    let low = remote_cover_state(source.to_string(), asset.to_string(), selection, profile)
        .unwrap()
        .expect("170x240@1 必须被报告");
    assert_eq!(
        low.state, "ready",
        "STATE-READ-4: 170x240@1 必须读到自己那条 ready（真机 bug：曾读到 340 的 failed）"
    );
    assert!(low.ready);

    let (selection, profile) = state_keys_for(340, 480);
    let high = remote_cover_state(source.to_string(), asset.to_string(), selection, profile)
        .unwrap()
        .expect("340x480@1 必须被报告");
    assert_eq!(high.state, "failed", "340x480@1 的旧失败态不受影响");
    assert!(!high.ready);
}
