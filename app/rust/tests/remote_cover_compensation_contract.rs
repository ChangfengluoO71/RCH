//! P1-C：cover 长期补偿（long compensation）契约。
//!
//! 冻结语义（审阅通过）：
//!
//! - `6h = earliest eligibility`，**不是** exact timer deadline。补偿是
//!   **session-event-driven**，不是 wall-clock-driven。
//! - 两个重试维度**绝不共用计数器**：
//!   - A（既有、原样保留）：`attempt` + `next_attempt_at` 的短退避（`attempt < 3`，delay ≤ 15 min）；
//!   - B（本轮新增）：`failed` 后 6h 获得**一次**长期补偿资格。
//! - retryable 的判定**不来自错误字符串**，而来自既有短退避处理的同一错误集合
//!   （`TransientNetwork | RateLimited`）：当短预算耗尽（`attempt >= 3`）时它落入
//!   `cover_job_failure_state` 的 `_ => Failed` 分支 —— 那就是 retryable terminal failure。
//! - crash safety：reconcile 只把 job 推进为 `pending` + `long_retry_pending=1`，
//!   **额度在 worker 真正 claim 时才被原子消耗**。
//! - `unsupported` / `blocked` 与 6h 补偿严格隔离。
//! - 显式人工 retry **不**重置 `long_retry_consumed`。

use rusqlite::{params, Connection};
use rust_lib_app::remote_scan::cover_model::{CoverJobKey, CoverJobState};
use rust_lib_app::remote_scan::cover_state::CoverJobUpsertCause;
use rust_lib_app::remote_scan::{cover_store, persistence};

const HOUR_MS: i64 = 60 * 60 * 1000;
const SIX_HOURS_MS: i64 = 6 * HOUR_MS;

// ---------------------------------------------------------------------------
// helpers
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

fn seed_job(conn: &Connection, key: &CoverJobKey, state: CoverJobState, generation: i64, epoch: &str, now: i64) {
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

/// 直接写 long-retry 三列（模拟"已经形成 failure episode"的 durable 状态）。
fn set_long_retry(conn: &Connection, key: &CoverJobKey, not_before: Option<i64>, consumed: i64, pending: i64) {
    conn.execute(
        "UPDATE remote_cover_job SET long_retry_not_before=?1,long_retry_consumed=?2,long_retry_pending=?3
          WHERE job_key=?4",
        params![not_before, consumed, pending, key.encode()],
    )
    .unwrap();
}

fn long_retry(conn: &Connection, key: &CoverJobKey) -> (Option<i64>, i64, i64) {
    conn.query_row(
        "SELECT long_retry_not_before,long_retry_consumed,long_retry_pending
           FROM remote_cover_job WHERE job_key=?1",
        [key.encode()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .unwrap()
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

// ---------------------------------------------------------------------------
// 2. 6h eligibility（round 4 / 5）
// ---------------------------------------------------------------------------

#[test]
fn retryable_failure_is_not_promoted_before_six_hours() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "asset");
    seed_job(&conn, &key, CoverJobState::Failed, 1, "e1", 0);
    set_long_retry(&conn, &key, Some(SIX_HOURS_MS), 0, 0);

    let report = cover_store::reconcile_cover_compensation_for_source_on(
        &conn,
        "source",
        42,
        SIX_HOURS_MS - 1,
        budget(64),
    )
    .unwrap();

    assert_eq!(report.compensation_promoted, 0, "must not promote before 6h");
    assert_eq!(state_of(&conn, &key), "failed");
    assert_eq!(long_retry(&conn, &key).1, 0, "budget must stay unused");
}

#[test]
fn retryable_failure_is_promoted_exactly_once_after_six_hours() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "asset");
    seed_job(&conn, &key, CoverJobState::Failed, 1, "e1", 0);
    set_long_retry(&conn, &key, Some(SIX_HOURS_MS), 0, 0);

    let report = cover_store::reconcile_cover_compensation_for_source_on(
        &conn,
        "source",
        42,
        SIX_HOURS_MS,
        budget(64),
    )
    .unwrap();
    assert_eq!(report.compensation_promoted, 1);
    assert_eq!(report.claimable_work, true);
    assert_eq!(state_of(&conn, &key), "pending");
    let (_, consumed, pending) = long_retry(&conn, &key);
    assert_eq!(consumed, 0, "reconcile must NOT consume the budget");
    assert_eq!(pending, 1, "it must be marked as a compensation pending");

    // 重复 reconciliation 不得再次推进 / 不得重复计数。
    let again = cover_store::reconcile_cover_compensation_for_source_on(
        &conn,
        "source",
        42,
        SIX_HOURS_MS + 1,
        budget(64),
    )
    .unwrap();
    assert_eq!(again.compensation_promoted, 0, "promotion must be at-most-once");
    assert_eq!(long_retry(&conn, &key).1, 0);
}

