//! P1-A：cover job 状态迁移的**集成**契约测试。
//!
//! 与 `remote_scan::cover_state` 里的纯函数规格测试不同，本文件驱动真实存储路径
//! （`cover_store::upsert_job_on` + `claim_next_job_on`），因此它能证明：
//!
//! 1. 显式 cause 真的改变落库状态（不是只改了一个没人用的纯函数）；
//! 2. 状态变化真的改变**候选队列**——变成 `pending` 的任务必须能被 claim，
//!    留在 `unsupported`/`blocked` 的任务必须仍然不可 claim。
//!
//! 改造前 `upsert_job_on` 用 `state=remote_cover_job.state` 无条件保留旧状态，
//! 因此除 Demand 保留类以外的行全部失败（RED）。

use rusqlite::Connection;
use rust_lib_app::remote_scan::cover_model::{CoverJobKey, CoverJobState};
use rust_lib_app::remote_scan::cover_state::CoverJobUpsertCause;
use rust_lib_app::remote_scan::cover_store;

fn connection() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    cover_store::migrate(&conn).unwrap();
    conn
}

fn job_key(tag: &str) -> CoverJobKey {
    CoverJobKey {
        source_id: "source".into(),
        asset_id: format!("asset-{tag}"),
        content_revision: "v1".into(),
        selection_revision: "default".into(),
        profile: "340x480@1".into(),
    }
}

/// 先落一条指定初始状态的记录（新记录 → 按请求状态建立）。
fn seed(conn: &Connection, key: &CoverJobKey, state: CoverJobState, now: i64) {
    let created = cover_store::upsert_job_on(
        conn,
        key,
        state,
        "background",
        10,
        1,
        "epoch",
        now,
        CoverJobUpsertCause::Demand,
    )
    .unwrap();
    assert_eq!(
        created.state, state,
        "seeding a brand new job must create it as requested"
    );
}

/// 施加一次带显式原因的记录写入，返回落库后的状态。
fn apply(conn: &Connection, key: &CoverJobKey, cause: CoverJobUpsertCause) -> CoverJobState {
    cover_store::upsert_job_on(
        conn,
        key,
        CoverJobState::Pending,
        "background",
        10,
        1,
        "epoch",
        200,
        cause,
    )
    .unwrap()
    .state
}

fn claimable(conn: &Connection) -> bool {
    cover_store::claim_next_job_on(conn, "worker", 10_000, 60_000)
        .unwrap()
        .is_some()
}

