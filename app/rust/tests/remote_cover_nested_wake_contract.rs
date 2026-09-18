//! P1-E：**nested outer transaction** 的 post-commit wake 契约（NW-1~NW-4）。
//!
//! 设计（冻结）：
//!
//! * inner mutation helper 在 `owned_tx == false`（调用者持有事务）时**绝不 emit**。
//! * mutation 与 revision bump 都发生在**外层事务**内。
//! * 外层 transaction owner 在**自己的 commit 成功之后**，按"是否真的有 durable
//!   observable change"决定是否 emit 一次。
//! * 不允许：inner 提前 emit / outer rollback 后仍 emit / inner+outer double emit。

use rusqlite::{params, Connection};
use rust_lib_app::remote_scan::cover_model::{CoverJobKey, CoverJobState};
use rust_lib_app::remote_scan::cover_revision_stream::{reset_wake_decided_count, wake_decided_count};
use rust_lib_app::remote_scan::cover_state::CoverJobUpsertCause;
use rust_lib_app::remote_scan::{cover_store, persistence};

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

fn bind_epoch(conn: &Connection, source_id: &str, generation: i64, epoch: &str, token: i64) {
    conn.execute(
        "INSERT OR REPLACE INTO remote_scan_epoch(
             source_id,generation,source_fingerprint,root_path,session_epoch,session_token)
         VALUES(?1,?2,'fp','/',?3,?4)",
        params![source_id, generation, epoch, token],
    )
    .unwrap();
}

fn revision(conn: &Connection, source_id: &str) -> i64 {
    cover_store::view_revision(conn, source_id).unwrap()
}

fn woken() -> u64 {
    wake_decided_count()
}

/// 在**调用者事务内**做一次 upsert（inner 不 emit），由外层决定是否 emit。
fn seed_inside(
    conn: &Connection,
    key: &CoverJobKey,
    state: CoverJobState,
    generation: i64,
    now: i64,
) {
    cover_store::upsert_job_on(
        conn,
        key,
        state,
        "background",
        10,
        generation,
        "e1",
        now,
        CoverJobUpsertCause::Demand,
    )
    .unwrap();
}

// ---------------------------------------------------------------- NW-1 / NW-4
#[test]
fn nw1_outer_commit_wakes_exactly_once_after_commit() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    reset_wake_decided_count();
    let before_rev = revision(&conn, "source");
    let before_wake = woken();

    // 外层事务：inner mutation 只 mutate（U-α：**不** bump、**不** emit），
    // revision 由**合规的 outer owner**在同一个事务内推进一次。
    {
        let tx = conn.unchecked_transaction().unwrap();
        seed_inside(&tx, &job_key("source", "a"), CoverJobState::Pending, 1, 100);

        assert_eq!(
            woken(),
            before_wake,
            "NW-1: the inner mutation must NOT emit inside the caller's transaction"
        );
        assert_eq!(
            revision(&tx, "source"),
            before_rev,
            "U-α: a borrowed-transaction upsert must NOT bump the revision (the outer owner owns it)"
        );

        // outer owner 的批量 bump：同一个事务内推进一次。
        cover_store::bump_view_revision_on(&tx, "source", 1, 100).unwrap();
        tx.commit().unwrap();
    }

    // 外层 commit 之后：调用者 emit 一次。
    rust_lib_app::remote_scan::cover_revision_stream::notify_cover_revision("source", None);

    assert_eq!(revision(&conn, "source") - before_rev, 1, "NW-1: revision +1");
    assert_eq!(
        woken() - before_wake,
        1,
        "NW-4: exactly one wake after the outer commit (no inner+outer double wake)"
    );
}

// ------------------------------------------------------------------- NW-2
#[test]
fn nw2_outer_rollback_persists_nothing_and_never_wakes() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    reset_wake_decided_count();
    let before_rev = revision(&conn, "source");
    let before_wake = woken();

    {
        let tx = conn.unchecked_transaction().unwrap();
        seed_inside(&tx, &job_key("source", "a"), CoverJobState::Pending, 1, 100);
        // 故意不 commit：drop(tx) == rollback
    }

    let rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM remote_cover_job WHERE source_id='source'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0, "NW-2: the rolled-back mutation must not persist");
    assert_eq!(revision(&conn, "source"), before_rev, "NW-2: revision must not persist");
    assert_eq!(woken(), before_wake, "NW-2: a rollback must never wake");
}

