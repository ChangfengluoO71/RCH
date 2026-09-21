//! Unified local catalog and cover-demand API.
//!
//! Directory views and cover reads are deliberately cache/SQLite only.  A
//! request merely records demand and wakes the cover worker; it never waits on
//! provider I/O, which keeps opening a cloud root responsive.

use crate::api::book::{CropRect, PageImage};
use crate::db;
use crate::frb_generated::StreamSink;
use crate::remote_scan::catalog::{self, CatalogEntryKind};
use crate::remote_scan::cover_model::{CoverJobKey, CoverJobState};
use crate::remote_scan::cover_state::CoverJobUpsertCause;
use crate::remote_scan::{cover_service, cover_store};
use anyhow::Result;
use rusqlite::{params, OptionalExtension};

pub struct CoverSelectionDto {
    pub page: u32,
    pub crop: Option<CropRect>,
    pub explicit_asset_id: Option<String>,
    pub revision: String,
}

pub struct CoverProfileDto {
    pub width: u32,
    pub height: u32,
    pub decoder_version: u32,
}

pub struct RemoteCoverStateDto {
    pub state: String,
    pub revision: i64,
    pub ready: bool,
    pub error_code: Option<String>,
    pub retry_at: Option<i64>,
    pub is_previous_revision: bool,
}

pub struct RemoteDirectoryEntryDto {
    pub asset_id: String,
    pub logical_path: String,
    pub provider_path: Option<String>,
    pub name: String,
    pub asset_kind: String,
    pub size: Option<u64>,
    pub representative_asset_id: Option<String>,
    pub cover: RemoteCoverStateDto,
}

pub struct RemoteDirectoryViewDto {
    pub source_id: String,
    pub logical_path: String,
    pub revision: i64,
    pub listing_complete: bool,
    pub has_more: bool,
    pub entries: Vec<RemoteDirectoryEntryDto>,
}

struct RouteRevision {
    logical_path: String,
    source_fingerprint: String,
    generation: i64,
    session_epoch: String,
    provider_id: String,
    provider_file_id: Option<String>,
}

fn kind_name(kind: CatalogEntryKind) -> &'static str {
    match kind {
        CatalogEntryKind::ArchiveFile => "ArchiveFile",
        CatalogEntryKind::ImageFile => "ImageFile",
        CatalogEntryKind::ImageFolder => "ImageFolder",
        CatalogEntryKind::ContainerDir => "ContainerDir",
        CatalogEntryKind::PlainDir => "PlainDir",
        CatalogEntryKind::Other => "Other",
    }
}

fn cover_dto(state: catalog::CatalogCoverState) -> RemoteCoverStateDto {
    RemoteCoverStateDto {
        state: state.state,
        revision: state.revision,
        ready: state.ready,
        error_code: state.error_code,
        retry_at: state.retry_at,
        is_previous_revision: state.is_previous_revision,
    }
}

pub async fn remote_directory_view(
    source_id: String,
    logical_path: String,
    offset: u32,
    limit: u32,
) -> Result<RemoteDirectoryViewDto, String> {
    let conn = db::get().lock().map_err(|e| e.to_string())?;
    catalog::directory_view_on(&conn, &source_id, &logical_path, offset, limit)
        .map(|view| RemoteDirectoryViewDto {
            source_id: view.source_id,
            logical_path: view.logical_path,
            revision: view.revision,
            listing_complete: view.listing_complete,
            has_more: view.has_more,
            entries: view
                .entries
                .into_iter()
                .map(|entry| RemoteDirectoryEntryDto {
                    asset_id: entry.asset_id,
                    logical_path: entry.logical_path,
                    provider_path: entry.provider_path,
                    name: entry.name,
                    asset_kind: kind_name(entry.kind).into(),
                    size: entry.size,
                    representative_asset_id: entry.representative_asset_id,
                    cover: cover_dto(entry.cover),
                })
                .collect(),
        })
        .map_err(|e| e.to_string())
}

