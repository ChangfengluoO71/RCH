//! P1-E：cover durable transition 必须推进 durable `remote_view_revision`（REV 契约）。
//!
//! 语义（审阅冻结）：
//!
//! - `remote_view_revision` 是 **source-scoped、monotonic、durable** 的版本事实；
//!   P1-E 复用它（不新增第二张 revision 表，不新增 `remote_cover_revision`）。
//! - **每个成功的逻辑 durable cover-state transition 恰好 +1**。
//! - **没有真正改变 durable 状态**的写（affected = 0、lease owner 不匹配、条件未满足）
//!   **不得**推进 revision —— 禁止制造"假 revision"。
//! - bump 必须与 state mutation 在**同一事务**内（crash 不得漏 bump）。
//! - bump 必须绑定该 job **真实的** `(source_id, generation)`，不得拿"当前最新 generation"猜。

use rusqlite::{params, Connection};
use rust_lib_app::remote_scan::cover_model::{CoverJobKey, CoverJobState};
use rust_lib_app::remote_scan::cover_state::CoverJobUpsertCause;
use rust_lib_app::remote_scan::{cover_store, persistence};
use rust_lib_app::{cache, db};
use rust_lib_app::remote_scan::cover_service;

const SIX_HOURS_MS: i64 = 6 * 60 * 60 * 1000;

// ---------------------------------------------------------------------------
// helpers（沿用既有 P1-C 契约测试的 fixture 模式）
// ---------------------------------------------------------------------------

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

fn connection() -> Connection {
    let conn = base_connection();
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

fn bind_epoch(conn: &Connection, source_id: &str, generation: i64, session_epoch: &str, token: i64) {
    conn.execute(
        "INSERT OR REPLACE INTO remote_scan_epoch(
             source_id,generation,source_fingerprint,root_path,session_epoch,session_token)
         VALUES(?1,?2,'fp','/',?3,?4)",
        params![source_id, generation, session_epoch, token],
    )
    .unwrap();
}

/// durable revision（无行 = 0）。
fn revision(conn: &Connection, source_id: &str) -> i64 {
    cover_store::view_revision(conn, source_id).unwrap()
}

fn seed_job(
    conn: &Connection,
    key: &CoverJobKey,
    state: CoverJobState,
    generation: i64,
    epoch: &str,
    now: i64,
) {
    cover_store::upsert_job_on(
        conn,
        key,
        state,
        "background",
        10,
        generation,
        epoch,
        now,
        CoverJobUpsertCause::Demand,
    )
    .unwrap();
}

fn state_of(conn: &Connection, key: &CoverJobKey) -> String {
    conn.query_row(
        "SELECT state FROM remote_cover_job WHERE job_key=?1",
        [key.encode()],
        |row| row.get(0),
    )
    .unwrap()
}

fn budget(max_jobs: usize) -> cover_store::ReconcileBudget {
    cover_store::ReconcileBudget {
        max_jobs,
        max_wall_time_ms: 5_000,
    }
}

/// REV-1 / REV-2 / REV-3 / REV-4 / REV-5 / REV-6 / REV-7 / REV-8 的统一断言：
/// 一次成功的逻辑 durable transition 恰好把 revision 推进 1。
fn assert_delta(conn: &Connection, source_id: &str, before: i64, what: &str) {
    let after = revision(conn, source_id);
    assert_eq!(
        after - before,
        1,
        "{what}: a single successful durable transition must bump the revision by exactly 1 \
         (before={before} after={after})"
    );
}

// ---------------------------------------------------------------------------
// REV-1：job create → pending
// ---------------------------------------------------------------------------

#[test]
fn rev1_job_creation_bumps_revision_once() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let before = revision(&conn, "source");
    let key = job_key("source", "asset");
    seed_job(&conn, &key, CoverJobState::Pending, 1, "e1", 0);
    assert_eq!(state_of(&conn, &key), "pending");
    assert_delta(&conn, "source", before, "REV-1 job create -> pending");
}

// ---------------------------------------------------------------------------
// REV-2：pending → running（claim）
// ---------------------------------------------------------------------------

#[test]
fn rev2_claim_bumps_revision_once() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "asset");
    seed_job(&conn, &key, CoverJobState::Pending, 1, "e1", 0);
    let before = revision(&conn, "source");

    let claimed = cover_store::claim_next_job_for_source_session_on(
        &conn, "source", "worker", 1_000, 60_000, 42,
    )
    .unwrap();
    assert!(claimed.is_some(), "the pending job must be claimable");
    assert_eq!(state_of(&conn, &key), "running");
    assert_delta(&conn, "source", before, "REV-2 pending -> running");
}