// ------------------------------------------------------------------- NW-3
#[test]
fn nw3_outer_commit_without_durable_change_does_not_wake() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    // 先建立一个 running 的 job（autocommit 路径，自身会 wake；之后归零计数）。
    let key = job_key("source", "a");
    seed_inside(&conn, &key, CoverJobState::Pending, 1, 50);
    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", 60, 60_000, 42)
        .unwrap();

    reset_wake_decided_count();
    let before_rev = revision(&conn, "source");
    let before_wake = woken();

    let mut changed_any = false;
    {
        let tx = conn.unchecked_transaction().unwrap();
        // wrong lease owner ⇒ affected = 0 ⇒ 无 durable observable change
        let changed = cover_store::mark_job_state_owned_on(
            &tx,
            &key,
            "not-the-owner",
            CoverJobState::Failed,
            Some("transient"),
            70,
        )
        .unwrap();
        changed_any |= changed;
        tx.commit().unwrap();
    }
    if changed_any {
        rust_lib_app::remote_scan::cover_revision_stream::notify_cover_revision("source", None);
    }

    assert!(!changed_any, "NW-3: a 0-affected mutation reports no change");
    assert_eq!(revision(&conn, "source"), before_rev, "NW-3: revision +0");
    assert_eq!(woken(), before_wake, "NW-3: no durable change ⇒ no wake even after commit");
}

// ------------------------------------------------- NW-1b（真实 production owner）
#[test]
fn nw1b_missing_covers_reconcile_batch_wakes_once() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    // 库里有两本 eligible、但完全没有 job 行的漫画（真实补齐场景）。
    for (id, path) in [("b1", "/a.cbz"), ("b2", "/b.cbz")] {
        conn.execute(
            "INSERT OR REPLACE INTO library_index(
                 id,source_id,parent_id,name,path,entry_type,asset_kind,content_fingerprint,
                 scan_generation,listing_complete,deleted,updated_at)
             VALUES(?1,'source','/root',?2,?3,'file','ArchiveFile','cf1',1,1,0,1)",
            params![id, path, path],
        )
        .unwrap();
    }

    reset_wake_decided_count();
    let before_rev = revision(&conn, "source");
    let before_wake = woken();

    let report = cover_store::reconcile_missing_covers_for_source_on(
        &conn,
        "source",
        42,
        1_000,
        cover_store::ReconcileBudget {
            max_jobs: 64,
            max_wall_time_ms: 5_000,
        },
    )
    .unwrap();

    assert!(report.jobs_created >= 1, "the replenishment must create cover jobs");
    // 一次批次：revision +1，wake 只 +1（即使创建了多本）。
    assert_eq!(revision(&conn, "source") - before_rev, 1, "NW-1b: revision +1");
    assert_eq!(woken() - before_wake, 1, "NW-1b: exactly one source-level wake");
}

// ---------------------------------------------------------------- NW-1c
/// `publish_staged_generation` 是**真实**的 borrowed-transaction owner：
/// 一次事务里 materialize ≥2 个 cover job，仍然只 bump 一次 revision、只发一次 wake。
#[test]
fn nw1c_publish_staged_generation_batch_wakes_exactly_once() {
    let conn = connection();
    bind_epoch(&conn, "source", 2, "e1", 42);

    // publish_staged_generation 的第一句就是显式守卫：
    //   if !current_generation_is_active(conn, source_id, generation, "Running") {
    //       return Err(rusqlite::Error::InvalidQuery); }
    // 因此 fixture 必须把该 generation 置为 Running，否则我们看到的 InvalidQuery
    // 其实是**生产守卫**，不是 SQL 错误。
    conn.execute(
        "INSERT OR REPLACE INTO remote_scan_state(source_id,status,mode,generation)
         VALUES('source','Running','full',2)",
        [],
    )
    .unwrap();

    // 还需要 book_sources + 一行 remote_scan_listing_stage（其 LIMIT 1 查询非 optional）。
    conn.execute(
        "INSERT OR REPLACE INTO book_sources(id,type,fingerprint,path,root_id) VALUES('source','webdav','fp','/','')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO remote_scan_listing_stage(
             source_id,generation,logical_path,content_fingerprint,asset_kind,entries_json,incremental,session_epoch)
         VALUES('source',2,'/root','cf-root','ImageFolder','[]',0,'e1')",
        [],
    )
    .unwrap();

    // 两个 staged cover 依赖（remote_cover_stage 的真实 PK/列）。
    for (book_key, dependency_path) in [("b1", "/a.cbz"), ("b2", "/b.cbz")] {
        conn.execute(
            "INSERT OR REPLACE INTO remote_cover_stage(
                 source_id,generation,book_key,dependency_path,dependency_fingerprint,profile,session_epoch)
             VALUES('source',2,?1,?2,'cf1','340x480@1','e1')",
            params![book_key, dependency_path],
        )
        .unwrap();
    }

    reset_wake_decided_count();
    let before_rev = revision(&conn, "source");
    let before_wake = woken();

    persistence::publish_staged_generation(&conn, "source", 2).unwrap();

    let jobs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM remote_cover_job WHERE source_id='source'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        jobs >= 2,
        "NW-1c: the staged generation must materialize >= 2 cover jobs (got {jobs})"
    );
    assert_eq!(
        revision(&conn, "source") - before_rev,
        1,
        "NW-1c: one atomic durable view change => exactly one revision bump for the whole batch"
    );
    assert_eq!(
        woken() - before_wake,
        1,
        "NW-1c: exactly one cover wake for the whole batch (no per-job wake, no double bump)"
    );
}
