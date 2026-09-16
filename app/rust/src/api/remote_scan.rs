use crate::api::source::remote_provider_adapter;
use crate::db;
use crate::reader::{blocking_request_governor, RequestPriority};
use crate::remote_scan::adapter::RemoteScanError;
use crate::remote_scan::engine::{
    CancellationToken, CommittedDirectory, CoverTask, RemoteScanEngine, RetryPolicy,
    ScanCommitSink, ScanDirectoryTask,
};
use crate::remote_scan::model::{
    normalize_path, RemoteScanMode, RemoteScanState, RemoteScanStatus,
};
use crate::remote_scan::persistence;
use rusqlite::{params, OptionalExtension};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone)]
pub struct RemoteScanJobDto {
    pub job_id: String,
    pub source_id: String,
    pub status: String,
    pub mode: String,
    pub generation: i64,
}

#[derive(Debug, Clone)]
pub struct RemoteScanStatusDto {
    pub source_id: String,
    pub status: String,
    pub mode: String,
    pub generation: i64,
    pub checkpoint: Option<String>,
    pub last_success_at: Option<i64>,
    pub error_code: Option<String>,
    pub processed: u64,
    pub total: u64,
}

#[derive(Clone)]
struct StartConfig {
    source_type: String,
    source_id: String,
    session: u64,
    root_path: String,
    mode: String,
}

struct ScanJob {
    job_id: String,
    config: StartConfig,
    status: Mutex<RemoteScanStatusDto>,
    token: CancellationToken,
}

fn jobs() -> &'static Mutex<HashMap<String, Arc<ScanJob>>> {
    static JOBS: OnceLock<Mutex<HashMap<String, Arc<ScanJob>>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn start_lock() -> &'static Mutex<()> {
    static START_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    START_LOCK.get_or_init(|| Mutex::new(()))
}