// ---------------------------------------------------------------------------
// REV-3：running → ready
// ---------------------------------------------------------------------------

#[test]
fn rev3_ready_bumps_revision_once() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "asset");
    seed_job(&conn, &key, CoverJobState::Pending, 1, "e1", 0);
    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", 1_000, 60_000, 42)
        .unwrap();
    let before = revision(&conn, "source");

    let ok = cover_store::mark_job_ready_owned_on(
        &conn, &key, "worker", 2_000, 340, 480, 4_096, "checksum",
    )
    .unwrap();
    assert!(ok, "the lease owner must be able to mark the job ready");
    assert_eq!(state_of(&conn, &key), "ready");
    assert_delta(&conn, "source", before, "REV-3 running -> ready");
}

// ---------------------------------------------------------------------------
// REV-4：running → retry_wait
// ---------------------------------------------------------------------------

#[test]
fn rev4_retry_wait_bumps_revision_once() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "asset");
    seed_job(&conn, &key, CoverJobState::Pending, 1, "e1", 0);
    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", 1_000, 60_000, 42)
        .unwrap();
    let before = revision(&conn, "source");

    let ok = cover_store::mark_job_failure_owned_on(
        &conn,
        &key,
        "worker",
        CoverJobState::RetryWait,
        Some("transient"),
        2_000,
        None,
    )
    .unwrap();
    assert!(ok);
    assert_eq!(state_of(&conn, &key), "retry_wait");
    assert_delta(&conn, "source", before, "REV-4 running -> retry_wait");
}

// ---------------------------------------------------------------------------
// REV-5：running → failed
// ---------------------------------------------------------------------------

#[test]
fn rev5_failed_bumps_revision_once() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "asset");
    seed_job(&conn, &key, CoverJobState::Pending, 1, "e1", 0);
    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", 1_000, 60_000, 42)
        .unwrap();
    let before = revision(&conn, "source");

    let ok = cover_store::mark_job_failure_owned_on(
        &conn,
        &key,
        "worker",
        CoverJobState::Failed,
        Some("transient"),
        2_000,
        Some(SIX_HOURS_MS),
    )
    .unwrap();
    assert!(ok);
    assert_eq!(state_of(&conn, &key), "failed");
    assert_delta(&conn, "source", before, "REV-5 running -> failed");
}

// ---------------------------------------------------------------------------
// REV-6：→ unsupported / → blocked
// ---------------------------------------------------------------------------

#[test]
fn rev6_unsupported_and_blocked_bump_revision_once_each() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);

    // unsupported
    let a = job_key("source", "asset-a");
    seed_job(&conn, &a, CoverJobState::Pending, 1, "e1", 0);
    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", 1_000, 60_000, 42)
        .unwrap();
    let before_a = revision(&conn, "source");
    let ok = cover_store::mark_job_state_owned_on(
        &conn,
        &a,
        "worker",
        CoverJobState::Unsupported,
        None,
        2_000,
    )
    .unwrap();
    assert!(ok);
    assert_eq!(state_of(&conn, &a), "unsupported");
    assert_delta(&conn, "source", before_a, "REV-6 running -> unsupported");

    // blocked
    let b = job_key("source", "asset-b");
    seed_job(&conn, &b, CoverJobState::Pending, 1, "e1", 3_000);
    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", 4_000, 60_000, 42)
        .unwrap();
    let before_b = revision(&conn, "source");
    let ok = cover_store::mark_job_state_owned_on(
        &conn,
        &b,
        "worker",
        CoverJobState::Blocked,
        Some("authExpired"),
        5_000,
    )
    .unwrap();
    assert!(ok);
    assert_eq!(state_of(&conn, &b), "blocked");
    assert_delta(&conn, "source", before_b, "REV-6 running -> blocked");
}

// ---------------------------------------------------------------------------
// REV-7：ready-missing 对账必须走**真实 production path**（P1-B），不得用
// `mark_job_state_owned_on` 模拟 —— 那个写函数的谓词是 `state='running'`，
// 对 ready 行 affected = 0，**根本不构成 ready → pending 的写路径**。
//
// 真实路径：`remote_cover_read` → `cover_service::read_cached_cover`：
// 探测到 ready 记录但磁盘字节缺失 → **同一事务内** 把 job/variant 对账回 pending
// → 并在该事务内推进 durable revision。
// ---------------------------------------------------------------------------