#[test]
fn explicit_cause_drives_the_durable_transition_matrix() {
    use CoverJobState as S;
    use CoverJobUpsertCause as C;

    // (原因, 初始状态, 期望状态, 是否必须进入候选队列)
    let rows: &[(C, S, S, bool)] = &[
        // Demand：保留排期与终态（既有契约，不得回退）
        (C::Demand, S::Pending, S::Pending, true),
        (C::Demand, S::Running, S::Running, false),
        (C::Demand, S::Ready, S::Ready, false),
        (C::Demand, S::RetryWait, S::RetryWait, false),
        (C::Demand, S::Failed, S::Failed, false),
        (C::Demand, S::Unsupported, S::Unsupported, false),
        (C::Demand, S::Blocked, S::Blocked, false),
        (C::Demand, S::Cancelled, S::Pending, true),
        // FullRescan：可重置 failed / retry_wait / cancelled
        (C::FullRescan, S::Pending, S::Pending, true),
        (C::FullRescan, S::Running, S::Running, false),
        (C::FullRescan, S::Ready, S::Ready, false),
        (C::FullRescan, S::RetryWait, S::Pending, true),
        (C::FullRescan, S::Failed, S::Pending, true),
        (C::FullRescan, S::Unsupported, S::Unsupported, false),
        (C::FullRescan, S::Blocked, S::Blocked, false),
        (C::FullRescan, S::Cancelled, S::Pending, true),
        // ManualRetry：除 ready / running 外一律重新排队
        (C::ManualRetry, S::Pending, S::Pending, true),
        (C::ManualRetry, S::Running, S::Running, false),
        (C::ManualRetry, S::Ready, S::Ready, false),
        (C::ManualRetry, S::RetryWait, S::Pending, true),
        (C::ManualRetry, S::Failed, S::Pending, true),
        (C::ManualRetry, S::Unsupported, S::Pending, true),
        (C::ManualRetry, S::Blocked, S::Pending, true),
        (C::ManualRetry, S::Cancelled, S::Pending, true),
        // CauseCleared：只解除 blocked / retry_wait
        (C::CauseCleared, S::Blocked, S::Pending, true),
        (C::CauseCleared, S::RetryWait, S::Pending, true),
        (C::CauseCleared, S::Failed, S::Failed, false),
        (C::CauseCleared, S::Unsupported, S::Unsupported, false),
        (C::CauseCleared, S::Running, S::Running, false),
        (C::CauseCleared, S::Ready, S::Ready, false),
        // CapabilityChanged：只解除 unsupported / retry_wait
        (C::CapabilityChanged, S::Unsupported, S::Pending, true),
        (C::CapabilityChanged, S::RetryWait, S::Pending, true),
        (C::CapabilityChanged, S::Failed, S::Failed, false),
        (C::CapabilityChanged, S::Blocked, S::Blocked, false),
        (C::CapabilityChanged, S::Running, S::Running, false),
        (C::CapabilityChanged, S::Ready, S::Ready, false),
    ];

    let mut failures = Vec::new();
    for (index, (cause, initial, expected, must_be_claimable)) in rows.iter().enumerate() {
        let conn = connection();
        // 每条用例用独立 asset，避免互相干扰。
        let key = job_key(&format!("{index}"));
        seed(&conn, &key, *initial, 100);
        let resolved = apply(&conn, &key, *cause);
        if resolved != *expected {
            failures.push(format!(
                "cause={cause:?} initial={} => got {} want {}",
                initial.as_str(),
                resolved.as_str(),
                expected.as_str()
            ));
            continue;
        }
        // 状态还必须真的改变候选队列，而不只是改了一个字段。
        if *expected != CoverJobState::RetryWait {
            let claimable = claimable(&conn);
            if claimable != *must_be_claimable {
                failures.push(format!(
                    "cause={cause:?} initial={} => {} but claimable={claimable} want {must_be_claimable}",
                    initial.as_str(),
                    resolved.as_str()
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "durable transition matrix violated in {} of {} rows:\n  {}",
        failures.len(),
        rows.len(),
        failures.join("\n  ")
    );
}

/// 冻结原则的集成版：`unsupported` / `blocked` 不得被普通需求或全量重扫解除，
/// 但必须能被各自的解除原因拉回候选队列。
#[test]
fn unsupported_and_blocked_only_recover_through_their_own_cause() {
    use CoverJobState as S;
    use CoverJobUpsertCause as C;

    for (state, own_cause) in [
        (S::Unsupported, C::CapabilityChanged),
        (S::Blocked, C::CauseCleared),
    ] {
        for cause in [
            C::Demand,
            C::FullRescan,
            C::CauseCleared,
            C::CapabilityChanged,
        ] {
            let conn = connection();
            let key = job_key(&format!("{}-{cause:?}", state.as_str()));
            seed(&conn, &key, state, 100);
            let resolved = apply(&conn, &key, cause);
            if cause == own_cause {
                assert_eq!(
                    resolved,
                    S::Pending,
                    "{} must be recovered by its own cause {cause:?}",
                    state.as_str()
                );
                assert!(
                    claimable(&conn),
                    "recovered job must enter the candidate queue"
                );
            } else {
                assert_eq!(
                    resolved,
                    state,
                    "{} must NOT be cleared by {cause:?}",
                    state.as_str()
                );
                assert!(
                    !claimable(&conn),
                    "{} must stay out of the candidate queue for {cause:?}",
                    state.as_str()
                );
            }
        }
    }
}
