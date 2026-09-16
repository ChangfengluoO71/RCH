use super::adapter::{RemoteProviderAdapter, RemoteScanError};
use super::model::{
    classify, classify_directory, fingerprint, is_ignored_name, normalize_path, RemoteAssetKind,
    RemoteEntry,
};
use crate::reader::{blocking_request_governor, RequestPriority};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_LIST_PAGES: usize = 256;
const MAX_DIRECTORY_ENTRIES: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[flutter_rust_bridge::frb(ignore)]
pub struct ScanDirectoryTask {
    pub source_id: String,
    pub logical_path: String,
    pub generation: i64,
    pub session_epoch: String,
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
            session_epoch: String::new(),
            incremental: false,
        }
    }

    pub fn with_session_epoch(mut self, session_epoch: impl Into<String>) -> Self {
        self.session_epoch = session_epoch.into();
        self
    }

    pub fn incremental(mut self) -> Self {
        self.incremental = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[flutter_rust_bridge::frb(ignore)]
pub struct CoverTask {
    pub source_id: String,
    pub logical_path: String,
    pub fingerprint: String,
    pub profile: String,
    pub generation: i64,
    pub session_epoch: String,
}

#[derive(Debug, Clone)]
#[flutter_rust_bridge::frb(ignore)]
pub struct CommittedDirectory {
    pub source_id: String,
    pub logical_path: String,
    pub generation: i64,
    pub session_epoch: String,
    pub fingerprint: String,
    pub asset_kind: RemoteAssetKind,
    pub entries: Vec<RemoteEntry>,
    pub incremental: bool,
}

#[flutter_rust_bridge::frb(ignore)]
pub trait ScanCommitSink: Send + Sync {
    fn previous_fingerprint(
        &self,
        _source_id: &str,
        _logical_path: &str,
    ) -> Result<Option<String>, RemoteScanError> {
        Ok(None)
    }
    fn stage_directory(&self, directory: CommittedDirectory) -> Result<(), RemoteScanError>;
    fn enqueue_cover(&self, task: CoverTask) -> Result<(), RemoteScanError>;
    fn spill_directory(&self, _task: ScanDirectoryTask) -> Result<(), RemoteScanError> {
        Err(RemoteScanError::Io("pending_store_unavailable".into()))
    }
    fn take_spilled_directory(
        &self,
        _source_id: &str,
        _generation: i64,
    ) -> Result<Option<ScanDirectoryTask>, RemoteScanError> {
        Ok(None)
    }
}

#[derive(Clone, Default)]
#[flutter_rust_bridge::frb(ignore)]
pub struct CancellationToken(Arc<(Mutex<bool>, Condvar)>);

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        *self.0 .0.lock().unwrap() = true;
        self.0 .1.notify_all();
    }
    pub fn is_cancelled(&self) -> bool {
        *self.0 .0.lock().unwrap()
    }
    fn wait_or_cancel(&self, delay: Duration) -> bool {
        let cancelled = self.0 .0.lock().unwrap();
        if *cancelled {
            return true;
        }
        *self.0 .1.wait_timeout(cancelled, delay).unwrap().0
    }
}

#[derive(Debug, Clone, Copy)]
#[flutter_rust_bridge::frb(ignore)]
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

