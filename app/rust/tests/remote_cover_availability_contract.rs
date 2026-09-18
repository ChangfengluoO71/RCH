//! P1-F：`cover_material_available` 是**纯**判断（只读缓存/文件系统）。
//!
//! 它必须：不 bump revision、不产生 wake、不改任何 durable state、不建 session。
//! 同时钉住"有字节 ⇒ true / 无字节 ⇒ false"。

use rusqlite::{params, Connection};
use rust_lib_app::remote_scan::cover_model::{CoverJobKey, CoverJobState};
use rust_lib_app::remote_scan::cover_revision_stream::{reset_wake_decided_count, wake_decided_count};
use rust_lib_app::remote_scan::cover_state::CoverJobUpsertCause;
use rust_lib_app::remote_scan::{cover_service, cover_store, persistence};
use rust_lib_app::{cache, db};

const SELECTION: &str = "page:2|crop:|asset:";
const PROFILE: &str = "170x240@1";

struct CacheRootGuard;
impl Drop for CacheRootGuard {
    fn drop(&mut self) {
        cache::set_custom_cache_root("");
    }
}

fn prepare(source_id: &str) -> CacheRootGuard {
    let root = std::env::temp_dir().join(format!("rch_f_avail_{source_id}"));
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
        "INSERT OR REPLACE INTO remote_scan_epoch(
             source_id,generation,source_fingerprint,root_path,session_epoch,session_token)
         VALUES(?1,1,'fp','/','e1',42)",
        params![source_id],
    )
    .unwrap();
    cover_store::upsert_job_on(
        &conn,
        &CoverJobKey {
            source_id: source_id.into(),
            asset_id: "asset".into(),
            content_revision: "content".into(),
            selection_revision: SELECTION.into(),
            profile: PROFILE.into(),
        },
        CoverJobState::Ready,
        "background",
        10,
        1,
        "e1",
        0,
        CoverJobUpsertCause::Demand,
    )
    .unwrap();
    drop(conn);
    CacheRootGuard
}

#[test]
fn f_availability_reflects_the_real_bytes() {
    let source = "f-avail-bytes";
    let _guard = prepare(source);

    // 无字节 ⇒ false
    assert!(
        !cover_service::cover_material_available(source, "asset", "content", SELECTION, PROFILE),
        "F: absent bytes => not available"
    );

    cache::remote_cover_cache_write(
        source, "asset", "content", SELECTION, PROFILE, 4, 4, &vec![7_u8; 4 * 4 * 4],
    )
    .unwrap();

    // 有字节 ⇒ true
    assert!(
        cover_service::cover_material_available(source, "asset", "content", SELECTION, PROFILE),
        "F: present bytes => available"
    );
}

#[test]
fn f_availability_is_a_pure_read() {
    let source = "f-avail-pure";
    let _guard = prepare(source);
    cache::remote_cover_cache_write(
        source, "asset", "content", SELECTION, PROFILE, 4, 4, &vec![9_u8; 4 * 4 * 4],
    )
    .unwrap();

    let (revision_before, jobs_before) = {
        let conn = db::get().lock().unwrap();
        let rev = cover_store::view_revision(&conn, source).unwrap();
        let jobs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM remote_cover_job WHERE source_id=?1",
                [source],
                |row| row.get(0),
            )
            .unwrap();
        (rev, jobs)
    };
    reset_wake_decided_count();
    let wake_before = wake_decided_count();

    for _ in 0..3 {
        let _ = cover_service::cover_material_available(source, "asset", "content", SELECTION, PROFILE);
    }

    let (revision_after, jobs_after) = {
        let conn = db::get().lock().unwrap();
        let rev = cover_store::view_revision(&conn, source).unwrap();
        let jobs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM remote_cover_job WHERE source_id=?1",
                [source],
                |row| row.get(0),
            )
            .unwrap();
        (rev, jobs)
    };
    assert_eq!(revision_after, revision_before, "F: availability check must not bump the revision");
    assert_eq!(jobs_after, jobs_before, "F: availability check must not mutate jobs");
    assert_eq!(
        wake_decided_count(),
        wake_before,
        "F: availability check must not wake"
    );
}