// ---------------------------------------------------------------------------
// 3. reconcile → claim 之间的 crash safety（round 7）
// ---------------------------------------------------------------------------

#[test]
fn a_crash_between_reconcile_and_claim_preserves_the_compensation_opportunity() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "asset");
    seed_job(&conn, &key, CoverJobState::Failed, 1, "e1", 0);
    set_long_retry(&conn, &key, Some(SIX_HOURS_MS), 0, 0);

    cover_store::reconcile_cover_compensation_for_source_on(&conn, "source", 42, SIX_HOURS_MS, budget(64))
        .unwrap();
    // 模拟崩溃/重启：不消耗任何额度，重新打开连接也只看 durable 状态。
    let (_, consumed, pending) = long_retry(&conn, &key);
    assert_eq!(consumed, 0, "no budget may be consumed before a real claim");
    assert_eq!(pending, 1);

    // 重启后该 job 必须仍然可被 claim（compensation 机会仍在）。
    let claimed = cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", SIX_HOURS_MS + 10, 60_000, 42)
        .unwrap()
        .expect("a compensation-pending job must still be claimable after a restart");
    assert_eq!(claimed.key, key);
    assert_eq!(claimed.state, CoverJobState::Running);
}

// ---------------------------------------------------------------------------
// 4. claim 才消耗额度（round 8）
// ---------------------------------------------------------------------------

#[test]
fn the_claim_is_what_consumes_the_long_retry_budget() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "asset");
    seed_job(&conn, &key, CoverJobState::Failed, 1, "e1", 0);
    set_long_retry(&conn, &key, Some(SIX_HOURS_MS), 0, 0);
    cover_store::reconcile_cover_compensation_for_source_on(&conn, "source", 42, SIX_HOURS_MS, budget(64))
        .unwrap();

    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", SIX_HOURS_MS + 10, 60_000, 42)
        .unwrap()
        .unwrap();

    let (_, consumed, pending) = long_retry(&conn, &key);
    assert_eq!(consumed, 1, "claiming must durably consume the compensation");
    assert_eq!(pending, 0, "the compensation-pending marker must be cleared");
}

// ---------------------------------------------------------------------------
// 5. 补偿再次失败 → 不形成 6h 无限循环（round 6）
// ---------------------------------------------------------------------------

#[test]
fn a_failed_compensation_does_not_start_another_six_hour_cycle() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "asset");
    seed_job(&conn, &key, CoverJobState::Failed, 1, "e1", 0);
    set_long_retry(&conn, &key, Some(SIX_HOURS_MS), 0, 0);

    // 第一轮：promote → claim（消耗）→ 再次终态失败（retryable）。
    cover_store::reconcile_cover_compensation_for_source_on(&conn, "source", 42, SIX_HOURS_MS, budget(64))
        .unwrap();
    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", SIX_HOURS_MS + 10, 60_000, 42)
        .unwrap()
        .unwrap();
    cover_store::mark_job_failure_owned_on(
        &conn,
        &key,
        "worker",
        CoverJobState::Failed,
        Some("transient"),
        SIX_HOURS_MS + 20,
        Some(SIX_HOURS_MS + 20 + SIX_HOURS_MS),
    )
    .unwrap();

    let (not_before, consumed, pending) = long_retry(&conn, &key);
    assert_eq!(consumed, 1, "the exhausted episode must stay marked as consumed");
    assert_eq!(pending, 0);
    assert_eq!(
        not_before,
        Some(SIX_HOURS_MS),
        "the original eligibility must not be pushed forward"
    );

    // 再过 6h（甚至 24h）也不得再自动补偿。
    for now in [2 * SIX_HOURS_MS + 30, 4 * SIX_HOURS_MS + 30] {
        let report =
            cover_store::reconcile_cover_compensation_for_source_on(&conn, "source", 42, now, budget(64))
                .unwrap();
        assert_eq!(
            report.compensation_promoted, 0,
            "a consumed episode must never be promoted again (now={now})"
        );
        assert_eq!(state_of(&conn, &key), "failed");
    }
}

