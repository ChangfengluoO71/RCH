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
        provider_path: None,
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
    let task = ScanDirectoryTask::new("source", "/", 7).with_session_epoch("spill-proof");
    assert!(engine.enqueue_directory(task.clone()).unwrap());
    assert!(!engine.enqueue_directory(task).unwrap());
    assert!(engine
        .enqueue_directory(
            ScanDirectoryTask::new("source", "/other", 7).with_session_epoch("spill-proof"),
        )
        .unwrap());
    let spilled = sink.spilled_tasks();
    assert_eq!(spilled.len(), 1);
    assert_eq!(spilled[0].session_epoch, "spill-proof");
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
        generation: 4,
        session_epoch: String::new(),
    };
    let sibling_dependency = CoverTask {
        source_id: "source".into(),
        logical_path: "/book/cover.jpg".into(),
        fingerprint: "cover-fp".into(),
        profile: "default".into(),
        generation: 4,
        session_epoch: String::new(),
    };
    persistence::stage_cover_task(&conn, 4, "shared-cover-key", &task).unwrap();
    persistence::stage_cover_task(&conn, 4, "shared-cover-key", &sibling_dependency).unwrap();
    conn.execute(
        "INSERT INTO remote_scan_state(source_id,status,mode,generation) VALUES('source','Running','Snapshot',4)",
        [],
    )
    .unwrap();
    persistence::publish_staged_generation(&conn, "source", 4).unwrap();
    assert_eq!(
        persistence::count_staged_cover_tasks(&conn, "source", 4).unwrap(),
        1,
        "multiple dependencies for one book must count as one comic"
    );
    persistence::finish_cover_task(
        &conn,
        "source",
        4,
        "shared-cover-key",
        "/book.cbz",
        "",
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
    let sibling_status: String = conn
        .query_row(
            "SELECT status FROM remote_cover_dependency WHERE book_key='shared-cover-key' AND dependency_path='/book/cover.jpg'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(sibling_status, "queued");
    let remaining_stage: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM remote_cover_stage WHERE book_key='shared-cover-key' AND dependency_path='/book/cover.jpg'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining_stage, 1);
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
fn remote_scan_epoch_flows_from_directory_task_to_commit_and_cover_tasks() {
    let adapter = Arc::new(FakeAdapter {
        pages: Mutex::new(VecDeque::from([Ok((
            vec![
                entry("book.cbz", "/book.cbz", false, Some(9), Some(3)),
                entry("chapter", "/chapter", true, None, None),
            ],
            None,
        ))])),
    });
    let sink = Arc::new(RecordingSink::default());
    let engine = RemoteScanEngine::new(adapter, sink.clone(), 4, 4, RetryPolicy::default());
    engine
        .enqueue_directory(
            ScanDirectoryTask::new("source", "/", 5).with_session_epoch("epoch-proof"),
        )
        .unwrap();
    engine.run_next().unwrap();

    let commits = sink.commits();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].generation, 5);
    assert_eq!(commits[0].session_epoch, "epoch-proof");
    let covers = sink.cover_tasks();
    assert_eq!(covers.len(), 1);
    assert_eq!(covers[0].generation, 5);
    assert_eq!(covers[0].session_epoch, "epoch-proof");
}

