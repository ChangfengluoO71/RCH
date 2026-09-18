//! P1-E：cover durable transition 的 **post-commit wake** 覆盖契约（STREAM Layer A）。
//!
//! 证据边界（重要）：
//!
//! * 本文件证明的是 **Rust 侧**：`production code reached post-commit notify decision`
//!   —— 即"成功 bump revision 的正常 production path 都存在对应的 commit-after-wake"。
//! * 它**不是** Dart/FRB delivery 计数：`wake_decided_count()` 只记录"决定唤醒"，
//!   与"事件是否真的送达 Dart 并被消费"是两件事。后者由 Flutter/coordinator 侧证明。
//!
//! 冻结原则：
//!
//! * 每个成功 bump 的正常 production path ⟺ 存在对应 commit-after-wake。
//! * 一个 atomic batch bump 一次，**只 wake 一次**。
//! * 无真实 durable 变化（0 affected / 竞争失败 / 等价 upsert）⇒ revision **与** wake 都 +0。
//! * rollback ⇒ durable state、revision、wake 三者都不变（证明 notify 不在事务内提前发生）。

use rusqlite::{params, Connection};
use rust_lib_app::remote_scan::cover_model::{CoverJobKey, CoverJobState};
use rust_lib_app::remote_scan::cover_revision_stream::{reset_wake_decided_count, wake_decided_count};
use rust_lib_app::remote_scan::cover_state::CoverJobUpsertCause;
use rust_lib_app::remote_scan::{cover_service, cover_store, persistence};

const SIX_HOURS_MS: i64 = 6 * 60 * 60 * 1000;
const SELECTION: &str = "page:2|crop:|asset:";
const PROFILE: &str = "170x240@1";

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

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

fn bind_epoch(conn: &Connection, source_id: &str, generation: i64, session_epoch: &str, token: i64) {
    conn.execute(
        "INSERT OR REPLACE INTO remote_scan_epoch(
             source_id,generation,source_fingerprint,root_path,session_epoch,session_token)
         VALUES(?1,?2,'fp','/',?3,?4)",
        params![source_id, generation, session_epoch, token],
    )
    .unwrap();
}

fn revision(conn: &Connection, source_id: &str) -> i64 {
    cover_store::view_revision(conn, source_id).unwrap()
}

fn seed(conn: &Connection, key: &CoverJobKey, state: CoverJobState, now: i64) {
    cover_store::upsert_job_on(
        conn,
        key,
        state,
        "background",
        10,
        1,
        "e1",
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

fn woken() -> u64 {
    wake_decided_count()
}

/// 一次逻辑 transition：revision 与 wake 必须**同步**（同 +1 / 同 +0）。
fn assert_sync(before_rev: i64, before_wake: u64, conn: &Connection, source: &str, what: &str) {
    let rev_delta = revision(conn, source) - before_rev;
    let wake_delta = woken() - before_wake;
    assert_eq!(rev_delta, 1, "{what}: revision must bump exactly once");
    assert_eq!(
        wake_delta, 1,
        "{what}: a successful bump MUST have a matching post-commit wake"
    );
}

fn assert_no_change(before_rev: i64, before_wake: u64, conn: &Connection, source: &str, what: &str) {
    assert_eq!(revision(conn, source), before_rev, "{what}: revision must not change");
    assert_eq!(woken(), before_wake, "{what}: no durable change => no wake");
}

// ---------------------------------------------------------------------------
// STREAM-1A..1F：每个成功 transition 都有 post-commit wake
// ---------------------------------------------------------------------------

#[test]
fn stream_1a_create_to_pending_wakes_once() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    reset_wake_decided_count();
    let before_rev = revision(&conn, "source");
    let before_wake = woken();

    seed(&conn, &job_key("source", "a"), CoverJobState::Pending, 0);

    assert_sync(before_rev, before_wake, &conn, "source", "STREAM-1A create -> pending");
}

#[test]
fn stream_1b_claim_to_running_wakes_once() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    seed(&conn, &job_key("source", "a"), CoverJobState::Pending, 0);
    reset_wake_decided_count();
    let before_rev = revision(&conn, "source");
    let before_wake = woken();

    let claimed = cover_store::claim_next_job_for_source_session_on(
        &conn, "source", "worker", 1_000, 60_000, 42,
    )
    .unwrap();
    assert!(claimed.is_some());

    assert_sync(before_rev, before_wake, &conn, "source", "STREAM-1B claim -> running");
}

