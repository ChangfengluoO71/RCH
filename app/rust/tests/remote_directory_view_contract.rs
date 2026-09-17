use rusqlite::params;
use rusqlite::Connection;
use rust_lib_app::remote_scan::catalog::{directory_view_on, CatalogEntryKind};
use rust_lib_app::remote_scan::model::{RemoteAssetKind, RemoteEntry};
use rust_lib_app::remote_scan::persistence;

fn fixture() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE book_sources(id TEXT PRIMARY KEY,type TEXT NOT NULL,fingerprint TEXT NOT NULL,path TEXT,root_id TEXT);
         CREATE TABLE library_index(
           id TEXT PRIMARY KEY,source_id TEXT,parent_id TEXT,name TEXT,path TEXT,entry_type TEXT,
           size INTEGER,modified_at INTEGER,asset_kind TEXT,content_fingerprint TEXT,
           scan_generation INTEGER,listing_complete INTEGER NOT NULL DEFAULT 0,deleted INTEGER NOT NULL DEFAULT 0,updated_at INTEGER);
         INSERT INTO book_sources(id,type,fingerprint,path) VALUES('source','115','fp','/');",
    )
    .unwrap();
    persistence::migrate(&conn).unwrap();
    let root = rust_lib_app::db::library_index_id("fp", "/");
    let container = rust_lib_app::db::library_index_id("fp", "/A");
    let images = rust_lib_app::db::library_index_id("fp", "/B");
    let a_file = rust_lib_app::db::library_index_id("fp", "/A/a.cbz");
    let b_one = rust_lib_app::db::library_index_id("fp", "/B/1.jpg");
    let b_two = rust_lib_app::db::library_index_id("fp", "/B/2.jpg");
    conn.execute_batch(&format!(
        "INSERT INTO library_index(id,source_id,parent_id,name,path,entry_type,asset_kind,content_fingerprint,scan_generation,listing_complete,deleted)
           VALUES('{container}','source','{root}','A','/A','dir','ContainerDir','A',1,1,0),
                 ('{images}','source','{root}','B','/B','dir','ImageFolder','B',1,1,0),
                 ('{a_file}','source','{container}','a.cbz','/A/a.cbz','file','ArchiveFile','a',1,1,0),
                 ('{b_one}','source','{images}','1.jpg','/B/1.jpg','file','ImageFile','b1',1,1,0),
                 ('{b_two}','source','{images}','2.jpg','/B/2.jpg','file','ImageFile','b2',1,1,0);",
    ))
    .unwrap();
    conn.execute(
        "INSERT INTO remote_listing_state(source_id,logical_path,content_fingerprint,scan_generation,listing_complete) VALUES('source','/','root',1,1)",
        [],
    )
    .unwrap();
    conn
}

#[test]
fn root_view_selects_container_and_image_folder_representatives_without_snapshot() {
    let conn = fixture();
    let view = directory_view_on(&conn, "source", "/", 0, 200).unwrap();
    assert_eq!(view.entries.len(), 2);
    let a = view.entries.iter().find(|entry| entry.name == "A").unwrap();
    assert_eq!(a.kind, CatalogEntryKind::ContainerDir);
    assert!(a.representative_asset_id.is_some());
    let b = view.entries.iter().find(|entry| entry.name == "B").unwrap();
    assert_eq!(b.kind, CatalogEntryKind::ImageFolder);
    assert!(b.representative_asset_id.is_some());
    assert!(view.listing_complete);
}

#[test]
fn newer_cover_job_keeps_previous_variant_marked_while_refreshing() {
    let conn = fixture();
    let asset_id = rust_lib_app::db::library_index_id("fp", "/A/a.cbz");
    conn.execute(
        "INSERT INTO remote_cover_variant(
             source_id,asset_id,content_revision,selection_revision,profile,
             blob_key,state,revision,is_previous_revision,updated_at)
         VALUES(?1,?2,'content-v1','default','340x480@1','blob-v1','ready',100,0,100)",
        params!["source", asset_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO remote_cover_job(
             job_key,source_id,asset_id,content_revision,selection_revision,profile,
             state,demand_kind,priority,attempt,generation,session_epoch,updated_at)
         VALUES(?1,?2,?3,'content-v2','default','340x480@1','running','background',0,0,1,'epoch',200)",
        params!["refresh-job", "source", asset_id],
    )
    .unwrap();

    let view = directory_view_on(&conn, "source", "/", 0, 200).unwrap();
    let container = view.entries.iter().find(|entry| entry.name == "A").unwrap();
    assert_eq!(container.cover.state, "running");
    assert!(!container.cover.ready);
    assert!(container.cover.is_previous_revision);
    assert_eq!(container.cover.revision, 200);
}

