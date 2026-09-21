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

/// 属于**部署 / 策略上限**类、修好之后本应能补上的封面原因码（可获得 6h 长期补偿）。
/// 与 [`COVER_REASONS`] 的包含关系有单测守住，新增码时不会静默漂移。
const COVER_RETRYABLE_REASONS: [&str; 6] = [
    COVER_REASON_NATIVE_LIB_MISSING,
    COVER_REASON_BYTES_LIMIT,
    COVER_REASON_PDF_BYTES_LIMIT,
    COVER_REASON_PIXELS,
    COVER_REASON_PARTIAL_DECODE,
    // 预算中止的**根因是"打开归档太贵"**（`zip` 按条目读 local header）。
    // 第 62 轮落地封面快通道后，这类失败再试一次就能成（实测 2GB CBZ：
    // 192 次读被截断 → 8 次读 9 秒拿到封面），所以从"终态"改为"可重试"。
    COVER_REASON_READ_BUDGET,
];

/// 账号 / 会话级的 provider 子原因：会话恢复或限流解除后应当重试（进 long retry）。
/// 这些码由 `provider_failure_code` 产出（有单测守住"确实能产出"）。
const PROVIDER_RETRYABLE_CODES: [&str; 4] = [
    "provider:rateLimited",
    "provider:timeout",
    "provider:unauthorized",
    "provider:forbidden",
];

