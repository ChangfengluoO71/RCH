use rusqlite::Connection;
use rust_lib_app::remote_scan::cover_model::{CoverJobKey, CoverJobState};
use rust_lib_app::remote_scan::cover_service::ConsumerRegistry;
use rust_lib_app::remote_scan::cover_store;

#[test]
fn ordinary_visible_demand_does_not_bypass_backoff_or_terminal_errors() {
    let conn = Connection::open_in_memory().unwrap();
    cover_store::migrate(&conn).unwrap();
    for state in [CoverJobState::RetryWait, CoverJobState::Unsupported, CoverJobState::Failed, CoverJobState::Blocked] {
        let key = CoverJobKey {
            source_id: "source".into(), asset_id: state.as_str().into(),
            content_revision: "v1".into(), selection_revision: "default".into(), profile: "170x240@1".into(),
        };
        cover_store::upsert_job_on(&conn, &key, state, "background", 10, 1, "epoch", 1).unwrap();
        let demanded = cover_store::upsert_job_on(&conn, &key, CoverJobState::Pending, "visible", 300, 1, "epoch", 2).unwrap();
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