// ---------------------------------------------------------------------------
// 6. ready 之后的新 failure episode 获得新额度（round 9）
// ---------------------------------------------------------------------------

#[test]
fn a_new_failure_episode_after_ready_regains_one_compensation() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "asset");
    seed_job(&conn, &key, CoverJobState::Failed, 1, "e1", 0);
    set_long_retry(&conn, &key, Some(SIX_HOURS_MS), 1, 0);

    // 业务恢复成功 → ready 必须清空 episode 状态。
    cover_store::upsert_job_on(
        &conn,
        &key,
        CoverJobState::Pending,
        "background",
        10,
        1,
        "e1",
        100,
        CoverJobUpsertCause::ManualRetry,
    )
    .unwrap();
    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", 110, 60_000, 42)
        .unwrap()
        .unwrap();
    assert!(cover_store::mark_job_ready_owned_on(
        &conn,
        &key,
        "worker",
        200,
        340,
        480,
        340 * 480 * 4,
        "checksum"
    )
    .unwrap());
    assert_eq!(long_retry(&conn, &key), (None, 0, 0), "ready must clear the episode");

    // 之后一次全新的终态失败（retryable）应重新获得一次 6h 资格。
    //
    // 注意：不能用 ManualRetry 来"制造"这次失败 —— P1-A 的冻结矩阵规定
    // `ManualRetry: ready -> ready`（人工 retry 不得把已就绪的封面降级）。
    // 生产里 ready 之后重新进入候选队列的真实路径是 P1-B 的字节缺失对账，
    // 因此这里直接沿用那条路径的写入方式。
    conn.execute(
        "UPDATE remote_cover_job SET state='pending',updated_at=300 WHERE job_key=?1",
        [key.encode()],
    )
    .unwrap();
    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", 310, 60_000, 42)
        .unwrap()
        .unwrap();
    cover_store::mark_job_failure_owned_on(
        &conn,
        &key,
        "worker",
        CoverJobState::Failed,
        Some("transient"),
        1_000,
        Some(1_000 + SIX_HOURS_MS),
    )
    .unwrap();
    let (not_before, consumed, _) = long_retry(&conn, &key);
    assert_eq!(consumed, 0, "a brand new episode must have a fresh budget");
    assert_eq!(not_before, Some(1_000 + SIX_HOURS_MS));

    let report = cover_store::reconcile_cover_compensation_for_source_on(
        &conn,
        "source",
        42,
        1_000 + SIX_HOURS_MS,
        budget(64),
    )
    .unwrap();
    assert_eq!(report.compensation_promoted, 1, "the new episode may be compensated once");
}

// ---------------------------------------------------------------------------
// 7. 人工 retry 不得偷偷刷新自动额度（§7）
// ---------------------------------------------------------------------------

#[test]
fn a_manual_retry_does_not_refresh_the_long_retry_budget() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "asset");
    seed_job(&conn, &key, CoverJobState::Failed, 1, "e1", 0);
    set_long_retry(&conn, &key, Some(SIX_HOURS_MS), 1, 0);

    // 显式人工 retry（P1-A 的 ManualRetry 原因）→ pending，但 episode 额度不得刷新。
    cover_store::upsert_job_on(
        &conn,
        &key,
        CoverJobState::Pending,
        "background",
        10,
        1,
        "e1",
        SIX_HOURS_MS + 50,
        CoverJobUpsertCause::ManualRetry,
    )
    .unwrap();
    assert_eq!(
        long_retry(&conn, &key),
        (Some(SIX_HOURS_MS), 1, 0),
        "a manual retry must not reset long_retry_consumed / not_before"
    );

    // 它再次终态失败后依旧不得获得新的自动补偿。
    cover_store::claim_next_job_for_source_session_on(&conn, "source", "worker", SIX_HOURS_MS + 60, 60_000, 42)
        .unwrap()
        .unwrap();
    cover_store::mark_job_failure_owned_on(
        &conn,
        &key,
        "worker",
        CoverJobState::Failed,
        Some("transient"),
        SIX_HOURS_MS + 70,
        Some(SIX_HOURS_MS + 70 + SIX_HOURS_MS),
    )
    .unwrap();
    assert_eq!(long_retry(&conn, &key).1, 1);
    let report = cover_store::reconcile_cover_compensation_for_source_on(
        &conn,
        "source",
        42,
        3 * SIX_HOURS_MS,
        budget(64),
    )
    .unwrap();
    assert_eq!(report.compensation_promoted, 0);
}

