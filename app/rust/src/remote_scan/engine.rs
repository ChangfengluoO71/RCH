use super::adapter::{RemoteProviderAdapter, RemoteScanError};
use super::model::{
    classify, classify_directory, fingerprint, is_ignored_name, normalize_path, RemoteAssetKind,
    RemoteEntry,
};
use crate::reader::{blocking_request_governor, RequestPriority};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MAX_LIST_PAGES: usize = 256;
const MAX_DIRECTORY_ENTRIES: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScanDirectoryTask {
    pub source_id: String,
    pub logical_path: String,
    pub generation: i64,
    pub incremental: bool,
}

impl ScanDirectoryTask {
    pub fn new(
        source_id: impl Into<String>,
        logical_path: impl Into<String>,
        generation: i64,
    ) -> Self {
        Self {
            source_id: source_id.into(),
            logical_path: normalize_path(&logical_path.into()),
            generation,
            incremental: false,
        }
    }

    pub fn incremental(mut self) -> Self {
        self.incremental = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CoverTask {
    pub source_id: String,
    pub logical_path: String,
    pub fingerprint: String,
    pub profile: String,
}

#[derive(Debug, Clone)]
pub struct CommittedDirectory {
    pub source_id: String,
    pub logical_path: String,
    pub generation: i64,
    pub fingerprint: String,
    pub asset_kind: RemoteAssetKind,
    pub entries: Vec<RemoteEntry>,
    pub incremental: bool,
}

pub trait ScanCommitSink: Send + Sync {
    fn previous_fingerprint(
        &self,
        _source_id: &str,
        _logical_path: &str,
    ) -> Result<Option<String>, RemoteScanError> {
        Ok(None)
    }
    fn commit_directory(&self, directory: CommittedDirectory) -> Result<(), RemoteScanError>;
    fn enqueue_cover(&self, task: CoverTask) -> Result<(), RemoteScanError>;
}

#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    max_retries: usize,
    base_delay: Duration,
    max_delay: Duration,
}

impl RetryPolicy {
    pub fn new(max_retries: usize, base_delay: Duration, max_delay: Duration) -> Self {
        Self {
            max_retries,
            base_delay,
            max_delay,
        }
    }

    fn retry_delay(&self, error: &RemoteScanError, attempt: usize) -> Option<Duration> {
        if attempt >= self.max_retries {
            return None;
        }
        let exponential = self
            .base_delay
            .saturating_mul(1u32.checked_shl(attempt as u32).unwrap_or(u32::MAX));
        let bounded = exponential.min(self.max_delay);
        match error {
            RemoteScanError::RateLimited { retry_after_ms } => Some(
                Duration::from_millis(retry_after_ms.unwrap_or(bounded.as_millis() as u64))
                    .min(self.max_delay),
            ),
            RemoteScanError::TransientNetwork(_) => Some(bounded),
            _ => None,
        }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new(3, Duration::from_millis(250), Duration::from_secs(8))
    }
}

struct TaskQueue {
    capacity: usize,
    pending: VecDeque<ScanDirectoryTask>,
    keys: HashSet<ScanDirectoryTask>,
}

pub struct RemoteScanEngine {
    adapter: Arc<dyn RemoteProviderAdapter>,
    sink: Arc<dyn ScanCommitSink>,
    directories: Mutex<TaskQueue>,
    cover_capacity: usize,
    cover_keys: Mutex<HashSet<CoverTask>>,
    retry: RetryPolicy,
    token: CancellationToken,
}

impl RemoteScanEngine {
    pub fn new<A, S>(
        adapter: Arc<A>,
        sink: Arc<S>,
        directory_capacity: usize,
        cover_capacity: usize,
        retry: RetryPolicy,
    ) -> Self
    where
        A: RemoteProviderAdapter + 'static,
        S: ScanCommitSink + 'static,
    {
        Self::with_token(
            adapter,
            sink,
            directory_capacity,
            cover_capacity,
            retry,
            CancellationToken::new(),
        )
    }

    pub fn with_token<A, S>(
        adapter: Arc<A>,
        sink: Arc<S>,
        directory_capacity: usize,
        cover_capacity: usize,
        retry: RetryPolicy,
        token: CancellationToken,
    ) -> Self
    where
        A: RemoteProviderAdapter + 'static,
        S: ScanCommitSink + 'static,
    {
        Self::from_dyn(
            adapter,
            sink,
            directory_capacity,
            cover_capacity,
            retry,
            token,
        )
    }

    pub fn from_dyn(
        adapter: Arc<dyn RemoteProviderAdapter>,
        sink: Arc<dyn ScanCommitSink>,
        directory_capacity: usize,
        cover_capacity: usize,
        retry: RetryPolicy,
        token: CancellationToken,
    ) -> Self {
        Self {
            adapter,
            sink,
            directories: Mutex::new(TaskQueue {
                capacity: directory_capacity.max(1),
                pending: VecDeque::new(),
                keys: HashSet::new(),
            }),
            cover_capacity: cover_capacity.max(1),
            cover_keys: Mutex::new(HashSet::new()),
            retry,
            token,
        }
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.token.clone()
    }