fn profile_key(profile: &CoverProfileDto) -> String {
    format!(
        "{}x{}@{}",
        profile.width, profile.height, profile.decoder_version
    )
}

fn selection_key(selection: &CoverSelectionDto) -> String {
    let explicit = selection
        .explicit_asset_id
        .as_deref()
        .unwrap_or_default()
        .trim();
    if !selection.revision.trim().is_empty() {
        // The caller normally supplies a deterministic revision. Include an
        // explicit representative as a final component nevertheless, so two
        // otherwise-identical selections cannot share a cover job by mistake.
        return if explicit.is_empty() {
            selection.revision.clone()
        } else {
            format!("{}|asset:{explicit}", selection.revision)
        };
    }
    let crop = selection
        .crop
        .as_ref()
        .map(|crop| format!("{:.5},{:.5},{:.5},{:.5}", crop.x, crop.y, crop.w, crop.h));
    format!(
        "page:{}|crop:{}|asset:{}",
        selection.page,
        crop.unwrap_or_default(),
        explicit
    )
}

fn route_revision_on(
    conn: &rusqlite::Connection,
    source_id: &str,
    asset_id: &str,
) -> Result<RouteRevision, String> {
    // During a running generation preview is the freshest route. This is
    // important for opaque providers (115/夸克/百度), where the file id can
    // change even when the normalized logical path remains the same.
    let preview = conn
        .query_row(
            "SELECT p.logical_path,p.source_fingerprint,p.generation,p.session_epoch,
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
                Ok(RouteRevision {
                    logical_path: row.get(0)?,
                    source_fingerprint: row.get(1)?,
                    generation: row.get(2)?,
                    session_epoch: row.get(3)?,
                    provider_id: row.get(4)?,
                    provider_file_id: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|_| "资源尚未完成索引".to_string())?;
    if let Some(preview) = preview {
        return Ok(preview);
    }
    let route = conn
        .query_row(
        "SELECT logical_path,source_fingerprint,generation,session_epoch,provider_id,provider_file_id
         FROM remote_asset_route WHERE source_id=?1 AND asset_id=?2",
        params![source_id, asset_id],
        |row| {
            Ok(RouteRevision {
                logical_path: row.get(0)?,
                source_fingerprint: row.get(1)?,
                generation: row.get(2)?,
                session_epoch: row.get(3)?,
                provider_id: row.get(4)?,
                provider_file_id: row.get(5)?,
            })
        },
        )
        .optional()
        .map_err(|_| "资源尚未完成索引".to_string())?;
    route.ok_or_else(|| "资源尚未完成索引".to_string())
}

fn route_session_matches_on(
    conn: &rusqlite::Connection,
    source_id: &str,
    route: &RouteRevision,
    session: u64,
) -> bool {
    let token = match i64::try_from(session) {
        Ok(token) => token,
        Err(_) => return false,
    };
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM remote_scan_epoch
            WHERE source_id=?1 AND generation=?2
              AND session_epoch=?3 AND session_token=?4)",
        params![source_id, route.generation, route.session_epoch, token],
        |row| row.get(0),
    )
    .unwrap_or(false)
}