#[test]
fn stream_1c_ready_wakes_once() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "a");
    seed(&conn, &key, CoverJobState::Pending, 0);
    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", 1_000, 60_000, 42)
        .unwrap();
    reset_wake_decided_count();
    let before_rev = revision(&conn, "source");
    let before_wake = woken();

    let ok = cover_store::mark_job_ready_owned_on(
        &conn, &key, "worker", 2_000, 340, 480, 4_096, "checksum",
    )
    .unwrap();
    assert!(ok);

    // E-SCAN-TERMINAL-READY 的核心生产事件：*必须*有 post-commit wake，
    // 不能只靠 missed-event recovery 兜底。
    assert_sync(before_rev, before_wake, &conn, "source", "STREAM-1C running -> ready");
}

#[test]
fn stream_1d_failure_and_state_transitions_wake_once() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);

    let a = job_key("source", "a");
    seed(&conn, &a, CoverJobState::Pending, 0);
    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", 1_000, 60_000, 42)
        .unwrap();
    reset_wake_decided_count();
    let mut rev = revision(&conn, "source");
    let mut wake = woken();
    cover_store::mark_job_failure_owned_on(
        &conn, &a, "worker", CoverJobState::RetryWait, Some("transient"), 2_000, None,
    )
    .unwrap();
    assert_sync(rev, wake, &conn, "source", "STREAM-1D running -> retry_wait");
    rev = revision(&conn, "source");
    wake = woken();

    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", 3_000, 60_000, 42)
        .unwrap();
    rev = revision(&conn, "source");
    wake = woken();

    cover_store::mark_job_failure_owned_on(
        &conn,
        &a,
        "worker",
        CoverJobState::Failed,
        Some("transient"),
        4_000,
        Some(SIX_HOURS_MS),
    )
    .unwrap();
    assert_sync(rev, wake, &conn, "source", "STREAM-1D running -> failed");
    rev = revision(&conn, "source");
    wake = woken();

    let b = job_key("source", "b");
    seed(&conn, &b, CoverJobState::Pending, 5_000);
    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", 6_000, 60_000, 42)
        .unwrap();
    rev = revision(&conn, "source");
    wake = woken();
    cover_store::mark_job_state_owned_on(
        &conn, &b, "worker", CoverJobState::Blocked, Some("authExpired"), 7_000,
    )
    .unwrap();
    assert_sync(rev, wake, &conn, "source", "STREAM-1D running -> blocked");
}

#[test]
fn stream_1f_compensation_batch_wakes_once_regardless_of_batch_size() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    // 三本同时 eligible 的 failed job —— 一次 reconcile batch 全部 promotion。
    for asset in ["a", "b", "c"] {
        let key = job_key("source", asset);
        seed(&conn, &key, CoverJobState::Failed, 0);
        conn.execute(
            "UPDATE remote_cover_job SET long_retry_not_before=?1,long_retry_consumed=0,long_retry_pending=0
              WHERE job_key=?2",
            params![SIX_HOURS_MS, key.encode()],
        )
        .unwrap();
    }
    reset_wake_decided_count();
    let before_rev = revision(&conn, "source");
    let before_wake = woken();

    let report = cover_store::reconcile_cover_compensation_for_source_on(
        &conn, "source", 42, SIX_HOURS_MS, budget(64),
    )
    .unwrap();
    assert_eq!(report.compensation_promoted, 3, "the batch must promote all three");

    // atomic batch：revision +1，wake **也只 +1**（source-generation token，不是 row counter）。
    assert_sync(before_rev, before_wake, &conn, "source", "STREAM-1F compensation batch");
}

// ---------------------------------------------------------------------------
// STREAM-2：没有真实 durable 变化 ⇒ revision 与 wake 都 +0
//（U-B：**等价 upsert 不属于此类** —— 它刷新 updated_at，见 UPSERT-OBS-1/2）
// ---------------------------------------------------------------------------

