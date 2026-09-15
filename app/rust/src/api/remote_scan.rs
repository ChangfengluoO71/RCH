use crate::api::source::remote_provider_adapter;
use crate::db;
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
    fn commit_directory(&self, directory: CommittedDirectory) -> Result<(), RemoteScanError> {
        let conn = db::get().lock().unwrap();
        let latest: Option<i64> = conn
            .query_row(
                "SELECT generation FROM remote_scan_state WHERE source_id=?1",
                [&directory.source_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| RemoteScanError::Io("database_read_failed".into()))?;
        if latest.is_some_and(|generation| generation > directory.generation) {
            return Err(RemoteScanError::Cancelled);
        }
        persistence::upsert_complete_listing(
            &conn,
            &directory.source_id,
            &normalize_path(&directory.logical_path),
            &directory.entries,
            directory.generation,
            &directory.fingerprint,
            true,
        )
        .map_err(|_| RemoteScanError::Io("manifest_commit_failed".into()))?;
        conn.execute(
            "UPDATE library_index SET asset_kind=?1, content_fingerprint=?2, scan_generation=?3, listing_complete=1, updated_at=?4 WHERE source_id=?5 AND path=?6",
            params![format!("{:?}", directory.asset_kind), directory.fingerprint, directory.generation, db::now_ms(), directory.source_id, normalize_path(&directory.logical_path)],
        ).map_err(|_| RemoteScanError::Io("directory_classification_failed".into()))?;
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
        conn.execute(
            "INSERT INTO remote_cover_dependency(book_key,dependency_path,dependency_fingerprint,profile,status) VALUES(?1,?2,?3,?4,'queued') ON CONFLICT(book_key,dependency_path) DO UPDATE SET dependency_fingerprint=excluded.dependency_fingerprint,profile=excluded.profile,status='queued'",
            params![book_key, normalize_path(&task.logical_path), task.fingerprint, task.profile],
        ).map_err(|_| RemoteScanError::Io("cover_dependency_commit_failed".into()))?;
        Ok(())
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
    if let Some(existing) = jobs().lock().unwrap().get(&config.source_id).cloned() {
        let active = matches!(
            existing.status.lock().unwrap().status.as_str(),
            "running" | "paused"
        );
        if active && !resume {
            return Ok(job_dto(&existing));
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
        let engine =
            RemoteScanEngine::from_dyn(adapter, sink, 1024, 4096, RetryPolicy::default(), token);
        let initial = ScanDirectoryTask::new(&job.config.source_id, root, generation);
        let initial = if mode == "incremental" {
            initial.incremental()
        } else {
            initial
        };
        let mut outcome = engine.enqueue_directory(initial).map(|_| ());
        while outcome.is_ok() && engine.has_pending() {
            outcome = engine.run_next().map(|worked| {
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
            return;
        }
        match outcome {
            Ok(()) => {
                status.status = "complete".into();
                status.last_success_at = Some(db::now_ms());
                status.error_code = None;
            }
            Err(error) => {
                status.status = "degraded".into();
                status.error_code = Some(error_code(&error).into());
            }
        }
        persist_terminal(&status);
    });
    Ok(response)
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
    conn.query_row(
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
    ).optional().ok().flatten()
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
    Ok(())
}