pub async fn remote_cover_request(
    source_id: String,
    session: u64,
    asset_id: String,
    consumer_id: String,
    selection: CoverSelectionDto,
    profile: CoverProfileDto,
) -> Result<RemoteCoverStateDto, String> {
    if session == 0 {
        return Err("登录状态已失效".into());
    }
    // Constructing the adapter validates that the runtime session still
    // exists; no listing or download is performed here.
    let conn = db::get().lock().map_err(|e| e.to_string())?;
    let mut route = route_revision_on(&conn, &source_id, &asset_id)?;
    if !route_session_matches_on(&conn, &source_id, &route, session) {
        // A completed generation can be safely rebound after process
        // restart; running/failed generations remain rejected so an expired
        // provider session cannot publish into the new one.
        let rebound = crate::remote_scan::persistence::rebind_completed_generation_session(
            &conn,
            &source_id,
            route.generation,
            session,
        )
        .map_err(|_| "登录状态已失效".to_string())?;
        if rebound.is_some() {
            route = route_revision_on(&conn, &source_id, &asset_id)?;
        }
    }
    if !route_session_matches_on(&conn, &source_id, &route, session) {
        return Err("登录状态已失效".into());
    }
    let RouteRevision {
        logical_path,
        source_fingerprint: route_fingerprint,
        generation,
        session_epoch,
        provider_id,
        provider_file_id,
    } = route;
    let live_in_library: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM library_index
             WHERE source_id=?1 AND id=?2 AND deleted=0)",
            params![source_id, asset_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    let live_in_preview: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM remote_scan_preview p
                JOIN remote_scan_state s ON s.source_id=p.source_id
                                         AND s.generation=p.generation
                                         AND s.status='Running'
                WHERE p.source_id=?1 AND p.asset_id=?2 AND p.session_epoch <> '')",
            params![source_id, asset_id],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !live_in_library && !live_in_preview {
        return Err("远程资源已失效".into());
    }
    let current_fingerprint: Option<String> = conn
        .query_row(
            "SELECT fingerprint FROM book_sources WHERE id=?1",
            [&source_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(current_fingerprint) = current_fingerprint.filter(|value| !value.is_empty()) {
        if current_fingerprint != route_fingerprint {
            return Err("书源配置已变更".into());
        }
    }
    // Persist an explicit user-selected representative when supplied.  The
    // selection is only accepted for a live asset from the same source; the
    // directory projection remains atomic and is still validated by catalog
    // queries on the next refresh.
    if let Some(explicit_asset_id) = selection
        .explicit_asset_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let explicit: Option<(Option<String>, String)> = conn
            .query_row(
                "SELECT parent_id,entry_type FROM library_index
                 WHERE source_id=?1 AND id=?2 AND deleted=0",
                params![source_id, explicit_asset_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let explicit = match explicit {
            Some(value) => Some(value),
            None => conn
                .query_row(
                    "SELECT parent_asset_id,entry_type FROM remote_scan_preview
                     WHERE source_id=?1 AND asset_id=?2
                     ORDER BY generation DESC LIMIT 1",
                    params![source_id, explicit_asset_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| e.to_string())?,
        };
        let Some((explicit_parent, _explicit_kind)) = explicit else {
            return Err("自定义封面资源已失效".into());
        };
        let directory_asset_id = conn
            .query_row(
                "SELECT entry_type FROM library_index WHERE source_id=?1 AND id=?2",
                params![source_id, asset_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .filter(|entry_type| entry_type == "dir")
            .map(|_| asset_id.clone())
            .or_else(|| {
                conn.query_row(
                    "SELECT entry_type FROM remote_scan_preview
                     WHERE source_id=?1 AND asset_id=?2
                     ORDER BY generation DESC LIMIT 1",
                    params![source_id, asset_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .ok()
                .flatten()
                .filter(|entry_type| entry_type == "dir")
                .map(|_| asset_id.clone())
            })
            .or(explicit_parent);
        if let Some(directory_asset_id) = directory_asset_id {
            conn.execute(
                "INSERT INTO remote_directory_cover(
                    source_id,directory_asset_id,representative_asset_id,
                    selection_reason,revision,completeness)
                 VALUES(?1,?2,?3,'user_explicit',?4,'complete')
                 ON CONFLICT(source_id,directory_asset_id) DO UPDATE SET
                    representative_asset_id=excluded.representative_asset_id,
                    selection_reason=excluded.selection_reason,
                    revision=excluded.revision,
                    completeness=excluded.completeness",
                params![
                    source_id,
                    directory_asset_id,
                    explicit_asset_id,
                    db::now_ms()
                ],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    if provider_id != "unknown" {
        let adapter = crate::api::source::remote_provider_adapter(&provider_id, session, "/")
            .map_err(|_| "登录状态已失效".to_string())?;
        if let Some(provider_file_id) = provider_file_id.as_deref() {
            adapter.register_path(&logical_path, provider_file_id);
        }
    }
    let content_revision: Option<String> = conn
        .query_row(
            "SELECT COALESCE(content_fingerprint,'') FROM library_index WHERE id=?1 AND source_id=?2",
            params![asset_id, source_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();
    let content_revision = content_revision
        .filter(|value| !value.is_empty())
        .or_else(|| {
            conn.query_row(
                "SELECT content_fingerprint FROM remote_scan_preview
                 WHERE source_id=?1 AND asset_id=?2
                 ORDER BY generation DESC LIMIT 1",
                params![source_id, asset_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .ok()
            .flatten()
            .flatten()
            .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| session_epoch.clone());
    let key = CoverJobKey {
        source_id: source_id.clone(),
        asset_id: asset_id.clone(),
        content_revision,
        selection_revision: selection_key(&selection),
        profile: profile_key(&profile),
    };
    let job = cover_store::upsert_job_on(
        &conn,
        &key,
        CoverJobState::Pending,
        "visible",
        300,
        generation,
        &session_epoch,
        db::now_ms(),
        CoverJobUpsertCause::Demand,
    )
    .map_err(|e| e.to_string())?;
    cover_service::consumers().attach(&consumer_id, &key);
    crate::api::remote_scan::wake_remote_cover_worker(source_id.clone(), session);
    Ok(RemoteCoverStateDto {
        state: job.state.as_str().into(),
        revision: job.updated_at,
        ready: job.state == CoverJobState::Ready,
        error_code: job.error_code,
        retry_at: job.next_attempt_at,
        is_previous_revision: false,
    })
}

pub async fn remote_cover_read(
    source_id: String,
    asset_id: String,
    selection: CoverSelectionDto,
    profile: CoverProfileDto,
) -> Result<Option<PageImage>, String> {
    let selection_revision = selection_key(&selection);
    let profile_key = profile_key(&profile);
    let cached =
        cover_service::read_cached_cover(&source_id, &asset_id, &selection_revision, &profile_key)?;
    if cached.is_none() {
        // P1-B：只读路径探测到"bytes 缺失"时，`read_cached_cover` 可能刚刚把一条
        // `ready` 记录对账回 `pending`。此处必须保证**确实存在消费者**，否则这本漫画
        // 的封面会永久停在 pending，直到下一次全量扫描才可能被重试。
        //
        // 用 source-level wake：调用方只需要稳定的 source identity，不需要持有
        // runtime session token。当前没有可用 session 时它是 no-op（pending 保持
        // 持久化、不伪造 session、不做任何远程访问）。
        crate::api::remote_scan::wake_cover_worker_for_source(&source_id);
    }
    Ok(cached.map(|(rgba, width, height)| PageImage {
        rgba,
        width,
        height,
    }))
}

pub fn remote_cover_release(consumer_id: String) -> Result<(), String> {
    cover_service::consumers().release(&consumer_id);
    Ok(())
}

/// 用户主动"重试失败封面"：把该源**当前档**的终态失败重新排队，返回本次排队数。
///
/// 为什么需要（2026-09-21 真机）：失败是粘性的，而失败原因可能早已修好
/// （真机 DB 里成片的 `cover_native_lib_missing` 就是 Release 缺 pdfium.dll 那几分钟留下的），
/// 界面却一直显示"获取失败"——需要一个**轻量**逃生口（不清缓存、不动其它档）。
pub fn remote_cover_retry_failed(source_id: String, limit: u32) -> Result<u32, String> {
    let source_id = source_id.trim().to_string();
    if source_id.is_empty() {
        return Ok(0);
    }
    let now = db::now_ms();
    let requeued = {
        let conn = db::get().lock().map_err(|error| error.to_string())?;
        let profile = crate::api::remote_scan::cover_quality_profile_on(&conn);
        cover_store::requeue_failed_for_source_on(&conn, &source_id, &profile, limit, now)
            .map_err(|error| error.to_string())?
    };
    if requeued > 0 {
        crate::remote_scan::cover_revision_stream::notify_cover_revision(&source_id, None);
        crate::api::remote_scan::wake_cover_worker_for_source(&source_id);
    }
    Ok(requeued)
}

pub fn remote_cover_retry(
    source_id: String,
    session: u64,
    asset_ids: Vec<String>,
) -> Result<(), String> {
    if session == 0 {
        return Err("登录状态已失效".into());
    }
    let source_type: String = db::get()
        .lock()
        .map_err(|e| e.to_string())?
        .query_row(
            "SELECT type FROM book_sources WHERE id=?1",
            [&source_id],
            |row| row.get(0),
        )
        .map_err(|_| "书源不存在".to_string())?;
    crate::api::source::remote_provider_adapter(&source_type, session, "/")
        .map_err(|_| "登录状态已失效".to_string())?;
    let conn = db::get().lock().map_err(|e| e.to_string())?;
    if let Some(generation) = conn
        .query_row(
            "SELECT generation FROM remote_scan_state
             WHERE source_id=?1 AND status='Succeeded'",
            [&source_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
    {
        let _ = crate::remote_scan::persistence::rebind_completed_generation_session(
            &conn, &source_id, generation, session,
        );
    }
    let mut sql = String::from(
        "UPDATE remote_cover_job SET state='pending',attempt=0,next_attempt_at=NULL,
         error_code=NULL,updated_at=?1,
         generation=COALESCE((SELECT route.generation FROM remote_asset_route route
                               WHERE route.source_id=remote_cover_job.source_id
                                 AND route.asset_id=remote_cover_job.asset_id),generation),
          session_epoch=COALESCE((SELECT route.session_epoch FROM remote_asset_route route
                                  WHERE route.source_id=remote_cover_job.source_id
                                    AND route.asset_id=remote_cover_job.asset_id),session_epoch)
         WHERE source_id=?2 AND state IN ('retry_wait','failed')",
    );
    if !asset_ids.is_empty() {
        sql.push_str(" AND asset_id IN (");
        sql.push_str(
            &std::iter::repeat_n("?", asset_ids.len())
                .collect::<Vec<_>>()
                .join(","),
        );
        sql.push(')');
    }
    let mut values: Vec<Box<dyn rusqlite::ToSql>> =
        vec![Box::new(db::now_ms()), Box::new(source_id.clone())];
    values.extend(
        asset_ids
            .into_iter()
            .map(|id| Box::new(id) as Box<dyn rusqlite::ToSql>),
    );
    conn.execute(
        &sql,
        rusqlite::params_from_iter(values.iter().map(|value| value.as_ref())),
    )
    .map_err(|e| e.to_string())?;
    drop(conn);
    crate::api::remote_scan::wake_remote_cover_worker(source_id, session);
    Ok(())
}

/// P1-E：cover durable truth 可能变化的**事实**。
///
/// 刻意**不含** authoritative 状态：没有 state / error_code / retry 信息 /
/// authoritative revision；consumer 收到后必须自己重读 durable state。
pub struct CoverRevisionEvent {
    /// 发生变化的 source。
    pub source_id: String,
    /// transition 天然知道时才带；无法确定唯一 asset（例如一次 compensation
    /// batch 改了多本）时为 None，由 Dart 让该 source 的 mounted cards 各自 reread。
    pub asset_id: Option<String>,
}

/// P1-E：Dart coordinator 订阅 cover revision wake-up。
///
/// **单 subscriber**：再次调用**替换**当前 sink（resubscribe），不做 broadcast
/// registry；send 失败只清掉失效 sink，不影响 worker / durable state。
/// 传输是 best-effort，且事件只在 durable mutation **提交成功之后**发出。
pub fn subscribe_cover_revisions(sink: StreamSink<CoverRevisionEvent>) {
    crate::remote_scan::cover_revision_stream::install_sink(sink);
}

/// P1-E：**只读**读取某 asset 当前的 durable cover state。
///
/// 契约（硬）：
/// * read only —— **不** enqueue、**不** wake、**不** 建 session、**不** 访问 provider、
///   **不** 修改任何 durable state，也**不** bump revision；
/// * 直接复用内部 `catalog::cover_state_for` 与既有 `cover_dto`，不另建平行状态 enum；
/// * `Ok(None)` 表示该 asset **没有**任何 job/variant 记录（UI 语义 = "no job"）。
///
/// 这是 UI 消费 durable truth 的唯一读入口 —— 禁止用 `remote_cover_request`（它会 enqueue）
/// 代替本函数。
///
/// **第 79 轮续（真机 bug 修复）**：必须传入调用方**实际使用**的 `selection` + `profile`。
/// 曾经这里读的是写死的 `default` / `340x480@1`，而卡片按 `coverQuality` 请求
/// `170x240@1` ⇒ 图已 ready 但墙面仍按另一 profile 的旧 `failed` 显示"获取失败"。
pub fn remote_cover_state(
    source_id: String,
    asset_id: String,
    selection: CoverSelectionDto,
    profile: CoverProfileDto,
) -> Result<Option<RemoteCoverStateDto>, String> {
    let source_id = source_id.trim().to_string();
    let asset_id = asset_id.trim().to_string();
    if source_id.is_empty() || asset_id.is_empty() {
        return Ok(None);
    }
    let selection_revision = selection_key(&selection);
    let profile_key = profile_key(&profile);
    let conn = db::get().lock().map_err(|error| error.to_string())?;
    // "no job" 必须按**是否存在任何相关 durable 记录**判定，而不是猜测某个 state 字符串。
    // 谓词与读路径自身使用的键完全一致（只读 EXISTS，无副作用）。
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM remote_cover_job
                  WHERE source_id=?1 AND asset_id=?2
                    AND selection_revision=?3 AND profile=?4
                 UNION ALL
                 SELECT 1 FROM remote_cover_variant
                  WHERE source_id=?1 AND asset_id=?2
                    AND selection_revision=?3 AND profile=?4)",
            params![source_id, asset_id, selection_revision, profile_key],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !exists {
        return Ok(None);
    }
    let state = catalog::cover_state_for(
        &conn,
        &source_id,
        Some(&asset_id),
        &selection_revision,
        &profile_key,
    )
    .map_err(|error| error.to_string())?;
    Ok(Some(cover_dto(state)))
}

pub fn remote_view_revision(source_id: String) -> Result<i64, String> {
    let conn = db::get().lock().map_err(|e| e.to_string())?;
    cover_store::view_revision(&conn, &source_id).map_err(|e| e.to_string())
}

/// 第 80 轮：封面缓存"重置到当前档位"的结果摘要（供界面提示与测试断言）。
pub struct RemoteCoverResetDto {
    /// 被删除的、**非当前档位**的变体行数。
    pub purged_variants: u32,
    /// 被删除的、**非当前档位**的任务行数。
    pub purged_jobs: u32,
    /// 被删除的孤儿 blob 行数（及其磁盘文件）。
    pub purged_blobs: u32,
    /// 重新排队（attempt 归零）的终态失败任务数。
    pub requeued_jobs: u32,
    /// 被 bump 封面 revision 的源数量。
    pub sources: u32,
    /// 生效的档位（便于界面显示"已切到 340x480@1"）。
    pub profile: String,
}

/// 第 80 轮：把封面缓存**重置到当前 `coverQuality` 档位**，并让终态失败重新有机会。
///
/// 为什么需要（有真机/桌面数据支撑）：
/// 1. **抓错档**：扫描过去固定抓 340×480，而卡片按设置取图（低 = 170×240）⇒ 实测
///    「金牌得主」目录 340 档 ready 15 本、170 档只有 15 本而另一批 11 本是预算失败；
/// 2. **终态失败不会自愈**：`attempt>=3` 判永久失败（`next_attempt_at=0`、无 6h 补偿），
///    用户库里 `1.pdf`–`7.pdf`（attempt 4–5）与 8 个 MOBI 都卡死在这种状态 ——
///    清掉它们重新排队，是让"改过上限/修过 bug"之后的重新尝试真正生效的唯一路径。
///
/// 做四件事（幂等，只动**封面**数据，不碰书架索引）：
/// 1. 删除非当前档的 `remote_cover_job` / `remote_cover_variant` 行；
/// 2. 删除不再被任何变体或引用指向的 `remote_cover_blob` 行，并删除其磁盘文件；
/// 3. 把当前档里 **`state='failed'`** 的任务重新排队（`attempt=0`、清错误码与租约）；
///    `blocked`（需重新登录）与 `unsupported`（格式本身给不出封面）保持原样 —— 重试无意义；
/// 4. bump 各源封面 revision，界面据此重新读 durable state。
pub fn remote_cover_reset_to_current_profile() -> Result<RemoteCoverResetDto, String> {
    let conn = db::get().lock().map_err(|error| error.to_string())?;
    let profile =
        crate::api::remote_scan::cover_quality_profile_on(&conn);
    let now = db::now_ms();

    // 1) 非当前档的任务与变体。
    let purged_jobs = conn
        .execute("DELETE FROM remote_cover_job WHERE profile <> ?1", params![profile])
        .map_err(|error| error.to_string())? as u32;
    let purged_variants = conn
        .execute(
            "DELETE FROM remote_cover_variant WHERE profile <> ?1",
            params![profile],
        )
        .map_err(|error| error.to_string())? as u32;

    // 2) 孤儿 blob：先取出路径，删行，再删文件（文件删不掉不阻塞 —— 下次启动的 GC 会再试）。
    let orphans: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT blob_key,relative_path FROM remote_cover_blob
                  WHERE blob_key NOT IN (
                        SELECT blob_key FROM remote_cover_variant
                         WHERE blob_key IS NOT NULL AND blob_key <> ''
                        UNION
                        SELECT blob_key FROM remote_cover_ref)",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())?;
        let mut paths = Vec::new();
        for (blob_key, relative_path) in rows {
            conn.execute(
                "DELETE FROM remote_cover_blob WHERE blob_key=?1",
                params![blob_key],
            )
            .map_err(|error| error.to_string())?;
            if !relative_path.is_empty() {
                paths.push(relative_path);
            }
        }
        paths
    };
    let root = crate::cache::cache_root();
    for relative_path in &orphans {
        let _ = std::fs::remove_file(root.join(relative_path));
    }

    // 3) 当前档的终态失败 ⇒ 重新排队。
    let requeued_jobs = conn
        .execute(
            "UPDATE remote_cover_job
                SET state='pending',attempt=0,error_code=NULL,next_attempt_at=NULL,
                    lease_owner=NULL,lease_until=NULL,long_retry_pending=0,
                    long_retry_consumed=0,long_retry_not_before=NULL,updated_at=?1
              WHERE profile=?2 AND state='failed'",
            params![now, profile],
        )
        .map_err(|error| error.to_string())? as u32;

    // 4) 让界面重新读 durable state。
    let sources: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT source_id FROM remote_cover_job GROUP BY source_id
                 UNION
                 SELECT source_id FROM remote_cover_variant GROUP BY source_id",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())?;
        rows
    };
    for source_id in &sources {
        // listing_generation 用 0：重置不改变目录清单，只让封面 revision 前进，
        // 界面据此重读 durable state（与扫描完成时的 bump 同一张表）。
        let _ = cover_store::bump_view_revision_on(&conn, source_id, 0, now);
    }

    Ok(RemoteCoverResetDto {
        purged_variants,
        purged_jobs,
        purged_blobs: orphans.len() as u32,
        requeued_jobs,
        sources: sources.len() as u32,
        profile,
    })
}

#[cfg(test)]
mod reset_tests {
    use super::*;
    use crate::remote_scan::cover_model::{CoverJobKey, CoverJobState};
    use crate::remote_scan::cover_state::CoverJobUpsertCause;

    /// 第 80 轮：重置只清"非当前档"，当前档的 ready 必须原样保留，
    /// 当前档的终态 failed 必须重新排队（attempt 归零），blocked/unsupported 不动。
    #[test]
    fn reset_purges_other_profiles_and_requeues_terminal_failures() {
        // 注意：API 自己会锁 `db::get()`，所以播种必须在**作用域内**完成并释放锁，
        // 否则 std::sync::Mutex 不可重入 ⇒ 自死锁（第一版就是这样挂住的）。
        let source = "reset-contract";
        let current;
        let other;
        {
            let conn = db::get().lock().unwrap();
            crate::remote_scan::persistence::migrate(&conn).unwrap();
            cover_store::migrate(&conn).unwrap();
            for table in ["remote_cover_job", "remote_cover_variant", "remote_scan_epoch"] {
                conn.execute(&format!("DELETE FROM {table} WHERE source_id=?1"), [source])
                    .unwrap();
            }
            let key = |profile: &str, asset: &str| CoverJobKey {
                source_id: source.into(),
                asset_id: asset.into(),
                content_revision: "c1".into(),
                selection_revision: cover_store::DEFAULT_SELECTION_REVISION.into(),
                profile: profile.into(),
            };
            let seed = |profile: &str, asset: &str, state: CoverJobState| {
                cover_store::upsert_job_on(
                    &conn,
                    &key(profile, asset),
                    state,
                    "visible",
                    300,
                    1,
                    "e1",
                    1_000,
                    CoverJobUpsertCause::Demand,
                )
                .unwrap();
            };
            current = crate::api::remote_scan::cover_quality_profile_on(&conn);
            other = if current == "170x240@1" { "340x480@1" } else { "170x240@1" };
            seed(&current, "keep-ready", CoverJobState::Ready);
            seed(&current, "requeue-me", CoverJobState::Failed);
            seed(&current, "stay-blocked", CoverJobState::Blocked);
            seed(other, "other-ready", CoverJobState::Ready);
            seed(other, "other-failed", CoverJobState::Failed);
        }

        let result = remote_cover_reset_to_current_profile().unwrap();
        assert_eq!(result.profile, current);
        assert!(result.purged_jobs >= 2, "另一档的两条必须被清掉");
        assert!(result.requeued_jobs >= 1, "当前档的 failed 必须重新排队");

        let conn = db::get().lock().unwrap();
        let key = |profile: &str, asset: &str| CoverJobKey {
            source_id: source.into(),
            asset_id: asset.into(),
            content_revision: "c1".into(),
            selection_revision: cover_store::DEFAULT_SELECTION_REVISION.into(),
            profile: profile.into(),
        };
        let other_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM remote_cover_job WHERE profile=?1",
                params![other],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(other_rows, 0, "非当前档不得残留");

        let (state, attempt): (String, i64) = conn
            .query_row(
                "SELECT state,attempt FROM remote_cover_job WHERE job_key=?1",
                params![key(&current, "requeue-me").encode()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, "pending");
        assert_eq!(attempt, 0, "重排队必须把 attempt 归零（否则又会被 attempt>=3 判死）");

        let blocked: String = conn
            .query_row(
                "SELECT state FROM remote_cover_job WHERE job_key=?1",
                params![key(&current, "stay-blocked").encode()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(blocked, "blocked", "blocked 重试无意义，必须保持原样");

        let ready: String = conn
            .query_row(
                "SELECT state FROM remote_cover_job WHERE job_key=?1",
                params![key(&current, "keep-ready").encode()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ready, "ready", "当前档已 ready 的封面不得被动到");
    }
}
