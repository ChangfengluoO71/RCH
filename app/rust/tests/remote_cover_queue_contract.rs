use rusqlite::Connection;
use rust_lib_app::remote_scan::cover_model::{CoverJobKey, CoverJobState};
use rust_lib_app::remote_scan::cover_service::ConsumerRegistry;
use rust_lib_app::remote_scan::cover_store;
use rust_lib_app::remote_scan::cover_state::CoverJobUpsertCause;

#[test]
fn ordinary_visible_demand_does_not_bypass_backoff_or_terminal_errors() {
    let conn = Connection::open_in_memory().unwrap();
    cover_store::migrate(&conn).unwrap();
    for state in [CoverJobState::RetryWait, CoverJobState::Unsupported, CoverJobState::Failed, CoverJobState::Blocked] {
        let key = CoverJobKey {
            source_id: "source".into(), asset_id: state.as_str().into(),
            content_revision: "v1".into(), selection_revision: "default".into(), profile: "170x240@1".into(),
        };
        cover_store::upsert_job_on(&conn, &key, state, "background", 10, 1, "epoch", 1, CoverJobUpsertCause::Demand).unwrap();
        let demanded = cover_store::upsert_job_on(&conn, &key, CoverJobState::Pending, "visible", 300, 1, "epoch", 2, CoverJobUpsertCause::Demand).unwrap();
        assert_eq!(demanded.state, state, "visible demand must not restart {}", state.as_str());
        assert_eq!(demanded.priority, 300);
    }
}

#[test]
fn identical_jobs_are_upserted_once_and_consumer_release_is_scoped() {
    let conn = Connection::open_in_memory().unwrap();
    cover_store::migrate(&conn).unwrap();
    let key = CoverJobKey {
        source_id: "source".into(),
        asset_id: "asset".into(),
        content_revision: "content".into(),
        selection_revision: "selection".into(),
        profile: "200x300".into(),
    };
    let first = cover_store::upsert_job_on(
        &conn,
        &key,
        CoverJobState::Pending,
        "background",
        1,
        1,
        "session-a",
        10,
        CoverJobUpsertCause::Demand,
)
    .unwrap();
    let second = cover_store::upsert_job_on(
        &conn,
        &key,
        CoverJobState::Pending,
        "visible",
        10,
        1,
        "session-a",
        11,
        CoverJobUpsertCause::Demand,
)
    .unwrap();
    assert_eq!(first.key, second.key);
    assert_eq!(second.priority, 10);
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM remote_cover_job", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);

    let registry = ConsumerRegistry::default();
    registry.attach("consumer-a", &key);
    registry.attach("consumer-b", &key);
    assert_eq!(registry.consumer_count(&key), 2);
    registry.release("consumer-a");
    assert_eq!(registry.consumer_count(&key), 1);
    assert!(registry.is_attached("consumer-b", &key));
}

#[test]
fn claim_is_short_lived_prioritized_and_recovery_requeues_expired_leases() {
    let conn = Connection::open_in_memory().unwrap();
    cover_store::migrate(&conn).unwrap();
    let low = CoverJobKey {
        source_id: "source".into(),
        asset_id: "low".into(),
        content_revision: "v1".into(),
        selection_revision: "default".into(),
        profile: "340x480@1".into(),
    };
    let high = CoverJobKey {
        asset_id: "high".into(),
        ..low.clone()
    };
    cover_store::upsert_job_on(
        &conn,
        &low,
        CoverJobState::Pending,
        "background",
        10,
        1,
        "epoch",
        100,
        CoverJobUpsertCause::Demand,
)
    .unwrap();
    cover_store::upsert_job_on(
        &conn,
        &high,
        CoverJobState::Pending,
        "visible",
        300,
        1,
        "epoch",
        101,
        CoverJobUpsertCause::Demand,
)
    .unwrap();
    let claimed = cover_store::claim_next_job_on(&conn, "worker-a", 200, 1_000)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.key.asset_id, "high");
    assert_eq!(claimed.state, CoverJobState::Running);
    assert_eq!(claimed.lease_owner.as_deref(), Some("worker-a"));
    assert_eq!(claimed.attempt, 1);

    let recovered = cover_store::recover_expired_leases_on(&conn, 1_500).unwrap();
    assert_eq!(recovered, 1);
    let recovered_job = cover_store::load_job_on(&conn, &high.encode())
        .unwrap()
        .unwrap();
    assert_eq!(recovered_job.state, CoverJobState::Pending);
    assert!(recovered_job.lease_owner.is_none());
}

#[test]
fn retry_wait_is_not_claimed_before_next_attempt() {
    let conn = Connection::open_in_memory().unwrap();
    cover_store::migrate(&conn).unwrap();
    let key = CoverJobKey {
        source_id: "source".into(),
        asset_id: "asset".into(),
        content_revision: "v1".into(),
        selection_revision: "default".into(),
        profile: "340x480@1".into(),
    };
    cover_store::upsert_job_on(
        &conn,
        &key,
        CoverJobState::RetryWait,
        "background",
        10,
        1,
        "epoch",
        100,
        CoverJobUpsertCause::Demand,
)
    .unwrap();
    conn.execute(
        "UPDATE remote_cover_job SET next_attempt_at=500 WHERE job_key=?1",
        [key.encode()],
    )
    .unwrap();
    assert!(cover_store::claim_next_job_on(&conn, "worker", 499, 1_000)
        .unwrap()
        .is_none());
    assert!(cover_store::claim_next_job_on(&conn, "worker", 500, 1_000)
        .unwrap()
        .is_some());
}