#[test]
fn remote_scan_pending_directory_roundtrip_preserves_session_epoch() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE book_sources(id TEXT PRIMARY KEY,type TEXT NOT NULL,fingerprint TEXT NOT NULL,path TEXT,root_id TEXT);\
         CREATE TABLE library_index(\
           id TEXT PRIMARY KEY,source_id TEXT,parent_id TEXT,name TEXT,path TEXT,entry_type TEXT,\
           size INTEGER,modified_at INTEGER,asset_kind TEXT,content_fingerprint TEXT,\
           scan_generation INTEGER,listing_complete INTEGER NOT NULL DEFAULT 0,deleted INTEGER NOT NULL DEFAULT 0,updated_at INTEGER);",
    )
    .unwrap();
    persistence::migrate(&conn).unwrap();
    conn.execute(
        "INSERT INTO book_sources VALUES('source','webdav','canonical-source','/',NULL)",
        [],
    )
    .unwrap();
    let session_epoch = persistence::bind_scan_epoch(&conn, "source", 6, "/", 42).unwrap();
    conn.execute(
        "INSERT INTO remote_scan_state(source_id,status,mode,generation) VALUES('source','Running','Snapshot',6)",
        [],
    )
    .unwrap();

    persistence::store_pending_task(
        &conn,
        &ScanDirectoryTask::new("source", "/chapter", 6)
            .with_session_epoch(session_epoch.clone())
            .incremental(),
    )
    .unwrap();
    let restored = persistence::take_pending_task(&conn, "source", 6)
        .unwrap()
        .unwrap();

    assert_eq!(restored.logical_path, "/chapter");
    assert!(restored.incremental);
    assert_eq!(restored.session_epoch, session_epoch);
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
        provider_path: None,
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
        "",
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
        "",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO remote_scan_state(source_id,status,mode,generation) VALUES('source','Running','Snapshot',2)",
        [],
    )
    .unwrap();
    persistence::publish_staged_generation(&conn, "source", 2).unwrap();

    let rows: (i64, i64) = conn.query_row(
        "SELECT SUM(path='/kept.cbz' AND deleted=0),SUM(path='/gone.cbz' AND deleted=1) FROM library_index WHERE source_id='source'",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(rows, (1, 1));
}

#[test]
fn publication_rejects_a_generation_without_current_persisted_scan_state() {
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
    conn.execute(
        "INSERT INTO book_sources VALUES('source','canonical-source')",
        [],
    )
    .unwrap();
    persistence::stage_complete_listing(
        &conn,
        "source",
        "/",
        &[entry("book.cbz", "/book.cbz", false, Some(1), Some(1))],
        2,
        "root-v2",
        RemoteAssetKind::ContainerDir,
        false,
        "",
    )
    .unwrap();

    assert!(persistence::publish_staged_generation(&conn, "source", 2).is_err());
    let published: i64 = conn
        .query_row("SELECT COUNT(*) FROM library_index", [], |row| row.get(0))
        .unwrap();
    assert_eq!(published, 0);
}

/// Provider names are part of the user-visible scan/status contract.  Keep the
/// matrix in one fixture so a provider cannot accidentally get a different
/// pagination, classification, or error policy in tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderKind {
    WebDav,
    Sftp,
    Baidu,
    Cloud115,
    Quark,
}

impl ProviderKind {
    const ALL: [Self; 5] = [
        Self::WebDav,
        Self::Sftp,
        Self::Baidu,
        Self::Cloud115,
        Self::Quark,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::WebDav => "WebDAV",
            Self::Sftp => "SFTP",
            Self::Baidu => "Baidu",
            Self::Cloud115 => "115",
            Self::Quark => "Quark",
        }
    }

    fn source_type(self) -> &'static str {
        match self {
            Self::WebDav => "webdav",
            Self::Sftp => "sftp",
            Self::Baidu => "baidu",
            Self::Cloud115 => "115",
            Self::Quark => "quark",
        }
    }

    fn source_id(self) -> String {
        format!("contract-{}", self.source_type())
    }
}

/// A provider-labelled fake adapter.  It intentionally models only the
/// adapter boundary (pages and typed outcomes); no provider credentials,
/// headers, or full URLs are represented here.
struct ProviderFixture {
    provider: ProviderKind,
    pages: Mutex<VecDeque<Result<(Vec<RemoteEntry>, Option<String>), RemoteScanError>>>,
    requests: Mutex<Vec<(String, Option<String>)>>,
}

