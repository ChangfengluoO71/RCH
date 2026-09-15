use rusqlite::{params, Connection};
use rust_lib_app::remote_scan::adapter::{
    RemoteCapabilities, RemoteProviderAdapter, RemoteScanError,
};
use rust_lib_app::remote_scan::engine::{
    CancellationToken, CommittedDirectory, CoverTask, RemoteScanEngine, RetryPolicy,
    ScanCommitSink, ScanDirectoryTask,
};
use rust_lib_app::remote_scan::model::{fingerprint, normalize_path, RemoteAssetKind, RemoteEntry};
use rust_lib_app::remote_scan::persistence;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct RecordingSink {
    commits: Mutex<Vec<CommittedDirectory>>,
    covers: Mutex<Vec<CoverTask>>,
    spilled: Mutex<Vec<ScanDirectoryTask>>,
}

impl RecordingSink {
    fn commits(&self) -> Vec<CommittedDirectory> {
        self.commits.lock().unwrap().clone()
    }
    fn cover_tasks(&self) -> Vec<CoverTask> {
        self.covers.lock().unwrap().clone()
    }
    fn spilled_tasks(&self) -> Vec<ScanDirectoryTask> {
        self.spilled.lock().unwrap().clone()
    }
}

impl ScanCommitSink for RecordingSink {
    fn stage_directory(&self, directory: CommittedDirectory) -> Result<(), RemoteScanError> {
        self.commits.lock().unwrap().push(directory);
        Ok(())
    }
    fn enqueue_cover(&self, task: CoverTask) -> Result<(), RemoteScanError> {
        self.covers.lock().unwrap().push(task);
        Ok(())
    }
    fn spill_directory(&self, task: ScanDirectoryTask) -> Result<(), RemoteScanError> {
        self.spilled.lock().unwrap().push(task);
        Ok(())
    }
    fn take_spilled_directory(
        &self,
        _source_id: &str,
        _generation: i64,
    ) -> Result<Option<ScanDirectoryTask>, RemoteScanError> {
        Ok(self.spilled.lock().unwrap().pop())
    }
}

struct FakeAdapter {
    pages: Mutex<VecDeque<Result<(Vec<RemoteEntry>, Option<String>), RemoteScanError>>>,
}

impl RemoteProviderAdapter for FakeAdapter {
    fn list(
        &self,
        _path: &str,
        _cursor: Option<&str>,
    ) -> Result<(Vec<RemoteEntry>, Option<String>), RemoteScanError> {
        self.pages
            .lock()
            .unwrap()
            .pop_front()
            .expect("configured page")
    }
    fn read_range(
        &self,
        _path: &str,
        _offset: u64,
        _length: u64,
    ) -> Result<Vec<u8>, RemoteScanError> {
        Err(RemoteScanError::Unsupported)
    }
    fn read_file_limited(&self, _path: &str, _max_bytes: u64) -> Result<Vec<u8>, RemoteScanError> {
        Err(RemoteScanError::Unsupported)
    }
    fn normalize_path(&self, path: &str) -> String {
        normalize_path(path)
    }
    fn capabilities(
        &self,
        _path: &str,
        _fingerprint: &str,
    ) -> Result<RemoteCapabilities, RemoteScanError> {
        Ok(RemoteCapabilities::default())
    }
}

fn entry(
    name: &str,
    path: &str,
    is_dir: bool,
    size: Option<u64>,
    mtime: Option<i64>,
) -> RemoteEntry {
    RemoteEntry {
        name: name.into(),
        logical_path: path.into(),
        is_dir,
        size,
        mtime,
        asset_kind: RemoteAssetKind::Other,
    }
}

#[test]
fn remote_scan_canonical_root_filters_system_entries_and_fingerprint_distinguishes_unknown_metadata_and_kind(
) {
    assert_eq!(normalize_path(""), "/");
    assert_eq!(normalize_path("\\"), "/");
    let unknown = entry("page.jpg", "/book/page.jpg", false, None, None);
    let zero = entry("page.jpg", "/book/page.jpg", false, Some(0), Some(0));
    assert_ne!(fingerprint(&[unknown.clone()]), fingerprint(&[zero]));
    let mut other_kind = unknown.clone();
    other_kind.asset_kind = RemoteAssetKind::ImageFile;
    assert_ne!(fingerprint(&[unknown]), fingerprint(&[other_kind]));
}