const REV7_SELECTION: &str = "page:2|crop:|asset:";
const REV7_PROFILE: &str = "170x240@1";

fn rev7_prepare(source_id: &str) {
    let root = std::env::temp_dir().join(format!("rch_rev7_{source_id}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    cache::set_custom_cache_root(root.to_str().unwrap());

    let conn = db::get().lock().unwrap();
    persistence::migrate(&conn).unwrap();
    cover_store::migrate(&conn).unwrap();
    for table in [
        "remote_cover_job",
        "remote_cover_variant",
        "remote_scan_epoch",
        "remote_view_revision",
    ] {
        conn.execute(&format!("DELETE FROM {table} WHERE source_id=?1"), [source_id])
            .unwrap();
    }
    conn.execute(
        "INSERT OR REPLACE INTO book_sources(id,type,name) VALUES(?1,'115','rev7')",
        [source_id],
    )
    .unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO remote_scan_epoch(
             source_id,generation,source_fingerprint,root_path,session_epoch,session_token)
         VALUES(?1,1,'fp','/','epoch',42)",
        [source_id],
    )
    .unwrap();
    // ready 的 job + ready 的 variant，updated_at 一致；磁盘字节由调用方决定是否有。
    cover_store::upsert_job_on(
        &conn,
        &CoverJobKey {
            source_id: source_id.into(),
            asset_id: "asset".into(),
            content_revision: "content".into(),
            selection_revision: REV7_SELECTION.into(),
            profile: REV7_PROFILE.into(),
        },
        CoverJobState::Ready,
        "background",
        10,
        1,
        "epoch",
        1,
        CoverJobUpsertCause::Demand,
    )
    .unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO remote_cover_variant(
             source_id,asset_id,content_revision,selection_revision,profile,state,revision,updated_at)
         VALUES(?1,'asset','content',?2,?3,'ready',1,1)",
        params![source_id, REV7_SELECTION, REV7_PROFILE],
    )
    .unwrap();
}

fn rev7_revision(source_id: &str) -> i64 {
    let conn = db::get().lock().unwrap();
    cover_store::view_revision(&conn, source_id).unwrap()
}

fn rev7_state(source_id: &str) -> String {
    let conn = db::get().lock().unwrap();
    conn.query_row(
        "SELECT state FROM remote_cover_job WHERE source_id=?1 AND asset_id='asset'",
        [source_id],
        |row| row.get(0),
    )
    .unwrap()
}

/// 驱动真实生产路径。
fn rev7_read(source_id: &str) {
    let _ = cover_service::read_cached_cover(source_id, "asset", REV7_SELECTION, REV7_PROFILE);
}

#[test]
fn rev7_ready_with_missing_material_is_reconciled_and_bumps_revision_once() {
    let source = "rev7-missing";
    rev7_prepare(source);
    let before = rev7_revision(source);
    assert_eq!(rev7_state(source), "ready");

    // 磁盘上没有字节 ⇒ 真实读路径必须探测到缺失并对账。
    rev7_read(source);

    assert_eq!(rev7_state(source), "pending", "ready + missing bytes => pending");
    let after = rev7_revision(source);
    assert_eq!(after - before, 1, "the real reconcile transition must bump exactly once");
}

#[test]
fn rev7_ready_with_present_material_does_not_bump_revision() {
    let source = "rev7-present";
    rev7_prepare(source);
    // 写入真实 cover material（size = profile 170x240）。
    cache::remote_cover_cache_write(
        source,
        "asset",
        "content",
        REV7_SELECTION,
        REV7_PROFILE,
        170,
        240,
        &vec![3_u8; 170 * 240 * 4],
    )
    .unwrap();
    let before = rev7_revision(source);

    rev7_read(source);

    assert_eq!(rev7_state(source), "ready", "material present => no reconcile");
    assert_eq!(
        rev7_revision(source),
        before,
        "a read that changes nothing must NOT bump the revision"
    );
}

// ---------------------------------------------------------------------------
// REV-8：long compensation failed → pending
// ---------------------------------------------------------------------------

