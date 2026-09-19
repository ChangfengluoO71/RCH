use crate::api::source::remote_provider_adapter;
use crate::db;
use crate::reader::{blocking_request_governor, RequestPriority};
use crate::remote_scan::adapter::RemoteScanError;
use crate::remote_scan::cover_state::CoverJobUpsertCause;
use crate::remote_scan::engine::{
    CancellationToken, CommittedDirectory, CoverTask, RemoteScanEngine, RetryPolicy,
    ScanCommitSink, ScanDirectoryTask,
};
use crate::remote_scan::model::{
    classify, normalize_path, RemoteAssetKind, RemoteEntry, RemoteScanMode, RemoteScanState,
    RemoteScanStatus,
};
use crate::remote_scan::persistence;
use crate::source::ByteSource;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use sha2::Digest;
use std::collections::{HashMap, HashSet};
use std::io;
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
    /// Directory discovery phase is intentionally separate from cover
    /// readiness.  `processed/total` remain for old clients only.
    pub listing_phase: String,
    pub directories_checked: u64,
    pub discovered_books: u64,
    pub discovery_complete: bool,
    pub ready_books: u64,
    pub active_books: u64,
    pub pending_books: u64,
    pub retry_books: u64,
    pub blocked_books: u64,
    pub unsupported_books: u64,
    pub failed_books: u64,
    /// P1-F：`ready` **且字节真的可用**的漫画数（缓存/文件系统校验，锁外计算）。
    pub available_books: u64,
    /// P1-F：等待中的漫画数 = pending + retry + stale-ready + no-job。
    pub waiting_books: u64,
    /// P1-F：真实未知 durable state 的漫画数（默认 0；不变量违例也会在此显式暴露）。
    pub other_books: u64,
    pub view_revision: i64,
}