#[test]
fn remote_scan_duplicate_directory_tasks_coalesce_and_overflow_spills_with_bounded_memory() {
    let adapter = Arc::new(FakeAdapter {
        pages: Mutex::new(VecDeque::new()),
    });
    let sink = Arc::new(RecordingSink::default());
    let engine = RemoteScanEngine::new(adapter, sink.clone(), 1, 1, RetryPolicy::default());
    let task = ScanDirectoryTask::new("source", "/", 7);
    assert!(engine.enqueue_directory(task.clone()).unwrap());
    assert!(!engine.enqueue_directory(task).unwrap());
    assert!(engine
        .enqueue_directory(ScanDirectoryTask::new("source", "/other", 7))
        .unwrap());
    assert_eq!(sink.spilled_tasks().len(), 1);
}

#[test]
fn remote_scan_auth_and_missing_errors_are_never_retried() {
    for error in [
        RemoteScanError::Unauthorized,
        RemoteScanError::Forbidden,
        RemoteScanError::NotFound,
    ] {
        let adapter = Arc::new(FakeAdapter {
            pages: Mutex::new(VecDeque::from([Err(error.clone())])),
        });
        let sink = Arc::new(RecordingSink::default());
        let engine = RemoteScanEngine::new(
            adapter,
            sink,
            1,
            1,
            RetryPolicy::new(3, Duration::from_millis(1), Duration::from_millis(2)),
        );
        engine
            .enqueue_directory(ScanDirectoryTask::new("source-no-retry", "/", 1))
            .unwrap();
        assert_eq!(engine.run_next().unwrap_err(), error);
    }
}

#[test]
fn remote_scan_retry_wait_is_interrupted_by_cancellation() {
    let adapter = Arc::new(FakeAdapter {
        pages: Mutex::new(VecDeque::from([Err(RemoteScanError::RateLimited {
            retry_after_ms: Some(5_000),
        })])),
    });
    let sink = Arc::new(RecordingSink::default());
    let token = CancellationToken::new();
    let engine = Arc::new(RemoteScanEngine::with_token(
        adapter,
        sink,
        1,
        1,
        RetryPolicy::new(3, Duration::from_secs(5), Duration::from_secs(5)),
        token.clone(),
    ));
    engine
        .enqueue_directory(ScanDirectoryTask::new("source-cancel-wait", "/", 1))
        .unwrap();
    let worker = {
        let engine = Arc::clone(&engine);
        std::thread::spawn(move || engine.run_next())
    };
    std::thread::sleep(Duration::from_millis(25));
    token.cancel();
    assert!(matches!(
        worker.join().unwrap(),
        Err(RemoteScanError::Cancelled)
    ));
}

#[test]
fn remote_cover_dependency_is_consumed_into_partial_cache() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE book_sources(id TEXT PRIMARY KEY,fingerprint TEXT);\
         CREATE TABLE library_index(\
           id TEXT PRIMARY KEY,source_id TEXT,parent_id TEXT,name TEXT,path TEXT,entry_type TEXT,\
           size INTEGER,modified_at INTEGER,asset_kind TEXT,content_fingerprint TEXT,\
           scan_generation INTEGER,listing_complete INTEGER NOT NULL DEFAULT 0,deleted INTEGER NOT NULL DEFAULT 0,updated_at INTEGER);",
    )
    .unwrap();
    persistence::migrate(&conn).unwrap();
    let task = CoverTask {
        source_id: "source".into(),
        logical_path: "/book.cbz".into(),
        fingerprint: "fp".into(),
        profile: "default".into(),
    };
    persistence::stage_cover_task(&conn, 4, "shared-cover-key", &task).unwrap();
    persistence::publish_staged_generation(&conn, "source", 4).unwrap();
    persistence::finish_cover_task(
        &conn,
        "source",
        4,
        "shared-cover-key",
        "partial_ready",
        Some(&[1, 2, 3]),
    )
    .unwrap();
    let (status, bytes): (String, Vec<u8>) = conn
        .query_row(
            "SELECT d.status,c.bytes FROM remote_cover_dependency d JOIN remote_cover_partial_cache c USING(book_key) WHERE d.book_key='shared-cover-key'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "partial_ready");
    assert_eq!(bytes, vec![1, 2, 3]);
}