#[test]
fn view_is_stable_and_caps_limit() {
    let conn = fixture();
    let view = directory_view_on(&conn, "source", "/", 0, 10_000).unwrap();
    assert!(view
        .entries
        .windows(2)
        .all(|items| items[0].name <= items[1].name));
    assert!(view.entries.len() <= 200);
}

#[test]
fn running_generation_exposes_preview_without_overwriting_authoritative_rows() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE book_sources(
           id TEXT PRIMARY KEY,type TEXT NOT NULL,fingerprint TEXT NOT NULL,
           path TEXT,root_id TEXT,deleted INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE library_index(
           id TEXT PRIMARY KEY,source_id TEXT,parent_id TEXT,name TEXT,path TEXT,
           entry_type TEXT,size INTEGER,modified_at INTEGER,asset_kind TEXT,
           content_fingerprint TEXT,scan_generation INTEGER,
           listing_complete INTEGER NOT NULL DEFAULT 0,
           deleted INTEGER NOT NULL DEFAULT 0,updated_at INTEGER);
         INSERT INTO book_sources(id,type,fingerprint,path,root_id)
           VALUES('source','115','fp','/','/');",
    )
    .unwrap();
    persistence::migrate(&conn).unwrap();
    conn.execute(
        "INSERT INTO remote_scan_state(source_id,status,mode,generation)
         VALUES('source','Running','Snapshot',2)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO remote_scan_epoch(
             source_id,generation,source_fingerprint,root_path,session_epoch,session_token)
         VALUES('source',2,'fp','/','epoch-v2',2)",
        [],
    )
    .unwrap();
    let old = RemoteEntry {
        name: "old.cbz".into(),
        logical_path: "/old.cbz".into(),
        provider_path: Some("old-pickcode".into()),
        is_dir: false,
        size: Some(10),
        mtime: Some(1),
        asset_kind: RemoteAssetKind::ArchiveFile,
    };
    persistence::upsert_complete_listing(&conn, "source", "/", &[old], 1, "old", true).unwrap();
    let fresh = RemoteEntry {
        name: "new.cbz".into(),
        logical_path: "/new.cbz".into(),
        provider_path: Some("new-pickcode".into()),
        is_dir: false,
        size: Some(20),
        mtime: Some(2),
        asset_kind: RemoteAssetKind::ArchiveFile,
    };
    persistence::stage_complete_listing(
        &conn,
        "source",
        "/",
        std::slice::from_ref(&fresh),
        2,
        "new",
        RemoteAssetKind::ContainerDir,
        false,
        "epoch-v2",
    )
    .unwrap();
    persistence::materialize_preview_listing(
        &conn,
        "source",
        "/",
        std::slice::from_ref(&fresh),
        2,
        "epoch-v2",
        "new",
        RemoteAssetKind::ContainerDir,
    )
    .unwrap();

    let preview = directory_view_on(&conn, "source", "/", 0, 200).unwrap();
    assert!(!preview.listing_complete);
    assert_eq!(preview.entries.len(), 1);
    assert_eq!(preview.entries[0].name, "new.cbz");
    assert_eq!(
        preview.entries[0].provider_path.as_deref(),
        Some("new-pickcode")
    );

    persistence::discard_staged_generation(&conn, "source", 2).unwrap();
    let after_discard = directory_view_on(&conn, "source", "/", 0, 200).unwrap();
    assert_eq!(after_discard.entries.len(), 1);
    assert_eq!(after_discard.entries[0].name, "old.cbz");
    let preview_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM remote_scan_preview WHERE source_id=?1",
            params!["source"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(preview_rows, 0);
}