#[flutter_rust_bridge::frb(ignore)]
pub struct RemoteScanEngine {
    adapter: Arc<dyn RemoteProviderAdapter>,
    sink: Arc<dyn ScanCommitSink>,
    directories: Mutex<TaskQueue>,
    cover_keys: Mutex<HashSet<CoverTask>>,
    retry: RetryPolicy,
    token: CancellationToken,
    active_scan: Mutex<Option<(String, i64)>>,
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
        _cover_capacity: usize,
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
            cover_keys: Mutex::new(HashSet::new()),
            retry,
            token,
            active_scan: Mutex::new(None),
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
            drop(queue);
            self.sink.spill_directory(task)?;
            return Ok(true);
        }
        *self.active_scan.lock().unwrap() = Some((task.source_id.clone(), task.generation));
        queue.keys.insert(task.clone());
        queue.pending.push_front(task);
        Ok(true)
    }

    pub fn has_pending(&self) -> bool {
        !self.directories.lock().unwrap().pending.is_empty()
    }

    pub fn run_next(&self) -> Result<bool, RemoteScanError> {
        self.run_next_internal(None)
    }

    /// Process one queued directory using a listing already fetched by the
    /// browser. This is intentionally one-shot: callers pass the seed only
    /// for the initial root task, then continue with [`Self::run_next`].
    pub fn run_next_with_initial_entries(
        &self,
        initial_entries: Vec<RemoteEntry>,
    ) -> Result<bool, RemoteScanError> {
        self.run_next_internal(Some(initial_entries))
    }

    fn run_next_internal(
        &self,
        initial_entries: Option<Vec<RemoteEntry>>,
    ) -> Result<bool, RemoteScanError> {
        if self.token.is_cancelled() {
            return Err(RemoteScanError::Cancelled);
        }
        let mut task = {
            let mut queue = self.directories.lock().unwrap();
            queue.pending.pop_front().inspect(|task| {
                queue.keys.remove(task);
            })
        };
        if task.is_none() {
            let active = self.active_scan.lock().unwrap().clone();
            if let Some((source_id, generation)) = active {
                task = self.sink.take_spilled_directory(&source_id, generation)?;
            }
        }
        let Some(task) = task else {
            return Ok(false);
        };
        let governor = blocking_request_governor();
        let _permit = governor
            .acquire(RequestPriority::Scan)
            .map_err(|_| RemoteScanError::Provider("request_queue_full".into()))?;
        let entries = match initial_entries {
            Some(entries) => self.normalize_entries(entries)?,
            None => self.list_complete(&task.source_id, &task.logical_path)?,
        };
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
        self.sink.stage_directory(CommittedDirectory {
            source_id: task.source_id.clone(),
            logical_path: task.logical_path.clone(),
            generation: task.generation,
            session_epoch: task.session_epoch.clone(),
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
                    ScanDirectoryTask::new(&task.source_id, &entry.logical_path, task.generation)
                        .with_session_epoch(task.session_epoch.clone());
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
                    generation: task.generation,
                    session_epoch: task.session_epoch.clone(),
                })?;
            }
        }
        if directory_kind == RemoteAssetKind::ImageFolder {
            self.enqueue_cover(CoverTask {
                source_id: task.source_id,
                logical_path: task.logical_path,
                fingerprint: directory_fingerprint,
                profile: "default".into(),
                generation: task.generation,
                session_epoch: task.session_epoch,
            })?;
        }
        Ok(true)
    }

    fn enqueue_cover(&self, task: CoverTask) -> Result<(), RemoteScanError> {
        let mut keys = self.cover_keys.lock().unwrap();
        if !keys.insert(task.clone()) {
            return Ok(());
        }
        drop(keys);
        let result = self.sink.enqueue_cover(task.clone());
        self.cover_keys.lock().unwrap().remove(&task);
        result
    }

    fn list_complete(
        &self,
        source_id: &str,
        path: &str,
    ) -> Result<Vec<RemoteEntry>, RemoteScanError> {
        let mut cursor = None;
        let mut seen_cursors = HashSet::new();
        let mut entries = Vec::new();
        for _ in 0..MAX_LIST_PAGES {
            let mut attempt = 0;
            let (page, next) = loop {
                if self.token.is_cancelled() {
                    return Err(RemoteScanError::Cancelled);
                }
                self.wait_for_source_turn(source_id)?;
                match self.adapter.list(path, cursor.as_deref()) {
                    Ok(page) => break page,
                    Err(error) => match self.retry.retry_delay(&error, attempt) {
                        Some(delay) => {
                            attempt += 1;
                            if self.token.wait_or_cancel(delay) {
                                return Err(RemoteScanError::Cancelled);
                            }
                        }
                        None => return Err(error),
                    },
                }
            };
            for entry in self.normalize_entries(page)? {
                if entries
                    .iter()
                    .any(|existing: &RemoteEntry| existing.logical_path == entry.logical_path)
                {
                    continue;
                }
                entries.push(entry);
            }
            if entries.len() > MAX_DIRECTORY_ENTRIES {
                return Err(RemoteScanError::MalformedResponse(
                    "directory_entry_limit".into(),
                ));
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

    fn normalize_entries(
        &self,
        entries: Vec<RemoteEntry>,
    ) -> Result<Vec<RemoteEntry>, RemoteScanError> {
        let mut normalized = Vec::new();
        for mut entry in entries {
            if is_ignored_name(&entry.name) {
                continue;
            }
            entry.logical_path = self.adapter.normalize_path(&entry.logical_path);
            entry.asset_kind = classify(&entry.name, entry.is_dir);
            if normalized
                .iter()
                .any(|existing: &RemoteEntry| existing.logical_path == entry.logical_path)
            {
                continue;
            }
            normalized.push(entry);
            if normalized.len() > MAX_DIRECTORY_ENTRIES {
                return Err(RemoteScanError::MalformedResponse(
                    "directory_entry_limit".into(),
                ));
            }
        }
        Ok(normalized)
    }

    fn wait_for_source_turn(&self, source_key: &str) -> Result<(), RemoteScanError> {
        static GATES: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
        let gates = GATES.get_or_init(|| Mutex::new(HashMap::new()));
        let wait = {
            let mut gates = gates.lock().unwrap();
            let now = Instant::now();
            let next = gates.entry(source_key.to_string()).or_insert(now);
            let wait = next.saturating_duration_since(now);
            *next = now.max(*next) + Duration::from_millis(25);
            wait
        };
        if !wait.is_zero() && self.token.wait_or_cancel(wait) {
            return Err(RemoteScanError::Cancelled);
        }
        Ok(())
    }
}