impl ProviderFixture {
    fn new(
        provider: ProviderKind,
        pages: impl IntoIterator<Item = Result<(Vec<RemoteEntry>, Option<String>), RemoteScanError>>,
    ) -> Self {
        Self {
            provider,
            pages: Mutex::new(pages.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<(String, Option<String>)> {
        self.requests.lock().unwrap().clone()
    }
}

impl RemoteProviderAdapter for ProviderFixture {
    fn list(
        &self,
        path: &str,
        cursor: Option<&str>,
    ) -> Result<(Vec<RemoteEntry>, Option<String>), RemoteScanError> {
        self.requests
            .lock()
            .unwrap()
            .push((path.to_string(), cursor.map(str::to_string)));
        self.pages
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| panic!("{} fixture ran out of pages", self.provider.label()))
    }

    fn read_range(
        &self,
        _path: &str,
        _offset: u64,
        _length: u64,
    ) -> Result<Vec<u8>, RemoteScanError> {
        Err(RemoteScanError::RangeUnavailable)
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

fn provider_entry(
    name: &str,
    path: &str,
    is_dir: bool,
    size: Option<u64>,
    mtime: Option<i64>,
) -> RemoteEntry {
    entry(name, path, is_dir, size, mtime)
}

fn provider_engine(
    provider: ProviderKind,
    pages: impl IntoIterator<Item = Result<(Vec<RemoteEntry>, Option<String>), RemoteScanError>>,
    sink: Arc<RecordingSink>,
    retry: RetryPolicy,
) -> (Arc<ProviderFixture>, RemoteScanEngine) {
    let adapter = Arc::new(ProviderFixture::new(provider, pages));
    let engine = RemoteScanEngine::new(adapter.clone(), sink, 16, 16, retry);
    (adapter, engine)
}

#[test]
fn provider_matrix_labels_complete_pagination_and_rejects_partial_listing() {
    for provider in ProviderKind::ALL {
        let label = provider.label();
        let source_id = provider.source_id();
        let sink = Arc::new(RecordingSink::default());
        let (adapter, engine) = provider_engine(
            provider,
            [
                Ok((
                    vec![provider_entry(
                        "first.cbz",
                        "/first.cbz",
                        false,
                        Some(10),
                        None,
                    )],
                    Some("page-2".into()),
                )),
                Ok((
                    vec![provider_entry(
                        "second.cbz",
                        "/second.cbz",
                        false,
                        Some(20),
                        Some(2),
                    )],
                    None,
                )),
            ],
            sink.clone(),
            RetryPolicy::new(0, Duration::from_millis(1), Duration::from_millis(1)),
        );
        engine
            .enqueue_directory(ScanDirectoryTask::new(&source_id, "/", 1))
            .unwrap();
        assert!(engine.run_next().unwrap(), "{label} complete listing");
        let commits = sink.commits();
        assert_eq!(commits.len(), 1, "{label} commit count");
        assert_eq!(commits[0].entries.len(), 2, "{label} page merge");
        assert_eq!(commits[0].entries[0].mtime, None, "{label} missing mtime");
        assert_eq!(
            adapter.requests(),
            vec![
                ("/".to_string(), None),
                ("/".to_string(), Some("page-2".to_string())),
            ],
            "{label} cursor propagation"
        );

        let partial_sink = Arc::new(RecordingSink::default());
        let (_, partial_engine) = provider_engine(
            provider,
            [
                Ok((
                    vec![provider_entry(
                        "only-first-page.cbz",
                        "/only-first-page.cbz",
                        false,
                        Some(1),
                        None,
                    )],
                    Some("missing-page".into()),
                )),
                Err(RemoteScanError::TransientNetwork("page interrupted".into())),
            ],
            partial_sink.clone(),
            RetryPolicy::new(0, Duration::from_millis(1), Duration::from_millis(1)),
        );
        partial_engine
            .enqueue_directory(ScanDirectoryTask::new(&source_id, "/", 2))
            .unwrap();
        let error = partial_engine.run_next().unwrap_err();
        assert!(
            matches!(error, RemoteScanError::TransientNetwork(_)),
            "{label} partial error"
        );
        assert!(partial_sink.commits().is_empty(), "{label} partial commit");
    }
}

#[test]
fn provider_matrix_classifies_nested_image_folders_and_retains_unknown_mtime() {
    for provider in ProviderKind::ALL {
        let label = provider.label();
        let source_id = provider.source_id();
        let sink = Arc::new(RecordingSink::default());
        let (_, engine) = provider_engine(
            provider,
            [
                Ok((
                    vec![provider_entry("Series", "/Series", true, None, None)],
                    None,
                )),
                Ok((
                    vec![
                        provider_entry("01.jpg", "/Series/01.jpg", false, Some(10), None),
                        provider_entry("2.png", "/Series/2.png", false, Some(11), Some(4)),
                        provider_entry(".hidden.jpg", "/Series/.hidden.jpg", false, Some(99), None),
                    ],
                    None,
                )),
            ],
            sink.clone(),
            RetryPolicy::new(0, Duration::from_millis(1), Duration::from_millis(1)),
        );
        engine
            .enqueue_directory(ScanDirectoryTask::new(&source_id, "/", 3))
            .unwrap();
        engine.run_next().unwrap();
        engine.run_next().unwrap();

        let commits = sink.commits();
        let nested = commits
            .iter()
            .find(|commit| commit.logical_path == "/Series")
            .unwrap_or_else(|| panic!("{label} nested image-folder commit missing"));
        assert_eq!(
            nested.asset_kind,
            RemoteAssetKind::ImageFolder,
            "{label} image folder"
        );
        assert_eq!(nested.entries.len(), 2, "{label} hidden child filtering");
        assert!(
            nested.entries.iter().any(|item| item.mtime.is_none()),
            "{label} missing mtime"
        );
        let cover_tasks = sink.cover_tasks();
        assert_eq!(cover_tasks.len(), 1, "{label} folder cover task");
        assert_eq!(cover_tasks[0].logical_path, "/Series", "{label} cover path");
    }
}

#[test]
fn provider_matrix_keeps_auth_http_and_range_failures_typed_without_commit() {
    let outcomes = [
        ("auth-expired", RemoteScanError::Unauthorized),
        ("403", RemoteScanError::Forbidden),
        ("404", RemoteScanError::NotFound),
        (
            "429",
            RemoteScanError::RateLimited {
                retry_after_ms: Some(25),
            },
        ),
        ("range-unavailable", RemoteScanError::RangeUnavailable),
    ];
    for provider in ProviderKind::ALL {
        let label = provider.label();
        for (outcome_label, expected) in &outcomes {
            let sink = Arc::new(RecordingSink::default());
            let source_id = provider.source_id();
            let (_, engine) = provider_engine(
                provider,
                [Err(expected.clone())],
                sink.clone(),
                RetryPolicy::new(0, Duration::from_millis(1), Duration::from_millis(1)),
            );
            engine
                .enqueue_directory(ScanDirectoryTask::new(&source_id, "/", 4))
                .unwrap();
            let actual = engine.run_next().unwrap_err();
            assert_eq!(&actual, expected, "{label} {outcome_label} typed outcome");
            assert!(sink.commits().is_empty(), "{label} {outcome_label} commit");
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RangeFixtureResponse {
    status: u16,
    content_range: Option<&'static str>,
    body: Vec<u8>,
}

/// Validate the minimal HTTP partial-read contract at the test boundary.
/// Production adapters currently expose owned `Vec<u8>` values, so this pure
/// fixture keeps status/header validation testable without changing transport
/// semantics or pretending a full response is a valid range response.
fn parse_range_fixture(
    response: &RangeFixtureResponse,
    requested_offset: u64,
    requested_length: u64,
) -> Result<Vec<u8>, RemoteScanError> {
    if response.status != 206 || requested_length == 0 {
        return Err(RemoteScanError::RangeUnavailable);
    }
    let Some(header) = response.content_range else {
        return Err(RemoteScanError::RangeUnavailable);
    };
    let Some((unit, range_and_total)) = header.split_once(' ') else {
        return Err(RemoteScanError::RangeUnavailable);
    };
    if unit != "bytes" {
        return Err(RemoteScanError::RangeUnavailable);
    }
    let Some((range, total)) = range_and_total.split_once('/') else {
        return Err(RemoteScanError::RangeUnavailable);
    };
    let Some((start, end)) = range.split_once('-') else {
        return Err(RemoteScanError::RangeUnavailable);
    };
    let Ok(start) = start.parse::<u64>() else {
        return Err(RemoteScanError::RangeUnavailable);
    };
    let Ok(end) = end.parse::<u64>() else {
        return Err(RemoteScanError::RangeUnavailable);
    };
    let Ok(total) = total.parse::<u64>() else {
        return Err(RemoteScanError::RangeUnavailable);
    };
    let Some(expected_end) = requested_offset.checked_add(requested_length - 1) else {
        return Err(RemoteScanError::RangeUnavailable);
    };
    if start != requested_offset
        || end != expected_end
        || total <= end
        || response.body.len() as u64 != requested_length
    {
        return Err(RemoteScanError::RangeUnavailable);
    }
    Ok(response.body.clone())
}

#[test]
fn http_range_fixture_accepts_only_206_with_matching_content_range() {
    let valid = RangeFixtureResponse {
        status: 206,
        content_range: Some("bytes 0-0/1024"),
        body: vec![0x89],
    };
    assert_eq!(parse_range_fixture(&valid, 0, 1).unwrap(), vec![0x89]);

    let invalid = [
        (
            "200-full-response",
            RangeFixtureResponse {
                status: 200,
                content_range: None,
                body: vec![0x89],
            },
        ),
        (
            "416",
            RangeFixtureResponse {
                status: 416,
                content_range: Some("bytes */1024"),
                body: Vec::new(),
            },
        ),
        (
            "missing-content-range",
            RangeFixtureResponse {
                status: 206,
                content_range: None,
                body: vec![0x89],
            },
        ),
        (
            "malformed-content-range",
            RangeFixtureResponse {
                status: 206,
                content_range: Some("not-a-range"),
                body: vec![0x89],
            },
        ),
        (
            "mismatched-start",
            RangeFixtureResponse {
                status: 206,
                content_range: Some("bytes 1-1/1024"),
                body: vec![0x89],
            },
        ),
        (
            "mismatched-end-and-body",
            RangeFixtureResponse {
                status: 206,
                content_range: Some("bytes 0-1/1024"),
                body: vec![0x89],
            },
        ),
        (
            "invalid-total",
            RangeFixtureResponse {
                status: 206,
                content_range: Some("bytes 0-0/0"),
                body: vec![0x89],
            },
        ),
    ];
    for (label, response) in invalid {
        assert_eq!(
            parse_range_fixture(&response, 0, 1),
            Err(RemoteScanError::RangeUnavailable),
            "{label}"
        );
    }
}

fn provider_deletion_db(provider: ProviderKind) -> (Connection, String) {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE book_sources(id TEXT PRIMARY KEY,type TEXT NOT NULL,fingerprint TEXT NOT NULL,path TEXT,root_id TEXT);
         CREATE TABLE library_index(
           id TEXT PRIMARY KEY,source_id TEXT NOT NULL,parent_id TEXT,name TEXT,path TEXT,entry_type TEXT,
           size INTEGER,modified_at INTEGER,asset_kind TEXT,content_fingerprint TEXT,
           scan_generation INTEGER,listing_complete INTEGER NOT NULL DEFAULT 0,deleted INTEGER NOT NULL DEFAULT 0,updated_at INTEGER);",
    )
    .unwrap();
    persistence::migrate(&conn).unwrap();
    let source_id = provider.source_id();
    conn.execute(
        "INSERT INTO book_sources(id,type,fingerprint,path,root_id) VALUES(?1,?2,?3,'/',NULL)",
        rusqlite::params![
            source_id,
            provider.source_type(),
            format!("fp-{}", provider.source_type())
        ],
    )
    .unwrap();
    (conn, source_id)
}

fn insert_provider_child(conn: &Connection, source_id: &str, source_fp: &str, path: &str) {
    let parent = path
        .rsplit_once('/')
        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
        .unwrap();
    conn.execute(
        "INSERT INTO library_index(id,source_id,parent_id,name,path,entry_type,scan_generation,listing_complete,deleted,updated_at)
         VALUES(?1,?2,?3,?4,?5,'file',1,1,0,1)",
        rusqlite::params![
            rust_lib_app::db::library_index_id(source_fp, path),
            source_id,
            rust_lib_app::db::library_index_id(source_fp, parent),
            path.rsplit('/').next().unwrap(),
            path,
        ],
    )
    .unwrap();
}

#[test]
fn provider_matrix_only_complete_listing_creates_verified_tombstones() {
    for provider in ProviderKind::ALL {
        let label = provider.label();
        let (conn, source_id) = provider_deletion_db(provider);
        let source_fp = format!("fp-{}", provider.source_type());
        insert_provider_child(&conn, &source_id, &source_fp, "/gone.cbz");
        insert_provider_child(&conn, &source_id, &source_fp, "/kept.cbz");
        let gone_key =
            rust_lib_app::db::book_key_of(provider.source_type(), &source_id, "/gone.cbz");
        conn.execute(
            "INSERT INTO remote_cover_dependency(book_key,dependency_path,dependency_fingerprint,profile,status)
             VALUES(?1,'/gone.cbz','cover-fp','default','ready')",
            [&gone_key],
        )
        .unwrap();

        let epoch = persistence::bind_scan_epoch(&conn, &source_id, 2, "/", 2).unwrap();
        conn.execute(
            "INSERT INTO remote_scan_state(source_id,status,mode,generation) VALUES(?1,'Running','Snapshot',2)",
            [&source_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO remote_listing_state(source_id,logical_path,content_fingerprint,scan_generation,listing_complete) VALUES(?1,'/','root-v2',2,1)",
            [&source_id],
        )
        .unwrap();

        persistence::replace_verified_children(
            &conn,
            &source_id,
            "/",
            &["/kept.cbz".into()],
            2,
            true,
        )
        .unwrap();
        conn.execute(
            "UPDATE remote_scan_state SET status='Succeeded' WHERE source_id=?1",
            [&source_id],
        )
        .unwrap();
        assert!(
            persistence::verify_remote_tombstone(&conn, &source_id, "/gone.cbz").unwrap(),
            "{label} verified tombstone ({epoch})"
        );
        let tombstones = persistence::load_verified_remote_tombstones(&conn, &source_id).unwrap();
        assert_eq!(tombstones.len(), 1, "{label} tombstone count");
        assert_eq!(
            tombstones[0].logical_path, "/gone.cbz",
            "{label} tombstone path"
        );
        assert_eq!(
            tombstones[0].dependency_paths,
            vec!["/gone.cbz"],
            "{label} dependency"
        );

        let (partial_conn, partial_source_id) = provider_deletion_db(provider);
        insert_provider_child(&partial_conn, &partial_source_id, &source_fp, "/gone.cbz");
        persistence::bind_scan_epoch(&partial_conn, &partial_source_id, 2, "/", 2).unwrap();
        partial_conn
            .execute(
                "INSERT INTO remote_scan_state(source_id,status,mode,generation) VALUES(?1,'Running','Snapshot',2)",
                [&partial_source_id],
            )
            .unwrap();
        partial_conn
            .execute(
                "INSERT INTO remote_listing_state(source_id,logical_path,content_fingerprint,scan_generation,listing_complete) VALUES(?1,'/','partial',2,0)",
                [&partial_source_id],
            )
            .unwrap();
        persistence::replace_verified_children(
            &partial_conn,
            &partial_source_id,
            "/",
            &[],
            2,
            false,
        )
        .unwrap();
        let deleted: i64 = partial_conn
            .query_row(
                "SELECT COUNT(*) FROM library_index WHERE source_id=?1 AND deleted=1",
                [&partial_source_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(deleted, 0, "{label} partial listing must retain old row");
    }
}
