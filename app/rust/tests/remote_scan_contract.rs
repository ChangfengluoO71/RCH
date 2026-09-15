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
}

impl RecordingSink {
    fn commits(&self) -> Vec<CommittedDirectory> {
        self.commits.lock().unwrap().clone()
    }
    fn cover_tasks(&self) -> Vec<CoverTask> {
        self.covers.lock().unwrap().clone()
    }
}

impl ScanCommitSink for RecordingSink {
    fn commit_directory(&self, directory: CommittedDirectory) -> Result<(), RemoteScanError> {
        self.commits.lock().unwrap().push(directory);
        Ok(())
    }
    fn enqueue_cover(&self, task: CoverTask) -> Result<(), RemoteScanError> {
        self.covers.lock().unwrap().push(task);
        Ok(())
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
fn remote_scan_duplicate_directory_tasks_coalesce_and_queue_capacity_is_bounded() {
    let adapter = Arc::new(FakeAdapter {
        pages: Mutex::new(VecDeque::new()),
    });
    let sink = Arc::new(RecordingSink::default());
    let engine = RemoteScanEngine::new(adapter, sink, 1, 1, RetryPolicy::default());
    let task = ScanDirectoryTask::new("source", "/", 7);
    assert!(engine.enqueue_directory(task.clone()).unwrap());
    assert!(!engine.enqueue_directory(task).unwrap());
    assert!(engine
        .enqueue_directory(ScanDirectoryTask::new("source", "/other", 7))
        .is_err());
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