#[test]
fn stream_2_no_durable_change_means_no_wake() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "a");
    seed(&conn, &key, CoverJobState::Pending, 0);
    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", 1_000, 60_000, 42)
        .unwrap();
    reset_wake_decided_count();
    let rev = revision(&conn, "source");
    let wake = woken();

    // (a) claim 竞争失败：没有可 claim 的 job
    let none = cover_store::claim_next_job_for_source_session_on(
        &conn, "source", "worker", 2_000, 60_000, 42,
    )
    .unwrap();
    assert!(none.is_none());
    assert_no_change(rev, wake, &conn, "source", "STREAM-2(a) lost claim race");

    // (b) wrong lease owner
    let ok = cover_store::mark_job_ready_owned_on(
        &conn, &key, "not-the-owner", 3_000, 340, 480, 4_096, "checksum",
    )
    .unwrap();
    assert!(!ok);
    assert_no_change(rev, wake, &conn, "source", "STREAM-2(b) wrong lease owner");

    // 注意（U-B，已裁决）：**等价 upsert 不算 no-op** —— `upsert_job_on` 的
    // `ON CONFLICT ... DO UPDATE SET updated_at=excluded.updated_at` 是无条件刷新
    // 时间戳的，而 `refresh_status_counts` 按 `updated_at` 取 latest-per-asset，
    // 因此等价 upsert 会改变"当前 job"的选择 ⇒ 下游可观察 ⇒ +1 revision/+1 wake
    // 才是正确行为。该语义由 UPSERT-OBS-1 / UPSERT-OBS-2 钉住。
    let _ = state_of(&conn, &key);
}

// ---------------------------------------------------------------------------
// UPSERT-OBS-1/2：钉住 U-B —— `updated_at` 刷新是可观察语义
// ---------------------------------------------------------------------------

/// 复刻 `refresh_status_counts` 的 latest-per-asset 选择（按 updated_at 取最新）。
fn latest_state_for_asset(conn: &Connection, source: &str, asset: &str, generation: i64) -> String {
    conn.query_row(
        "SELECT state FROM remote_cover_job
          WHERE source_id=?1 AND asset_id=?2 AND generation=?3
          ORDER BY updated_at DESC, job_key DESC LIMIT 1",
        params![source, asset, generation],
        |row| row.get(0),
    )
    .unwrap()
}

fn upsert_at(conn: &Connection, key: &CoverJobKey, state: CoverJobState, generation: i64, now: i64) {
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

#[test]
fn upsert_obs_1_equivalent_upsert_still_refreshes_updated_at() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "a");
    upsert_at(&conn, &key, CoverJobState::Ready, 1, 100);
    let before_updated: i64 = conn
        .query_row(
            "SELECT updated_at FROM remote_cover_job WHERE job_key=?1",
            [key.encode()],
            |row| row.get(0),
        )
        .unwrap();

    reset_wake_decided_count();
    let before_rev = revision(&conn, "source");
    let before_wake = woken();

    // 所有 lifecycle / retry / session 字段都相同，只有 now 不同。
    upsert_at(&conn, &key, CoverJobState::Ready, 1, 300);

    let after_updated: i64 = conn
        .query_row(
            "SELECT updated_at FROM remote_cover_job WHERE job_key=?1",
            [key.encode()],
            |row| row.get(0),
        )
        .unwrap();
    assert_ne!(
        before_updated, after_updated,
        "UPSERT-OBS-1: the upsert refreshes updated_at unconditionally"
    );
    assert_eq!(after_updated, 300);
    // ⇒ 不是 no-op：revision 与 wake 都必须推进（U-B）。
    assert_sync(before_rev, before_wake, &conn, "source", "UPSERT-OBS-1");
}

#[test]
fn upsert_obs_2_equivalent_upsert_can_regain_latest_per_asset() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);

    // 同一 asset 上的两行（不同 content_revision），updated_at 顺序决定"当前 job"。
    let mut older = job_key("source", "a");
    older.content_revision = "old".into();
    let mut newer = job_key("source", "a");
    newer.content_revision = "new".into();

    upsert_at(&conn, &older, CoverJobState::Ready, 1, 100);
    upsert_at(&conn, &newer, CoverJobState::Failed, 1, 200);
    assert_eq!(
        latest_state_for_asset(&conn, "source", "a", 1),
        "failed",
        "the newer row is currently the visible truth for this asset"
    );

    reset_wake_decided_count();
    let before_rev = revision(&conn, "source");
    let before_wake = woken();

    // 对**旧** row 做等价 upsert（state / generation / session 均不变），只更新 now。
    upsert_at(&conn, &older, CoverJobState::Ready, 1, 300);

    assert_eq!(
        latest_state_for_asset(&conn, "source", "a", 1),
        "ready",
        "UPSERT-OBS-2: the refreshed timestamp makes the old row latest again — \
         this IS a downstream-observable change, so the wake is correct"
    );
    assert_sync(before_rev, before_wake, &conn, "source", "UPSERT-OBS-2");
}