#[test]
fn remote_scan_cancellation_before_commit_retains_the_previous_manifest() {
    let adapter = Arc::new(FakeAdapter {
        pages: Mutex::new(VecDeque::from([Ok((
            vec![entry("book.cbz", "/book.cbz", false, Some(9), Some(3))],
            None,
        ))])),
    });
    let sink = Arc::new(RecordingSink::default());
    let token = CancellationToken::new();
    token.cancel();
    let engine =
        RemoteScanEngine::with_token(adapter, sink.clone(), 2, 2, RetryPolicy::default(), token);
    engine
        .enqueue_directory(ScanDirectoryTask::new("source", "/", 3))
        .unwrap();
    assert!(matches!(engine.run_next(), Err(RemoteScanError::Cancelled)));
    assert!(sink.commits().is_empty());
}

#[test]
fn remote_scan_rate_limit_retry_is_bounded_and_complete_directory_enqueues_unique_covers() {
    let adapter = Arc::new(FakeAdapter {
        pages: Mutex::new(VecDeque::from([
            Err(RemoteScanError::RateLimited {
                retry_after_ms: Some(1),
            }),
            Err(RemoteScanError::RateLimited {
                retry_after_ms: Some(1),
            }),
            Ok((
                vec![
                    entry("book.cbz", "/book.cbz", false, Some(9), Some(3)),
                    entry("book.cbz", "/book.cbz", false, Some(9), Some(3)),
                    entry("$RECYCLE.BIN", "/$RECYCLE.BIN", true, None, None),
                ],
                None,
            )),
        ])),
    });
    let sink = Arc::new(RecordingSink::default());
    let policy = RetryPolicy::new(2, Duration::from_millis(1), Duration::from_millis(2));
    let engine = RemoteScanEngine::new(adapter, sink.clone(), 2, 2, policy);
    engine
        .enqueue_directory(ScanDirectoryTask::new("source", "/", 4))
        .unwrap();
    engine.run_next().unwrap();
    assert_eq!(sink.commits().len(), 1);
    assert_eq!(sink.commits()[0].entries.len(), 1);
    assert_eq!(sink.cover_tasks().len(), 1);
    assert_eq!(sink.cover_tasks()[0].logical_path, "/book.cbz");
}

#[test]
fn remote_scan_manifest_keeps_typed_metadata_and_per_entry_fingerprint() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE book_sources(id TEXT PRIMARY KEY,fingerprint TEXT);\
         CREATE TABLE library_index(\
           id TEXT PRIMARY KEY,source_id TEXT,parent_id TEXT,name TEXT,path TEXT,entry_type TEXT,\
           size INTEGER,modified_at INTEGER,asset_kind TEXT,content_fingerprint TEXT,\
           scan_generation INTEGER,listing_complete INTEGER NOT NULL DEFAULT 0,deleted INTEGER NOT NULL DEFAULT 0,updated_at INTEGER);",
    ).unwrap();
    persistence::migrate(&conn).unwrap();
    conn.execute(
        "INSERT INTO book_sources VALUES('source','canonical-source')",
        [],
    )
    .unwrap();
    let image = RemoteEntry {
        name: "page.avif".into(),
        logical_path: "/album/page.avif".into(),
        is_dir: false,
        size: None,
        mtime: Some(42),
        asset_kind: RemoteAssetKind::ImageFile,
    };
    persistence::upsert_complete_listing(
        &conn,
        "source",
        "/album",
        std::slice::from_ref(&image),
        9,
        "directory-fingerprint",
        true,
    )
    .unwrap();
    let stored: (Option<i64>, Option<i64>, String, String) = conn.query_row(
        "SELECT size,modified_at,asset_kind,content_fingerprint FROM library_index WHERE path='/album/page.avif'",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).unwrap();
    assert_eq!(stored.0, None);
    assert_eq!(stored.1, Some(42));
    assert_eq!(stored.2, "ImageFile");
    assert_eq!(stored.3, fingerprint(&[image]));
    let listing_fingerprint: String = conn.query_row(
        "SELECT content_fingerprint FROM remote_listing_state WHERE source_id=?1 AND logical_path=?2",
        params!["source", "/album"], |row| row.get(0),
    ).unwrap();
    assert_eq!(listing_fingerprint, "directory-fingerprint");
}