// ---------------------------------------------------------------------------
// 8. unsupported / blocked 隔离（round 10 / 11 + §8）
// ---------------------------------------------------------------------------

#[test]
fn unsupported_is_never_promoted_by_time_or_by_a_session_event() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let key = job_key("source", "asset");
    seed_job(&conn, &key, CoverJobState::Unsupported, 1, "e1", 0);
    // 即使（错误地）被写上 eligibility，也不得被推进。
    set_long_retry(&conn, &key, Some(0), 0, 0);

    let report = cover_store::reconcile_cover_compensation_for_source_on(
        &conn,
        "source",
        42,
        10 * SIX_HOURS_MS,
        budget(64),
    )
    .unwrap();
    assert_eq!(report.compensation_promoted, 0, "unsupported must not be time-promoted");
    assert_eq!(state_of(&conn, &key), "unsupported");
}

#[test]
fn blocked_is_only_cleared_by_its_own_blocker_type() {
    // auth/session blocker：新有效 session 可以解除。
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let auth = job_key("source", "auth-asset");
    seed_job(&conn, &auth, CoverJobState::Blocked, 1, "e1", 0);
    conn.execute(
        "UPDATE remote_cover_job SET error_code='authExpired' WHERE job_key=?1",
        [auth.encode()],
    )
    .unwrap();
    // 与 auth 无关的 blocker：不得被 session-ready 解除。
    let other = job_key("source", "other-asset");
    seed_job(&conn, &other, CoverJobState::Blocked, 1, "e1", 0);
    conn.execute(
        "UPDATE remote_cover_job SET error_code='forbidden' WHERE job_key=?1",
        [other.encode()],
    )
    .unwrap();

    let report = cover_store::reconcile_cover_compensation_for_source_on(
        &conn,
        "source",
        42,
        10 * SIX_HOURS_MS,
        budget(64),
    )
    .unwrap();
    assert_eq!(
        state_of(&conn, &auth),
        "pending",
        "an auth/session blocker must be cleared by a new valid session"
    );
    assert_eq!(
        state_of(&conn, &other),
        "blocked",
        "a non-session blocker must NOT be cleared by a session event"
    );
    assert_eq!(report.blocker_cleared, 1);
    assert_eq!(
        report.claimable_work, true,
        "the cleared blocker must produce claimable work"
    );
}

// ---------------------------------------------------------------------------
// 9. library replenishment（round 12–16）
// ---------------------------------------------------------------------------

fn seed_library_asset(conn: &Connection, source_id: &str, path: &str, fingerprint: &str) -> String {
    let asset_id = rust_lib_app::db::library_index_id(fingerprint, path);
    conn.execute(
        "INSERT OR REPLACE INTO library_index(
             id,source_id,name,path,entry_type,asset_kind,content_fingerprint,
             listing_complete,deleted,updated_at)
         VALUES(?1,?2,'book.cbz',?3,'file','ArchiveFile',?4,1,0,1)",
        params![asset_id, source_id, path, fingerprint],
    )
    .unwrap();
    asset_id
}

#[test]
fn replenishment_creates_a_pending_job_for_a_library_asset_without_any_job() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    let asset_id = seed_library_asset(&conn, "source", "/book.cbz", "fp-1");

    let report = cover_store::reconcile_missing_covers_for_source_on(
        &conn,
        "source",
        42,
        1_000,
        budget(64),
    )
    .unwrap();
    assert_eq!(report.jobs_created, 1);
    assert_eq!(report.claimable_work, true);

    let key = CoverJobKey {
        source_id: "source".into(),
        asset_id,
        content_revision: "fp-1".into(),
        selection_revision: "default".into(),
        profile: "340x480@1".into(),
    };
    assert_eq!(state_of(&conn, &key), "pending");

    // 再次运行：不得重复创建（asset/source 去重 + pending 不重复）。
    let again = cover_store::reconcile_missing_covers_for_source_on(
        &conn,
        "source",
        42,
        1_100,
        budget(64),
    )
    .unwrap();
    assert_eq!(again.jobs_created, 0);
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM remote_cover_job WHERE source_id='source'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "replenishment must not duplicate the job");
}