#[derive(Clone)]
struct StartConfig {
    source_type: String,
    source_id: String,
    session: u64,
    root_path: String,
    mode: String,
    initial_listing_json: Option<String>,
    force_recheck: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitialListingEntry {
    name: String,
    path: String,
    logical_path: Option<String>,
    is_dir: bool,
    size: Option<u64>,
    mtime: Option<i64>,
}

fn initial_scan_path(configured_root: &str) -> String {
    normalize_path(configured_root)
}

fn parse_initial_listing(
    listing_json: Option<&str>,
) -> std::result::Result<Option<Vec<(RemoteEntry, String)>>, String> {
    let Some(listing_json) = listing_json.filter(|json| !json.trim().is_empty()) else {
        return Ok(None);
    };
    let entries: Vec<InitialListingEntry> = serde_json::from_str(listing_json)
        .map_err(|_| "invalid initial remote listing".to_string())?;
    Ok(Some(
        entries
            .into_iter()
            .map(|entry| {
                let logical_path = entry.logical_path.unwrap_or_else(|| entry.path.clone());
                let remote = RemoteEntry {
                    name: entry.name.clone(),
                    logical_path,
                    provider_path: Some(entry.path.clone()),
                    is_dir: entry.is_dir,
                    size: entry.size,
                    mtime: entry.mtime,
                    asset_kind: classify(&entry.name, entry.is_dir),
                };
                (remote, entry.path)
            })
            .collect(),
    ))
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

fn cover_workers() -> &'static Mutex<HashSet<String>> {
    static WORKERS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    WORKERS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Wake the durable cover queue for a source.  A source has at most one
/// worker process in this address space; individual jobs are still leased in
/// SQLite so a scanner and a visible request cannot publish the same key
/// concurrently.  The worker exits when no due job remains.
pub(crate) fn wake_remote_cover_worker(source_id: String, session: u64) {
    if session == 0 || source_id.trim().is_empty() {
        return;
    }
    // Include the runtime session in the single-flight key.  A previous
    // session can be winding down after a token refresh; it must not prevent
    // the newly authenticated session from taking over the durable queue.
    let worker_key = format!("{source_id}:{session}");
    let should_spawn = cover_workers().lock().unwrap().insert(worker_key.clone());
    if !should_spawn {
        return;
    }
    std::thread::spawn(move || {
        run_remote_cover_worker(&source_id, session);
        cover_workers().lock().unwrap().remove(&worker_key);
    });
}

/// Source-level 唤醒原语（P1-B）。
///
/// 调用方只需要**稳定的 source identity**，不需要持有或传播 runtime session
/// token —— token 由本函数从 `remote_scan_epoch` 解析。当前没有可用 session 时
/// 什么都不做（不伪造 session、不新建无权限 session、不做任何远程访问），
/// pending 工作保持持久化，等待后续 session/source attach 触发的下一次唤醒。
///
/// 复用既有 `wake_remote_cover_worker`：同一套 in-process single-flight、
/// 同一套「队空可退出、再有 pending 可重新启动」语义，不新建第二套 registry。
pub(crate) fn wake_cover_worker_for_source(source_id: &str) {
    let source_id = source_id.trim();
    if source_id.is_empty() {
        return;
    }
    // 联网开关关闭时不得产生新的网络工作；pending 保持持久化。
    if !current_cover_fetch_enabled() {
        return;
    }
    let token = {
        let Ok(conn) = db::get().lock() else {
            return;
        };
        match crate::remote_scan::cover_store::resolve_cover_wake_session(
            &conn,
            source_id,
            db::now_ms(),
        ) {
            Ok(token) => token,
            Err(_) => None,
        }
    };
    // 没有可用 session：不伪造 session、不新建无权限 session、不做任何远程访问。
    let Some(token) = token else {
        return;
    };
    wake_remote_cover_worker(source_id.to_string(), token);
}

/// 长期补偿的 retryable 判定。
///
/// **不引入任何错误字符串推断**：这里复用的正是既有短退避处理的那一个错误集合
/// （`TransientNetwork | RateLimited`）。当短预算耗尽（`attempt >= 3`）时，这类失败
/// 会落入 `cover_job_failure_state` 的 `_ => Failed` 分支 —— 那就是 retryable terminal
/// failure，可获得一次 6h 长期补偿资格。其余错误一律视为永久失败。
fn long_retry_is_retryable(error: &RemoteScanError) -> bool {
    matches!(
        error,
        RemoteScanError::TransientNetwork(_) | RemoteScanError::RateLimited { .. }
    )
}

/// `notify_source_session_ready` 的返回：只描述本次 reconciliation 做了什么。
#[derive(Debug, Default, Clone)]
pub struct RemoteCoverReconcileDto {
    /// 该 source 当前是否存在可信的 source/session 绑定。false 表示本次什么都没做。
    pub binding_available: bool,
    /// 被推进为可 claim 的长期补偿 job 数。
    pub compensation_promoted: u32,
    /// 因 auth/session blocker 解除而回到候选队列的 job 数。
    pub blocker_cleared: u32,
    /// 因 library 缺口而新建的 pending job 数。
    pub jobs_created: u32,
    /// 本次是否产生了可 claim 的工作（Rust 侧据此已复用 P1-B 的 worker wake）。
    pub claimable: bool,
    /// 是否因预算上限提前结束（剩余工作留给后续 session 事件）。
    pub truncated: bool,
}

/// **source-session lifecycle 事实通知**（P1-C）。
///
/// Dart 只报告一个它本来就掌握的事实：某个 source 已成功获得/更新为当前有效 session。
/// Dart **不**查 failed job、**不**算 6 小时、**不**改 retry state、**不**决定
/// unsupported/blocked、**不**决定 worker 是否启动 —— 这些全部由 Rust 拥有。
///
/// Rust 侧职责：
/// 1. 验证该 source/session 关系（source 必须存在）；
/// 2. 复用既有可信路径 `rebind_completed_generation_session`，让已完成的 generation
///    绑定到当前有效 session；
/// 3. 验证现在确实存在该 session 的绑定；**取不到就什么都不做**（不猜、不伪造）；
/// 4. 对该 source 执行 **bounded、source-scoped** 的 reconciliation；
/// 5. 如产生 claimable work，复用 P1-B 的 `wake_cover_worker_for_source`。
///
/// 幂等：重复通知不会重复生成 job、不会重复消耗长期补偿、不会重复 spawn consumer。
/// 本函数自身**不做任何 provider 网络请求**。
pub async fn notify_source_session_ready(
    source_id: String,
    session: u64,
) -> std::result::Result<RemoteCoverReconcileDto, String> {
    use crate::remote_scan::cover_store::{
        reconcile_cover_compensation_for_source_on, reconcile_missing_covers_for_source_on,
        ReconcileBudget,
    };
    let source_id = source_id.trim().to_string();
    if source_id.is_empty() {
        return Err("书源无效".into());
    }
    if session == 0 {
        return Err("登录状态已失效".into());
    }
    let mut report = RemoteCoverReconcileDto::default();
    // 联网开关关闭时不产生新的网络工作；durable 工作保留，等开关恢复后的下一次事件。
    if !current_cover_fetch_enabled() {
        return Ok(report);
    }
    let claimable = {
        let conn = db::get().lock().map_err(|error| error.to_string())?;
        let known: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM book_sources WHERE id=?1)",
                [&source_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !known {
            return Err("书源不存在".into());
        }
        // 已完成 generation 重绑到当前 session（复用既有可信路径；非 complete 代际会被
        // 该函数自身拒绝，因此这里逐个尝试是安全的）。
        let generations: Vec<i64> = conn
            .prepare("SELECT generation FROM remote_scan_epoch WHERE source_id=?1 ORDER BY generation DESC")
            .map_err(|error| error.to_string())?
            .query_map([&source_id], |row| row.get(0))
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<Vec<i64>>>()
            .map_err(|error| error.to_string())?;
        for generation in generations {
            let _ = crate::remote_scan::persistence::rebind_completed_generation_session(
                &conn, &source_id, generation, session,
            );
        }
        // 验证绑定；拿不到可信绑定就什么都不做。
        let session_token = i64::try_from(session).map_err(|_| "登录状态已失效".to_string())?;
        let bound: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM remote_scan_epoch
                                WHERE source_id=?1 AND session_token=?2 AND session_epoch<>'')",
                rusqlite::params![source_id, session_token],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if !bound {
            false
        } else {
            report.binding_available = true;
            let now = db::now_ms();
            let budget = ReconcileBudget::default();
            let compensation =
                reconcile_cover_compensation_for_source_on(&conn, &source_id, session, now, budget)
                    .map_err(|error| error.to_string())?;
            report.compensation_promoted = compensation.compensation_promoted as u32;
            report.blocker_cleared = compensation.blocker_cleared as u32;
            report.truncated = compensation.truncated;
            let spent = compensation.compensation_promoted + compensation.blocker_cleared;
            let remaining = budget.max_jobs.saturating_sub(spent);
            let mut missing_created = 0;
            if remaining > 0 {
                let gap = reconcile_missing_covers_for_source_on(
                    &conn,
                    &source_id,
                    session,
                    now,
                    ReconcileBudget {
                        max_jobs: remaining,
                        ..budget
                    },
                )
                .map_err(|error| error.to_string())?;
                missing_created = gap.jobs_created;
                report.jobs_created = gap.jobs_created as u32;
                report.truncated = report.truncated || gap.truncated;
            }
            compensation.claimable_work || missing_created > 0
        }
    };
    report.claimable = claimable;
    if claimable {
        wake_cover_worker_for_source(&source_id);
    }
    Ok(report)
}

/// RG-A / A-3：cover 失败后的**纯决策**（唯一来源）。
///
/// 契约（由本模块内 `#[cfg(test)]` 单元测试覆盖）：
///
/// * `429 + Retry-After` ⇒ **优先采用服务端值**（忽略本地指数退避）；
/// * `429` 无 `Retry-After` ⇒ 指数退避 `1s, 2s, 4s …`（cap = `2^10 s`）；
/// * `TransientNetwork` ⇒ **同一个**有界指数退避；
/// * `Forbidden` / 405 等**不在**该集合 ⇒ 绝不进入 retry bucket；
/// * `attempt >= 3`（既有阈值，**RG-A 不修改**）⇒ 落终态，交给 P1-C 长期补偿；
/// * **纯函数**：不触 DB、不 enqueue、不 wake、不 sleep、不读时钟。
///
/// `retry_after_ms` 是**相对延迟**；绝对时间（`next_attempt_at`）由调用方用
/// `db::now_ms()` 组合后落库，因此本函数不需要 `now_ms` 参数。
pub(crate) struct CoverFailureDecision {
    pub(crate) state: crate::remote_scan::cover_model::CoverJobState,
    pub(crate) error_code: Option<String>,
    pub(crate) retry_after_ms: Option<u64>,
}

pub(crate) fn cover_job_failure_decision(
    error: &RemoteScanError,
    attempt: i64,
) -> CoverFailureDecision {
    use crate::remote_scan::cover_model::CoverJobState;
    fn decide(
        state: CoverJobState,
        code: Option<String>,
        retry_after_ms: Option<u64>,
    ) -> CoverFailureDecision {
        CoverFailureDecision {
            state,
            error_code: code,
            retry_after_ms,
        }
    }
    match error {
        RemoteScanError::RangeUnavailable | RemoteScanError::Unsupported => decide(
            CoverJobState::Unsupported,
            Some(error_code(error).into()),
            None,
        ),
        RemoteScanError::Unauthorized | RemoteScanError::Forbidden => decide(
            CoverJobState::Blocked,
            Some(error_code(error).into()),
            None,
        ),
        RemoteScanError::TransientNetwork(_) | RemoteScanError::RateLimited { .. }
            if attempt < 3 =>
        {
            let retry_after = match error {
                RemoteScanError::RateLimited { retry_after_ms } => *retry_after_ms,
                _ => None,
            };
            decide(
                CoverJobState::RetryWait,
                Some(error_code(error).into()),
                retry_after.or(Some((1_i64 << attempt.clamp(0, 10)) as u64 * 1_000)),
            )
        }
        RemoteScanError::Cancelled => {
            decide(CoverJobState::Cancelled, Some("cancelled".into()), None)
        }
        _ => decide(CoverJobState::Failed, Some(error_code(error).into()), None),
    }
}

/// 薄适配：保持既有调用点签名与行为不变（决策唯一来源见上方纯函数）。
fn cover_job_failure_state(
    error: &RemoteScanError,
    attempt: i64,
) -> (
    crate::remote_scan::cover_model::CoverJobState,
    Option<String>,
    Option<u64>,
) {
    let decision = cover_job_failure_decision(error, attempt);
    (decision.state, decision.error_code, decision.retry_after_ms)
}

fn parse_cover_profile(profile: &str) -> (u32, u32) {
    let mut parts = profile.split('@').next().unwrap_or_default().split('x');
    let width = parts.next().and_then(|value| value.parse().ok());
    let height = parts.next().and_then(|value| value.parse().ok());
    match (width, height) {
        (Some(width), Some(height)) if width > 0 && height > 0 => (width, height),
        _ => (REMOTE_COVER_WIDTH, REMOTE_COVER_HEIGHT),
    }
}

/// Decode the stable selection key used by the Dart cover repository. Older
/// jobs use `default`, and custom callers may provide an opaque revision; in
/// both cases page zero/no crop is the safe fallback rather than silently
/// replacing an explicit user selection with a different page.
fn parse_cover_selection(selection: &str) -> (u32, Option<(f64, f64, f64, f64)>) {
    let mut page = 0;
    let mut crop = None;
    for part in selection.split('|') {
        if let Some(value) = part.strip_prefix("page:") {
            page = value.parse::<u32>().unwrap_or(0);
        } else if let Some(value) = part.strip_prefix("crop:") {
            let values = value
                .split(',')
                .map(str::trim)
                .map(|item| item.parse::<f64>())
                .collect::<std::result::Result<Vec<_>, _>>();
            if let Ok(values) = values {
                if let [x, y, w, h] = values.as_slice() {
                    if [*x, *y, *w, *h].iter().all(|value| value.is_finite())
                        && *w > 0.0
                        && *h > 0.0
                    {
                        crop = Some((*x, *y, *w, *h));
                    }
                }
            }
        }
    }
    (page, crop)
}

fn cover_route_for_job(
    source_id: &str,
    asset_id: &str,
) -> Option<(String, String, Option<String>)> {
    let conn = db::get().lock().ok()?;
    // Prefer a current preview route while a generation is running. The
    // logical path may be unchanged while an opaque provider id is refreshed;
    // using the previous authoritative route would then fetch stale content.
    let preview = conn
        .query_row(
            "SELECT p.logical_path,
                    COALESCE((SELECT type FROM book_sources WHERE id=?1),'unknown'),
                    p.provider_file_id
             FROM remote_scan_preview p
             JOIN remote_scan_state s ON s.source_id=p.source_id
                                      AND s.generation=p.generation
                                      AND s.status='Running'
             WHERE p.source_id=?1 AND p.asset_id=?2
               AND p.session_epoch <> ''
               AND p.source_fingerprint=(SELECT fingerprint FROM book_sources WHERE id=?1)
             ORDER BY p.generation DESC LIMIT 1",
            params![source_id, asset_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .ok()?;
    if preview.is_some() {
        return preview;
    }
    let route = conn
        .query_row(
            "SELECT logical_path,provider_id,provider_file_id
             FROM remote_asset_route WHERE source_id=?1 AND asset_id=?2",
            params![source_id, asset_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .ok()?;
    route
}

fn run_remote_cover_worker(source_id: &str, session: u64) {
    let source_type = db::get().lock().ok().and_then(|conn| {
        conn.query_row(
            "SELECT type FROM book_sources WHERE id=?1",
            [source_id],
            |row| row.get::<_, String>(0),
        )
        .ok()
    });
    let Some(source_type) = source_type else {
        return;
    };
    let owner = format!("cover:{source_id}:{session}");
    loop {
        // The setting is checked at every claim boundary.  A toggle cannot
        // interrupt an already-issued HTTP request, but it must prevent the
        // next queued job from starting; pending work remains durable for a
        // later explicit request or scan.
        if !current_cover_fetch_enabled() {
            return;
        }
        let (job, next_retry_at) = {
            let Ok(conn) = db::get().lock() else { return };
            let _ = crate::remote_scan::cover_store::recover_expired_leases_on(&conn, db::now_ms());
            let job = match crate::remote_scan::cover_store::claim_next_job_for_source_session_on(
                &conn,
                source_id,
                &owner,
                db::now_ms(),
                10 * 60 * 1000,
                session,
            ) {
                Ok(job) => job,
                Err(_) => return,
            };
            let next_retry_at = if job.is_none() {
                conn.query_row(
                    "SELECT MIN(next_attempt_at) FROM remote_cover_job
                     WHERE source_id=?1 AND state='retry_wait'",
                    [source_id],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .ok()
                .flatten()
            } else {
                None
            };
            (job, next_retry_at)
        };
        let Some(job) = job else {
            // Keep a retry_wait worker alive until the next durable deadline;
            // no worker slot or SQLite lock is held while waiting.  New
            // pending/visible jobs still wake a fresh worker immediately.
            if let Some(next_retry_at) = next_retry_at {
                let now = db::now_ms();
                if next_retry_at > now {
                    std::thread::sleep(std::time::Duration::from_millis(
                        (next_retry_at - now).min(30_000) as u64,
                    ));
                    continue;
                }
            }
            return;
        };
        let route = cover_route_for_job(source_id, &job.key.asset_id);
        let Some((logical_path, provider_id, provider_file_id)) = route else {
            if let Ok(conn) = db::get().lock() {
                let _ = crate::remote_scan::cover_store::mark_job_state_owned_on(
                    &conn,
                    &job.key,
                    &owner,
                    crate::remote_scan::cover_model::CoverJobState::Failed,
                    Some("route_missing"),
                    db::now_ms(),
                );
            }
            continue;
        };
        let budget_key = cover_budget_key(source_id, &provider_id);
        if let crate::remote_scan::provider_budget::BudgetDecision::WaitUntil(deadline) =
            crate::remote_scan::provider_budget::global().try_reserve(&budget_key, db::now_ms())
        {
            if let Ok(conn) = db::get().lock() {
                let _ = crate::remote_scan::cover_store::release_job_lease_on(
                    &conn,
                    &job.key,
                    &owner,
                    db::now_ms(),
                );
            }
            let wait_ms = deadline.saturating_sub(db::now_ms()).clamp(1, 30_000) as u64;
            std::thread::sleep(std::time::Duration::from_millis(wait_ms));
            continue;
        }
        let adapter = crate::api::source::remote_provider_adapter(
            if provider_id == "unknown" {
                &source_type
            } else {
                &provider_id
            },
            session,
            "/",
        );
        let Ok(adapter) = adapter else {
            if let Ok(conn) = db::get().lock() {
                let _ = crate::remote_scan::cover_store::mark_job_state_owned_on(
                    &conn,
                    &job.key,
                    &owner,
                    crate::remote_scan::cover_model::CoverJobState::Blocked,
                    Some("authExpired"),
                    db::now_ms(),
                );
            }
            return;
        };
        if let Some(provider_file_id) = provider_file_id.as_deref() {
            adapter.register_path(&logical_path, provider_file_id);
        }
        let (profile_width, profile_height) = parse_cover_profile(&job.key.profile);
        let (cover_page, cover_crop) = parse_cover_selection(&job.key.selection_revision);
        // 标注 Cover 优先级：底层 CDN Range 门按它决定排队次序，否则封面会被
        // 当作前台请求去抢当前阅读页的许可。
        let result = crate::source::gate::with_priority(RequestPriority::Cover, || {
            cover_source_info(source_id, &logical_path, cover_page)
                .ok_or(RemoteScanError::NotFound)
                .and_then(|info| {
                    fetch_remote_cover_image_with_dimensions(
                        Arc::clone(&adapter),
                        &logical_path,
                        &info.name,
                        info.size,
                        &info.fingerprint,
                        info.asset_kind,
                        info.image_path,
                        profile_width,
                        profile_height,
                        cover_page,
                        cover_crop,
                    )
                })
        });
        match result {
            Ok(image) => {
                // The legacy path-only cache is kept for the default profile
                // only.  Custom profiles are published exclusively through
                // the versioned cache so their pixel dimensions cannot be
                // confused with the historical 340x480 payload.
                let legacy_ok = if (profile_width, profile_height)
                    == (REMOTE_COVER_WIDTH, REMOTE_COVER_HEIGHT)
                    && job.key.selection_revision == "default"
                {
                    write_remote_cover_cache(adapter.as_ref(), &logical_path, &image)
                } else {
                    true
                };
                let cache_ok = legacy_ok
                    && crate::cache::remote_cover_cache_write(
                        source_id,
                        &job.key.asset_id,
                        &job.key.content_revision,
                        &job.key.selection_revision,
                        &job.key.profile,
                        image.width,
                        image.height,
                        &image.rgba,
                    )
                    .is_ok();
                if let Ok(conn) = db::get().lock() {
                    if cache_ok {
                        let checksum = format!("{:x}", sha2::Sha256::digest(&image.rgba));
                        if !crate::remote_scan::cover_store::mark_job_ready_owned_on(
                            &conn,
                            &job.key,
                            &owner,
                            db::now_ms(),
                            image.width,
                            image.height,
                            image.rgba.len() as u64,
                            &checksum,
                        )
                        .unwrap_or(false)
                        {
                            let _ = crate::remote_scan::cover_store::mark_job_state_owned_on(
                                &conn,
                                &job.key,
                                &owner,
                                crate::remote_scan::cover_model::CoverJobState::Failed,
                                Some("coverStorage"),
                                db::now_ms(),
                            );
                        }
                    } else {
                        let _ = crate::remote_scan::cover_store::mark_job_state_owned_on(
                            &conn,
                            &job.key,
                            &owner,
                            crate::remote_scan::cover_model::CoverJobState::Failed,
                            Some("coverStorage"),
                            db::now_ms(),
                        );
                    }
                }
            }
            Err(error) => {
                let (state, code, retry_after) = cover_job_failure_state(&error, job.attempt);
                if let Ok(conn) = db::get().lock() {
                    if state == crate::remote_scan::cover_model::CoverJobState::RetryWait {
                        let _ = crate::remote_scan::cover_store::mark_retry_wait_owned_on(
                            &conn,
                            &job.key,
                            &owner,
                            db::now_ms(),
                            retry_after,
                            code.as_deref(),
                        );
                    } else {
                        // P1-C：终态失败必须记录 failure episode。retryable（沿用既有短退避
                        // 的同一错误集合）在短预算耗尽后获得一次 6h 长期补偿资格；永久失败
                        // 传 None（保持 long_retry_not_before 为 NULL，不参与补偿）。
                        let now = db::now_ms();
                        let episode = if state
                            == crate::remote_scan::cover_model::CoverJobState::Failed
                            && long_retry_is_retryable(&error)
                        {
                            Some(now.saturating_add(
                                crate::remote_scan::cover_store::LONG_RETRY_DELAY_MS,
                            ))
                        } else {
                            None
                        };
                        let _ = crate::remote_scan::cover_store::mark_job_failure_owned_on(
                            &conn,
                            &job.key,
                            &owner,
                            state,
                            code.as_deref(),
                            now,
                            episode,
                        );
                    }
                }
            }
        }
    }
}

/// Resolve a non-secret account identity for the cross-task budget. Newer
/// Android databases use `credential_ref`; desktop/legacy rows may expose an
/// app/client id. If neither exists, source-scoped throttling is the safe
/// fallback and never reads Cookie/password columns.
fn cover_budget_key(source_id: &str, provider_id: &str) -> String {
    let identity = db::get()
        .lock()
        .ok()
        .and_then(|conn| {
            let has_ref: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('book_sources') WHERE name='credential_ref')",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(false);
            if has_ref {
                conn.query_row(
                    "SELECT COALESCE(NULLIF(credential_ref,''),NULLIF(client_id,''),id)
                     FROM book_sources WHERE id=?1",
                    [source_id],
                    |row| row.get::<_, String>(0),
                )
                .ok()
            } else {
                conn.query_row(
                    "SELECT COALESCE(NULLIF(client_id,''),id)
                     FROM book_sources WHERE id=?1",
                    [source_id],
                    |row| row.get::<_, String>(0),
                )
                .ok()
            }
        })
        .unwrap_or_else(|| source_id.to_string());
    crate::remote_scan::provider_budget::account_key(provider_id, &identity, "")
}

fn start_lock() -> &'static Mutex<()> {
    static START_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    START_LOCK.get_or_init(|| Mutex::new(()))
}

fn next_job_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("remote-scan-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

struct SqliteScanSink {
    /// Runtime session is intentionally kept only in memory. It is used to
    /// wake the shared cover worker as preview pages are discovered; the
    /// durable queue stores only the derived session epoch.
    session: u64,
}

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
    fn should_recheck_directory(
        &self,
        source_id: &str,
        logical_path: &str,
    ) -> Result<bool, RemoteScanError> {
        let conn = db::get()
            .lock()
            .map_err(|_| RemoteScanError::Io("database_read_failed".into()))?;
        persistence::directory_recheck_due(&conn, source_id, logical_path, db::now_ms())
            .map_err(|_| RemoteScanError::Io("database_read_failed".into()))
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
        // Publish a non-authoritative overlay immediately. The root browser
        // can therefore resolve asset identities and enqueue covers before
        // the recursive generation finishes, while deletion proof remains
        // gated on the later authoritative publish transaction.
        persistence::materialize_preview_listing(
            &conn,
            &directory.source_id,
            &normalize_path(&directory.logical_path),
            &directory.entries,
            directory.generation,
            &directory.session_epoch,
            &directory.fingerprint,
            directory.asset_kind,
        )
        .map_err(|_| RemoteScanError::Io("preview_commit_failed".into()))?;
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
            .map_err(|_| RemoteScanError::Io("cover_dependency_stage_failed".into()))?;
        // Materialize the same key in the durable queue during discovery.
        // `publish_staged_generation` upserts it again later, so this is
        // idempotent and lets the preview projection show covers early.
        let source_fp: String = conn
            .query_row(
                "SELECT COALESCE(fingerprint,'') FROM book_sources WHERE id=?1",
                [&task.source_id],
                |row| row.get(0),
            )
            .map_err(|_| RemoteScanError::Io("source_identity_lookup_failed".into()))?;
        let key = crate::remote_scan::cover_model::CoverJobKey {
            source_id: task.source_id.clone(),
            asset_id: crate::db::library_index_id(&source_fp, &task.logical_path),
            content_revision: task.fingerprint.clone(),
            selection_revision: "default".into(),
            profile: "340x480@1".into(),
        };
        crate::remote_scan::cover_store::upsert_job_on(
            &conn,
            &key,
            crate::remote_scan::cover_model::CoverJobState::Pending,
            "background",
            10,
            task.generation,
            &task.session_epoch,
            crate::db::now_ms(),
            CoverJobUpsertCause::Demand,
        )
        .map_err(|_| RemoteScanError::Io("cover_queue_commit_failed".into()))?;
        self.wake_cover_worker(&task.source_id);
        Ok(())
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

impl SqliteScanSink {
    fn wake_cover_worker(&self, source_id: &str) {
        wake_remote_cover_worker(source_id.to_string(), self.session);
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
        RemoteScanError::HttpStatus { .. } => "httpStatus",
    }
}

/// Choose the mode for an automatic/manual start using the last durable
/// terminal state and an explicit full-scan proof. A successful terminal row
/// by itself is not enough: legacy builds could persist `Succeeded` for an
/// incremental generation, so the first run after upgrading must establish a
/// full baseline before any incremental scan is allowed.
fn effective_scan_mode(
    requested: &str,
    persisted_status: Option<&str>,
    has_full_baseline: bool,
    resume: bool,
) -> String {
    if resume || !matches!(persisted_status, Some("Succeeded")) || !has_full_baseline {
        "full".to_string()
    } else {
        requested.to_string()
    }
}

const REMOTE_COVER_WIDTH: u32 = 340;
const REMOTE_COVER_HEIGHT: u32 = 480;
const MAX_REMOTE_COVER_BYTES: u64 = 32 * 1024 * 1024;

fn parse_bool_setting(value: Option<String>) -> bool {
    match value
        .as_deref()
        .map(str::trim)
        .map(|value| value.trim_matches('"'))
    {
        Some("false") | Some("0") => false,
        Some("true") | Some("1") => true,
        // Keep the historical default when an older install has no key or a
        // malformed value.  A malformed setting must not silently disable
        // remote cover loading.
        _ => true,
    }
}

fn current_cover_fetch_enabled() -> bool {
    match db::get().lock() {
        Ok(conn) => cover_fetch_enabled_from_conn(&conn),
        Err(_) => true,
    }
}

fn cover_fetch_enabled_from_conn(conn: &rusqlite::Connection) -> bool {
    match db::load_setting_on(conn, "remoteCoverFetchEnabled") {
        Some(value) => parse_bool_setting(Some(value)),
        None => true,
    }
}

/// A bounded random-access source backed by the provider adapter. ZIP/CBZ
/// parsing can therefore read the tail directory and the first page without
/// downloading the whole book or retaining file-sized buffers.
#[flutter_rust_bridge::frb(ignore)]
struct AdapterByteSource {
    adapter: Arc<dyn crate::remote_scan::adapter::RemoteProviderAdapter>,
    path: String,
    length: u64,
}

impl ByteSource for AdapterByteSource {
    fn len(&self) -> u64 {
        self.length
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || offset >= self.length {
            return Ok(0);
        }
        let requested = (self.length - offset).min(buf.len() as u64) as usize;
        let bytes = self
            .adapter
            .read_range(&self.path, offset, requested as u64)
            .map_err(|error| io::Error::other(error.to_string()))?;
        if bytes.len() > requested {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "remote range response exceeded requested length",
            ));
        }
        buf[..bytes.len()].copy_from_slice(&bytes);
        Ok(bytes.len())
    }
}

fn cover_error(error: impl std::fmt::Display) -> RemoteScanError {
    // Do not expose provider URLs, credentials, or archive parser details in
    // persisted status. The UI only needs a stable placeholder reason.
    let _ = error;
    RemoteScanError::Provider("cover_decode_failed".into())
}

/// Decode an actual first page for an archive or image-folder cover. The
/// previous implementation treated the first 256 KiB of a ZIP as an image;
/// this function keeps the range-only policy while passing bytes through the
/// existing document and image decoders.
#[allow(clippy::too_many_arguments)]
fn fetch_remote_cover_image_with_dimensions(
    adapter: Arc<dyn crate::remote_scan::adapter::RemoteProviderAdapter>,
    path: &str,
    document_name: &str,
    size: Option<u64>,
    fingerprint: &str,
    asset_kind: RemoteAssetKind,
    image_path: Option<(String, Option<u64>, String)>,
    cover_width: u32,
    cover_height: u32,
    page: u32,
    crop: Option<(f64, f64, f64, f64)>,
) -> Result<crate::decode::DecodedImage, RemoteScanError> {
    let (read_path, read_size, read_fingerprint, is_archive) = match asset_kind {
        RemoteAssetKind::ArchiveFile => (path.to_string(), size, fingerprint.to_string(), true),
        RemoteAssetKind::ImageFolder | RemoteAssetKind::ImageFile => {
            let Some((image_path, image_size, image_fingerprint)) = image_path else {
                return Err(RemoteScanError::Unsupported);
            };
            (image_path, image_size, image_fingerprint, false)
        }
        _ => return Err(RemoteScanError::Unsupported),
    };
    let Some(read_size) = read_size.filter(|value| *value > 0) else {
        return Err(RemoteScanError::MalformedResponse(
            "cover_size_missing".into(),
        ));
    };
    if read_size > MAX_REMOTE_COVER_BYTES && !is_archive {
        return Err(RemoteScanError::MalformedResponse(
            "cover_size_limit".into(),
        ));
    }
    // Hold the shared Cover permit across capability probing as well as the
    // actual range reads.  For opaque providers (115/Quark/Baidu), probing
    // itself performs a downurl + HTTP Range request; doing it before the
    // permit would let a scan bypass the foreground-reader reservation.
    // The governor permit is the *coarse* cross-provider guard; it is scoped to
    // the network stages only. Decoding is pure local CPU work and must not
    // hold a network permit (审阅冻结决策 4：不再跨越解码持有网络许可).
    //
    // For 115/Quark the fine-grained priority is carried by the CDN Range gate
    // itself (see `source::gate`), which queues every individual request by
    // priority. This permit remains because it is also the only protection for
    // providers without their own gate (WebDAV / SFTP).
    let governor = blocking_request_governor();
    let bytes = {
        let _permit = governor
            .acquire(RequestPriority::Cover)
            .map_err(|_| RemoteScanError::Provider("request_queue_full".into()))?;
        let capabilities = adapter.capabilities(&read_path, &read_fingerprint)?;
        if !capabilities.range_read {
            return Err(RemoteScanError::RangeUnavailable);
        }
        if is_archive {
            let source = AdapterByteSource {
                adapter: Arc::clone(&adapter),
                path: read_path,
                length: read_size,
            };
            // 115/夸克/百度 use an opaque id as the logical path; dispatch the
            // parser by the real file name stored in library_index.
            let document =
                crate::document::open_document(source, document_name).map_err(cover_error)?;
            document.page_bytes(page).map_err(cover_error)?
        } else {
            adapter.read_range(&read_path, 0, read_size)?
        }
    };
    crate::decode::decode_cover(&bytes, cover_width, cover_height, crop).map_err(cover_error)
}

#[derive(Clone)]
struct CoverSourceInfo {
    name: String,
    asset_kind: RemoteAssetKind,
    size: Option<u64>,
    fingerprint: String,
    image_path: Option<(String, Option<u64>, String)>,
}

fn cover_source_info(source_id: &str, path: &str, page: u32) -> Option<CoverSourceInfo> {
    let conn = db::get().lock().ok()?;
    let normalized_path = normalize_path(path);
    let indexed: Option<(String, String, Option<i64>, Option<String>)> = conn
        .query_row(
            "SELECT name,asset_kind,size,content_fingerprint FROM library_index
             WHERE source_id=?1 AND path=?2 AND deleted=0",
            params![source_id, normalized_path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .ok()?;
    let preview: Option<(String, String, Option<i64>, Option<String>)> = conn
        .query_row(
            "SELECT p.name,p.asset_kind,p.size,p.content_fingerprint
             FROM remote_scan_preview p
             JOIN remote_scan_state s ON s.source_id=p.source_id
                                      AND s.generation=p.generation
                                      AND s.status='Running'
             WHERE p.source_id=?1 AND p.logical_path=?2
               AND p.session_epoch <> ''
               AND p.source_fingerprint=(SELECT fingerprint FROM book_sources WHERE id=?1)
             ORDER BY p.generation DESC LIMIT 1",
            params![source_id, normalized_path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .ok()?;
    let (name, asset_kind, size, fingerprint) = preview.or(indexed)?;
    let asset_kind = match asset_kind.as_str() {
        "ArchiveFile" => RemoteAssetKind::ArchiveFile,
        "ImageFolder" => RemoteAssetKind::ImageFolder,
        "ImageFile" => RemoteAssetKind::ImageFile,
        _ => return None,
    };
    let fingerprint = fingerprint.filter(|value| !value.is_empty())?;
    let image_path = if asset_kind == RemoteAssetKind::ImageFolder {
        let source_fingerprint: String = conn
            .query_row(
                "SELECT fingerprint FROM book_sources WHERE id=?1",
                [source_id],
                |row| row.get(0),
            )
            .ok()?;
        let parent_id = db::library_index_id(&source_fingerprint, &normalized_path);
        let generation: Option<i64> = conn
            .query_row(
                "SELECT generation FROM remote_scan_state
                 WHERE source_id=?1 AND status='Running'",
                [source_id],
                |row| row.get(0),
            )
            .optional()
            .ok()?;
        let mut images = if let Some(generation) = generation {
            conn.prepare(
                "SELECT logical_path,name,size,content_fingerprint FROM remote_scan_preview
                 WHERE source_id=?1 AND generation=?2 AND parent_asset_id=?3
                   AND session_epoch <> ''
                   AND source_fingerprint=(SELECT fingerprint FROM book_sources WHERE id=?1)",
            )
            .ok()?
            .query_map(params![source_id, generation, parent_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .ok()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .ok()?
        } else {
            Vec::new()
        };
        if images.is_empty() {
            images = conn
                .prepare(
                    "SELECT path,name,size,content_fingerprint FROM library_index
                 WHERE source_id=?1 AND parent_id=?2 AND asset_kind='ImageFile' AND deleted=0",
                )
                .ok()?
                .query_map(params![source_id, parent_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })
                .ok()?
                .collect::<rusqlite::Result<Vec<_>>>()
                .ok()?;
        }
        images.sort_by(|a, b| crate::util::natural_cmp(&a.1, &b.1));
        images
            .into_iter()
            .nth(usize::try_from(page).ok()?)
            .and_then(|(image_path, _image_name, image_size, image_fingerprint)| {
                Some((
                    image_path,
                    image_size.and_then(|value| u64::try_from(value).ok()),
                    image_fingerprint.filter(|value| !value.is_empty())?,
                ))
            })
    } else {
        None
    };
    Some(CoverSourceInfo {
        name,
        asset_kind,
        size: size.and_then(|value| u64::try_from(value).ok()),
        fingerprint,
        image_path,
    })
}

fn refresh_cover_progress(status: &mut RemoteScanStatusDto, generation: i64) {
    if let Ok(conn) = db::get().lock() {
        if let Ok(total) =
            persistence::count_staged_cover_tasks(&conn, &status.source_id, generation)
        {
            status.total = status.total.max(total);
        }
        refresh_status_counts(status, &conn, generation);
    }
    // P1-F：锁已释放 ⇒ 在此做缓存可用性校验。
    crate::remote_scan::cover_progress::apply_cover_availability(status);
}

/// Populate the comic-level counters from the durable index/job projection.
/// Queries are best-effort for compatibility with pre-migration databases;
/// a missing additive table yields zero rather than blocking a scan status.
fn refresh_status_counts(
    status: &mut RemoteScanStatusDto,
    conn: &rusqlite::Connection,
    generation: i64,
) {
    let source_id = &status.source_id;
    let published_directories = conn
        .query_row(
            "SELECT COUNT(*) FROM remote_listing_state WHERE source_id=?1 AND scan_generation=?2",
            params![source_id, generation],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64;
    let staged_directories = conn
        .query_row(
            "SELECT COUNT(*) FROM remote_scan_listing_stage WHERE source_id=?1 AND generation=?2",
            params![source_id, generation],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64;
    // Staged pages are already discovered and are safe to expose as
    // progress, but only published rows can establish deletion proof.
    status.directories_checked = published_directories.max(staged_directories);
    let indexed_books = conn
        .query_row(
            "SELECT COUNT(DISTINCT id) FROM library_index
             WHERE source_id=?1 AND scan_generation=?2 AND deleted=0
               AND ((entry_type='file' AND asset_kind='ArchiveFile')
                    OR (entry_type='dir' AND asset_kind='ImageFolder'))",
            params![source_id, generation],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64;
    let staged_books =
        persistence::count_staged_cover_tasks(conn, source_id, generation).unwrap_or(0);
    status.discovered_books = indexed_books.max(staged_books);

    let mut counts = [0_u64; 7];
    if let Ok(mut stmt) = conn.prepare(
        "SELECT asset_id,state,updated_at,content_revision,selection_revision,profile
           FROM remote_cover_job
          WHERE source_id=?1 AND generation=?2",
    ) {
        if let Ok(rows) = stmt.query_map(params![source_id, generation], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        }) {
            // P1-F：latest-per-asset（与 UI 的"当前 job"语义一致），
            // 并保留 cover 身份供**锁外**做缓存可用性校验。
            let mut latest_by_asset: HashMap<String, (String, i64, String, String, String)> =
                HashMap::new();
            for row in rows.flatten() {
                let replace = latest_by_asset
                    .get(&row.0)
                    .is_none_or(|(_, updated_at, _, _, _)| row.2 >= *updated_at);
                if replace {
                    latest_by_asset.insert(row.0, (row.1, row.2, row.3, row.4, row.5));
                }
            }
            let tracked_distinct = latest_by_asset.len() as u64;
            let mut other_unknown = 0_u64;
            let mut ready_identities: Vec<(String, String, String, String)> = Vec::new();
            for (asset_id, (state, _, content_revision, selection_revision, profile)) in
                latest_by_asset.into_iter()
            {
                let index = match state.as_str() {
                    "ready" => Some(0),
                    "running" => Some(1),
                    "pending" => Some(2),
                    "retry_wait" => Some(3),
                    "blocked" => Some(4),
                    "unsupported" => Some(5),
                    "failed" => Some(6),
                    // P1-F：真实未知 durable state **不得静默丢弃**。
                    _ => None,
                };
                match index {
                    Some(index) => counts[index] = counts[index].saturating_add(1),
                    None => other_unknown = other_unknown.saturating_add(1),
                }
                if state == "ready" {
                    ready_identities.push((
                        asset_id,
                        content_revision,
                        selection_revision,
                        profile,
                    ));
                }
            }
            crate::remote_scan::cover_progress::publish_cover_availability_inputs(
                tracked_distinct,
                other_unknown,
                ready_identities,
            );
        }
    }
    status.ready_books = counts[0];
    status.active_books = counts[1];
    status.pending_books = counts[2];
    status.retry_books = counts[3];
    status.blocked_books = counts[4];
    status.unsupported_books = counts[5];
    status.failed_books = counts[6];
    // During discovery the compatibility stage may precede materialization of
    // `remote_cover_job` (the authoritative listing is still being built).
    // Surface those comics as waiting instead of falsely reporting zero
    // pending work.
    let staged_unmaterialized =
        persistence::count_staged_cover_tasks(conn, source_id, generation).unwrap_or(0);
    if staged_unmaterialized > 0
        && status.ready_books == 0
        && status.active_books == 0
        && status.pending_books == 0
        && status.retry_books == 0
        && status.blocked_books == 0
        && status.unsupported_books == 0
        && status.failed_books == 0
    {
        status.pending_books = staged_unmaterialized;
        // P1-F：这批 staged-only 漫画已被 pending 代表 ⇒ 通知语义层从 no_job 扣除。
        crate::remote_scan::cover_progress::publish_staged_pending_represented(
            staged_unmaterialized,
        );
    }
    status.view_revision =
        crate::remote_scan::cover_store::view_revision(conn, source_id).unwrap_or(0);
    let completed_directories = conn
        .query_row(
            "SELECT COUNT(*) FROM remote_listing_state
             WHERE source_id=?1 AND scan_generation=?2 AND listing_complete=1",
            params![source_id, generation],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64;
    // A terminal error alone is not proof that discovery finished.  The
    // authoritative listing rows are published atomically, so matching
    // complete-row counts is the durable signal for the two-phase scanner.
    status.discovery_complete = status.status != "running"
        && status.status != "queued"
        && status.directories_checked > 0
        && completed_directories == status.directories_checked;
    status.listing_phase = if status.discovery_complete {
        "complete".into()
    } else if status.directories_checked > 0 {
        "scanning".into()
    } else {
        "pending".into()
    };
    // Preserve old fields for old UI while making their meaning explicit for
    // new clients: total is discovered comics and processed is ready covers.
    if status.discovered_books > 0 || status.directories_checked > 0 {
        status.total = status.discovered_books;
        status.processed = status.ready_books;
    }
}

fn cover_source_fingerprint(source_id: &str) -> String {
    db::get()
        .lock()
        .ok()
        .and_then(|conn| {
            conn.query_row(
                "SELECT COALESCE(fingerprint,'') FROM book_sources WHERE id=?1",
                [source_id],
                |row| row.get(0),
            )
            .ok()
        })
        .unwrap_or_default()
}

fn cover_cache_paths(
    adapter: &dyn crate::remote_scan::adapter::RemoteProviderAdapter,
    logical_path: &str,
) -> Vec<String> {
    let mut paths = vec![logical_path.to_string()];
    if let Some(alias) = adapter.cache_path(logical_path) {
        if alias != logical_path {
            paths.push(alias);
        }
    }
    paths
}

fn write_remote_cover_cache(
    adapter: &dyn crate::remote_scan::adapter::RemoteProviderAdapter,
    logical_path: &str,
    image: &crate::decode::DecodedImage,
) -> bool {
    cover_cache_paths(adapter, logical_path)
        .into_iter()
        .all(|path| {
            crate::cache::cover_cache_write(
                &path,
                0,
                REMOTE_COVER_WIDTH,
                REMOTE_COVER_HEIGHT,
                None,
                &image.rgba,
            )
            .is_ok()
        })
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
    // RG-B 健壮性修复（A）：若该源**没有存活 job**，则持久化的 `running` 代际必然是
    // 崩溃/强制退出留下的残留 ⇒ 先落为中断终态再继续。否则下面的 epoch/baseline 证明
    // 会永久拒绝本次启动（用户表现为"扫描启动失败"且无法恢复）。
    if !jobs().lock().unwrap().contains_key(&config.source_id) {
        if let Ok(conn) = db::get().lock() {
            if persistence::recover_interrupted_scan(&conn, &config.source_id).unwrap_or(false) {
                eprintln!(
                    "[remote_scan] recovered residual running generation for {}",
                    config.source_id
                );
            }
        }
    }
    let adapter = remote_provider_adapter(&config.source_type, config.session, &config.root_path)
        .map_err(|_| "remote session unavailable".to_string())?;
    let initial_listing = if resume {
        None
    } else {
        parse_initial_listing(config.initial_listing_json.as_deref())?
    };
    // Preserve the provider root alias even when the configured source root
    // is a non-root path. Opaque providers additionally need each seeded
    // child mapping so recursive tasks can resolve ids without relisting the
    // root directory.
    let configured_root = initial_scan_path(&config.root_path);
    adapter.register_path(&configured_root, &config.root_path);
    if let Some(entries) = &initial_listing {
        for (entry, provider_path) in entries {
            adapter.register_path(&entry.logical_path, provider_path);
        }
    }
    let conn = db::get().lock().unwrap();
    // Rehydrate opaque provider ids from the durable route table before the
    // worker starts.  This makes a restarted scan able to follow nested 115,
    // Baidu and Quark paths without forcing the user to reopen each folder.
    if let Ok(mut routes) = conn.prepare(
        "SELECT logical_path,provider_file_id FROM remote_asset_route
         WHERE source_id=?1 AND provider_file_id IS NOT NULL
         ORDER BY generation DESC,route_revision DESC",
    ) {
        if let Ok(rows) = routes.query_map([&config.source_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) {
            let mut seen = HashSet::new();
            for (logical_path, provider_path) in rows.flatten() {
                if seen.insert(logical_path.clone()) {
                    adapter.register_path(&logical_path, &provider_path);
                }
            }
        }
    }
    let stored: Option<(i64, Option<String>, String)> = conn
        .query_row(
            "SELECT generation,checkpoint,status FROM remote_scan_state WHERE source_id=?1",
            [&config.source_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| "scan state unavailable".to_string())?;
    let generation = stored
        .as_ref()
        .map_or(1, |(generation, _, _)| generation.saturating_add(1));
    let persisted_status = stored.as_ref().map(|(_, _, status)| status.as_str());
    let has_full_baseline = persistence::has_full_scan_baseline(&conn, &config.source_id)
        .map_err(|_| "scan baseline unavailable".to_string())?;
    let mode = effective_scan_mode(&config.mode, persisted_status, has_full_baseline, resume);
    let checkpoint = stored.and_then(|(_, checkpoint, _)| checkpoint);
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
        // `processed`/`total` are comic-cover counts, not directory-task
        // counts. The exact total becomes known as staged listings discover
        // comic files; until then the UI explicitly says it is discovering.
        total: 0,
        listing_phase: "pending".into(),
        directories_checked: 0,
        discovered_books: 0,
        discovery_complete: false,
        ready_books: 0,
        active_books: 0,
        pending_books: 0,
        retry_books: 0,
        blocked_books: 0,
        unsupported_books: 0,
        failed_books: 0,
        available_books: 0,
        waiting_books: 0,
        other_books: 0,
        view_revision: 0,
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
            initial_listing_json: None,
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
    let root = initial_scan_path(&job.config.root_path);
    std::thread::spawn(move || {
        let sink: Arc<dyn ScanCommitSink> = Arc::new(SqliteScanSink {
            session: job.config.session,
        });
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
            let initial = initial.incremental();
            if job.config.force_recheck {
                initial.force_recheck()
            } else {
                initial
            }
        } else {
            initial
        };
        let mut seeded_listing = initial_listing;
        let mut outcome =
            engine
                .enqueue_directory(initial)
                .and_then(|_| match seeded_listing.take() {
                    Some(entries) => engine
                        .run_next_with_initial_entries(
                            entries.into_iter().map(|(entry, _)| entry).collect(),
                        )
                        .map(|_| ()),
                    None => engine.run_next().map(|_| ()),
                });
        if outcome.is_ok() {
            let mut status = job.status.lock().unwrap();
            refresh_cover_progress(&mut status, generation);
            status.checkpoint =
                persistence::load_checkpoint(&db::get().lock().unwrap(), &status.source_id)
                    .ok()
                    .flatten();
        }
        while outcome.is_ok() {
            let step = engine.run_next();
            if matches!(step, Ok(false)) {
                break;
            }
            outcome = step.map(|worked| {
                if worked {
                    let mut status = job.status.lock().unwrap();
                    refresh_cover_progress(&mut status, generation);
                    status.checkpoint =
                        persistence::load_checkpoint(&db::get().lock().unwrap(), &status.source_id)
                            .ok()
                            .flatten();
                }
            });
        }
        {
            let status = job.status.lock().unwrap();
            if matches!(status.status.as_str(), "paused" | "cancelled") {
                let _ = persistence::discard_staged_generation(
                    &db::get().lock().unwrap(),
                    &status.source_id,
                    generation,
                );
                persist_config_status(&status);
                return;
            }
        }
        match outcome {
            Ok(()) => {
                let source_id = job.status.lock().unwrap().source_id.clone();
                if token.is_cancelled() {
                    let _ = persistence::discard_staged_generation(
                        &db::get().lock().unwrap(),
                        &source_id,
                        generation,
                    );
                    let mut status = job.status.lock().unwrap();
                    status.status = "cancelled".into();
                    status.error_code = Some("cancelled".into());
                    persist_terminal(&status);
                    persist_config_status(&status);
                } else if persistence::publish_staged_generation(
                    &db::get().lock().unwrap(),
                    &source_id,
                    generation,
                )
                .is_ok()
                {
                    // Keep the job in `running` while covers are decoded so
                    // polling clients can observe comic-level progress. The
                    // status mutex is intentionally released during network
                    // and archive I/O.
                    {
                        let mut status = job.status.lock().unwrap();
                        refresh_cover_progress(&mut status, generation);
                        status.processed = 0;
                        status.error_code = None;
                    }
                    let cover_result = consume_staged_covers(&job, &source_id, generation);
                    let mut status = job.status.lock().unwrap();
                    if token.is_cancelled() {
                        let _ = persistence::discard_staged_generation(
                            &db::get().lock().unwrap(),
                            &source_id,
                            generation,
                        );
                        status.status = "cancelled".into();
                        status.error_code = Some("cancelled".into());
                    } else if !cover_result.storage_failed {
                        // A terminal listing is not an incremental baseline
                        // until a *full* generation has been durably proven
                        // for this exact source identity/root.  This marker is
                        // deliberately written after cover consumption so a
                        // crash or storage error cannot authorize a partial
                        // tree on the next launch.
                        let baseline_ok = if status.mode == "full" {
                            db::get()
                                .lock()
                                .ok()
                                .map(|conn| {
                                    persistence::mark_full_scan_succeeded(
                                        &conn, &source_id, generation,
                                    )
                                    .is_ok()
                                })
                                .unwrap_or(false)
                        } else {
                            true
                        };
                        if baseline_ok {
                            status.status = "complete".into();
                            status.last_success_at = Some(db::now_ms());
                            status.error_code = if cover_result.fetch_paused {
                                Some("coverFetchPaused".into())
                            } else if cover_result.range_unavailable {
                                Some("rangeUnavailable".into())
                            } else if cover_result.cover_unavailable {
                                Some("coverUnavailable".into())
                            } else {
                                None
                            };
                        } else {
                            status.status = "degraded".into();
                            status.error_code = Some("scanBaselineStorage".into());
                        }
                    } else {
                        status.status = "degraded".into();
                        status.error_code = Some("coverStorage".into());
                    }
                    persist_terminal(&status);
                    persist_config_status(&status);
                } else {
                    let _ = persistence::discard_staged_generation(
                        &db::get().lock().unwrap(),
                        &source_id,
                        generation,
                    );
                    let mut status = job.status.lock().unwrap();
                    status.status = "degraded".into();
                    status.error_code = Some("storage".into());
                    persist_terminal(&status);
                    persist_config_status(&status);
                }
            }
            Err(error) => {
                let source_id = job.status.lock().unwrap().source_id.clone();
                let _ = persistence::discard_staged_generation(
                    &db::get().lock().unwrap(),
                    &source_id,
                    generation,
                );
                let mut status = job.status.lock().unwrap();
                status.status = "degraded".into();
                status.error_code = Some(error_code(&error).into());
                persist_terminal(&status);
                persist_config_status(&status);
            }
        }
    });
    Ok(response)
}

fn consume_staged_covers(
    job: &Arc<ScanJob>,
    source_id: &str,
    generation: i64,
) -> CoverConsumeResult {
    // This function is the scanner/queue hand-off, not a network worker.  A
    // previous implementation decoded every cover inline here, which made a
    // large root scan monopolize the same request path used by the reader and
    // caused the avalanche observed in the UI.  Listing publication already
    // materializes one durable job per key; we only retire the compatibility
    // stage rows and wake the shared worker below.
    {
        let mut status = job.status.lock().unwrap();
        refresh_cover_progress(&mut status, generation);
        status.processed = 0;
    }
    let aggregate = CoverConsumeResult {
        fetch_paused: !current_cover_fetch_enabled(),
        ..CoverConsumeResult::default()
    };
    let source_fp = cover_source_fingerprint(source_id);
    loop {
        let task = match persistence::next_staged_cover_task(
            &db::get().lock().unwrap(),
            source_id,
            generation,
        ) {
            Ok(task) => task,
            Err(_) => return CoverConsumeResult::storage_failed(),
        };
        let Some((book_key, task)) = task else {
            break;
        };
        let conn = match db::get().lock() {
            Ok(conn) => conn,
            Err(_) => return CoverConsumeResult::storage_failed(),
        };
        let status = if let Ok(Some(existing)) = crate::remote_scan::cover_store::load_job_on(
            &conn,
            &crate::remote_scan::cover_model::CoverJobKey {
                source_id: source_id.to_string(),
                asset_id: crate::db::library_index_id(&source_fp, &task.logical_path),
                content_revision: task.fingerprint.clone(),
                selection_revision: "default".into(),
                profile: "340x480@1".into(),
            }
            .encode(),
        ) {
            if existing.state == crate::remote_scan::cover_model::CoverJobState::Ready {
                "partial_ready"
            } else {
                "queued"
            }
        } else {
            "queued"
        };
        if persistence::finish_cover_task(
            &conn,
            source_id,
            generation,
            &book_key,
            &task.logical_path,
            &task.session_epoch,
            status,
            None,
        )
        .is_err()
        {
            return CoverConsumeResult::storage_failed();
        }
    }
    wake_remote_cover_worker(source_id.to_string(), job.config.session);
    aggregate
}

#[flutter_rust_bridge::frb(ignore)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct CoverConsumeResult {
    range_unavailable: bool,
    cover_unavailable: bool,
    fetch_paused: bool,
    storage_failed: bool,
}

impl CoverConsumeResult {
    fn storage_failed() -> Self {
        Self {
            storage_failed: true,
            ..Self::default()
        }
    }
}

#[cfg(test)]
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
    initial_listing_json: Option<String>,
) -> std::result::Result<RemoteScanJobDto, String> {
    start_job(
        StartConfig {
            source_type,
            source_id,
            session,
            root_path,
            mode: mode.to_ascii_lowercase(),
            initial_listing_json,
            force_recheck: false,
        },
        false,
    )
}

/// Start a user-requested scan. Manual incremental scans intentionally bypass
/// the automatic 15-minute directory TTL once, while the ordinary start API
/// retains the inexpensive automatic recheck behavior.
pub async fn remote_scan_start_manual(
    source_type: String,
    source_id: String,
    session: u64,
    root_path: String,
    mode: String,
    initial_listing_json: Option<String>,
) -> std::result::Result<RemoteScanJobDto, String> {
    start_job(
        StartConfig {
            source_type,
            source_id,
            session,
            root_path,
            mode: mode.to_ascii_lowercase(),
            initial_listing_json,
            force_recheck: true,
        },
        false,
    )
}

/// RG-B 健壮性修复（A）：应用启动时调用一次 —— 把崩溃/强制退出留下的 `running` 扫描残留
/// 恢复为**中断终态**，使这些源能重新扫描（否则后续 epoch/baseline 证明会永久拒绝启动）。
///
/// 返回被恢复的源数量（0 = 无残留）。启动时内存 job 表为空，因此该操作是安全的；
/// **运行期不要调用**（会把正在进行的扫描误标为中断）。
pub fn remote_scan_recover_interrupted_all() -> std::result::Result<u32, String> {
    let conn = db::get().lock().map_err(|e| e.to_string())?;
    persistence::recover_all_interrupted_scans(&conn).map_err(|e| e.to_string())
}

pub fn remote_scan_status(source_id: String) -> Option<RemoteScanStatusDto> {
    if let Some(job) = jobs().lock().unwrap().get(&source_id) {
        let mut status = job.status.lock().unwrap().clone();
        if let Ok(conn) = db::get().lock() {
            let generation = status.generation;
            refresh_status_counts(&mut status, &conn, generation);
        }
        // P1-F：锁已释放 ⇒ 在此做缓存可用性校验。
        crate::remote_scan::cover_progress::apply_cover_availability(&mut status);
        return Some(status);
    }
    let conn = db::get().lock().ok()?;
    let durable_cover_progress = || {
        let source_type: String = conn
            .query_row(
                "SELECT type FROM book_sources WHERE id=?1",
                [&source_id],
                |row| row.get(0),
            )
            .ok()?;
        let prefix = format!("{source_type}|{source_id}|%");
        durable_cover_progress_on(&conn, &source_id, &prefix)
    };
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
                listing_phase: "pending".into(), directories_checked: 0,
                discovered_books: 0, discovery_complete: false,
                ready_books: 0, active_books: 0, pending_books: 0,
                retry_books: 0, blocked_books: 0, unsupported_books: 0,
                failed_books: 0, view_revision: 0,
                available_books: 0, waiting_books: 0, other_books: 0,
            })
        },
    ).optional().ok().flatten()?;
    // The dependency table is the last *successful* cover projection. Never
    // reuse that old count for a failed/interrupted generation, otherwise a
    // Quark full-scan failure can be rendered as a misleading completed
    // `N/N` progress after the app is restarted.
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
    // Older versions only persisted `Succeeded`/`Incremental` and therefore
    // cannot prove that a complete full-tree baseline ever existed.  Surface
    // that state as a required full scan instead of advertising a misleading
    // incremental completion; the next coordinator trigger will consequently
    // choose `full` and establish the marker above.
    let terminal_success = matches!(
        status.status.trim().to_ascii_lowercase().as_str(),
        "complete" | "completed" | "succeeded"
    );
    if terminal_success && !persistence::has_full_scan_baseline(&conn, &source_id).unwrap_or(false)
    {
        status.status = "degraded".into();
        status.mode = "full".into();
        status.error_code = Some("fullScanRequired".into());
        status.processed = 0;
        status.total = 0;
    }
    let (durable_total, durable_processed) = if status.status == "complete" {
        durable_cover_progress().unwrap_or((0, 0))
    } else {
        (0, 0)
    };
    status.total = durable_total;
    status.processed = durable_processed;
    let generation = status.generation;
    refresh_status_counts(&mut status, &conn, generation);
    // P1-F：先释放 DB 锁，再做缓存/文件系统校验。
    drop(conn);
    crate::remote_scan::cover_progress::apply_cover_availability(&mut status);
    Some(status)
}

/// Count only canonical cover dependencies that still point at a live
/// library-index asset. Alias rows are lookup/cleanup metadata, not extra
/// comics. Joining the current index also prevents an old generation or a
/// verified remote tombstone from inflating post-restart progress.
fn durable_cover_progress_on(
    conn: &rusqlite::Connection,
    source_id: &str,
    book_key_prefix: &str,
) -> Option<(u64, u64)> {
    conn.query_row(
        "SELECT COUNT(DISTINCT dependency.book_key),
                COUNT(DISTINCT CASE WHEN dependency.status IN ('partial_ready','placeholder') THEN dependency.book_key END)
         FROM remote_cover_dependency dependency
         JOIN library_index live_index
           ON live_index.source_id=?2
          AND live_index.path=dependency.dependency_path
          AND live_index.deleted=0
         WHERE dependency.book_key LIKE ?1
           AND dependency.status <> 'cache_alias'",
        params![book_key_prefix, source_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?.max(0) as u64,
                row.get::<_, i64>(1)?.max(0) as u64,
            ))
        },
    )
    .ok()
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
    use crate::remote_scan::model::{normalize_path, RemoteAssetKind};
    use std::io;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    struct CoverAdapter {
        started: mpsc::Sender<()>,
        alias: Option<String>,
    }

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
            let _ = self.started.send(());
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
        fn cache_path(&self, _logical_path: &str) -> Option<String> {
            self.alias.clone()
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
        let worker = std::thread::spawn(move || {
            fetch_safe_cover_partial(
                &CoverAdapter {
                    started: started_tx,
                    alias: None,
                },
                &task,
            )
        });
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

    #[test]
    fn initial_scan_path_preserves_configured_root_and_defaults_to_root() {
        assert_eq!(initial_scan_path("/books"), "/books");
        assert_eq!(initial_scan_path("books"), "/books");
        assert_eq!(initial_scan_path(""), "/");
        assert_eq!(initial_scan_path("/"), "/");
    }

    #[test]
    fn failed_persisted_scan_forces_a_new_full_scan() {
        assert_eq!(
            effective_scan_mode("incremental", None, false, false),
            "full"
        );
        assert_eq!(
            effective_scan_mode("incremental", Some("Failed"), true, false),
            "full"
        );
        assert_eq!(
            effective_scan_mode("incremental", Some("Cancelled"), true, false),
            "full"
        );
        assert_eq!(
            effective_scan_mode("incremental", Some("Succeeded"), true, false),
            "incremental"
        );
        assert_eq!(
            effective_scan_mode("incremental", Some("Running"), true, false),
            "full"
        );
        assert_eq!(
            effective_scan_mode("incremental", Some("Failed"), true, true),
            "full"
        );
    }

    #[test]
    fn successful_legacy_state_without_full_baseline_forces_full_scan() {
        assert_eq!(
            effective_scan_mode("incremental", Some("Succeeded"), false, false),
            "full"
        );
        assert_eq!(
            effective_scan_mode("full", Some("Succeeded"), false, false),
            "full"
        );
    }

    #[test]
    fn durable_progress_counts_live_canonical_dependencies_only() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE remote_cover_dependency(
                book_key TEXT NOT NULL,
                dependency_path TEXT NOT NULL,
                status TEXT NOT NULL
             );
             CREATE TABLE library_index(
                source_id TEXT NOT NULL,
                path TEXT NOT NULL,
                deleted INTEGER NOT NULL
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO remote_cover_dependency VALUES
             ('quark|s1|/live','/live.cbz','partial_ready'),
             ('quark|s1|/live','opaque-fid','cache_alias'),
             ('quark|s1|/gone','/gone.cbz','placeholder'),
             ('webdav|other|/unrelated','/unrelated.cbz','partial_ready')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO library_index VALUES
             ('s1','/live.cbz',0),
             ('s1','/gone.cbz',1),
             ('other','/unrelated.cbz',0)",
            [],
        )
        .unwrap();

        assert_eq!(
            durable_cover_progress_on(&conn, "s1", "quark|s1|%"),
            Some((1, 1))
        );
    }

    #[test]
    fn remote_cover_setting_defaults_on_and_accepts_persisted_boolean_values() {
        assert!(parse_bool_setting(None));
        assert!(parse_bool_setting(Some("true".into())));
        assert!(parse_bool_setting(Some("1".into())));
        assert!(parse_bool_setting(Some("\"true\"".into())));
        assert!(!parse_bool_setting(Some("false".into())));
        assert!(!parse_bool_setting(Some("0".into())));
        assert!(!parse_bool_setting(Some("\"false\"".into())));
        assert!(parse_bool_setting(Some("unexpected".into())));
    }

    #[test]
    fn opaque_provider_cover_cache_keeps_a_provider_id_alias() {
        let (started, _) = mpsc::channel();
        let adapter = CoverAdapter {
            started,
            alias: Some("opaque-fid".into()),
        };
        assert_eq!(
            cover_cache_paths(&adapter, "/series/book.cbz"),
            vec!["/series/book.cbz", "opaque-fid"]
        );
    }

    #[test]
    fn archive_cover_fetch_decodes_a_page_instead_of_returning_zip_bytes() {
        struct MemoryArchive {
            bytes: Vec<u8>,
        }
        impl RemoteProviderAdapter for MemoryArchive {
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
            fn read_range(
                &self,
                _: &str,
                offset: u64,
                length: u64,
            ) -> Result<Vec<u8>, RemoteScanError> {
                let start = offset as usize;
                let end = start.saturating_add(length as usize).min(self.bytes.len());
                if start >= self.bytes.len() {
                    return Ok(Vec::new());
                }
                Ok(self.bytes[start..end].to_vec())
            }
            fn read_file_limited(&self, _: &str, _: u64) -> Result<Vec<u8>, RemoteScanError> {
                Err(RemoteScanError::Unsupported)
            }
            fn normalize_path(&self, path: &str) -> String {
                normalize_path(path)
            }
            fn capabilities(
                &self,
                _: &str,
                _: &str,
            ) -> Result<RemoteCapabilities, RemoteScanError> {
                Ok(RemoteCapabilities {
                    range_read: true,
                    pagination: false,
                })
            }
        }

        let image = image::RgbaImage::from_pixel(3, 4, image::Rgba([20, 40, 60, 255]));
        let mut image_bytes = io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut image_bytes, image::ImageFormat::Png)
            .unwrap();
        let mut archive = io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut archive);
            writer
                .start_file("page1.png", zip::write::SimpleFileOptions::default())
                .unwrap();
            use io::Write;
            writer.write_all(image_bytes.get_ref()).unwrap();
            writer.finish().unwrap();
        }
        let archive_bytes = archive.into_inner();
        let archive_len = archive_bytes.len() as u64;
        let adapter = Arc::new(MemoryArchive {
            bytes: archive_bytes,
        });
        let decoded = fetch_remote_cover_image_with_dimensions(
            adapter,
            "/opaque-fid",
            "book.cbz",
            Some(archive_len),
            "fp",
            RemoteAssetKind::ArchiveFile,
            None,
            REMOTE_COVER_WIDTH,
            REMOTE_COVER_HEIGHT,
            0,
            None,
        )
        .unwrap();
        assert_eq!((decoded.width, decoded.height), (340, 480));
        assert_eq!(decoded.rgba.len(), 340 * 480 * 4);
    }

    #[test]
    fn image_folder_cover_fetch_reads_the_first_image_entry() {
        struct MemoryImage {
            bytes: Vec<u8>,
        }
        impl RemoteProviderAdapter for MemoryImage {
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
            fn read_range(
                &self,
                _: &str,
                offset: u64,
                length: u64,
            ) -> Result<Vec<u8>, RemoteScanError> {
                let start = offset as usize;
                let end = start.saturating_add(length as usize).min(self.bytes.len());
                Ok(self.bytes.get(start..end).unwrap_or_default().to_vec())
            }
            fn read_file_limited(&self, _: &str, _: u64) -> Result<Vec<u8>, RemoteScanError> {
                Err(RemoteScanError::Unsupported)
            }
            fn normalize_path(&self, path: &str) -> String {
                normalize_path(path)
            }
            fn capabilities(
                &self,
                _: &str,
                _: &str,
            ) -> Result<RemoteCapabilities, RemoteScanError> {
                Ok(RemoteCapabilities {
                    range_read: true,
                    pagination: false,
                })
            }
        }

        let image = image::RgbaImage::from_pixel(5, 6, image::Rgba([120, 80, 40, 255]));
        let mut image_bytes = io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut image_bytes, image::ImageFormat::Png)
            .unwrap();
        let image_bytes = image_bytes.into_inner();
        let image_len = image_bytes.len() as u64;
        let decoded = fetch_remote_cover_image_with_dimensions(
            Arc::new(MemoryImage { bytes: image_bytes }),
            "/folder",
            "folder",
            None,
            "folder-fp",
            RemoteAssetKind::ImageFolder,
            Some((
                "/folder/page1.png".into(),
                Some(image_len),
                "page-fp".into(),
            )),
            REMOTE_COVER_WIDTH,
            REMOTE_COVER_HEIGHT,
            0,
            None,
        )
        .unwrap();
        assert_eq!((decoded.width, decoded.height), (340, 480));
    }
}

#[cfg(test)]
mod wake_tests {
    use super::*;
    use crate::remote_scan::cover_model::{CoverJobKey, CoverJobState};
    use crate::remote_scan::cover_state::CoverJobUpsertCause;
    use crate::remote_scan::cover_store;
    use rusqlite::params;

    /// 每个用例使用独立 source id：全局 DB 与 worker registry 都是进程级的。
    /// 需要 `--test-threads=1`（项目全量门禁即如此运行）。
    fn prepare(source_id: &str, session_token: i64) {
        let conn = db::get().lock().unwrap();
        let _ = crate::remote_scan::persistence::migrate(&conn);
        let _ = cover_store::migrate(&conn);
        conn.execute(
            "DELETE FROM remote_cover_job WHERE source_id=?1",
            [source_id],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM remote_scan_epoch WHERE source_id=?1",
            [source_id],
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO book_sources(id,type,name) VALUES(?1,'115','wake-test')",
            [source_id],
        )
        .unwrap();
        bind_source_epoch(&conn, source_id, session_token);
    }

    fn bind_source_epoch(conn: &rusqlite::Connection, source_id: &str, token: i64) {
        conn.execute(
            "INSERT OR REPLACE INTO remote_scan_epoch(
                 source_id,generation,source_fingerprint,root_path,session_epoch,session_token)
             VALUES(?1,1,'fp','/','epoch-1',?2)",
            params![source_id, token],
        )
        .unwrap();
    }

    fn set_session_token(source_id: &str, token: i64) {
        let conn = db::get().lock().unwrap();
        conn.execute(
            "UPDATE remote_scan_epoch SET session_token=?1 WHERE source_id=?2",
            params![token, source_id],
        )
        .unwrap();
    }

    fn key(source_id: &str, asset_id: &str) -> CoverJobKey {
        CoverJobKey {
            source_id: source_id.into(),
            asset_id: asset_id.into(),
            content_revision: "v1".into(),
            selection_revision: "default".into(),
            profile: "340x480@1".into(),
        }
    }

    fn seed_pending(source_id: &str, asset_id: &str) {
        let conn = db::get().lock().unwrap();
        cover_store::upsert_job_on(
            &conn,
            &key(source_id, asset_id),
            CoverJobState::Pending,
            "background",
            10,
            1,
            "epoch-1",
            db::now_ms(),
            CoverJobUpsertCause::Demand,
        )
        .unwrap();
    }

    /// 造一个**未来的** retry_wait 期限：worker 会为它保持存活（睡眠切片 <= 30s），
    /// 这给出一个确定的"worker 正在运行"窗口，而无需长 sleep。
    fn seed_future_retry_wait(source_id: &str, asset_id: &str, in_ms: i64) {
        let conn = db::get().lock().unwrap();
        let job = key(source_id, asset_id);
        cover_store::upsert_job_on(
            &conn,
            &job,
            CoverJobState::RetryWait,
            "background",
            10,
            1,
            "epoch-1",
            db::now_ms(),
            CoverJobUpsertCause::Demand,
        )
        .unwrap();
        conn.execute(
            "UPDATE remote_cover_job SET next_attempt_at=?1 WHERE job_key=?2",
            params![db::now_ms() + in_ms, job.encode()],
        )
        .unwrap();
    }

    fn job_row(source_id: &str, asset_id: &str) -> Option<(String, i64, Option<String>)> {
        let conn = db::get().lock().unwrap();
        conn.query_row(
            "SELECT state,attempt,error_code FROM remote_cover_job WHERE job_key=?1",
            [key(source_id, asset_id).encode()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok()
    }

    fn is_pending(source_id: &str, asset_id: &str) -> bool {
        job_row(source_id, asset_id).map(|(state, _, _)| state == "pending") == Some(true)
    }

    fn keys_for(source_id: &str) -> usize {
        let prefix = format!("{source_id}:");
        cover_workers()
            .lock()
            .unwrap()
            .iter()
            .filter(|existing| existing.starts_with(&prefix))
            .count()
    }

    fn wait_until<F: Fn() -> bool>(predicate: F, timeout_ms: u64) -> bool {
        let started = std::time::Instant::now();
        loop {
            if predicate() {
                return true;
            }
            if started.elapsed().as_millis() >= u128::from(timeout_ms) {
                return predicate();
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// A：worker 正在运行（被未来 retry 期限留住）+ 新的 pending
    /// -> 唤醒正常、不出现第二个 worker、每个 job 恰好被 claim 一次。
    #[test]
    fn source_wake_drains_pending_without_duplicating_the_worker() {
        let source = "wake-a-source";
        prepare(source, 42);
        // 未来 retry 期限必须先就位：它让 worker 消费完 pending 后仍保持存活，
        // 从而给出一个确定的"worker 正在运行"窗口（否则 worker 会立刻退出）。
        seed_future_retry_wait(source, "asset-hold", 1_200);
        seed_pending(source, "asset-1");
        wake_cover_worker_for_source(source);

        assert!(
            wait_until(|| !is_pending(source, "asset-1"), 3_000),
            "the source wake must start a consumer that drains the pending job"
        );
        assert!(
            wait_until(|| keys_for(source) == 1, 1_000),
            "worker must stay alive for the future retry deadline"
        );

        seed_pending(source, "asset-2");
        let observed = keys_for(source);
        wake_cover_worker_for_source(source);
        assert!(
            observed <= 1 && keys_for(source) <= 1,
            "no duplicate worker may be spawned by a second wake"
        );

        assert!(
            wait_until(|| !is_pending(source, "asset-2"), 3_000),
            "the already-running worker must pick up the new pending job"
        );
        assert_eq!(
            job_row(source, "asset-2").unwrap().1,
            1,
            "a job must be claimed exactly once"
        );
    }

    /// B：worker 已退出之后，新的 pending + 可用 session -> 通过 source-level wake 重新启动。
    #[test]
    fn exited_worker_is_restarted_by_a_source_wake() {
        let source = "wake-b-source";
        prepare(source, 42);
        seed_pending(source, "asset-1");
        wake_cover_worker_for_source(source);
        assert!(wait_until(|| !is_pending(source, "asset-1"), 3_000));
        assert!(
            wait_until(|| keys_for(source) == 0, 2_000),
            "a drained worker must exit and release its single-flight key"
        );

        seed_pending(source, "asset-2");
        wake_cover_worker_for_source(source);
        assert!(
            wait_until(|| !is_pending(source, "asset-2"), 3_000),
            "a restarted worker must consume the new pending job"
        );
    }

    /// C：没有可用 session 时 -- 不消费、pending 不丢、不启动任何 worker
    /// （因此没有任何网络 I/O）；session attach 后再唤醒即可被处理。
    #[test]
    fn pending_without_a_session_survives_until_a_session_is_attached() {
        let source = "wake-c-source";
        prepare(source, 0);
        seed_pending(source, "asset-1");

        wake_cover_worker_for_source(source);
        std::thread::sleep(std::time::Duration::from_millis(150));

        assert_eq!(
            keys_for(source),
            0,
            "without a session the coordinator must not spawn a worker"
        );
        assert!(
            is_pending(source, "asset-1"),
            "pending work must stay durable when no session is available"
        );
        assert_eq!(
            job_row(source, "asset-1").unwrap().1,
            0,
            "must not be claimed"
        );

        set_session_token(source, 42);
        wake_cover_worker_for_source(source);
        assert!(
            wait_until(|| !is_pending(source, "asset-1"), 3_000),
            "after the session is attached the durable pending job must be consumed"
        );
    }

    /// D：连续多次 source wake -- single-flight 仍成立，只被消费一次。
    #[test]
    fn repeated_source_wakes_do_not_duplicate_consumption() {
        let source = "wake-d-source";
        prepare(source, 42);
        seed_pending(source, "asset-1");
        for _ in 0..5 {
            wake_cover_worker_for_source(source);
            assert!(
                keys_for(source) <= 1,
                "single-flight must never hold 2 keys"
            );
        }
        assert!(wait_until(|| !is_pending(source, "asset-1"), 3_000));
        assert_eq!(
            job_row(source, "asset-1").unwrap().1,
            1,
            "five wakes must still produce exactly one claim"
        );
    }
}

#[cfg(test)]
mod session_ready_tests {
    use super::*;
    use crate::remote_scan::cover_model::{CoverJobKey, CoverJobState};
    use crate::remote_scan::cover_state::CoverJobUpsertCause;
    use crate::remote_scan::cover_store::{self, LONG_RETRY_DELAY_MS};
    use rusqlite::params;

    const SESSION: u64 = 77;

    fn key(source_id: &str, asset_id: &str) -> CoverJobKey {
        CoverJobKey {
            source_id: source_id.into(),
            asset_id: asset_id.into(),
            content_revision: "v1".into(),
            selection_revision: cover_store::DEFAULT_SELECTION_REVISION.into(),
            profile: cover_store::DEFAULT_COVER_PROFILE.into(),
        }
    }

    /// 建 source + 一个已绑定的 epoch，并放一条 overdue、未消耗的 retryable 终态失败。
    fn prepare_overdue_failure(source_id: &str, generation: i64, session_epoch: &str) {
        let conn = db::get().lock().unwrap();
        let _ = crate::remote_scan::persistence::migrate(&conn);
        let _ = cover_store::migrate(&conn);
        conn.execute(
            "DELETE FROM remote_cover_job WHERE source_id=?1",
            [source_id],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM remote_scan_epoch WHERE source_id=?1",
            [source_id],
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO book_sources(id,type,name) VALUES(?1,'115','ready-test')",
            [source_id],
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO remote_scan_epoch(
                 source_id,generation,source_fingerprint,root_path,session_epoch,session_token)
             VALUES(?1,?2,'fp','/',?3,?4)",
            params![
                source_id,
                generation,
                session_epoch,
                i64::try_from(SESSION).unwrap()
            ],
        )
        .unwrap();
        let job = key(source_id, "asset");
        cover_store::upsert_job_on(
            &conn,
            &job,
            CoverJobState::Failed,
            "background",
            10,
            generation,
            session_epoch,
            0,
            CoverJobUpsertCause::Demand,
        )
        .unwrap();
        // 一个已经过期、尚未消耗的 failure episode。
        conn.execute(
            "UPDATE remote_cover_job
                SET long_retry_not_before=?1,long_retry_consumed=0,long_retry_pending=0
              WHERE job_key=?2",
            params![db::now_ms() - LONG_RETRY_DELAY_MS, job.encode()],
        )
        .unwrap();
    }

    fn job_row(source_id: &str, asset_id: &str) -> Option<(String, i64, i64, i64, Option<i64>)> {
        let conn = db::get().lock().unwrap();
        conn.query_row(
            "SELECT state,attempt,long_retry_consumed,long_retry_pending,long_retry_not_before
               FROM remote_cover_job WHERE job_key=?1",
            [key(source_id, asset_id).encode()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .ok()
    }

    fn wait_until<F: Fn() -> bool>(predicate: F, timeout_ms: u64) -> bool {
        let started = std::time::Instant::now();
        loop {
            if predicate() {
                return true;
            }
            if started.elapsed().as_millis() >= u128::from(timeout_ms) {
                return predicate();
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn notify(source_id: &str) -> RemoteCoverReconcileDto {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("tokio runtime");
        runtime
            .block_on(notify_source_session_ready(source_id.to_string(), SESSION))
            .expect("lifecycle notification must not fail")
    }

    /// 给该 source 补一个 library 缺口（用于验证补齐器经 lifecycle 事件生效）。
    fn seed_gap(source_id: &str, path: &str) {
        let conn = db::get().lock().unwrap();
        let asset_id = crate::db::library_index_id("fp", path);
        conn.execute(
            "INSERT OR REPLACE INTO library_index(
                 id,source_id,name,path,entry_type,asset_kind,content_fingerprint,
                 listing_complete,deleted,updated_at)
             VALUES(?1,?2,'book.cbz',?3,'file','ArchiveFile','fp',1,0,1)",
            params![asset_id, source_id, path],
        )
        .unwrap();
    }

    /// L1：新有效 session 建立 → overdue retryable failed 被推进 → worker 可消费。
    #[test]
    fn l1_session_creation_promotes_overdue_compensation_and_wakes_a_consumer() {
        let source = "ready-l1-source";
        prepare_overdue_failure(source, 3, "epoch-l1");

        let report = notify(source);
        assert!(
            report.binding_available,
            "the session binding must be recognized"
        );
        assert_eq!(report.compensation_promoted, 1);
        assert!(report.claimable, "promotion must report claimable work");

        // worker 必须真的把它领走（该源没有 route，因此是零网络的 route-missing 分支）。
        assert!(
            wait_until(
                || job_row(source, "asset").map(|r| r.0 != "pending") == Some(true),
                3_000
            ),
            "the lifecycle event must leave a live consumer behind"
        );
        let (_, attempt, consumed, pending, _) = job_row(source, "asset").unwrap();
        assert_eq!(attempt, 1, "claimed exactly once");
        assert_eq!(
            consumed, 1,
            "the claim is what consumes the compensation budget"
        );
        assert_eq!(
            pending, 0,
            "the compensation-pending marker must be cleared"
        );
    }

    /// L2：只是重复读取同一个已有 session（或重复通知）→ 不重复 job / 不重复 claim / 不多 consumer。
    #[test]
    fn l2_repeated_identical_notifications_do_not_duplicate_anything() {
        let source = "ready-l2-source";
        prepare_overdue_failure(source, 3, "epoch-l2");
        seed_gap(source, "/l2-book.cbz");

        let first = notify(source);
        assert_eq!(first.compensation_promoted, 1);
        assert_eq!(first.jobs_created, 1);
        assert!(wait_until(
            || job_row(source, "asset").map(|r| r.0 != "pending") == Some(true),
            3_000
        ));

        // 连续重复通知：不得重复生成 job、不得重复消耗、不得多起 consumer。
        for _ in 0..4 {
            let again = notify(source);
            assert_eq!(
                again.compensation_promoted, 0,
                "an open episode must not re-promote"
            );
            assert_eq!(again.jobs_created, 0, "no duplicate job may be created");
        }
        let count: i64 = db::get()
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM remote_cover_job WHERE source_id=?1",
                [source],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 2,
            "one promoted job + one replenished gap, never more"
        );
        assert_eq!(
            job_row(source, "asset").map(|r| r.1),
            Some(1),
            "no extra claim may happen"
        );
    }

    /// L3：session renewal —— 已完成的 generation 能绑定到当前有效 session，且补偿可恢复。
    #[test]
    fn l3_a_renewed_session_rebinds_a_completed_generation_and_recovers_compensation() {
        let source = "ready-l3-source";
        // 关键差异：epoch 里**没有**当前 session 的绑定（session_token=0，代表旧会话已失效）。
        {
            let conn = db::get().lock().unwrap();
            let _ = crate::remote_scan::persistence::migrate(&conn);
            let _ = cover_store::migrate(&conn);
            conn.execute("DELETE FROM remote_cover_job WHERE source_id=?1", [source])
                .unwrap();
            conn.execute("DELETE FROM remote_scan_epoch WHERE source_id=?1", [source])
                .unwrap();
            conn.execute("DELETE FROM remote_scan_state WHERE source_id=?1", [source])
                .unwrap();
            // 重绑的前置条件：book_sources 的身份必须与 epoch 的身份一致。
            conn.execute(
                "INSERT OR REPLACE INTO book_sources(id,type,name,fingerprint,path)
                 VALUES(?1,'115','ready-test','fp','/')",
                [source],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO remote_scan_epoch(
                     source_id,generation,source_fingerprint,root_path,session_epoch,session_token)
                 VALUES(?1,4,'fp','/','epoch-old',0)",
                [source],
            )
            .unwrap();
            // 该 generation 已经成功完成 —— 只有 Succeeded 代际允许被新 session 重绑。
            conn.execute(
                "INSERT OR REPLACE INTO remote_scan_state(source_id,status,mode,generation)
                 VALUES(?1,'Succeeded','Snapshot',4)",
                [source],
            )
            .unwrap();
        }
        // 载入一条 overdue 的 failed（挂在旧 epoch 上）。
        {
            let conn = db::get().lock().unwrap();
            let job = key(source, "asset");
            cover_store::upsert_job_on(
                &conn,
                &job,
                CoverJobState::Failed,
                "background",
                10,
                4,
                "epoch-old",
                0,
                CoverJobUpsertCause::Demand,
            )
            .unwrap();
            conn.execute(
                "UPDATE remote_cover_job
                    SET long_retry_not_before=?1,long_retry_consumed=0,long_retry_pending=0
                  WHERE job_key=?2",
                params![db::now_ms() - LONG_RETRY_DELAY_MS, job.encode()],
            )
            .unwrap();
        }

        // 续期/新会话就绪：必须重绑并恢复补偿。
        let report = notify(source);
        assert!(
            report.binding_available,
            "a completed generation must be rebindable to the renewed session"
        );
        assert_eq!(report.compensation_promoted, 1);
        assert!(wait_until(
            || job_row(source, "asset").map(|r| r.0 != "pending") == Some(true),
            3_000
        ));
        assert_eq!(job_row(source, "asset").map(|r| r.2), Some(1));
    }

    /// L4：相邻/重复事件下长期补偿最多消费一次，且不会把已消耗的 failed 重新变 pending、
    ///     也不会多 spawn worker。
    #[test]
    fn l4_adjacent_events_consume_the_long_retry_budget_at_most_once() {
        let source = "ready-l4-source";
        prepare_overdue_failure(source, 3, "epoch-l4");

        // 第一次通知：推进 + 消费。
        let first = notify(source);
        assert_eq!(first.compensation_promoted, 1);
        assert!(wait_until(
            || job_row(source, "asset").map(|r| r.0 != "pending") == Some(true),
            3_000
        ));
        let (_, attempt, consumed, _, not_before) = job_row(source, "asset").unwrap();
        assert_eq!(attempt, 1);
        assert_eq!(consumed, 1);
        assert!(not_before.is_some(), "the exhausted episode stays recorded");

        // 之后无论来多少次事件：都不得再自动补偿这一条。
        for _ in 0..3 {
            let again = notify(source);
            assert_eq!(again.compensation_promoted, 0);
        }
        assert_eq!(
            job_row(source, "asset").map(|r| r.1),
            Some(1),
            "still exactly one claim"
        );

        // 且不得出现第二个 worker 键。
        let prefix = format!("{source}:");
        let workers = cover_workers()
            .lock()
            .unwrap()
            .iter()
            .filter(|existing| existing.starts_with(&prefix))
            .count();
        assert!(workers <= 1, "single-flight must hold");
    }
}

#[cfg(test)]
mod rg_a_cover_failure_decision_tests {
    //! RG-A / A-3：退避**决策数学**契约（纯函数，不涉及 worker orchestration）。
    use super::*;
    use crate::remote_scan::cover_model::CoverJobState;

    fn decision(error: &RemoteScanError, attempt: i64) -> super::CoverFailureDecision {
        super::cover_job_failure_decision(error, attempt)
    }

    #[test]
    fn rg_a_decision_rate_limited_prefers_server_retry_after() {
        let d = decision(
            &RemoteScanError::RateLimited {
                retry_after_ms: Some(7_000),
            },
            2,
        );
        assert_eq!(d.state, CoverJobState::RetryWait);
        assert_eq!(d.retry_after_ms, Some(7_000), "server value must win over 4s");
    }

    #[test]
    fn rg_a_decision_rate_limited_without_retry_after_uses_exponential() {
        for (attempt, expected_ms) in [(0_i64, 1_000_u64), (1, 2_000), (2, 4_000)] {
            let d = decision(
                &RemoteScanError::RateLimited {
                    retry_after_ms: None,
                },
                attempt,
            );
            assert_eq!(d.state, CoverJobState::RetryWait);
            assert_eq!(d.retry_after_ms, Some(expected_ms), "attempt {attempt}");
        }
    }

    #[test]
    fn rg_a_decision_transient_uses_the_same_bounded_exponential() {
        let d = decision(&RemoteScanError::TransientNetwork("x".into()), 1);
        assert_eq!(d.state, CoverJobState::RetryWait);
        assert_eq!(d.retry_after_ms, Some(2_000));
        let r = decision(
            &RemoteScanError::RateLimited {
                retry_after_ms: None,
            },
            1,
        );
        assert_eq!(d.retry_after_ms, r.retry_after_ms);
    }

    #[test]
    fn rg_a_decision_non_retryable_never_enters_retry_bucket() {
        let forbidden = decision(&RemoteScanError::Forbidden, 0);
        assert_eq!(forbidden.state, CoverJobState::Blocked);
        assert_eq!(forbidden.retry_after_ms, None);

        let unauthorized = decision(&RemoteScanError::Unauthorized, 0);
        assert_eq!(unauthorized.state, CoverJobState::Blocked);
        assert_eq!(unauthorized.retry_after_ms, None);

        let method_not_allowed = decision(
            &RemoteScanError::HttpStatus {
                stage: "range_probe".into(),
                status: 405,
            },
            0,
        );
        assert_eq!(method_not_allowed.state, CoverJobState::Failed);
        assert_eq!(method_not_allowed.retry_after_ms, None);

        for state in [
            forbidden.state,
            unauthorized.state,
            method_not_allowed.state,
        ] {
            assert_ne!(state, CoverJobState::RetryWait);
        }
    }

    /// 边界 5：退避上界。
    ///
    /// **RG-A 实测发现**：在 `attempt < 3` 的既有阈值下，**可达**延迟只有 1s / 2s / 4s；
    /// 表达式中的 `clamp(0, 10)`（cap = 2^10 s）是**防御性上界**，当前阈值下**不可达**
    /// —— 除非未来放宽 attempt 阈值（RG-A **不**修改它）。
    ///
    /// 因此本用例钉住两件真实事实：
    /// 1. 可达延迟始终落在防御性上界之内；
    /// 2. `attempt >= 3` 一律终态（无延迟），cap 根本不会被触及。
    #[test]
    fn rg_a_decision_reachable_delays_stay_within_the_defensive_cap() {
        for attempt in [0_i64, 1, 2] {
            let d = decision(&RemoteScanError::TransientNetwork("x".into()), attempt);
            let delay = d
                .retry_after_ms
                .expect("reachable attempts must carry a delay");
            assert!(
                delay <= 1024 * 1_000,
                "delay {delay} must stay within the defensive cap"
            );
        }
        for attempt in [3_i64, 10, 11, 64] {
            let d = decision(&RemoteScanError::TransientNetwork("x".into()), attempt);
            assert_eq!(
                d.retry_after_ms, None,
                "attempt {attempt} must be terminal (cap must not be reachable)"
            );
            assert_eq!(d.state, CoverJobState::Failed);
        }
    }

    #[test]
    fn rg_a_decision_attempt_three_is_terminal_and_unchanged() {
        let transient = decision(&RemoteScanError::TransientNetwork("x".into()), 3);
        assert_eq!(transient.state, CoverJobState::Failed);
        assert_eq!(transient.retry_after_ms, None);

        let limited = decision(
            &RemoteScanError::RateLimited {
                retry_after_ms: Some(9_000),
            },
            3,
        );
        assert_eq!(
            limited.state,
            CoverJobState::Failed,
            "even a server Retry-After must not extend the short budget"
        );
        assert_eq!(limited.retry_after_ms, None);
    }

    #[test]
    fn rg_a_decision_other_categories_map_as_before() {
        assert_eq!(
            decision(&RemoteScanError::RangeUnavailable, 0).state,
            CoverJobState::Unsupported
        );
        assert_eq!(
            decision(&RemoteScanError::Unsupported, 0).state,
            CoverJobState::Unsupported
        );
        assert_eq!(
            decision(&RemoteScanError::Cancelled, 0).state,
            CoverJobState::Cancelled
        );
    }

    #[test]
    fn rg_a_decision_is_pure_and_deterministic() {
        let error = RemoteScanError::RateLimited {
            retry_after_ms: None,
        };
        let a = decision(&error, 2);
        let b = decision(&error, 2);
        assert_eq!(a.state, b.state);
        assert_eq!(a.error_code, b.error_code);
        assert_eq!(a.retry_after_ms, b.retry_after_ms);
        let negative = decision(&error, -5);
        assert_eq!(negative.retry_after_ms, Some(1_000));
    }
}