#[test]
fn staged_generation_is_invisible_until_publish_and_discard_retains_previous_generation() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE book_sources(id TEXT PRIMARY KEY,fingerprint TEXT);\
         CREATE TABLE library_index(\
           id TEXT PRIMARY KEY,source_id TEXT,parent_id TEXT,name TEXT,path TEXT,entry_type TEXT,\
           size INTEGER,modified_at INTEGER,asset_kind TEXT,content_fingerprint TEXT,\
           scan_generation INTEGER,listing_complete INTEGER NOT NULL DEFAULT 0,deleted INTEGER NOT NULL DEFAULT 0,updated_at INTEGER);",
    ).unwrap();
    persistence::migrate(&conn).unwrap();
    conn.execute(
        "INSERT INTO book_sources VALUES('source','canonical-source')",
        [],
    )
    .unwrap();
    let old = entry("old.cbz", "/old.cbz", false, Some(1), Some(1));
    persistence::upsert_complete_listing(&conn, "source", "/", &[old], 1, "old", true).unwrap();

    let fresh = entry("new.cbz", "/new.cbz", false, Some(2), Some(2));
    persistence::stage_complete_listing(
        &conn,
        "source",
        "/",
        &[fresh],
        2,
        "new",
        RemoteAssetKind::ContainerDir,
        false,
    )
    .unwrap();
    let visible_before: i64 = conn.query_row(
        "SELECT COUNT(*) FROM library_index WHERE source_id='source' AND path='/new.cbz' AND listing_complete=1",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(visible_before, 0);

    persistence::discard_staged_generation(&conn, "source", 2).unwrap();
    let old_visible: i64 = conn.query_row(
        "SELECT COUNT(*) FROM library_index WHERE source_id='source' AND path='/old.cbz' AND deleted=0",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(old_visible, 1);
}

#[test]
fn successful_generation_publishes_and_reconciles_missing_children_atomically() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE book_sources(id TEXT PRIMARY KEY,fingerprint TEXT);\
         CREATE TABLE library_index(\
           id TEXT PRIMARY KEY,source_id TEXT,parent_id TEXT,name TEXT,path TEXT,entry_type TEXT,\
           size INTEGER,modified_at INTEGER,asset_kind TEXT,content_fingerprint TEXT,\
           scan_generation INTEGER,listing_complete INTEGER NOT NULL DEFAULT 0,deleted INTEGER NOT NULL DEFAULT 0,updated_at INTEGER);",
    ).unwrap();
    persistence::migrate(&conn).unwrap();
    conn.execute(
        "INSERT INTO book_sources VALUES('source','canonical-source')",
        [],
    )
    .unwrap();
    persistence::upsert_complete_listing(
        &conn,
        "source",
        "/",
        &[entry("gone.cbz", "/gone.cbz", false, Some(1), Some(1))],
        1,
        "old",
        true,
    )
    .unwrap();
    persistence::stage_complete_listing(
        &conn,
        "source",
        "/",
        &[entry("kept.cbz", "/kept.cbz", false, Some(2), Some(2))],
        2,
        "new",
        RemoteAssetKind::ContainerDir,
        false,
    )
    .unwrap();
    persistence::publish_staged_generation(&conn, "source", 2).unwrap();

    let rows: (i64, i64) = conn.query_row(
        "SELECT SUM(path='/kept.cbz' AND deleted=0),SUM(path='/gone.cbz' AND deleted=1) FROM library_index WHERE source_id='source'",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(rows, (1, 1));
}