#[test]
fn ready_publish_records_blob_metadata_and_reference_atomically() {
    let conn = Connection::open_in_memory().unwrap();
    cover_store::migrate(&conn).unwrap();
    let key = CoverJobKey {
        source_id: "source".into(),
        asset_id: "asset".into(),
        content_revision: "content".into(),
        selection_revision: "default".into(),
        profile: "340x480@1".into(),
    };
    cover_store::upsert_job_on(
        &conn,
        &key,
        CoverJobState::Pending,
        "background",
        10,
        1,
        "epoch",
        1,
        CoverJobUpsertCause::Demand,
)
    .unwrap();
    cover_store::claim_next_job_on(&conn, "worker", 2, 1_000)
        .unwrap()
        .unwrap();
    assert!(cover_store::mark_job_ready_owned_on(
        &conn,
        &key,
        "worker",
        3,
        340,
        480,
        340 * 480 * 4,
        "checksum",
    )
    .unwrap());
    let blob: (String, i64, i64, String) = conn
        .query_row(
            "SELECT relative_path,byte_size,width,checksum FROM remote_cover_blob WHERE blob_key=?1",
            [key.encode()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert!(blob.0.starts_with("cache/cover/"));
    assert_eq!(blob.1, 340 * 480 * 4);
    assert_eq!(blob.2, 340);
    assert_eq!(blob.3, "checksum");
    let refs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM remote_cover_ref WHERE source_id='source' AND asset_id='asset'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(refs, 1);
}

/// 2026-09-21（真机："手机墙上大量获取失败，可图其实抓得到"）：用户主动"重试失败封面"
/// 必须把**该源当前档的终态失败**重新排队（attempt/退避/长期补偿一并重置 + 推进 revision），
/// 且**绝不动**其它档位、其它状态、其它源。
#[test]
fn requeue_failed_for_source_only_touches_current_profile_failures() {
    let conn = Connection::open_in_memory().unwrap();
    cover_store::migrate(&conn).unwrap();
    let key = |source: &str, asset: &str, profile: &str| CoverJobKey {
        source_id: source.into(),
        asset_id: asset.into(),
        content_revision: "v1".into(),
        selection_revision: "default".into(),
        profile: profile.into(),
    };
    let state_of = |key: &CoverJobKey| -> String {
        conn.query_row(
            "SELECT state FROM remote_cover_job WHERE job_key=?1",
            [key.encode()],
            |row| row.get(0),
        )
        .unwrap()
    };

    // 目标：本源当前档的两条 failed（其中一条带长期补偿标记，必须一并清掉）
    let a = key("s1", "a", "170x240@1");
    let b = key("s1", "b", "170x240@1");
    for (k, attempt) in [(&a, 3_i64), (&b, 1_i64)] {
        cover_store::upsert_job_on(
            &conn, k, CoverJobState::Failed, "background", 10, 1, "e1", attempt,
            CoverJobUpsertCause::Demand,
        )
        .unwrap();
    }
    conn.execute(
        "UPDATE remote_cover_job SET long_retry_pending=1,long_retry_consumed=1,
                long_retry_not_before=99 WHERE job_key=?1",
        [a.encode()],
    )
    .unwrap();

    // 干扰项：其它档位的 failed、其它状态的 pending、其它源的 failed
    let other_profile = key("s1", "c", "340x480@1");
    let other_state = key("s1", "d", "170x240@1");
    let other_source = key("s2", "e", "170x240@1");
    cover_store::upsert_job_on(
        &conn, &other_profile, CoverJobState::Failed, "background", 10, 1, "e1", 1,
        CoverJobUpsertCause::Demand,
    )
    .unwrap();
    cover_store::upsert_job_on(
        &conn, &other_state, CoverJobState::Pending, "background", 10, 1, "e1", 0,
        CoverJobUpsertCause::Demand,
    )
    .unwrap();
    cover_store::upsert_job_on(
        &conn, &other_source, CoverJobState::Failed, "background", 10, 1, "e1", 1,
        CoverJobUpsertCause::Demand,
    )
    .unwrap();

    let revision_before = cover_store::view_revision(&conn, "s1").unwrap();
    let requeued =
        cover_store::requeue_failed_for_source_on(&conn, "s1", "170x240@1", 10, 5_000).unwrap();

    assert_eq!(requeued, 2, "只应重排该源当前档的两条 failed");
    assert_eq!(state_of(&a), "pending");
    assert_eq!(state_of(&b), "pending");
    assert_eq!(state_of(&other_profile), "failed", "其它档位不得被动");
    assert_eq!(state_of(&other_state), "pending", "非 failed 状态不得被动");
    assert_eq!(state_of(&other_source), "failed", "其它源不得被动");
    assert!(
        cover_store::view_revision(&conn, "s1").unwrap() > revision_before,
        "必须推进 revision，界面才能重读 durable state"
    );

    // attempt/退避/长期补偿三列都要归零，否则重试一次又会被挡回去。
    let (attempt, long_pending, long_consumed, long_not_before): (i64, i64, i64, Option<i64>) = conn
        .query_row(
            "SELECT attempt,long_retry_pending,long_retry_consumed,long_retry_not_before
               FROM remote_cover_job WHERE job_key=?1",
            [a.encode()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(attempt, 0);
    assert_eq!(long_pending, 0);
    assert_eq!(long_consumed, 0);
    assert_eq!(long_not_before, None);

    // 幂等：再点一次不应再改动任何行。
    assert_eq!(
        cover_store::requeue_failed_for_source_on(&conn, "s1", "170x240@1", 10, 6_000).unwrap(),
        0
    );
    // limit=0 是明确的无操作（避免误传一把清空）。
    assert_eq!(
        cover_store::requeue_failed_for_source_on(&conn, "s1", "340x480@1", 0, 6_000).unwrap(),
        0
    );
}