fn next_job_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("remote-scan-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

struct SqliteScanSink {}

impl ScanCommitSink for SqliteScanSink {
    #[flutter_rust_bridge::frb(ignore)]
    fn previous_fingerprint(
        &self,
        source_id: &str,
        logical_path: &str,
    ) -> Result<Option<String>, RemoteScanError> {
        db::get().lock().unwrap().query_row(
            "SELECT content_fingerprint FROM remote_listing_state WHERE source_id=?1 AND logical_path=?2 AND listing_complete=1",
            params![source_id, normalize_path(logical_path)], |row| row.get(0),
        ).optional().map_err(|_| RemoteScanError::Io("database_read_failed".into()))
    }

    #[flutter_rust_bridge::frb(ignore)]
    fn stage_directory(&self, directory: CommittedDirectory) -> Result<(), RemoteScanError> {
        let conn = db::get().lock().unwrap();
        if !persistence::current_generation_is_active_with_epoch(
            &conn,
            &directory.source_id,
            directory.generation,
            "Running",
            &directory.session_epoch,
        )
        .map_err(|_| RemoteScanError::Io("source_proof_lookup_failed".into()))?
        {
            return Err(RemoteScanError::Cancelled);
        }
        persistence::stage_complete_listing(
            &conn,
            &directory.source_id,
            &normalize_path(&directory.logical_path),
            &directory.entries,
            directory.generation,
            &directory.fingerprint,
            directory.asset_kind,
            directory.incremental,
            &directory.session_epoch,
        )
        .map_err(|_| RemoteScanError::Io("manifest_commit_failed".into()))?;
        let state = RemoteScanState {
            source_id: directory.source_id,
            status: RemoteScanStatus::Running,
            mode: if directory.incremental {
                RemoteScanMode::Incremental
            } else {
                RemoteScanMode::Snapshot
            },
            generation: directory.generation,
            checkpoint: Some(normalize_path(&directory.logical_path)),
            last_success_at: None,
            error_code: None,
        };
        persistence::mark_scan_status(&conn, &state)
            .map_err(|_| RemoteScanError::Io("checkpoint_commit_failed".into()))
    }

    #[flutter_rust_bridge::frb(ignore)]
    fn enqueue_cover(&self, task: CoverTask) -> Result<(), RemoteScanError> {
        let conn = db::get().lock().unwrap();
        let source_type: String = conn
            .query_row(
                "SELECT type FROM book_sources WHERE id=?1",
                [&task.source_id],
                |row| row.get(0),
            )
            .map_err(|_| RemoteScanError::Io("source_lookup_failed".into()))?;
        let book_key = db::book_key_of(&source_type, &task.source_id, &task.logical_path);
        if !persistence::current_generation_is_active_with_epoch(
            &conn,
            &task.source_id,
            task.generation,
            "Running",
            &task.session_epoch,
        )
        .map_err(|_| RemoteScanError::Io("source_proof_lookup_failed".into()))?
        {
            return Err(RemoteScanError::Cancelled);
        }
        persistence::stage_cover_task(&conn, task.generation, &book_key, &task)
            .map_err(|_| RemoteScanError::Io("cover_dependency_stage_failed".into()))
    }

    #[flutter_rust_bridge::frb(ignore)]
    fn spill_directory(&self, task: ScanDirectoryTask) -> Result<(), RemoteScanError> {
        persistence::store_pending_task(&db::get().lock().unwrap(), &task)
            .map_err(|_| RemoteScanError::Io("pending_store_failed".into()))
    }

    #[flutter_rust_bridge::frb(ignore)]
    fn take_spilled_directory(
        &self,
        source_id: &str,
        generation: i64,
    ) -> Result<Option<ScanDirectoryTask>, RemoteScanError> {
        persistence::take_pending_task(&db::get().lock().unwrap(), source_id, generation)
            .map_err(|_| RemoteScanError::Io("pending_load_failed".into()))
    }
}

fn persist_terminal(status: &RemoteScanStatusDto) {
    let state = RemoteScanState {
        source_id: status.source_id.clone(),
        status: match status.status.as_str() {
            "complete" => RemoteScanStatus::Succeeded,
            "running" => RemoteScanStatus::Running,
            _ => RemoteScanStatus::Failed,
        },
        mode: if status.mode == "incremental" {
            RemoteScanMode::Incremental
        } else {
            RemoteScanMode::Snapshot
        },
        generation: status.generation,
        checkpoint: status.checkpoint.clone(),
        last_success_at: status.last_success_at,
        error_code: status.error_code.clone(),
    };
    if let Ok(conn) = db::get().lock() {
        let _ = persistence::mark_scan_status(&conn, &state);
    }
}

fn persist_config_status(status: &RemoteScanStatusDto) {
    if let Ok(conn) = db::get().lock() {
        let _ = conn.execute(
            "UPDATE remote_scan_config SET status=?1,updated_at=?2 WHERE source_id=?3 AND generation=?4",
            params![status.status, db::now_ms(), status.source_id, status.generation],
        );
    }
}

fn error_code(error: &RemoteScanError) -> &'static str {
    match error {
        RemoteScanError::Unauthorized => "authExpired",
        RemoteScanError::Forbidden => "forbidden",
        RemoteScanError::NotFound => "notFound",
        RemoteScanError::RateLimited { .. } => "rateLimited",
        RemoteScanError::TransientNetwork(_) => "transient",
        RemoteScanError::RangeUnavailable => "rangeUnavailable",
        RemoteScanError::MalformedResponse(_) => "malformed",
        RemoteScanError::Cancelled => "cancelled",
        RemoteScanError::Unsupported => "unsupported",
        RemoteScanError::Io(_) => "storage",
        RemoteScanError::Provider(_) => "provider",
    }
}

fn job_dto(job: &ScanJob) -> RemoteScanJobDto {
    let status = job.status.lock().unwrap();
    RemoteScanJobDto {
        job_id: job.job_id.clone(),
        source_id: status.source_id.clone(),
        status: status.status.clone(),
        mode: status.mode.clone(),
        generation: status.generation,
    }
}