#[test]
fn rev8_long_compensation_promotion_bumps_revision_once() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "asset");
    seed_job(&conn, &key, CoverJobState::Failed, 1, "e1", 0);
    conn.execute(
        "UPDATE remote_cover_job SET long_retry_not_before=?1,long_retry_consumed=0,long_retry_pending=0
          WHERE job_key=?2",
        params![SIX_HOURS_MS, key.encode()],
    )
    .unwrap();
    let before = revision(&conn, "source");

    let report = cover_store::reconcile_cover_compensation_for_source_on(
        &conn,
        "source",
        42,
        SIX_HOURS_MS,
        budget(64),
    )
    .unwrap();
    assert_eq!(report.compensation_promoted, 1);
    assert_eq!(state_of(&conn, &key), "pending");
    assert_delta(&conn, "source", before, "REV-8 failed -> pending (long compensation)");
}

// ---------------------------------------------------------------------------
// REV-9：失败的 mutation（无真实 durable 变化）不得推进 revision
// ---------------------------------------------------------------------------

#[test]
fn rev9_mutations_without_durable_effect_do_not_bump_revision() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "asset");
    seed_job(&conn, &key, CoverJobState::Pending, 1, "e1", 0);
    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", 1_000, 60_000, 42)
        .unwrap();
    let before = revision(&conn, "source");

    // (a) 错误 lease owner：affected = 0
    let ok = cover_store::mark_job_ready_owned_on(
        &conn, &key, "not-the-owner", 2_000, 340, 480, 4_096, "checksum",
    )
    .unwrap();
    assert!(!ok, "a foreign owner must not be able to mutate the job");
    assert_eq!(state_of(&conn, &key), "running");
    assert_eq!(
        revision(&conn, "source"),
        before,
        "REV-9(a) a 0-affected mutation must NOT bump the revision"
    );

    // (b) claim 竞争失败（没有可 claim 的 job）
    let none = cover_store::claim_next_job_for_source_session_on(
        &conn, "source", "worker", 3_000, 60_000, 42,
    )
    .unwrap();
    assert!(none.is_none(), "there is no claimable job left");
    assert_eq!(
        revision(&conn, "source"),
        before,
        "REV-9(b) a lost claim race must NOT bump the revision"
    );

    // (c) lease 续约失败（不是 owner）
    let leased = cover_store::lease_job_on(&conn, &key, "not-the-owner", 4_000, 60_000).unwrap();
    assert!(!leased);
    assert_eq!(
        revision(&conn, "source"),
        before,
        "REV-9(c) a failed lease renewal must NOT bump the revision"
    );
}

// ---------------------------------------------------------------------------
// 统一：单一逻辑 transition 不得稳定产生 delta > 1
// ---------------------------------------------------------------------------

#[test]
fn rev_no_transition_ever_bumps_more_than_once() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "asset");

    let mut previous = revision(&conn, "source");
    seed_job(&conn, &key, CoverJobState::Pending, 1, "e1", 0);
    let mut current = revision(&conn, "source");
    assert_eq!(current - previous, 1, "create must bump exactly once");
    previous = current;

    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", 1_000, 60_000, 42)
        .unwrap();
    current = revision(&conn, "source");
    assert_eq!(current - previous, 1, "claim must bump exactly once");
    previous = current;

    cover_store::mark_job_ready_owned_on(
        &conn, &key, "worker", 2_000, 340, 480, 4_096, "checksum",
    )
    .unwrap();
    current = revision(&conn, "source");
    assert_eq!(
        current - previous,
        1,
        "ready must bump exactly once (no helper-stack double bump)"
    );
}

// ---------------------------------------------------------------------------
// generation 绑定：旧 generation 的 job 不得推进新 generation 的 revision 语义
// ---------------------------------------------------------------------------

#[test]
fn rev_bump_uses_the_jobs_own_generation() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "asset");
    seed_job(&conn, &key, CoverJobState::Pending, 1, "e1", 0);
    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", 1_000, 60_000, 42)
        .unwrap();
    cover_store::mark_job_ready_owned_on(
        &conn, &key, "worker", 2_000, 340, 480, 4_096, "checksum",
    )
    .unwrap();

    // revision 行记录的 listing_generation 必须来自该 job 自身的 generation（=1），
    // 而不是任何"当前最新"猜测。
    let recorded: i64 = conn
        .query_row(
            "SELECT listing_generation FROM remote_view_revision WHERE source_id=?1",
            ["source"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(recorded, 1, "the bump must be bound to the job's own generation");
}