    pub fn enqueue_directory(&self, task: ScanDirectoryTask) -> Result<bool, RemoteScanError> {
        let mut queue = self.directories.lock().unwrap();
        if queue.keys.contains(&task) {
            return Ok(false);
        }
        if queue.pending.len() >= queue.capacity {
            return Err(RemoteScanError::Provider("scan_queue_full".into()));
        }
        queue.keys.insert(task.clone());
        queue.pending.push_back(task);
        Ok(true)
    }

    pub fn has_pending(&self) -> bool {
        !self.directories.lock().unwrap().pending.is_empty()
    }

    pub fn run_next(&self) -> Result<bool, RemoteScanError> {
        if self.token.is_cancelled() {
            return Err(RemoteScanError::Cancelled);
        }
        let task = {
            let mut queue = self.directories.lock().unwrap();
            let Some(task) = queue.pending.pop_front() else {
                return Ok(false);
            };
            queue.keys.remove(&task);
            task
        };
        let _permit = blocking_request_governor()
            .acquire(RequestPriority::Scan)
            .map_err(|_| RemoteScanError::Provider("request_queue_full".into()))?;
        let entries = self.list_complete(&task.logical_path)?;
        if self.token.is_cancelled() {
            return Err(RemoteScanError::Cancelled);
        }
        let directory_fingerprint = fingerprint(&entries);
        let directory_kind = classify_directory(&entries);
        let changed = self
            .sink
            .previous_fingerprint(&task.source_id, &task.logical_path)?
            .as_deref()
            != Some(&directory_fingerprint);
        self.sink.commit_directory(CommittedDirectory {
            source_id: task.source_id.clone(),
            logical_path: task.logical_path.clone(),
            generation: task.generation,
            fingerprint: directory_fingerprint.clone(),
            asset_kind: directory_kind,
            entries: entries.clone(),
            incremental: task.incremental,
        })?;
        if self.token.is_cancelled() {
            return Err(RemoteScanError::Cancelled);
        }

        for entry in &entries {
            if entry.is_dir && (!task.incremental || changed) {
                let child =
                    ScanDirectoryTask::new(&task.source_id, &entry.logical_path, task.generation);
                self.enqueue_directory(if task.incremental {
                    child.incremental()
                } else {
                    child
                })?;
            } else if entry.asset_kind == RemoteAssetKind::ArchiveFile {
                self.enqueue_cover(CoverTask {
                    source_id: task.source_id.clone(),
                    logical_path: entry.logical_path.clone(),
                    fingerprint: fingerprint(std::slice::from_ref(entry)),
                    profile: "default".into(),
                })?;
            }
        }
        if directory_kind == RemoteAssetKind::ImageFolder {
            self.enqueue_cover(CoverTask {
                source_id: task.source_id,
                logical_path: task.logical_path,
                fingerprint: directory_fingerprint,
                profile: "default".into(),
            })?;
        }
        Ok(true)
    }

    fn enqueue_cover(&self, task: CoverTask) -> Result<(), RemoteScanError> {
        let mut keys = self.cover_keys.lock().unwrap();
        if !keys.insert(task.clone()) {
            return Ok(());
        }
        if keys.len() > self.cover_capacity {
            keys.remove(&task);
            return Err(RemoteScanError::Provider("cover_queue_full".into()));
        }
        drop(keys);
        self.sink.enqueue_cover(task)
    }

    fn list_complete(&self, path: &str) -> Result<Vec<RemoteEntry>, RemoteScanError> {
        let mut cursor = None;
        let mut seen_cursors = HashSet::new();
        let mut entries = Vec::new();
        for _ in 0..MAX_LIST_PAGES {
            let mut attempt = 0;
            let (page, next) = loop {
                if self.token.is_cancelled() {
                    return Err(RemoteScanError::Cancelled);
                }
                match self.adapter.list(path, cursor.as_deref()) {
                    Ok(page) => break page,
                    Err(error) => match self.retry.retry_delay(&error, attempt) {
                        Some(delay) => {
                            attempt += 1;
                            std::thread::sleep(delay);
                        }
                        None => return Err(error),
                    },
                }
            };
            for mut entry in page {
                if is_ignored_name(&entry.name) {
                    continue;
                }
                entry.logical_path = self.adapter.normalize_path(&entry.logical_path);
                entry.asset_kind = classify(&entry.name, entry.is_dir);
                if entries
                    .iter()
                    .any(|existing: &RemoteEntry| existing.logical_path == entry.logical_path)
                {
                    continue;
                }
                entries.push(entry);
                if entries.len() > MAX_DIRECTORY_ENTRIES {
                    return Err(RemoteScanError::MalformedResponse(
                        "directory_entry_limit".into(),
                    ));
                }
            }
            match next {
                None => return Ok(entries),
                Some(next) if seen_cursors.insert(next.clone()) => cursor = Some(next),
                Some(_) => {
                    return Err(RemoteScanError::MalformedResponse(
                        "pagination_cursor_cycle".into(),
                    ))
                }
            }
        }
        Err(RemoteScanError::MalformedResponse(
            "pagination_page_limit".into(),
        ))
    }
}
