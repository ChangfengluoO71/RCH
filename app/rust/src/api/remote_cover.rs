//! Unified local catalog and cover-demand API.
//!
//! Directory views and cover reads are deliberately cache/SQLite only.  A
//! request merely records demand and wakes the cover worker; it never waits on
//! provider I/O, which keeps opening a cloud root responsive.

use crate::api::book::{CropRect, PageImage};
use crate::db;
use crate::remote_scan::catalog::{self, CatalogEntryKind};
use crate::remote_scan::cover_model::{CoverJobKey, CoverJobState};
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

pub fn remote_view_revision(source_id: String) -> Result<i64, String> {
    let conn = db::get().lock().map_err(|e| e.to_string())?;
    cover_store::view_revision(&conn, &source_id).map_err(|e| e.to_string())
}