fn start_job(config: StartConfig, resume: bool) -> std::result::Result<RemoteScanJobDto, String> {
    if !matches!(config.mode.as_str(), "full" | "incremental") {
        return Err("invalid scan mode".into());
    }
    if matches!(config.source_type.as_str(), "local" | "smb") {
        return Err("source is local-only".into());
    }
    let _reservation = start_lock().lock().unwrap();
    {
        let conn = db::get().lock().unwrap();
        if !persistence::requested_root_matches_source(&conn, &config.source_id, &config.root_path)
            .map_err(|_| "source proof unavailable".to_string())?
        {
            return Err("source root changed".into());
        }
    }
    if let Some(existing) = jobs().lock().unwrap().get(&config.source_id).cloned() {
        let active = matches!(
            existing.status.lock().unwrap().status.as_str(),
            "running" | "paused"
        );
        if active {
            if !resume {
                return Ok(job_dto(&existing));
            }
            existing.token.cancel();
        }
    }
    let adapter = remote_provider_adapter(&config.source_type, config.session, &config.root_path)
        .map_err(|_| "remote session unavailable".to_string())?;
    let conn = db::get().lock().unwrap();
    let stored: Option<(i64, Option<String>)> = conn
        .query_row(
            "SELECT generation,checkpoint FROM remote_scan_state WHERE source_id=?1",
            [&config.source_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| "scan state unavailable".to_string())?;
    let never_started = stored.is_none();
    let generation = stored
        .as_ref()
        .map_or(1, |(generation, _)| generation.saturating_add(1));
    let mode = if never_started || resume {
        "full".to_string()
    } else {
        config.mode.clone()
    };
    let checkpoint = stored.and_then(|(_, checkpoint)| checkpoint);
    let session_epoch = persistence::bind_scan_epoch(
        &conn,
        &config.source_id,
        generation,
        &config.root_path,
        config.session,
    )
    .map_err(|_| "scan session proof unavailable".to_string())?;
    drop(conn);
    let token = CancellationToken::new();
    let status = RemoteScanStatusDto {
        source_id: config.source_id.clone(),
        status: "running".into(),
        mode: mode.clone(),
        generation,
        checkpoint: checkpoint.clone(),
        last_success_at: None,
        error_code: None,
        processed: 0,
        total: 1,
    };
    persist_terminal(&status);
    if let Ok(conn) = db::get().lock() {
        let _ = conn.execute(
            "INSERT INTO remote_scan_config(source_id,source_type,root_path,mode,generation,status,updated_at) VALUES(?1,?2,?3,?4,?5,'running',?6)
             ON CONFLICT(source_id) DO UPDATE SET source_type=excluded.source_type,root_path=excluded.root_path,mode=excluded.mode,generation=excluded.generation,status='running',updated_at=excluded.updated_at",
            params![config.source_id, config.source_type, normalize_path(&config.root_path), mode, generation, db::now_ms()],
        );
    }
    let job = Arc::new(ScanJob {
        job_id: next_job_id(),
        config: StartConfig {
            mode: mode.clone(),
            ..config
        },
        status: Mutex::new(status),
        token: token.clone(),
    });
    jobs()
        .lock()
        .unwrap()
        .insert(job.config.source_id.clone(), Arc::clone(&job));
    let response = job_dto(&job);
    let root = "/".to_string();
    std::thread::spawn(move || {
        let sink: Arc<dyn ScanCommitSink> = Arc::new(SqliteScanSink {});
        let cover_adapter = Arc::clone(&adapter);
        let engine = RemoteScanEngine::from_dyn(
            adapter,
            Arc::clone(&sink),
            1024,
            4096,
            RetryPolicy::default(),
            token.clone(),
        );
        let initial = ScanDirectoryTask::new(&job.config.source_id, root, generation)
            .with_session_epoch(session_epoch);
        let initial = if mode == "incremental" {
            initial.incremental()
        } else {
            initial
        };
        let mut outcome = engine.enqueue_directory(initial).map(|_| ());
        while outcome.is_ok() {
            let step = engine.run_next();
            if matches!(step, Ok(false)) {
                break;
            }
            outcome = step.map(|worked| {
                if worked {
                    let mut status = job.status.lock().unwrap();
                    status.processed += 1;
                    status.total = status
                        .total
                        .max(status.processed + u64::from(engine.has_pending()));
                    status.checkpoint =
                        persistence::load_checkpoint(&db::get().lock().unwrap(), &status.source_id)
                            .ok()
                            .flatten();
                }
            });
        }
        let mut status = job.status.lock().unwrap();
        if matches!(status.status.as_str(), "paused" | "cancelled") {
            let _ = persistence::discard_staged_generation(
                &db::get().lock().unwrap(),
                &status.source_id,
                generation,
            );
            persist_config_status(&status);
            return;
        }
        match outcome {
            Ok(()) => {
                if token.is_cancelled() {
                    let _ = persistence::discard_staged_generation(
                        &db::get().lock().unwrap(),
                        &status.source_id,
                        generation,
                    );
                    status.status = "cancelled".into();
                    status.error_code = Some("cancelled".into());
                } else if persistence::publish_staged_generation(
                    &db::get().lock().unwrap(),
                    &status.source_id,
                    generation,
                )
                .is_ok()
                {
                    status.status = "complete".into();
                    status.last_success_at = Some(db::now_ms());
                    status.error_code = None;
                    consume_staged_covers(&status.source_id, generation, cover_adapter);
                } else {
                    let _ = persistence::discard_staged_generation(
                        &db::get().lock().unwrap(),
                        &status.source_id,
                        generation,
                    );
                    status.status = "degraded".into();
                    status.error_code = Some("storage".into());
                }
            }
            Err(error) => {
                let _ = persistence::discard_staged_generation(
                    &db::get().lock().unwrap(),
                    &status.source_id,
                    generation,
                );
                status.status = "degraded".into();
                status.error_code = Some(error_code(&error).into());
            }
        }
        persist_terminal(&status);
        persist_config_status(&status);
    });
    Ok(response)
}

fn consume_staged_covers(
    source_id: &str,
    generation: i64,
    adapter: Arc<dyn crate::remote_scan::adapter::RemoteProviderAdapter>,
) {
    loop {
        let task =
            persistence::next_staged_cover_task(&db::get().lock().unwrap(), source_id, generation)
                .ok()
                .flatten();
        let Some((book_key, task)) = task else {
            break;
        };
        let result = fetch_safe_cover_partial(adapter.as_ref(), &task);
        let (status, bytes) = match result {
            Ok(bytes) if !bytes.is_empty() => ("partial_ready", Some(bytes)),
            _ => ("placeholder", None),
        };
        if persistence::finish_cover_task(
            &db::get().lock().unwrap(),
            source_id,
            generation,
            &book_key,
            &task.logical_path,
            &task.session_epoch,
            status,
            bytes.as_deref(),
        )
        .is_err()
        {
            break;
        }
    }
}

fn fetch_safe_cover_partial(
    adapter: &dyn crate::remote_scan::adapter::RemoteProviderAdapter,
    task: &CoverTask,
) -> Result<Vec<u8>, RemoteScanError> {
    let capabilities = adapter.capabilities(&task.logical_path, &task.fingerprint)?;
    if !capabilities.range_read {
        return Err(RemoteScanError::RangeUnavailable);
    }
    let governor = blocking_request_governor();
    let _permit = governor
        .acquire(RequestPriority::Cover)
        .map_err(|_| RemoteScanError::Provider("request_queue_full".into()))?;
    adapter.read_range(&task.logical_path, 0, 256 * 1024)
}

pub async fn remote_scan_start(
    source_type: String,
    source_id: String,
    session: u64,
    root_path: String,
    mode: String,
) -> std::result::Result<RemoteScanJobDto, String> {
    start_job(
        StartConfig {
            source_type,
            source_id,
            session,
            root_path,
            mode: mode.to_ascii_lowercase(),
        },
        false,
    )
}

pub fn remote_scan_status(source_id: String) -> Option<RemoteScanStatusDto> {
    if let Some(job) = jobs().lock().unwrap().get(&source_id) {
        return Some(job.status.lock().unwrap().clone());
    }
    let conn = db::get().lock().ok()?;
    let mut status = conn.query_row(
        "SELECT status,mode,generation,checkpoint,last_success_at,error_code FROM remote_scan_state WHERE source_id=?1", [&source_id],
        |row| {
            let persisted_status: String = row.get(0)?;
            let persisted_mode: String = row.get(1)?;
            Ok(RemoteScanStatusDto {
                source_id: source_id.clone(),
                status: match persisted_status.as_str() { "Succeeded" => "complete", "Running" => "running", _ => "degraded" }.into(),
                mode: if persisted_mode == "Incremental" { "incremental" } else { "full" }.into(),
                generation: row.get(2)?, checkpoint: row.get(3)?, last_success_at: row.get(4)?, error_code: row.get(5)?, processed: 0, total: 0,
            })
        },
    ).optional().ok().flatten()?;
    let durable_status: Option<String> = conn
        .query_row(
            "SELECT status FROM remote_scan_config WHERE source_id=?1 AND generation=?2",
            params![source_id, status.generation],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten();
    if let Some(durable_status) = durable_status {
        status.status = durable_status;
    }
    Some(status)
}

pub fn remote_scan_pause(source_id: String) -> std::result::Result<(), String> {
    let job = jobs()
        .lock()
        .unwrap()
        .get(&source_id)
        .cloned()
        .ok_or_else(|| "scan job not found".to_string())?;
    job.token.cancel();
    let mut status = job.status.lock().unwrap();
    status.status = "paused".into();
    status.error_code = Some("paused".into());
    persist_terminal(&status);
    persist_config_status(&status);
    Ok(())
}

pub fn remote_scan_resume(source_id: String) -> std::result::Result<(), String> {
    let job = jobs()
        .lock()
        .unwrap()
        .get(&source_id)
        .cloned()
        .ok_or_else(|| "scan job not found".to_string())?;
    let config = job.config.clone();
    start_job(config, true).map(|_| ())
}

pub fn remote_scan_cancel(source_id: String) -> std::result::Result<(), String> {
    let job = jobs()
        .lock()
        .unwrap()
        .get(&source_id)
        .cloned()
        .ok_or_else(|| "scan job not found".to_string())?;
    job.token.cancel();
    let mut status = job.status.lock().unwrap();
    status.status = "cancelled".into();
    status.error_code = Some("cancelled".into());
    persist_terminal(&status);
    persist_config_status(&status);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_scan::adapter::{RemoteCapabilities, RemoteProviderAdapter};
    use crate::remote_scan::model::normalize_path;
    use std::sync::mpsc;
    use std::time::Duration;

    struct CoverAdapter(mpsc::Sender<()>);

    impl RemoteProviderAdapter for CoverAdapter {
        fn list(
            &self,
            _path: &str,
            _cursor: Option<&str>,
        ) -> Result<(Vec<crate::remote_scan::model::RemoteEntry>, Option<String>), RemoteScanError>
        {
            Err(RemoteScanError::Unsupported)
        }
        fn read_range(
            &self,
            _path: &str,
            _offset: u64,
            _length: u64,
        ) -> Result<Vec<u8>, RemoteScanError> {
            let _ = self.0.send(());
            Ok(vec![1, 2, 3])
        }
        fn read_file_limited(
            &self,
            _path: &str,
            _max_bytes: u64,
        ) -> Result<Vec<u8>, RemoteScanError> {
            panic!("safe cover worker must never fall back to a whole-book read")
        }
        fn normalize_path(&self, path: &str) -> String {
            normalize_path(path)
        }
        fn capabilities(
            &self,
            _path: &str,
            _fingerprint: &str,
        ) -> Result<RemoteCapabilities, RemoteScanError> {
            Ok(RemoteCapabilities {
                range_read: true,
                pagination: false,
            })
        }
    }

    #[test]
    fn real_safe_cover_worker_uses_shared_governor_and_only_range_reads() {
        let governor = blocking_request_governor();
        let held = [
            governor.acquire(RequestPriority::Foreground).unwrap(),
            governor.acquire(RequestPriority::Foreground).unwrap(),
            governor.acquire(RequestPriority::Foreground).unwrap(),
        ];
        let (started_tx, started_rx) = mpsc::channel();
        let task = CoverTask {
            source_id: "cover-source".into(),
            logical_path: "/book.cbz".into(),
            fingerprint: "fingerprint".into(),
            profile: "default".into(),
            generation: 0,
            session_epoch: String::new(),
        };
        let worker =
            std::thread::spawn(move || fetch_safe_cover_partial(&CoverAdapter(started_tx), &task));
        assert!(started_rx.recv_timeout(Duration::from_millis(100)).is_err());
        drop(held);
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(worker.join().unwrap().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn range_unavailable_remains_placeholder_without_whole_book_fallback() {
        struct NoRange;
        impl RemoteProviderAdapter for NoRange {
            fn list(
                &self,
                _: &str,
                _: Option<&str>,
            ) -> Result<
                (Vec<crate::remote_scan::model::RemoteEntry>, Option<String>),
                RemoteScanError,
            > {
                Err(RemoteScanError::Unsupported)
            }
            fn read_range(&self, _: &str, _: u64, _: u64) -> Result<Vec<u8>, RemoteScanError> {
                panic!("range read must not start")
            }
            fn read_file_limited(&self, _: &str, _: u64) -> Result<Vec<u8>, RemoteScanError> {
                panic!("whole-book fallback is forbidden")
            }
            fn normalize_path(&self, path: &str) -> String {
                normalize_path(path)
            }
            fn capabilities(
                &self,
                _: &str,
                _: &str,
            ) -> Result<RemoteCapabilities, RemoteScanError> {
                Ok(RemoteCapabilities::default())
            }
        }
        let task = CoverTask {
            source_id: "source".into(),
            logical_path: "/book.cbz".into(),
            fingerprint: "fp".into(),
            profile: "default".into(),
            generation: 0,
            session_epoch: String::new(),
        };
        assert_eq!(
            fetch_safe_cover_partial(&NoRange, &task).unwrap_err(),
            RemoteScanError::RangeUnavailable
        );
    }
}
