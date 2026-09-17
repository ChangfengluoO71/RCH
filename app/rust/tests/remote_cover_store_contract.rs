use rusqlite::Connection;
use rust_lib_app::remote_scan::persistence;

fn base_connection() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE book_sources(id TEXT PRIMARY KEY,type TEXT NOT NULL,fingerprint TEXT NOT NULL,path TEXT,root_id TEXT);
         CREATE TABLE library_index(
           id TEXT PRIMARY KEY,source_id TEXT,parent_id TEXT,name TEXT,path TEXT,entry_type TEXT,
           size INTEGER,modified_at INTEGER,asset_kind TEXT,content_fingerprint TEXT,
           scan_generation INTEGER,listing_complete INTEGER NOT NULL DEFAULT 0,deleted INTEGER NOT NULL DEFAULT 0,updated_at INTEGER);
         INSERT INTO book_sources(id,type,fingerprint,path) VALUES('source','115','fingerprint-v1','/');
         INSERT INTO library_index(id,source_id,name,path,entry_type,listing_complete,deleted)
           VALUES('legacy-key','source','legacy.cbz','/legacy.cbz','file',1,0);",
    )
    .unwrap();
    conn
}

#[test]
fn cover_schema_is_additive_and_idempotent() {
    let conn = base_connection();
    persistence::migrate(&conn).unwrap();
    persistence::migrate(&conn).unwrap();

    for table in [
        "remote_asset_route",
        "remote_directory_cover",
        "remote_cover_job",
        "remote_cover_blob",
        "remote_cover_variant",
        "remote_cover_ref",
        "remote_view_revision",
    ] {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists, "missing additive table {table}");
    }

    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('remote_listing_state')")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(columns.iter().any(|name| name == "last_checked_at"));
    assert!(columns.iter().any(|name| name == "recheck_after"));

    let legacy: String = conn
        .query_row(
            "SELECT path FROM library_index WHERE id='legacy-key'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(legacy, "/legacy.cbz");
}