#[test]
fn replenishment_leaves_live_and_terminal_states_alone() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);

    // ready 且磁盘存在 → 不动（这里用"有 job 行"表达：replenisher 只补"完全没有 job"的缺口）。
    let ready_asset = seed_library_asset(&conn, "source", "/ready.cbz", "fp-ready");
    let ready_key = CoverJobKey {
        source_id: "source".into(),
        asset_id: ready_asset,
        content_revision: "fp-ready".into(),
        selection_revision: "default".into(),
        profile: "340x480@1".into(),
    };
    seed_job(&conn, &ready_key, CoverJobState::Ready, 1, "e1", 0);
    // running → 不动
    let running_asset = seed_library_asset(&conn, "source", "/running.cbz", "fp-running");
    let running_key = CoverJobKey {
        source_id: "source".into(),
        asset_id: running_asset,
        content_revision: "fp-running".into(),
        selection_revision: "default".into(),
        profile: "340x480@1".into(),
    };
    seed_job(&conn, &running_key, CoverJobState::Running, 1, "e1", 0);
    // failed → 不加新 job（terminal 语义只由它自己的原因解除）
    let failed_asset = seed_library_asset(&conn, "source", "/failed.cbz", "fp-failed");
    let failed_key = CoverJobKey {
        source_id: "source".into(),
        asset_id: failed_asset,
        content_revision: "fp-failed".into(),
        selection_revision: "default".into(),
        profile: "340x480@1".into(),
    };
    seed_job(&conn, &failed_key, CoverJobState::Failed, 1, "e1", 0);

    let report =
        cover_store::reconcile_missing_covers_for_source_on(&conn, "source", 42, 2_000, budget(64)).unwrap();
    assert_eq!(report.jobs_created, 0, "existing job rows must never be duplicated");
    assert_eq!(state_of(&conn, &ready_key), "ready");
    assert_eq!(state_of(&conn, &running_key), "running");
    assert_eq!(state_of(&conn, &failed_key), "failed");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM remote_cover_job WHERE source_id='source'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 3);
}

#[test]
fn replenishment_is_bounded_by_its_budget() {
    let conn = connection();
    bind_epoch(&conn, "source", 1, "e1", 42);
    for index in 0..25 {
        seed_library_asset(
            &conn,
            "source",
            &format!("/book-{index}.cbz"),
            &format!("fp-{index}"),
        );
    }

    let report =
        cover_store::reconcile_missing_covers_for_source_on(&conn, "source", 42, 3_000, budget(10)).unwrap();
    assert_eq!(report.jobs_created, 10, "one pass must respect max_jobs");
    assert!(report.truncated, "a bounded pass must report that more work remains");

    // 后续 pass 继续推进，而不是一次 fan-out 出全部 25 个。
    let next =
        cover_store::reconcile_missing_covers_for_source_on(&conn, "source", 42, 3_100, budget(10)).unwrap();
    assert_eq!(next.jobs_created, 10);
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM remote_cover_job WHERE source_id='source'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(total, 20);
}

// ---------------------------------------------------------------------------
// 10. source 隔离
// ---------------------------------------------------------------------------

#[test]
fn reconciliation_is_strictly_source_scoped() {
    let conn = connection();
    bind_epoch(&conn, "source-a", 1, "e1", 42);
    bind_epoch(&conn, "source-b", 1, "e1", 42);
    let a = job_key("source-a", "asset");
    let b = job_key("source-b", "asset");
    seed_job(&conn, &a, CoverJobState::Failed, 1, "e1", 0);
    seed_job(&conn, &b, CoverJobState::Failed, 1, "e1", 0);
    set_long_retry(&conn, &a, Some(0), 0, 0);
    set_long_retry(&conn, &b, Some(0), 0, 0);

    let report =
        cover_store::reconcile_cover_compensation_for_source_on(&conn, "source-a", 42, 1, budget(64)).unwrap();
    assert_eq!(report.compensation_promoted, 1);
    assert_eq!(state_of(&conn, &a), "pending");
    assert_eq!(
        state_of(&conn, &b),
        "failed",
        "a session event for source-a must never touch source-b"
    );
}