// ---------------------------------------------------------------------------
// STREAM-3：rollback ⇒ durable state / revision / wake 三者都不变
// ---------------------------------------------------------------------------

#[test]
fn stream_3_rollback_leaves_state_revision_and_wake_untouched() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "a");
    seed(&conn, &key, CoverJobState::Pending, 0);
    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", 1_000, 60_000, 42)
        .unwrap();
    reset_wake_decided_count();
    let rev = revision(&conn, "source");
    let wake = woken();
    let state_before = state_of(&conn, &key);

    // 调用者自持事务：函数不拥有事务 ⇒ 不 emit；随后**不提交**直接丢弃 ⇒ rollback。
    {
        let tx = conn.unchecked_transaction().unwrap();
        let ok = cover_store::mark_job_state_owned_on(
            &tx,
            &key,
            "worker",
            CoverJobState::Failed,
            Some("transient"),
            2_000,
        )
        .unwrap();
        assert!(ok, "the mutation itself succeeds inside the caller's transaction");
        // drop(tx) == rollback
    }

    assert_eq!(state_of(&conn, &key), state_before, "rollback must not persist the mutation");
    assert_eq!(revision(&conn, "source"), rev, "rollback must not persist the revision");
    assert_eq!(
        woken(),
        wake,
        "STREAM-3: a rolled-back transaction must NOT wake (notify is not emitted inside the tx)"
    );
}

// ---------------------------------------------------------------------------
// STREAM-1E：P1-B ready-missing 对账（真实 production path）也有 wake
// ---------------------------------------------------------------------------

#[test]
fn stream_1e_ready_missing_reconcile_wakes_once() {
    let root = std::env::temp_dir().join("rch_stream1e");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    rust_lib_app::cache::set_custom_cache_root(root.to_str().unwrap());
    let _guard = CacheRootGuard;

    let conn = rust_lib_app::db::get().lock().unwrap();
    persistence::migrate(&conn).unwrap();
    cover_store::migrate(&conn).unwrap();
    for table in [
        "remote_cover_job",
        "remote_cover_variant",
        "remote_scan_epoch",
        "remote_view_revision",
    ] {
        conn.execute(&format!("DELETE FROM {table} WHERE source_id=?1"), ["s1e"])
            .unwrap();
    }
    conn.execute(
        "INSERT OR REPLACE INTO book_sources(id,type,name) VALUES('s1e','115','stream1e')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO remote_scan_epoch(
             source_id,generation,source_fingerprint,root_path,session_epoch,session_token)
         VALUES('s1e',1,'fp','/','epoch',42)",
        [],
    )
    .unwrap();
    cover_store::upsert_job_on(
        &conn,
        &CoverJobKey {
            source_id: "s1e".into(),
            asset_id: "asset".into(),
            content_revision: "content".into(),
            selection_revision: SELECTION.into(),
            profile: PROFILE.into(),
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
         VALUES('s1e','asset','content',?1,?2,'ready',1,1)",
        params![SELECTION, PROFILE],
    )
    .unwrap();

    // 必须先释放全局 DB 锁：read_cached_cover 内部会再次加锁（否则死锁）。
    drop(conn);

    reset_wake_decided_count();
    let before_rev = {
        let conn = rust_lib_app::db::get().lock().unwrap();
        revision(&conn, "s1e")
    };
    let before_wake = woken();

    // 真实 production path：探测到 ready 但字节缺失 → 对账回 pending。
    let _ = cover_service::read_cached_cover("s1e", "asset", SELECTION, PROFILE).unwrap();

    let conn = rust_lib_app::db::get().lock().unwrap();
    assert_sync(before_rev, before_wake, &conn, "s1e", "STREAM-1E ready-missing reconcile");
}

/// 恢复默认 cache root，避免污染同进程内其它测试。
struct CacheRootGuard;
impl Drop for CacheRootGuard {
    fn drop(&mut self) {
        rust_lib_app::cache::set_custom_cache_root("");
    }
}