/// 长期补偿的 retryable 判定。
///
/// 基础集合不变（短退避那一个错误集合 `TransientNetwork | RateLimited`：短预算耗尽
/// `attempt >= 3` 后落入 `_ => Failed`，获得一次 6h 长期补偿）。② 起额外放行两类
/// **可修复的失败**——否则它们会被永久结案、永远不自愈：
///
/// * `Provider(message)` 归类为 rateLimited / timeout / unauthorized / forbidden
///   ⇒ 账号级或会话级原因，换个会话就该再试；
/// * `MalformedResponse(code)` 里属于**部署或策略上限**的码（原生库缺失、字节上限、
///   像素守卫、窗口截断）⇒ 用户修好部署（③-1 实测：Debug 构建缺 `pdfium.dll` 时
///   267 本 PDF 封面全灭）或调大上限后，这批封面应当能自动补上。
///
/// 仍然**只做固定枚举判定**，不做自由文本推断；`provider_failure_code` 本身也是
/// 枚举化分类（① 的安全子原因）。
fn long_retry_is_retryable(error: &RemoteScanError) -> bool {
    match error {
        RemoteScanError::TransientNetwork(_) | RemoteScanError::RateLimited { .. } => true,
        RemoteScanError::Provider(message) => {
            PROVIDER_RETRYABLE_CODES.contains(&provider_failure_code(message))
        }
        RemoteScanError::MalformedResponse(code) => {
            COVER_RETRYABLE_REASONS.contains(&code.as_str())
        }
        _ => false,
    }
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
                // 档位取**用户当前设置**（与卡片读状态用的是同一份映射）——
                // 写死常量会让"缺口补齐"永远补不到卡片看的那个档位（真机实测：
                // 设置 low 时每次会话事件造 64 条 340 档任务 + bump revision ⇒ 封面文案抖动）。
                let profile = cover_quality_profile_on(&conn);
                let gap = reconcile_missing_covers_for_source_on(
                    &conn,
                    &source_id,
                    session,
                    now,
                    ReconcileBudget {
                        max_jobs: remaining,
                        ..budget
                    },
                    &profile,
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
    // 诊断通道（第 55 轮）：让"自愈/重挂"在日志里可见（只有确实做了事才写一行，
    // 避免每次 UI 事件都刷屏）。`compensation_promoted` 含本轮新增的
    // "解析类终态失败重挂"（见 `cover_store` 的 (1b) 桶）。
    if report.compensation_promoted > 0 || report.blocker_cleared > 0 || report.jobs_created > 0 {
        crate::remote_scan::diag::note(&format!(
            "cover_reconcile source={} promoted={} cleared={} created={} truncated={} claimable={}",
            source_id,
            report.compensation_promoted,
            report.blocker_cleared,
            report.jobs_created,
            report.truncated,
            report.claimable
        ));
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

/// 扫描/预览创建**封面任务**时使用的 profile：跟随用户设置 `coverQuality`。
///
/// 第 79 轮续7（用户确认）：过去这里写死 `340x480@1`，而卡片按设置取图（"低" =
/// `170x240@1`）⇒ 真机实测 586 本 PDF 里 340 档 ready 142 本、170 档只有 **42** 本 ——
/// 扫描辛苦抓回来的封面，卡片一张都用不上（要么占位、要么靠跨档回退凑）。
///
/// 档位映射与 Dart `CoverQuality.size`（`app/lib/store/models.dart`）保持一致；
/// 设置缺失/未知时回落到 `DEFAULT_COVER_PROFILE`（= 中档 340x480@1，与历史行为一致）。
pub(crate) fn cover_quality_profile_on(conn: &rusqlite::Connection) -> String {
    let quality: Option<String> = conn
        .query_row(
            "SELECT value FROM app_settings WHERE key='coverQuality' AND deleted=0",
            [],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten();
    match quality.as_deref().map(str::trim) {
        Some("low") => "170x240@1".to_string(),
        Some("high") => "510x720@1".to_string(),
        _ => crate::remote_scan::cover_store::DEFAULT_COVER_PROFILE.to_string(),
    }
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
    // 优先用 preview 路由（它可能是**最新**的路由：逻辑路径不变而不透明 provider id 已刷新）。
    //
    // **不能要求 `remote_scan_state.status='Running'`**：真实库审查（第 55 轮）实测——
    // 扫描收尾的那一刻（12:56:11 发布索引 vs 12:56:11–12:56:29 的封面失败）两条跳
    // 同时不可用：preview 因"不再 Running"被跳过、而 library_index 尚未落行
    // ⇒ 225 个封面被判 `notFound` 终态、再无重试。这里改为"最新代际 + 身份守卫"，
    // 身份守卫（`session_epoch<>''` + `source_fingerprint=book_sources.fingerprint`）
    // 保证不会读到别的书源/别的会话的残留。
    let preview = conn
        .query_row(
            "SELECT p.logical_path,
                    COALESCE((SELECT type FROM book_sources WHERE id=?1),'unknown'),
                    p.provider_file_id
             FROM remote_scan_preview p
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

/// 阅读活跃期：后台封面让路的判定窗口与让路后的重试间隔（第 79 轮，真机驱动）。
const COVER_BACKGROUND_YIELD_IDLE_MS: i64 = 20_000;
const COVER_BACKGROUND_YIELD_SLEEP_MS: u64 = 2_000;

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
        // 第 79 轮真机结论：手机上门控 4 请求/秒、2 并发，几百个**后台**封面会把带宽与
        // CDN 连接吃满，前台翻页只能夹在中间 ⇒ 用户看到"翻页一直转圈"。
        // 阅读活跃期（最近 COVER_BACKGROUND_YIELD_IDLE_MS 内有过前台网络取页）只让
        // **visible**（用户正看着的封面）继续，background 释放租约稍后重试 ——
        // 任务不丢，只是让路。
        if job.demand_kind == "background"
            && crate::reader::foreground_read_idle_ms()
                .is_some_and(|idle| idle < COVER_BACKGROUND_YIELD_IDLE_MS)
        {
            if let Ok(conn) = db::get().lock() {
                let _ = crate::remote_scan::cover_store::release_job_lease_on(
                    &conn,
                    &job.key,
                    &owner,
                    db::now_ms(),
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(
                COVER_BACKGROUND_YIELD_SLEEP_MS,
            ));
            continue;
        }
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
                        CoverFetchTrace {
                            source_id,
                            asset_id: &job.key.asset_id,
                        },
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
                // 诊断通道（第 55 轮）：封面失败此前**只落库、不进日志**，
                // 导致应用连跑 2.5 小时、225 个失败而 scan_diag.log 一行没有。
                // 只写安全枚举码 + 源/代际/尝试次数 + 资产短标签（脱敏见 `diag`）。
                crate::remote_scan::diag::note(&format!(
                    "cover_fail source={} gen={} attempt={} state={} code={} asset={}",
                    job.key.source_id,
                    job.generation,
                    job.attempt,
                    state.as_str(),
                    code.as_deref().unwrap_or("-"),
                    crate::remote_scan::diag::safe_asset_label(&job.key.asset_id)
                ));
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
            profile: cover_quality_profile_on(&conn),
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
    // 扫描**完成**是"资产从此可解析"的时刻，必须在这里补一次封面 reconcile：
    // `notify_source_session_ready` 只接受**已完成**代际，若 session-ready 事件恰好
    // 落在扫描中途（实测：用户点全量扫描的同一秒触发了启动会话预热），那次事件会
    // 静默跳过（拿不到可信绑定）⇒ 遗留失败不会被自愈重挂。
    // 这里直接读该代际的 session_token，不依赖内存里的会话。
    if status.status == "complete" {
        if let Ok(conn) = db::get().lock() {
            let token: Option<i64> = conn
                .query_row(
                    "SELECT session_token FROM remote_scan_epoch
                      WHERE source_id=?1 AND generation=?2",
                    rusqlite::params![status.source_id, status.generation],
                    |row| row.get(0),
                )
                .ok();
            if let Some(token) = token.filter(|value| *value > 0) {
                if let Ok(report) =
                    crate::remote_scan::cover_store::reconcile_cover_compensation_for_source_on(
                        &conn,
                        &status.source_id,
                        token as u64,
                        crate::db::now_ms(),
                        crate::remote_scan::cover_store::ReconcileBudget::default(),
                    )
                {
                    if report.compensation_promoted > 0
                        || report.blocker_cleared > 0
                        || report.jobs_created > 0
                    {
                        crate::remote_scan::diag::note(&format!(
                            "cover_reconcile source={} trigger=scan_complete promoted={} cleared={} created={} truncated={}",
                            status.source_id,
                            report.compensation_promoted,
                            report.blocker_cleared,
                            report.jobs_created,
                            report.truncated
                        ));
                    }
                }
            }
        }
    }
    // 诊断通道（第 55 轮）：扫描终态此前不进日志，问题只能靠翻库。
    crate::remote_scan::diag::note(&format!(
        "scan_terminal source={} status={} mode={} gen={} checked={} error={}",
        status.source_id,
        status.status,
        status.mode,
        status.generation,
        status.processed,
        status.error_code.as_deref().unwrap_or("-")
    ));
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
        RemoteScanError::MalformedResponse(code) => safe_malformed_code(code.as_str()),
        RemoteScanError::Cancelled => "cancelled",
        RemoteScanError::Unsupported => "unsupported",
        RemoteScanError::Io(_) => "storage",
        RemoteScanError::Provider(message) => provider_failure_code(message),
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
/// 封面窗口阶梯的第一档：长图/超大单图只需要最上面的一条带，先按最小窗口取。
/// 旧实现在 32MB 处直接返回"封面不可得"（`cover_size_limit`），该放弃分支已删除：
/// 现在只有"超过硬上限"才拒绝，且必须给出具体原因。
const COVER_HEAD_BYTES: u64 = 24 * 1024 * 1024;
/// 第二档放大：第一档解码不成功（缓冲区被窗口截断）时续读到这么大。
const COVER_HEAD_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// 硬上限：超过它就不再尝试读取。128MB 是"覆盖面 × 手机峰值内存"的折中，
/// 也是唯一需要按设备情况调整的旋钮。
const COVER_FETCH_LIMIT_BYTES: u64 = 128 * 1024 * 1024;
/// PDF 封面的文件大小上限。
///
/// **第 79 轮续8（独立评审 I-1）**：这条上限的前提已经失效 —— 它写于"PDF 必须先整包
/// 交给 pdfium"的时代（原注释：不做"下载几百 MB 换一张封面"）。改惰性按需读之后，
/// 一枚 PDF 封面实测只花 **约 292 KB**（`1.pdf` 57.5 MB：打开 8 次读/25 KB + 首页
/// 3 次读/267 KB），真正的成本护栏是 `COVER_READ_BUDGET_*`。留着 128 MB 的硬拒，
/// 代价是用户库里最大的三本（`9.pdf` 204 MB / `10.pdf` 180 MB / `8.pdf` 164 MB）
/// **一枚封面都拿不到**（`cover_pdf_bytes_limit`，终态）—— 恰好是本轮动机点名的文件。
/// 因此放宽到 512 MB：只挡真正的异常巨物，实际取数由读取预算兜底。
const COVER_PDF_MAX_BYTES: u64 = 512 * 1024 * 1024;
/// 快通道允许的单页字节上限（超过就交回常规路径；正常漫画页远小于此）。
const COVER_FAST_PAGE_MAX_BYTES: usize = 64 * 1024 * 1024;
/// 解码像素守卫：窗口放大后仍可能碰到"小体积 → 巨大位图"的解压炸弹。
/// RGBA 峰值 ≈ 像素数 × 4（64MP ≈ 256MB），超过即给具体原因而不是让设备 OOM。
const COVER_MAX_PIXELS: u64 = 64_000_000;
/// 高宽比超过它的源图/页面视为长条：封面只取顶部一条，不再取中间那条。
const COVER_LONG_STRIP_ASPECT: f64 = 3.0;
/// 封面失败的安全子原因（唯一真源）：这些码会进入 `remote_cover_job.error_code`
/// 与本地诊断日志，因此只允许本文件的常量；provider / 解码器原文一律不得出现。
const COVER_REASON_SIZE_MISSING: &str = "cover_size_missing";
const COVER_REASON_BYTES_LIMIT: &str = "cover_bytes_limit";
const COVER_REASON_PDF_BYTES_LIMIT: &str = "cover_pdf_bytes_limit";
const COVER_REASON_PARTIAL_DECODE: &str = "cover_partial_decode_failed";
const COVER_REASON_DECODE: &str = "cover_decode_failed";
const COVER_REASON_PIXELS: &str = "cover_pixels_too_large";
/// ③-1 真实数据复测新增：归档/PDF 的"打不开"必须再分三层，否则部署问题会被
/// 当成"文件坏/封面不可得"而永久结案（实测：开发机 Debug 构建缺 `pdfium.dll` 时，
/// 40/40 个失败样本全部落进同一码，无法自证）。
const COVER_REASON_NATIVE_LIB_MISSING: &str = "cover_native_lib_missing";
const COVER_REASON_DOCUMENT_OPEN: &str = "cover_document_open_failed";
const COVER_REASON_PAGE_RENDER: &str = "cover_page_render_failed";
/// 单封面读取超出预算（归档打开成本过高：`zip` 对每个条目都要读一次 local
/// header 做校验）。**可重试**：第 62 轮的封面快通道把这条路径的请求数降到常数级，
/// 同一个文件再试一次可以成功（实测 2GB CBZ：192 次读被截断 → 8 次读 9 秒成功）。
const COVER_REASON_READ_BUDGET: &str = "cover_read_budget_exceeded";
const COVER_REASONS: [&str; 10] = [
    COVER_REASON_SIZE_MISSING,
    COVER_REASON_BYTES_LIMIT,
    COVER_REASON_PDF_BYTES_LIMIT,
    COVER_REASON_PARTIAL_DECODE,
    COVER_REASON_DECODE,
    COVER_REASON_PIXELS,
    COVER_REASON_NATIVE_LIB_MISSING,
    COVER_REASON_DOCUMENT_OPEN,
    COVER_REASON_PAGE_RENDER,
    COVER_REASON_READ_BUDGET,
];

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

/// 单次封面抓取允许从远端读取的**总字节预算**（归档/PDF 路径）。
///
/// 为什么必须有（第 60 轮实测）：115 上一本 2.08GB 的“`.zip`”让归档打开逻辑在
/// **文件尾部顺序扫描了 233MB**（EOCD 定位不到就一直往回找），单枚封面吃掉
/// 233MB 流量 + 数分钟；而后台 worker 是**单线程** ⇒ 一枚封面就把整条队列堵死
/// （`ready` 长时间为 0，用户体感"很慢"）。
/// 预算让这类病态归档**快速失败并给出具体码**（`cover_read_budget_exceeded`），
/// 而不是拖着整条队列。
const COVER_READ_BUDGET_BYTES: u64 = 64 * 1024 * 1024;
/// 单次封面抓取的**远端读取次数**上限。
/// 为什么字节预算不够：实测这些读是 **16KB 级**，115 CDN 单次往返 ~243ms，
/// 24MB 预算要 1500+ 次 ⇒ 仍是 6 分钟。次数上限才能界定时延。
///
/// 第 81 轮续（真机，MOBI/PDF 惰性读之后）：归档类封面现在走**惰性文档读**，
/// 取数被精确到"首页那一条记录/那一张图"。但**单条记录本身可以很大**
/// （真机 33.9/143/146/180 MB 的 MOBI 全部 `cover_read_budget_exceeded`，
/// 而 ≤79 MB 的成功 ⇒ 代价与整本大小成正比）：在 4 请求/秒的门控下，
/// 读一条 5–15 MB 的记录要几十次请求、十几秒 ⇒ 旧的 192 次/15 s 会把**可救**的封面判死。
/// 因此放宽到 384 次 / 30 s / 64 MB：仍然有界（不会回到分钟级转圈），
/// 但给"单条大记录"留出空间。真正不可救的仍会快速失败并给出具体码。
const COVER_READ_BUDGET_READS: u64 = 384;
/// 单次封面抓取的**挂钟**上限（兜底：远端变慢时也要让 worker 走下一个 job）。
///
/// 第 79 轮续 3（真机）：原值 45 s 在**手机 + 4 请求/秒门控**下意味着"一张卡片转圈
/// 45 秒以上"（`attempt=2` 再翻倍），用户看到的是"直接一直转圈"。降到 15 s：
/// 不可救的重封面**快速失败**并给出 `cover_read_budget_exceeded`，不再拖住整条队列与
/// 共享门控。字节预算（24MB）保持不变 —— 不牺牲"重但可救"的封面成功率。
const COVER_READ_BUDGET_MS: u128 = 30_000;

/// A bounded random-access source backed by the provider adapter. ZIP/CBZ
/// parsing can therefore read the tail directory and the first page without
/// downloading the whole book or retaining file-sized buffers.
#[flutter_rust_bridge::frb(ignore)]
struct AdapterByteSource {
    adapter: Arc<dyn crate::remote_scan::adapter::RemoteProviderAdapter>,
    path: String,
    length: u64,
    /// 本次抓取已从远端读取的字节（预算见 [`COVER_READ_BUDGET_BYTES`]）。
    read_bytes: std::sync::atomic::AtomicU64,
    /// 本次抓取已发生的远端读取次数（预算见 [`COVER_READ_BUDGET_READS`]）。
    read_calls: std::sync::atomic::AtomicU64,
    /// 抓取起点（预算见 [`COVER_READ_BUDGET_MS`]）。
    started_at: std::time::Instant,
    /// 预算上限（常量注入，便于单测用极小预算验证）。
    read_budget: u64,
}

impl AdapterByteSource {
    fn new(
        adapter: Arc<dyn crate::remote_scan::adapter::RemoteProviderAdapter>,
        path: String,
        length: u64,
    ) -> Self {
        Self::with_budget(adapter, path, length, COVER_READ_BUDGET_BYTES)
    }

    fn with_budget(
        adapter: Arc<dyn crate::remote_scan::adapter::RemoteProviderAdapter>,
        path: String,
        length: u64,
        read_budget: u64,
    ) -> Self {
        Self {
            adapter,
            path,
            length,
            read_bytes: std::sync::atomic::AtomicU64::new(0),
            read_calls: std::sync::atomic::AtomicU64::new(0),
            started_at: std::time::Instant::now(),
            read_budget,
        }
    }
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
        // 预算：病态归档（例如尾部 233MB 的 EOCD 扫描）必须快速失败，
        // 具体失败码由 `cover_open_reason` 从这段稳定文案映射出来。
        let used = self
            .read_bytes
            .load(std::sync::atomic::Ordering::Relaxed);
        let calls = self
            .read_calls
            .load(std::sync::atomic::Ordering::Relaxed);
        let elapsed_ms = self.started_at.elapsed().as_millis();
        if used.saturating_add(requested as u64) > self.read_budget
            || calls >= COVER_READ_BUDGET_READS
            || elapsed_ms > COVER_READ_BUDGET_MS
        {
            let detail = format!(
                "{used}/{} bytes, {calls}/{} reads, {elapsed_ms}ms",
                self.read_budget, COVER_READ_BUDGET_READS
            );
            // 第 81 轮续5：预算失败必须自证"**哪一条**先超"（字节/次数/时间）。
            // 为什么就地记录：上游 `open_document` 的错误路径会把这段文案**映射成具体码**
            // （`cover_open_reason` → `cover_read_budget_exceeded`），到 `cover.fetch`
            // 那个 span 里数字已经丢了 —— 我第一版就是在那里判 `contains` 而永远不成立 ✗。
            // 就地写诊断（scan_diag.log，桌面与手机都能读）+ 一条 perf 事件（无 provider 原文）。
            crate::remote_scan::diag::note(&format!(
                "cover_budget detail={detail} asset={}",
                crate::remote_scan::diag::safe_asset_label(&self.path)
            ));
            if crate::perf::enabled() {
                let mut fields = serde_json::Map::new();
                fields.insert("detail".into(), serde_json::json!(detail.clone()));
                fields.insert(
                    "asset".into(),
                    serde_json::json!(crate::remote_scan::diag::safe_asset_label(&self.path)),
                );
                crate::perf::event("cover.budget", fields);
            }
            return Err(io::Error::other(format!(
                "cover-read-budget exceeded: {detail}"
            )));
        }
        // 网络读**逐次**持 Cover 许可（归档路径此前完全不持许可；
        // 审阅冻结决策 4 要求许可只覆盖网络段、不跨越解码）。
        let governor = blocking_request_governor();
        let _permit = governor
            .acquire(RequestPriority::Cover)
            .map_err(|_| io::Error::other("cover_read_queue_full"))?;
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
        // ⑤：归档/PDF 封面的字节数只能在这里数——这条路径**不经过**
        // `SourceReadAtBytes`（③-1 真实数据复测：PDF 封面抓取 0 个 `source.read_at`
        // 事件），漏掉它等于把"整包下载"的最大一笔藏起来。
        crate::perf::bump(crate::perf::Counter::CoverDocumentReads);
        crate::perf::add(crate::perf::Counter::CoverBytesFetched, bytes.len() as u64);
        self.read_bytes
            .fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
        self.read_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        buf[..bytes.len()].copy_from_slice(&bytes);
        Ok(bytes.len())
    }
}

/// 构造一个封面失败原因。参数必须是上面的常量：绝不把 provider / 解码器原文
/// 塞进持久化状态（契约由 `safe_malformed_code` 的白名单与单测共同保证）。
fn cover_reason(code: &'static str) -> RemoteScanError {
    RemoteScanError::MalformedResponse(code.to_string())
}

/// 允许进入持久化状态与诊断日志的 `MalformedResponse` 内部码白名单。
/// 只有封面自己的 9 个原因码会变成具体码；其余内部码（含 range probe 的契约码）
/// 与旧行为一致地折叠成 `malformed`——既保持 UI 既有提示不变，也保证
/// provider / 解码器原文永远不可能出现在数据库或日志里。
fn safe_malformed_code(code: &str) -> &'static str {
    COVER_REASONS
        .iter()
        .copied()
        .find(|known| *known == code)
        .unwrap_or("malformed")
}

/// 是否值得"放大窗口再试一次"。只有一种失败值得：缓冲区只是文件的前一段、
/// 像素数据还没读完。头部不是图片、像素超守卫、整页都解不开都不值得再读网络。
fn cover_error_worth_escalating(error: &RemoteScanError) -> bool {
    match error {
        RemoteScanError::MalformedResponse(code) => code.as_str() == COVER_REASON_PARTIAL_DECODE,
        _ => false,
    }
}

/// 封面窗口阶梯：由小到大，最后一跳覆盖整个文件。
/// 返回 `None` 表示超过硬上限——调用方必须给出具体原因，而不是笼统地当"不可得"。
fn cover_read_windows(read_size: u64) -> Option<Vec<u64>> {
    if read_size == 0 || read_size > COVER_FETCH_LIMIT_BYTES {
        return None;
    }
    let mut windows = Vec::new();
    for limit in [
        COVER_HEAD_BYTES,
        COVER_HEAD_MAX_BYTES,
        COVER_FETCH_LIMIT_BYTES,
    ] {
        let window = read_size.min(limit);
        if windows.last() != Some(&window) {
            windows.push(window);
        }
    }
    Some(windows)
}

/// 长条源（高/宽 > [`COVER_LONG_STRIP_ASPECT`]）且调用方没有给显式裁剪时，
/// 封面只取顶部一条：条带高度按目标封面比例（h/w）反推，即"最上面那一格画面"。
/// 普通比例返回 `None`，保持既有中心裁剪行为不变；显式 `crop:` 永不被覆盖。
fn cover_top_band_crop(
    image_width: u32,
    image_height: u32,
    cover_width: u32,
    cover_height: u32,
) -> Option<(f64, f64, f64, f64)> {
    if image_width == 0 || image_height == 0 || cover_width == 0 || cover_height == 0 {
        return None;
    }
    let aspect = f64::from(image_height) / f64::from(image_width);
    if aspect <= COVER_LONG_STRIP_ASPECT {
        return None;
    }
    let band = (f64::from(cover_height) / f64::from(cover_width) / aspect).clamp(0.0, 1.0);
    Some((0.0, 0.0, 1.0, band))
}

/// 只读头部拿像素尺寸，不做整图解码。这四种格式的尺寸信息都在文件最前面
/// （PNG 在首个 IDAT 之前，JPEG 在 SOF，WebP/GIF 在文件头），因此"头部读不出来"
/// 意味着放大窗口也没有意义（不是截断问题）。
fn cover_image_dimensions(bytes: &[u8]) -> Result<(u32, u32), RemoteScanError> {
    let reader = image::ImageReader::new(io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| cover_reason(COVER_REASON_DECODE))?;
    reader
        .into_dimensions()
        .map_err(|_| cover_reason(COVER_REASON_DECODE))
}

/// 解码一个有界缓冲区，复用既有 `decode_cover` 的裁剪管线（不新造管线）。
/// `truncated` 表示"这个缓冲区可能只是文件的前一段"，失败码据此区分：
/// 截断 → `cover_partial_decode_failed`（值得放大窗口），否则 → `cover_decode_failed`。
fn decode_cover_bounded(
    bytes: &[u8],
    cover_width: u32,
    cover_height: u32,
    crop: Option<(f64, f64, f64, f64)>,
    truncated: bool,
) -> Result<crate::decode::DecodedImage, RemoteScanError> {
    let magic = crate::decode::sniff_image_magic(bytes);
    let (image_width, image_height) = match cover_image_dimensions(bytes) {
        Ok(dimensions) => dimensions,
        Err(error) => {
            // 诊断（安全）：只记**格式标签**与长度，绝不记内容或原文。
            // 这是"为什么解不开"的答案来源（bmp/tiff/heif/zip/text-like…）。
            if crate::perf::enabled() {
                let mut fields = serde_json::Map::new();
                fields.insert(
                    "magic".into(),
                    serde_json::json!(crate::decode::image_magic_label(magic)),
                );
                fields.insert("len".into(), serde_json::json!(bytes.len()));
                crate::perf::event("cover.probe", fields);
            }
            return Err(error);
        }
    };
    if u64::from(image_width) * u64::from(image_height) > COVER_MAX_PIXELS {
        // 解压炸弹：在分配整图之前挡住，而不是让设备 OOM。
        return Err(cover_reason(COVER_REASON_PIXELS));
    }
    let crop =
        crop.or_else(|| cover_top_band_crop(image_width, image_height, cover_width, cover_height));
    crate::decode::decode_cover(bytes, cover_width, cover_height, crop).map_err(|_| {
        cover_reason(if truncated {
            COVER_REASON_PARTIAL_DECODE
        } else {
            COVER_REASON_DECODE
        })
    })
}

/// 归档封面最多向后扫几页（封面页不是图片时，退到下一张真正可解码的图）。
///
/// 动机（③ 实测）：`mobi::image_records()` 只做非图片魔数**黑名单**，KF8/AZW3 的
/// CSS/HTML/资源记录会被当成"页"，`page_bytes(0)` 不是图片 ⇒ 封面永远失败。
/// 有界（3）保证不会为了封面把整本书扫一遍。
const COVER_PAGE_SCAN_LIMIT: u32 = 3;

/// **PDF** 封面最多向后扫几页。
///
/// 第 79 轮续 3（真机）：PDF 页是整张扫描图，一页的取数窗口就是 1–2.5 MB，扫四页加上
/// pdfium 的 xref/对象读取，一本就能吃掉 10–24 MB、几十秒 ⇒ 手机上门控被占满、卡片
/// 长时间转圈。PDF 只试"首页 + 次页"两页；其它格式（记录可能不是图片的 KF8/AZW3）
/// 仍按 [`COVER_PAGE_SCAN_LIMIT`]。
const COVER_PDF_PAGE_SCAN_LIMIT: u32 = 1;

/// 按归档类型取封面扫页上限（PDF 收紧，其它格式保持既有契约）。
fn cover_page_scan_limit(asset_kind: &str) -> u32 {
    if asset_kind == "pdf" {
        COVER_PDF_PAGE_SCAN_LIMIT
    } else {
        COVER_PAGE_SCAN_LIMIT
    }
}

/// 取"第一张真正可解码的页"作为封面：先试用户选择/默认页，再**有界**向后扫。
/// 只有"这一页不是可解码图片"才换页；上限/像素类失败换页无意义，直接返回。
fn decode_first_usable_page(
    document: &dyn crate::document::Document,
    first_page: u32,
    cover_width: u32,
    cover_height: u32,
    crop: Option<(f64, f64, f64, f64)>,
    scan_limit: u32,
) -> Result<crate::decode::DecodedImage, RemoteScanError> {
    let mut last = cover_reason(COVER_REASON_PAGE_RENDER);
    for offset in 0..=scan_limit {
        let page = first_page.saturating_add(offset);
        if page >= document.page_count() {
            break; // 没有更多页
        }
        // 第 79 轮：按封面尺寸取页（`page_bytes_for_display`）。对 PDF 而言这是"让 pdfium
        // 直接按 340 宽栅格化"，而不是先渲染 1600px 长条页再缩小 —— 真机实测单页
        // 0.7–6.8 s / 输出最大 7.3 MB，栅格化面积按宽度平方降下来（长条页实测 ≈15×）。
        //
        // 第 79 轮续8（独立评审 C-3）：取页**失败**必须带着自己的码上抛，不能 `break`。
        // PDF 改惰性读之后，读取预算烧穿/网络失败都发生在**取页**阶段，而旧写法把它们
        // 一律吞成 `cover_page_render_failed`（**不在可重试表**⇒永久失败），恰好与
        // "快速失败并给具体码、重但可救的封面能自愈"的意图相反。
        let bytes = match document.page_bytes_for_display(page, cover_width) {
            Ok(bytes) => bytes,
            Err(error) => {
                let text = format!("{error:#}").to_ascii_lowercase();
                return Err(if text.contains("cover-read-budget") {
                    cover_reason(COVER_REASON_READ_BUDGET)
                } else {
                    cover_reason(COVER_REASON_PAGE_RENDER)
                });
            }
        };
        match decode_cover_bounded(&bytes, cover_width, cover_height, crop, false) {
            Ok(image) => return Ok(image),
            Err(error) => {
                if error_code(&error) != COVER_REASON_DECODE {
                    return Err(error);
                }
                last = error;
            }
        }
    }
    Err(last)
}

/// ⑤ 取证上下文：按源/按 job 记录封面抓取字节，用数据证明"封面不再整包下载"。
/// `asset_id` 是书源作用域内的标识（可能含逻辑路径），但不含 provider 直链、
/// Cookie 或响应正文——与 `perf` 模块的脱敏约束一致。
#[derive(Clone, Copy)]
struct CoverFetchTrace<'a> {
    source_id: &'a str,
    asset_id: &'a str,
}

impl<'a> CoverFetchTrace<'a> {
    /// 一次封面抓取事件。事件流关闭时 `perf` 自身会短路，这里只做常量拼接。
    /// 字段名用 `asset_kind` 而**不是** `kind`：`kind` 是事件流的保留键
    /// （曾把事件类型覆盖成 `pdf`，导致按 kind 过滤全部失效）。
    fn span(self, asset_kind: &'static str, size: u64) -> crate::perf::Span {
        crate::perf::Span::new("cover.fetch")
            .field_str("source", self.source_id)
            .field_str("job", self.asset_id)
            .field_str("asset_kind", asset_kind)
            .field_u64("size", size)
    }
}

/// 按窗口阶梯抓取并解码封面：增量续读（不从头重读），只有"缓冲区被截断"这一种
/// 失败会放大窗口重试一次；仍失败才返回具体原因。网络读取逐次持 Cover 许可，
/// 解码始终在许可之外（审阅冻结决策 4：不跨越解码持有网络许可）。
#[allow(clippy::too_many_arguments)]
fn fetch_cover_by_windows(
    adapter: &dyn crate::remote_scan::adapter::RemoteProviderAdapter,
    path: &str,
    read_size: u64,
    windows: &[u64],
    cover_width: u32,
    cover_height: u32,
    crop: Option<(f64, f64, f64, f64)>,
    trace: CoverFetchTrace<'_>,
) -> Result<crate::decode::DecodedImage, RemoteScanError> {
    let governor = blocking_request_governor();
    let mut buffer: Vec<u8> = Vec::new();
    let mut last_error: Option<RemoteScanError> = None;
    for (attempt, window) in windows.iter().enumerate() {
        let missing = window.saturating_sub(buffer.len() as u64);
        let delta = if missing == 0 {
            Vec::new()
        } else {
            let _permit = governor
                .acquire(RequestPriority::Cover)
                .map_err(|_| RemoteScanError::Provider("request_queue_full".into()))?;
            adapter.read_range(path, buffer.len() as u64, missing)?
        };
        if missing > 0 && delta.is_empty() {
            // 有界读取没有任何进展：再放大窗口只会得到同样的空结果。
            break;
        }
        if missing > 0 {
            crate::perf::bump(crate::perf::Counter::CoverRangeReads);
            crate::perf::add(crate::perf::Counter::CoverBytesFetched, delta.len() as u64);
        }
        buffer.extend_from_slice(&delta);
        let truncated = (buffer.len() as u64) < read_size;
        let span = trace
            .span("image", read_size)
            .field_u64("attempt", attempt as u64 + 1)
            .field_u64("window", *window)
            .field_u64("bytes", delta.len() as u64);
        match decode_cover_bounded(&buffer, cover_width, cover_height, crop, truncated) {
            Ok(image) => {
                span.end();
                return Ok(image);
            }
            Err(error) => {
                span.field_str("code", error_code(&error)).end();
                crate::perf::bump(crate::perf::Counter::CoverFailures);
                if !cover_error_worth_escalating(&error) {
                    return Err(error);
                }
                if attempt + 1 < windows.len() {
                    crate::perf::bump(crate::perf::Counter::CoverEscalations);
                }
                last_error = Some(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| cover_reason(COVER_REASON_PARTIAL_DECODE)))
}

/// 归档/PDF"打不开"的**安全**细分：只按固定枚举落库，绝不放原文。
///
/// 为什么要单列 `cover_native_lib_missing`：③-1 真实数据复测中，40/40 个失败样本
/// 全部落进同一个笼统码，事后才查出运行实例的 `RCH.exe` 同目录缺 `pdfium.dll`
/// ——部署问题被误判成"封面不可得"。原生库缺失必须能自证。
fn cover_open_reason(error: &anyhow::Error) -> RemoteScanError {
    let text = format!("{error:#}").to_ascii_lowercase();
    if text.contains("cover-read-budget") {
        // 预算中止是**我们自己**的决定，不是文件损坏：给独立码（终态）。
        cover_reason(COVER_REASON_READ_BUDGET)
    } else if text.contains("pdfium") {
        cover_reason(COVER_REASON_NATIVE_LIB_MISSING)
    } else {
        cover_reason(COVER_REASON_DOCUMENT_OPEN)
    }
}

/// 归档封面：按真实文件名的格式解析，只取所需的那一页（自动路径 = 第 1 页）。
/// 归档仍走 `AdapterByteSource` 的按需 Range 读取（PDF 除外：pdfium 需要整包）；
/// 该路径的字节由 `AdapterByteSource::read_at` 计入 `CoverBytesFetched`
/// （**不**经过 `SourceReadAtBytes`）。
#[allow(clippy::too_many_arguments)]
fn fetch_cover_from_document(
    adapter: &Arc<dyn crate::remote_scan::adapter::RemoteProviderAdapter>,
    read_path: String,
    read_size: u64,
    document_name: &str,
    page: u32,
    cover_width: u32,
    cover_height: u32,
    crop: Option<(f64, f64, f64, f64)>,
    trace: CoverFetchTrace<'_>,
) -> Result<crate::decode::DecodedImage, RemoteScanError> {
    let asset_kind = if document_name.to_lowercase().ends_with(".pdf") {
        "pdf"
    } else {
        "archive"
    };
    let span = trace.span(asset_kind, read_size);
    let source = AdapterByteSource::new(Arc::clone(adapter), read_path, read_size);
    // 第 62 轮：`.zip/.cbz` 的封面先走**最小 ZIP 读取器**（请求数与文件大小/条目数
    // 无关）。常规路径的 `zip::ZipArchive::new` 会对**每个条目**读一次 local header
    // 做校验 ⇒ 一本 2.08GB、~5000 条目的 CBZ 要上万次请求（115 CDN 单次 243ms
    // ⇒ 约 40 分钟/枚），而 worker 是单线程。快通道拿不到（非 ZIP / ZIP64 /
    // 超限 / 解压失败）就照旧回退，行为不变。
    let lower_name = document_name.to_lowercase();
    if lower_name.ends_with(".zip") || lower_name.ends_with(".cbz") {
        if let Ok(Some(page_bytes)) =
            crate::document::zip::first_image_bytes_via_central_directory(
                &source,
                COVER_FAST_PAGE_MAX_BYTES,
            )
        {
            if let Ok(image) =
                decode_cover_bounded(&page_bytes, cover_width, cover_height, crop, false)
            {
                span.end();
                return Ok(image);
            }
        }
    }
    // 115/夸克/百度 use an opaque id as the logical path; dispatch the
    // parser by the real file name stored in library_index.
    // 封面页取"第一张真正可解码的图"（有界向后扫），见 `decode_first_usable_page`。
    // 扫页上限按格式取：PDF 收紧（页是整张扫描图，扫多了会烧穿封面读取预算）。
    let scan_limit = cover_page_scan_limit(asset_kind);
    // 2026-09-21：封面走**专用入口**——MOBI 只探测到第一张图就停，避免
    // "每条候选记录一次远端 Range"把 30 s 封面预算烧穿（真机 281 条超时）。
    // 其它格式行为不变（见 `document::open_cover_document`）。
    let outcome = crate::document::open_cover_document(source, document_name)
        .map_err(|error| cover_open_reason(&error))
        .and_then(|document| {
            decode_first_usable_page(
                document.as_ref(),
                page,
                cover_width,
                cover_height,
                crop,
                scan_limit,
            )
        });
    match &outcome {
        Ok(_) => {
            span.end();
        }
        Err(error) => {
            let mut span = span.field_str("code", error_code(error));
            // 第 81 轮续2（用户要求：先补诊断）：预算类失败必须能自证"是**哪一条**先超" ——
            // 字节(64MB) / 次数(384) / 时间(30s)。此前只落一个笼统的
            // `cover_read_budget_exceeded`，只能靠猜（真机 `2.mobi` 放宽预算后仍失败，
            // 无法判断是单条记录太大还是次数/时间不够）。
            //
            // 这段文字由**本模块**生成（`AdapterByteSource::read_at`），只含
            // used/budget/reads/elapsed 数字，不含 provider 原文、URL 或 Cookie ——
            // 与 perf / diag 的脱敏约束一致；错误码本身仍留在白名单内（不被污染）。
            let text = format!("{error:#}");
            if text.contains("cover-read-budget") {
                let detail = text.trim().replace(['\n', '\r'], " ");
                span = span.field_str("budget", detail.clone());
                crate::remote_scan::diag::note(&format!(
                    "cover_budget source={} code={} detail={}",
                    trace.source_id,
                    error_code(error),
                    detail
                ));
            }
            span.end();
            crate::perf::bump(crate::perf::Counter::CoverFailures);
        }
    }
    outcome
}

/// Decode an actual first page for an archive or image-folder cover. The
/// previous implementation treated the first 256 KiB of a ZIP as an image;
/// this function keeps the range-only policy while passing bytes through the
/// existing document and image decoders.
///
/// ③-1：超大单图不再在 32MB 处直接放弃（`cover_size_limit` 放弃分支已删除），
/// 改为"按窗口有界读取 → 解码 → 放大窗口再试一次 → 仍失败给具体原因"；
/// 长条源在自动路径上只截顶部一条带（见 `cover_top_band_crop`）。
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
    trace: CoverFetchTrace<'_>,
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
        return Err(cover_reason(COVER_REASON_SIZE_MISSING));
    };
    // PDF 必须先整包交给 pdfium（`PdfBook::open`）：先按独立上限拒绝，给出
    // 具体原因，不静默跳过，也不做"下载几百 MB 换一张封面"。
    let is_pdf = is_archive && document_name.to_lowercase().ends_with(".pdf");
    if is_pdf && read_size > COVER_PDF_MAX_BYTES {
        return Err(cover_reason(COVER_REASON_PDF_BYTES_LIMIT));
    }
    // 非归档先算窗口阶梯：超过硬上限在这里就变成具体原因，而不是笼统失败。
    let windows = if is_archive {
        Vec::new()
    } else {
        cover_read_windows(read_size).ok_or_else(|| cover_reason(COVER_REASON_BYTES_LIMIT))?
    };
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
    // providers without their own gate (WebDAV / SFTP). 窗口阶梯的每次续读各自
    // 持许可（`fetch_cover_by_windows`），解码一律在许可之外。
    let governor = blocking_request_governor();
    {
        let _permit = governor
            .acquire(RequestPriority::Cover)
            .map_err(|_| RemoteScanError::Provider("request_queue_full".into()))?;
        let capabilities = adapter.capabilities(&read_path, &read_fingerprint)?;
        if !capabilities.range_read {
            return Err(RemoteScanError::RangeUnavailable);
        }
    }
    if is_archive {
        return fetch_cover_from_document(
            &adapter,
            read_path,
            read_size,
            document_name,
            page,
            cover_width,
            cover_height,
            crop,
            trace,
        );
    }
    fetch_cover_by_windows(
        adapter.as_ref(),
        &read_path,
        read_size,
        &windows,
        cover_width,
        cover_height,
        crop,
        trace,
    )
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
    // 索引行按 **`hash(源指纹, 路径)` 语义**查，而不是 `source_id + path`：
    // 先认本源的行；本源没有时，接受**同指纹兄弟源**的行。
    //
    // 为什么：同一个远端库可能被两个书源指向（例如直连的 `115_…` 与同步镜像
    // `sync_…`），它们 `book_sources.fingerprint` **相同** ⇒ `library_index.id`
    // （PK）**也相同** ⇒ 后扫描的一方会覆盖对方的行。此时按 `source_id` 过滤就
    // 永远查不到，封面被判 `notFound` 且不再重试。
    // （第 55 轮真实库审查实测：115 源 225 个 `notFound` **全部**是这种"行被同指纹的
    // 另一源持有"——id 与路径都相同，只是 `source_id` 不同。）
    //
    // 安全性：命中前提是**源指纹相同**（`id = hash(指纹, 路径)`）且路径一致 ⇒
    // 只可能是"同一份远端库的同一条路径"，不会跨库串数据；
    // 指纹缺失/为空的源仍然只认自己的行（与旧行为一致，是严格的超集）。
    let indexed: Option<(String, String, Option<i64>, Option<String>)> = conn
        .query_row(
            "SELECT li.name,li.asset_kind,li.size,li.content_fingerprint
               FROM library_index li
              WHERE li.path=?2 AND li.deleted=0
                AND (li.source_id=?1
                     OR li.source_id IN (SELECT id FROM book_sources
                                          WHERE fingerprint=
                                                (SELECT fingerprint FROM book_sources WHERE id=?1)
                                            AND COALESCE(fingerprint,'')<>''))
              ORDER BY CASE WHEN li.source_id=?1 THEN 0 ELSE 1 END, li.updated_at DESC
              LIMIT 1",
            params![source_id, normalized_path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .ok()?;
    // preview 元数据：**不要求扫描仍在 Running**（同 `cover_route_for_job` 的理由）。
    // staging 行只要身份守卫成立（`session_epoch<>''` + 源指纹一致）就是可信的；
    // 要求 Running 会在"扫描收尾 → 索引尚未落行"的窗口里制造永久 `notFound`
    // （第 55 轮真实库审查：225 行正是这样被记成终态的）。
    let preview: Option<(String, String, Option<i64>, Option<String>)> = conn
        .query_row(
            "SELECT p.name,p.asset_kind,p.size,p.content_fingerprint
             FROM remote_scan_preview p
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
        "SELECT asset_id,state,updated_at,content_revision,selection_revision,profile,
                COALESCE(error_code,'')
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
                row.get::<_, String>(6)?,
            ))
        }) {
            // P1-F：latest-per-asset（与 UI 的"当前 job"语义一致），
            // 并保留 cover 身份供**锁外**做缓存可用性校验。
            let mut latest_by_asset: HashMap<String, (String, i64, String, String, String, String)> =
                HashMap::new();
            for row in rows.flatten() {
                let replace = latest_by_asset
                    .get(&row.0)
                    .is_none_or(|(_, updated_at, _, _, _, _)| row.2 >= *updated_at);
                if replace {
                    latest_by_asset
                        .insert(row.0, (row.1, row.2, row.3, row.4, row.5, row.6));
                }
            }
            let tracked_distinct = latest_by_asset.len() as u64;
            let mut other_unknown = 0_u64;
            let mut ready_identities: Vec<(String, String, String, String)> = Vec::new();
            for (asset_id, (state, _, content_revision, selection_revision, profile, error_code)) in
                latest_by_asset.into_iter()
            {
                // RG-B F1/F2：`route_missing` 表示该 job 的资产在当前列表/路由表里**已无路由**
                // （跨代际残留的孤儿封面任务），它**不是**真实的抓取失败 ⇒ 不计入"失败"桶，
                // 而是计入 other（UI 显示为"暂不可用 / 已失效"），避免失败率被孤儿严重虚高。
                // 与之配套：failed 是终态，worker 不会重试孤儿 job（不再空转）。
                let is_stale_orphan = state == "failed" && error_code == "route_missing";
                let index = match state.as_str() {
                    "ready" => Some(0),
                    "running" => Some(1),
                    "pending" => Some(2),
                    "retry_wait" => Some(3),
                    "blocked" => Some(4),
                    "unsupported" => Some(5),
                    "failed" if is_stale_orphan => None,
                    "failed" => Some(6),
                    // P1-F：真实未知 durable state **不得静默丢弃**；
                    // F1/F2：孤儿（route_missing）同样落到 other，保持"可见但不计入失败"。
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
                profile: cover_quality_profile_on(&conn),
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
            CoverFetchTrace {
                source_id: "cover-contract-source",
                asset_id: "/opaque-fid",
            },
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
            CoverFetchTrace {
                source_id: "cover-contract-source",
                asset_id: "/folder/page1.png",
            },
        )
        .unwrap();
        assert_eq!((decoded.width, decoded.height), (340, 480));
    }

    /// ③-1 契约：窗口阶梯必须"由小到大、最后一跳覆盖整个文件"，只有超过硬上限
    /// 才拒绝。旧实现在 32MB 处直接当"封面不可得"，这里断言它不会再回来。
    #[test]
    fn cover_window_ladder_is_bounded_and_never_gives_up_early() {
        let small = 8 * 1024 * 1024;
        assert_eq!(cover_read_windows(small), Some(vec![small]));

        let medium = 40 * 1024 * 1024;
        assert_eq!(
            cover_read_windows(medium),
            Some(vec![COVER_HEAD_BYTES, medium])
        );

        let large = 100 * 1024 * 1024;
        assert_eq!(
            cover_read_windows(large),
            Some(vec![
                COVER_HEAD_BYTES,
                COVER_HEAD_MAX_BYTES,
                large
            ])
        );

        assert_eq!(cover_read_windows(COVER_FETCH_LIMIT_BYTES), Some(vec![
            COVER_HEAD_BYTES,
            COVER_HEAD_MAX_BYTES,
            COVER_FETCH_LIMIT_BYTES,
        ]));
        assert_eq!(cover_read_windows(COVER_FETCH_LIMIT_BYTES + 1), None);
        assert_eq!(cover_read_windows(0), None);
    }

    /// 长条源只取顶部一条带；普通比例页面保持既有的中心裁剪。
    #[test]
    fn long_strip_cover_takes_the_top_band_only() {
        assert_eq!(cover_top_band_crop(800, 1200, 340, 480), None);

        let (x, y, w, h) = cover_top_band_crop(800, 20_000, 340, 480).unwrap();
        assert_eq!((x, y, w), (0.0, 0.0, 1.0));
        let expected = (480.0 / 340.0) / (20_000.0 / 800.0);
        assert!((h - expected).abs() < 1e-9, "band height must match the cover aspect");
        assert!(h < 0.06, "the band must come from the top, not the middle");

        assert_eq!(cover_top_band_crop(0, 20_000, 340, 480), None);
    }

    /// 具体失败码必须落在白名单内；provider / 解码器原文永远折叠成 `malformed`。
    #[test]
    fn cover_failure_codes_stay_inside_the_safe_allowlist() {
        for code in COVER_REASONS {
            assert_eq!(safe_malformed_code(code), code);
        }
        assert_eq!(safe_malformed_code("HTTP 403 token=secret"), "malformed");
        assert_eq!(
            error_code(&RemoteScanError::MalformedResponse(
                "https://cdn.example.com/x?sign=abc".into()
            )),
            "malformed"
        );
        assert_eq!(
            error_code(&cover_reason(COVER_REASON_PARTIAL_DECODE)),
            COVER_REASON_PARTIAL_DECODE
        );
    }

    /// 归档/PDF"打不开"的安全细分：**原生库缺失**（部署问题）必须与文件本身的问题分开，
    /// 且分类结果绝不能夹带原文（③-1 真实数据复测：缺 pdfium.dll 时全部失败样本
    /// 落进同一个笼统码，无法自证）。
    #[test]
    fn archive_open_failures_separate_missing_native_lib_from_document_errors() {
        let missing = anyhow::anyhow!(
            "无法加载 pdfium 动态库，请将 pdfium.dll 放在 RCH.exe 同目录。bind failed"
        );
        assert_eq!(
            error_code(&cover_open_reason(&missing)),
            COVER_REASON_NATIVE_LIB_MISSING
        );

        let broken = anyhow::anyhow!("invalid zip directory: offset out of range");
        assert_eq!(
            error_code(&cover_open_reason(&broken)),
            COVER_REASON_DOCUMENT_OPEN
        );

        // 安全性质：分类结果里不得出现原文片段。
        let classified = format!("{:?}", cover_open_reason(&missing));
        assert!(
            !classified.contains("放在 RCH.exe") && !classified.contains("bind failed"),
            "分类结果不得夹带原文: {classified}"
        );
    }

    /// ③ MOBI 实测：`page_bytes(0)` 可能**不是图片**（KF8/AZW3 的 CSS/HTML 资源记录
    /// 被 `image_records()` 的黑名单漏进来）。封面必须退到第一张真正可解码的页；
    /// 一页都不可解码时仍要给**具体**失败码，不能静默成功。
    #[test]
    fn cover_falls_back_to_the_first_decodable_page() {
        struct FakeBook {
            pages: Vec<Vec<u8>>,
        }
        impl crate::document::Document for FakeBook {
            fn page_count(&self) -> u32 {
                self.pages.len() as u32
            }
            fn page_bytes(&self, index: u32) -> anyhow::Result<Vec<u8>> {
                self.pages
                    .get(index as usize)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("越界"))
            }
        }

        let image = image::RgbaImage::from_pixel(4, 6, image::Rgba([10, 20, 30, 255]));
        let mut encoded = io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();

        let book = FakeBook {
            pages: vec![
                b"<!DOCTYPE html><html>not an image</html>".to_vec(),
                b"BM\x36\x00\x00\x00".to_vec(), // BMP：可命名，但当前构建不可解码
                encoded.into_inner(),
            ],
        };
        let cover = decode_first_usable_page(
            &book,
            0,
            REMOTE_COVER_WIDTH,
            REMOTE_COVER_HEIGHT,
            None,
            COVER_PAGE_SCAN_LIMIT,
        )
        .expect("第三页是可解码 PNG，应作为封面");
        assert_eq!((cover.width, cover.height), (340, 480));

        let broken = FakeBook {
            pages: vec![b"<html>".to_vec(), b"not an image".to_vec()],
        };
        let outcome = decode_first_usable_page(
            &broken,
            0,
            REMOTE_COVER_WIDTH,
            REMOTE_COVER_HEIGHT,
            None,
            COVER_PAGE_SCAN_LIMIT,
        );
        match outcome {
            Ok(_) => panic!("没有任何可解码页时不得判为成功"),
            Err(error) => assert_eq!(error_code(&error), COVER_REASON_DECODE),
        }
    }

    /// 第 79 轮续 3：扫页上限按格式取 —— PDF 收紧（真机：长条扫描本扫四页就能吃掉
    /// 10–24 MB、几十秒），其它格式保持既有契约（KF8/AZW3 的记录可能不是图片）。
    #[test]
    fn pdf_cover_scan_limit_is_tighter_than_other_archives() {
        assert_eq!(cover_page_scan_limit("pdf"), COVER_PDF_PAGE_SCAN_LIMIT);
        assert_eq!(cover_page_scan_limit("archive"), COVER_PAGE_SCAN_LIMIT);
        assert!(
            COVER_PDF_PAGE_SCAN_LIMIT < COVER_PAGE_SCAN_LIMIT,
            "PDF 必须比其它归档更紧，否则封面读取预算仍会被烧穿"
        );
    }

    /// 第 79 轮续7（用户确认）：扫描/预览建封面任务用的 profile 必须跟随设置
    /// `coverQuality`。真机证据：固定 340 档时，586 本 PDF 里 340 ready 142 本而
    /// 卡片要的 170 档只有 42 本 ⇒ 抓回来的图卡片用不上。
    #[test]
    fn scan_cover_profile_follows_cover_quality_setting() {
        let conn = db::get().lock().unwrap();
        let setting = "coverQuality";
        let restore: Option<String> = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key=?1",
                [setting],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        let set = |value: Option<&str>| {
            conn.execute("DELETE FROM app_settings WHERE key=?1", [setting])
                .unwrap();
            if let Some(value) = value {
                conn.execute(
                    "INSERT INTO app_settings(key,value,updated_at) VALUES(?1,?2,0)",
                    rusqlite::params![setting, value],
                )
                .unwrap();
            }
        };

        set(Some("low"));
        assert_eq!(cover_quality_profile_on(&conn), "170x240@1");
        set(Some("high"));
        assert_eq!(cover_quality_profile_on(&conn), "510x720@1");
        set(Some("medium"));
        assert_eq!(
            cover_quality_profile_on(&conn),
            crate::remote_scan::cover_store::DEFAULT_COVER_PROFILE
        );
        set(None);
        assert_eq!(
            cover_quality_profile_on(&conn),
            crate::remote_scan::cover_store::DEFAULT_COVER_PROFILE,
            "设置缺失时必须回落到历史默认档"
        );

        set(restore.as_deref());
    }

    /// ② 策略：**可修复的失败**必须拿到 6h 长期补偿资格，否则永久结案、永不自愈
    /// （③-1 实测：缺 `pdfium.dll` 导致 267 本 PDF 封面全灭，却因"永久失败"永不重试）。
    /// 同时守住两张表不漂移，并确认真正不可得的失败仍保持永久失败（不会无限重试）。
    #[test]
    fn fixable_cover_failures_earn_a_long_retry_episode() {
        // 1) 可重试封面码 ⊆ 已登记原因码
        for code in COVER_RETRYABLE_REASONS {
            assert!(
                COVER_REASONS.contains(&code),
                "可重试码 {code} 不在 COVER_REASONS 白名单里"
            );
        }
        // 2) provider 可重试码确实是分类器能产出的码
        for (message, expected) in [
            ("429 too many requests", "provider:rateLimited"),
            ("request timed out", "provider:timeout"),
            ("401 unauthorized", "provider:unauthorized"),
            ("403 Forbidden", "provider:forbidden"),
        ] {
            assert_eq!(provider_failure_code(message), expected);
            assert!(PROVIDER_RETRYABLE_CODES.contains(&expected));
        }

        // 3) 部署 / 策略上限类 ⇒ 可重试（由白名单驱动，避免手抄漏项）
        for code in COVER_RETRYABLE_REASONS {
            assert!(
                long_retry_is_retryable(&cover_reason(code)),
                "{code} 属于可修复失败，应获得长期补偿"
            );
        }
        // 4) 账号 / 会话级 provider 子原因 ⇒ 可重试
        assert!(long_retry_is_retryable(&RemoteScanError::Provider(
            "429 too many requests".into()
        )));
        assert!(long_retry_is_retryable(&RemoteScanError::Provider(
            "request timed out".into()
        )));
        assert!(long_retry_is_retryable(&RemoteScanError::TransientNetwork(
            "network".into()
        )));

        // 5) 真正不可得的失败仍必须永久失败（不能退化成无限重试）
        for terminal in [
            COVER_REASON_DOCUMENT_OPEN,
            COVER_REASON_PAGE_RENDER,
            COVER_REASON_DECODE,
            COVER_REASON_SIZE_MISSING,
        ] {
            assert!(
                !long_retry_is_retryable(&cover_reason(terminal)),
                "{terminal} 不应获得长期补偿"
            );
        }
        assert!(!long_retry_is_retryable(&RemoteScanError::Provider(
            "404 not found".into()
        )));
        assert!(!long_retry_is_retryable(&RemoteScanError::Unsupported));
    }


    /// 第 60 轮实测回归：病态归档（尾部 233MB 的 EOCD 扫描）必须被**读取预算**挡住，
    /// 而不是拖着单线程 worker 把整条封面队列堵死。
    #[test]
    fn adapter_byte_source_stops_at_the_read_budget() {
        use std::sync::atomic::{AtomicU64, Ordering};

        struct SweepingAdapter {
            calls: AtomicU64,
        }
        impl RemoteProviderAdapter for SweepingAdapter {
            fn list(
                &self,
                _: &str,
                _: Option<&str>,
            ) -> Result<(Vec<crate::remote_scan::model::RemoteEntry>, Option<String>), RemoteScanError>
            {
                Err(RemoteScanError::Unsupported)
            }
            fn read_range(
                &self,
                _path: &str,
                _offset: u64,
                length: u64,
            ) -> Result<Vec<u8>, RemoteScanError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![0u8; length as usize])
            }
            fn read_file_limited(&self, _: &str, _: u64) -> Result<Vec<u8>, RemoteScanError> {
                panic!("封面路径不得整包读取")
            }
            fn normalize_path(&self, path: &str) -> String {
                normalize_path(path)
            }
            fn capabilities(
                &self,
                _: &str,
                _: &str,
            ) -> Result<crate::remote_scan::adapter::RemoteCapabilities, RemoteScanError> {
                Err(RemoteScanError::Unsupported)
            }
        }

        let budget = 64 * 1024;
        let adapter = Arc::new(SweepingAdapter {
            calls: AtomicU64::new(0),
        });
        let source = AdapterByteSource::with_budget(
            adapter.clone(),
            "/huge.zip".into(),
            512 * 1024 * 1024,
            budget,
        );
        let mut buf = vec![0u8; 16 * 1024];
        let mut offset = 0u64;
        let mut error = None;
        for _ in 0..1000 {
            match source.read_at(offset, &mut buf) {
                Ok(n) => offset += n as u64,
                Err(e) => {
                    error = Some(e);
                    break;
                }
            }
        }
        let error = error.expect("超预算必须报错");
        assert!(
            error.to_string().contains("cover-read-budget"),
            "错误文案要能被 cover_open_reason 识别: {error}"
        );
        assert!(
            adapter.calls.load(Ordering::SeqCst) <= 8,
            "预算内最多几次网络读，实际 {}",
            adapter.calls.load(Ordering::SeqCst)
        );
        // 映射到具体失败码（而不是笼统的打开失败）
        let mapped = cover_open_reason(&anyhow::anyhow!("{error}"));
        assert_eq!(error_code(&mapped), COVER_REASON_READ_BUDGET);
    }

    /// 超过硬上限的单图必须"不读一个字节"就给具体原因，绝不整包下载。
    #[test]
    fn oversized_single_image_is_rejected_with_a_specific_reason_without_reading() {
        struct PanickingReader;
        impl RemoteProviderAdapter for PanickingReader {
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
                panic!("超过硬上限的封面不得发出任何读取请求")
            }
            fn read_file_limited(&self, _: &str, _: u64) -> Result<Vec<u8>, RemoteScanError> {
                panic!("safe cover worker must never fall back to a whole-book read")
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

        let oversized = COVER_FETCH_LIMIT_BYTES + 1;
        // `DecodedImage` 不实现 `Debug`（也不该为了测试去改共享解码类型），
        // 因此这里显式匹配而不是 `unwrap_err()`。
        let outcome = fetch_remote_cover_image_with_dimensions(
            Arc::new(PanickingReader),
            "/opaque-image",
            "huge.jpg",
            Some(oversized),
            "fp",
            RemoteAssetKind::ImageFile,
            Some(("/opaque-image".into(), Some(oversized), "fp".into())),
            REMOTE_COVER_WIDTH,
            REMOTE_COVER_HEIGHT,
            0,
            None,
            CoverFetchTrace {
                source_id: "cover-contract-source",
                asset_id: "/opaque-image",
            },
        );
        let error = match outcome {
            Ok(_) => panic!("超过硬上限的封面必须被具体原因拒绝"),
            Err(error) => error,
        };
        assert_eq!(error_code(&error), COVER_REASON_BYTES_LIMIT);
    }

    /// 第一档窗口被截断时必须放大窗口重试，并且只续读增量（不从头重读一遍）。
    #[test]
    fn truncated_window_escalates_once_and_reuses_the_prefix() {
        struct WindowedPng {
            bytes: Vec<u8>,
            requests: Mutex<Vec<(u64, u64)>>,
        }
        impl RemoteProviderAdapter for WindowedPng {
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
                self.requests.lock().unwrap().push((offset, length));
                let start = (offset as usize).min(self.bytes.len());
                let end = start
                    .saturating_add(length as usize)
                    .min(self.bytes.len());
                Ok(self.bytes[start..end].to_vec())
            }
            fn read_file_limited(&self, _: &str, _: u64) -> Result<Vec<u8>, RemoteScanError> {
                panic!("windowed cover fetch must never fall back to a whole-file read")
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

        let image = image::RgbaImage::from_fn(400, 800, |x, y| {
            image::Rgba([
                (x % 251) as u8,
                (y % 241) as u8,
                ((x + y) % 239) as u8,
                255,
            ])
        });
        let mut encoded = io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let full = encoded.into_inner();
        let first_window = 512_u64;
        assert!(
            full.len() as u64 > first_window + 1024,
            "fixture must be large enough for the first window to truncate it"
        );

        let adapter = WindowedPng {
            bytes: full.clone(),
            requests: Mutex::new(Vec::new()),
        };
        let decoded = fetch_cover_by_windows(
            &adapter,
            "/opaque-image",
            full.len() as u64,
            &[first_window, full.len() as u64],
            REMOTE_COVER_WIDTH,
            REMOTE_COVER_HEIGHT,
            None,
            CoverFetchTrace {
                source_id: "cover-contract-source",
                asset_id: "/opaque-image",
            },
        )
        .unwrap();
        assert_eq!((decoded.width, decoded.height), (340, 480));

        let requests = adapter.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 2, "must escalate exactly once");
        assert_eq!(requests[0], (0, first_window));
        assert_eq!(
            requests[1],
            (first_window, full.len() as u64 - first_window),
            "escalation must continue from the prefix instead of re-reading the file"
        );
    }

    /// 长条封面必须来自**顶部**而不是中间：用"顶部红色标记 + 下方向灰度渐变"的
    /// 合成长图走真实解码路径，断言封面首行是顶部标记、末行不是。
    /// 设置 `RCH_STEP31_PROOF=<path>` 时同时写出封面 PNG 供人工复核。
    #[test]
    fn long_strip_cover_is_rendered_from_the_top_band() {
        let (width, height) = (400_u32, 4_000_u32);
        let marker_rows = height / 10;
        let image = image::RgbaImage::from_fn(width, height, |_x, y| {
            if y < marker_rows {
                image::Rgba([220, 30, 30, 255])
            } else {
                let level = (y * 255 / height) as u8;
                image::Rgba([level, level, level, 255])
            }
        });
        let mut encoded = io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let bytes = encoded.into_inner();

        let cover =
            decode_cover_bounded(&bytes, REMOTE_COVER_WIDTH, REMOTE_COVER_HEIGHT, None, false)
                .unwrap();
        assert_eq!((cover.width, cover.height), (340, 480));

        let pixel = |x: u32, y: u32| -> (u8, u8, u8) {
            let offset = ((y * cover.width + x) * 4) as usize;
            (
                cover.rgba[offset],
                cover.rgba[offset + 1],
                cover.rgba[offset + 2],
            )
        };
        let (top_r, top_g, _) = pixel(0, 0);
        assert!(
            top_r > 150 && top_g < 100,
            "封面首行必须是长图顶部的标记，实际 ({top_r},{top_g})"
        );
        let (bottom_r, bottom_g, _) = pixel(0, cover.height - 1);
        assert!(
            !(bottom_r > 150 && bottom_g < 100),
            "封面不得取自长图中间"
        );

        if let Ok(path) = std::env::var("RCH_STEP31_PROOF") {
            let mut out = io::Cursor::new(Vec::new());
            let rendered =
                image::RgbaImage::from_raw(cover.width, cover.height, cover.rgba.clone())
                    .expect("rgba buffer must match the cover dimensions");
            image::DynamicImage::ImageRgba8(rendered)
                .write_to(&mut out, image::ImageFormat::Png)
                .unwrap();
            std::fs::write(&path, out.into_inner()).unwrap();
        }
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

/// RG-B ①：把提供商错误**归类为安全子原因**并持久化（只写枚举，**绝不**透传提供商原文，
/// 避免 URL/凭据/私有路径进入数据库与 UI）。
///
/// 背景：此前所有提供商失败都只记为笼统的 `provider`，导致"251 个失败到底是什么"无法归因
/// （见真实库：夸克 attempt 1 即有 251 个 provider 失败，首试即败且与成功同分钟 ⇒ 与资产相关）。
fn provider_failure_code(message: &str) -> &'static str {
    let text = message.to_ascii_lowercase();
    if text.contains("404") || text.contains("not found") || text.contains("不存在") {
        return "provider:notFound";
    }
    if text.contains("403") || text.contains("forbidden") || text.contains("denied") {
        return "provider:forbidden";
    }
    if text.contains("401") || text.contains("unauthor") || text.contains("登录") {
        return "provider:unauthorized";
    }
    if text.contains("429") || text.contains("rate") || text.contains("频繁") {
        return "provider:rateLimited";
    }
    if text.contains("timeout") || text.contains("timed out") || text.contains("超时") {
        return "provider:timeout";
    }
    if text.contains("decode") || text.contains("解码") || text.contains("invalid image") {
        return "provider:decodeFailed";
    }
    if text.contains("cover") || text.contains("封面") {
        return "provider:noCover";
    }
    "provider:other"
}

#[cfg(test)]
mod provider_failure_code_tests {
    use super::provider_failure_code;

    /// 分类必须安全（固定枚举）且**绝不泄露**提供商原文。
    #[test]
    fn classifies_safely_and_never_leaks_provider_text() {
        assert_eq!(provider_failure_code("HTTP 404 Not Found"), "provider:notFound");
        assert_eq!(provider_failure_code("403 Forbidden"), "provider:forbidden");
        assert_eq!(provider_failure_code("429 too many requests"), "provider:rateLimited");
        assert_eq!(provider_failure_code("request timed out"), "provider:timeout");
        assert_eq!(provider_failure_code("image decode failed"), "provider:decodeFailed");
        assert_eq!(provider_failure_code("封面缺失"), "provider:noCover");
        // 含敏感信息的未知错误 ⇒ 只留枚举，不透传。
        assert_eq!(
            provider_failure_code("https://secret.example/x?token=abc failed"),
            "provider:other"
        );
    }
}
