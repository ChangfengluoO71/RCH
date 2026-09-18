use rust_lib_app::{cache, db};
use rust_lib_app::remote_scan::{cover_model::{CoverJobKey, CoverJobState}, cover_service, cover_store};
use rust_lib_app::remote_scan::cover_state::CoverJobUpsertCause;

#[test]
fn missing_published_cover_becomes_pending_without_resetting_user_selection() {
    let root = std::env::temp_dir().join(format!("rch_missing_cover_{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    cache::set_custom_cache_root(root.to_str().unwrap());
    let key = CoverJobKey {
        source_id: "source".into(), asset_id: "asset".into(),
        content_revision: "content".into(), selection_revision: "page:2|crop:".into(),
        profile: "170x240@1".into(),
    };
    {
        let conn = db::get().lock().unwrap();
        cover_store::migrate(&conn).unwrap();
        cover_store::upsert_job_on(&conn, &key, CoverJobState::Ready, "background", 10, 1, "epoch", 1, CoverJobUpsertCause::Demand).unwrap();
        conn.execute("INSERT INTO remote_cover_variant(source_id,asset_id,content_revision,selection_revision,profile,state,revision,updated_at) VALUES('source','asset','content','page:2|crop:','170x240@1','ready',1,1)", []).unwrap();
    }
    assert!(cover_service::read_cached_cover("source", "asset", "page:2|crop:", "170x240@1").unwrap().is_none());
    let conn = db::get().lock().unwrap();
    let job = cover_store::load_job_on(&conn, &key.encode()).unwrap().unwrap();
    assert_eq!(job.state, CoverJobState::Pending);
    assert_eq!(job.key.selection_revision, "page:2|crop:");
    // Close the fixture DB before removing files on Windows.
    drop(conn);
    db::open_at(":memory:").unwrap();
    cache::set_custom_cache_root("");
    std::fs::remove_dir_all(root).unwrap();
}
