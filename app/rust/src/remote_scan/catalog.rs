//! Local catalog projection used by the remote directory view API.

use super::cover_store;
use crate::db;
use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogEntryKind {
    ArchiveFile,
    ImageFile,
    ImageFolder,
    ContainerDir,
    PlainDir,
    Other,
}

impl CatalogEntryKind {
    fn parse(value: Option<String>, entry_type: &str) -> Self {
        match value.as_deref().unwrap_or_default() {
            "ArchiveFile" => Self::ArchiveFile,
            "ImageFile" => Self::ImageFile,
            "ImageFolder" => Self::ImageFolder,
            "ContainerDir" => Self::ContainerDir,
            "PlainDir" => Self::PlainDir,
            _ if entry_type == "dir" => Self::PlainDir,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogCoverState {
    pub state: String,
    pub ready: bool,
    pub revision: i64,
    pub error_code: Option<String>,
    pub retry_at: Option<i64>,
    pub is_previous_revision: bool,
}

type CoverVariantRow = (String, i64, Option<String>, Option<i64>, i64, i64);

impl Default for CatalogCoverState {
    fn default() -> Self {
        Self {
            state: "pending".into(),
            ready: false,
            revision: 0,
            error_code: None,
            retry_at: None,
            is_previous_revision: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub asset_id: String,
    pub logical_path: String,
    pub provider_path: Option<String>,
    pub name: String,
    pub kind: CatalogEntryKind,
    pub size: Option<u64>,
    pub representative_asset_id: Option<String>,
    pub cover: CatalogCoverState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryView {
    pub source_id: String,
    pub logical_path: String,
    pub revision: i64,
    pub listing_complete: bool,
    pub has_more: bool,
    pub entries: Vec<CatalogEntry>,
}

#[derive(Debug, Clone)]
struct IndexRow {
    id: String,
    parent_id: Option<String>,
    name: String,
    path: String,
    entry_type: String,
    kind: CatalogEntryKind,
    size: Option<u64>,
    deleted: bool,
}

fn natural_cmp(a: &IndexRow, b: &IndexRow) -> Ordering {
    crate::util::natural_cmp(&a.name, &b.name).then_with(|| a.id.cmp(&b.id))
}

fn load_rows(conn: &Connection, source_id: &str) -> Result<Vec<IndexRow>> {
    let mut stmt = conn.prepare(
        "SELECT id,parent_id,name,path,entry_type,asset_kind,size,deleted
         FROM library_index WHERE source_id=?1",
    )?;
    let rows = stmt
        .query_map([source_id], |row| {
            let entry_type: String = row.get(4)?;
            Ok(IndexRow {
                id: row.get(0)?,
                parent_id: row.get(1)?,
                name: row.get(2)?,
                path: row.get(3)?,
                kind: CatalogEntryKind::parse(row.get(5)?, &entry_type),
                entry_type,
                size: row
                    .get::<_, Option<i64>>(6)?
                    .and_then(|value| u64::try_from(value).ok()),
                deleted: row.get::<_, i64>(7).unwrap_or(0) != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Load the active generation's non-authoritative preview rows. A preview is
/// eligible only while the persisted scan is running and the source
/// fingerprint/session epoch still match; after publish the rows are removed
/// and the normal library index becomes authoritative again.
fn load_preview_rows(
    conn: &Connection,
    source_id: &str,
    source_fingerprint: &str,
) -> Result<(Vec<IndexRow>, HashSet<String>)> {
    let generation: Option<i64> = conn
        .query_row(
            "SELECT generation FROM remote_scan_state
             WHERE source_id=?1 AND status='Running'",
            [source_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(generation) = generation else {
        return Ok((Vec::new(), HashSet::new()));
    };
    let mut stmt = conn.prepare(
        "SELECT asset_id,parent_asset_id,name,logical_path,entry_type,asset_kind,
                size,modified_at
         FROM remote_scan_preview
         WHERE source_id=?1 AND generation=?2 AND source_fingerprint=?3
           AND session_epoch <> ''",
    )?;
    let rows = stmt
        .query_map(params![source_id, generation, source_fingerprint], |row| {
            let entry_type: String = row.get(4)?;
            Ok(IndexRow {
                id: row.get(0)?,
                parent_id: row.get(1)?,
                name: row.get(2)?,
                path: row.get(3)?,
                kind: CatalogEntryKind::parse(row.get(5)?, &entry_type),
                entry_type,
                size: row
                    .get::<_, Option<i64>>(6)?
                    .and_then(|value| u64::try_from(value).ok()),
                deleted: false,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // Empty directories still need to override old children. The stage table
    // records those paths even when the preview row set is empty.
    let mut stage_stmt = conn.prepare(
        "SELECT logical_path FROM remote_scan_listing_stage
         WHERE source_id=?1 AND generation=?2
           AND session_epoch=(SELECT session_epoch FROM remote_scan_epoch
                              WHERE source_id=?1 AND generation=?2)",
    )?;
    let staged_parent_ids = stage_stmt
        .query_map(params![source_id, generation], |row| {
            let path: String = row.get(0)?;
            Ok(crate::db::library_index_id(
                source_fingerprint,
                &super::model::normalize_path(&path),
            ))
        })?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    Ok((rows, staged_parent_ids))
}

fn provider_path_for(conn: &Connection, source_id: &str, asset_id: &str) -> Result<Option<String>> {
    // A running generation may have replaced the provider's opaque id for an
    // existing logical path. Prefer that preview route until publication so
    // the card and the worker use the same current asset identity.
    let preview = conn
        .query_row(
            "SELECT p.provider_file_id FROM remote_scan_preview p
             JOIN remote_scan_state s ON s.source_id=p.source_id
                                      AND s.generation=p.generation
                                      AND s.status='Running'
             WHERE p.source_id=?1 AND p.asset_id=?2
               AND p.session_epoch <> ''
               AND p.source_fingerprint=(SELECT fingerprint FROM book_sources WHERE id=?1)
             ORDER BY p.generation DESC LIMIT 1",
            params![source_id, asset_id],
            |row| row.get(0),
        )
        .optional()?;
    if preview.is_some() {
        return Ok(preview);
    }
    let route = conn
        .query_row(
            "SELECT provider_file_id FROM remote_asset_route WHERE source_id=?1 AND asset_id=?2",
            params![source_id, asset_id],
            |row| row.get(0),
        )
        .optional()?;
    if route.is_some() {
        return Ok(route);
    }
    Ok(route)
}

fn row_is_valid(rows: &HashMap<String, IndexRow>, id: &str) -> bool {
    rows.get(id).is_some_and(|row| !row.deleted)
}

fn descendants_representative(
    rows: &HashMap<String, IndexRow>,
    by_parent: &HashMap<String, Vec<String>>,
    root: &IndexRow,
) -> Option<String> {
    let preferred = match root.kind {
        CatalogEntryKind::ImageFolder => Some(CatalogEntryKind::ImageFile),
        CatalogEntryKind::ContainerDir => Some(CatalogEntryKind::ArchiveFile),
        _ => None,
    };
    let mut queue = VecDeque::from([root.id.clone()]);
    let mut visited = HashSet::new();
    let mut budget = 256usize;
    while let Some(parent) = queue.pop_front() {
        if !visited.insert(parent.clone()) {
            continue;
        }
        if budget == 0 {
            break;
        }
        budget -= 1;
        let mut children = by_parent.get(&parent).cloned().unwrap_or_default();
        children.sort_by(|a, b| natural_cmp(&rows[a], &rows[b]));
        if let Some(kind) = preferred {
            if let Some(id) = children
                .iter()
                .find(|id| rows[*id].kind == kind && !rows[*id].deleted)
            {
                return Some(id.clone());
            }
        }
        for id in children {
            let child = &rows[&id];
            if child.deleted {
                continue;
            }
            if matches!(
                child.kind,
                CatalogEntryKind::ArchiveFile | CatalogEntryKind::ImageFile
            ) && preferred.is_none()
            {
                return Some(id);
            }
            if child.entry_type == "dir" {
                queue.push_back(id);
            }
        }
    }
    None
}

fn representative_for(
    conn: &Connection,
    source_id: &str,
    row: &IndexRow,
    rows: &HashMap<String, IndexRow>,
    by_parent: &HashMap<String, Vec<String>>,
) -> Result<Option<String>> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT representative_asset_id FROM remote_directory_cover
             WHERE source_id=?1 AND directory_asset_id=?2",
            params![source_id, row.id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    if let Some(id) = existing.filter(|id| row_is_valid(rows, id)) {
        return Ok(Some(id));
    }
    let mut children = by_parent.get(&row.id).cloned().unwrap_or_default();
    children.sort_by(|a, b| natural_cmp(&rows[a], &rows[b]));
    let preferred = match row.kind {
        CatalogEntryKind::ImageFolder => CatalogEntryKind::ImageFile,
        CatalogEntryKind::ContainerDir => CatalogEntryKind::ArchiveFile,
        _ => CatalogEntryKind::Other,
    };
    let result = if preferred != CatalogEntryKind::Other {
        children
            .iter()
            .find(|id| rows[*id].kind == preferred && !rows[*id].deleted)
            .cloned()
            .or_else(|| descendants_representative(rows, by_parent, row))
    } else {
        descendants_representative(rows, by_parent, row)
    };
    Ok(result)
}

/// P1-E：只读读取某 asset 的 durable cover state（供 api 层最薄 wrapper 复用）。
///
/// 第 79 轮续（真机 bug）：`selection_revision` 与 `profile` 必须由调用方传入，
/// **不能写死**。真机现象：卡片按设置 `coverQuality` 请求 `170x240@1`（`low`），
/// 而这里写死读 `340x480@1` ⇒ 明明 170 的图已 ready，墙面仍显示 340 那条旧的
/// `failed`（"详情页有图、海报墙获取失败"）。同一 asset 上两个 profile 的状态
/// 可以完全不同（真机库实测：170 ready / 340 failed 有 7 本）。
pub(crate) fn cover_state_for(
    conn: &Connection,
    source_id: &str,
    asset_id: Option<&str>,
    selection_revision: &str,
    profile: &str,
) -> Result<CatalogCoverState> {
    let Some(asset_id) = asset_id else {
        return Ok(CatalogCoverState::default());
    };
    let variant: Option<CoverVariantRow> = conn
        .query_row(
            "SELECT v.state,v.revision,j.error_code,j.next_attempt_at,v.is_previous_revision
                    ,v.updated_at
             FROM remote_cover_variant v
             LEFT JOIN remote_cover_job j
               ON j.source_id=v.source_id AND j.asset_id=v.asset_id
              AND j.content_revision=v.content_revision
              AND j.selection_revision=v.selection_revision
              AND j.profile=v.profile
             WHERE v.source_id=?1 AND v.asset_id=?2
               AND v.selection_revision=?3 AND v.profile=?4
             ORDER BY v.updated_at DESC LIMIT 1",
            params![source_id, asset_id, selection_revision, profile],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()?;
    // A preview page can enqueue a job before any variant exists. Also keep
    // a previous ready image visible while a newer content revision is being
    // processed by selecting whichever default-profile record was updated
    // most recently.
    let job: Option<(String, i64, Option<String>, Option<i64>)> = conn
        .query_row(
            "SELECT state,updated_at,error_code,next_attempt_at
             FROM remote_cover_job
             WHERE source_id=?1 AND asset_id=?2
               AND selection_revision=?3 AND profile=?4
             ORDER BY updated_at DESC LIMIT 1",
            params![source_id, asset_id, selection_revision, profile],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    match (variant, job) {
        (
            Some((
                variant_state,
                _variant_revision,
                _variant_error_code,
                _variant_retry_at,
                _variant_previous,
                variant_updated_at,
            )),
            Some((job_state, job_revision, job_error, job_retry)),
        ) if job_revision > variant_updated_at => {
            let is_previous_revision = variant_state == "ready" && job_state != "ready";
            Ok(CatalogCoverState {
                ready: job_state == "ready",
                state: job_state,
                revision: job_revision,
                error_code: job_error,
                retry_at: job_retry,
                is_previous_revision,
            })
        }
        (Some((state, revision, error_code, retry_at, previous, _)), _) => Ok(CatalogCoverState {
            ready: state == "ready",
            state,
            revision,
            error_code,
            retry_at,
            is_previous_revision: previous != 0,
        }),
        (None, Some((state, revision, error_code, retry_at))) => Ok(CatalogCoverState {
            ready: state == "ready",
            state,
            revision,
            error_code,
            retry_at,
            is_previous_revision: false,
        }),
        (None, None) => Ok(CatalogCoverState::default()),
    }
}

pub fn directory_view_on(
    conn: &Connection,
    source_id: &str,
    logical_path: &str,
    offset: u32,
    limit: u32,
) -> Result<DirectoryView> {
    let path = super::model::normalize_path(logical_path);
    let source_fp: String = conn
        .query_row(
            "SELECT fingerprint FROM book_sources WHERE id=?1 AND fingerprint IS NOT NULL AND fingerprint <> ''",
            [source_id],
            |row| row.get(0),
        )
        .map_err(|e| anyhow!("source identity unavailable: {e}"))?;
    let parent_id = db::library_index_id(&source_fp, &path);
    let all_rows = load_rows(conn, source_id)?;
    let (preview_rows, staged_parent_ids) = load_preview_rows(conn, source_id, &source_fp)?;
    let preview_asset_ids = preview_rows
        .iter()
        .map(|row| row.id.clone())
        .collect::<HashSet<_>>();
    let rows: HashMap<String, IndexRow> = all_rows
        .into_iter()
        .filter(|row| !row.deleted)
        .map(|row| (row.id.clone(), row))
        .collect();
    let mut rows = rows;
    for row in preview_rows {
        rows.insert(row.id.clone(), row);
    }
    let mut by_parent: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows.values() {
        if let Some(parent) = &row.parent_id {
            by_parent
                .entry(parent.clone())
                .or_default()
                .push(row.id.clone());
        }
    }
    // A staged page is authoritative for display of that parent only; absent
    // children must not leak in from the previous complete generation. This
    // does not affect deletion proof because the underlying library rows are
    // untouched until publish.
    if !staged_parent_ids.is_empty() {
        let preview_parent_ids = staged_parent_ids.clone();
        for parent_id in preview_parent_ids {
            let children = rows
                .values()
                .filter(|row| row.parent_id.as_deref() == Some(parent_id.as_str()))
                .filter(|row| preview_asset_ids.contains(&row.id))
                .map(|row| row.id.clone())
                .collect::<Vec<_>>();
            by_parent.insert(parent_id, children);
        }
    }
    let mut child_ids = by_parent.get(&parent_id).cloned().unwrap_or_default();
    child_ids.sort_by(|a, b| natural_cmp(&rows[a], &rows[b]));
    let offset = usize::try_from(offset).unwrap_or(usize::MAX);
    let requested = usize::try_from(limit).unwrap_or(200).min(200);
    let has_more = child_ids.len() > offset.saturating_add(requested);
    let revision = cover_store::view_revision(conn, source_id)?;
    let listing_complete: bool = if staged_parent_ids.contains(&parent_id) {
        false
    } else {
        conn
        .query_row(
            "SELECT listing_complete FROM remote_listing_state WHERE source_id=?1 AND logical_path=?2",
            params![source_id, path],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some_and(|value| value != 0)
    };
    let mut entries = Vec::new();
    for id in child_ids.into_iter().skip(offset).take(requested) {
        let row = &rows[&id];
        let representative = if row.entry_type == "dir" {
            representative_for(conn, source_id, row, &rows, &by_parent)?
        } else {
            None
        };
        // Image-folder covers are queued against the folder asset itself: the
        // worker resolves its first ordered image at extraction time.  Other
        // directory cards use the selected descendant representative.
        let cover_asset = if row.kind == CatalogEntryKind::ImageFolder {
            Some(row.id.as_str())
        } else {
            representative.as_deref().or(Some(row.id.as_str()))
        };
        entries.push(CatalogEntry {
            asset_id: row.id.clone(),
            logical_path: row.path.clone(),
            // Keep the entry's own route separate from the representative
            // asset used for its cover.  A directory card may point at a
            // child representative while navigation still needs the
            // directory's provider identifier.
            provider_path: provider_path_for(conn, source_id, &row.id)?,
            name: row.name.clone(),
            kind: row.kind,
            size: row.size,
            representative_asset_id: representative.clone(),
            // 目录视图里的 cover 只用于"变更检测/预览"（`source_browser.dart` 的 diff），
            // 墙面芯片显示的状态由卡片自己按**它实际请求的** profile 读取
            //（`remote_cover_state`）。这里保持默认展示 profile。
            cover: cover_state_for(
                conn,
                source_id,
                cover_asset,
                crate::remote_scan::cover_store::DEFAULT_SELECTION_REVISION,
                crate::remote_scan::cover_store::DEFAULT_COVER_PROFILE,
            )?,
        });
    }
    Ok(DirectoryView {
        source_id: source_id.to_string(),
        logical_path: path,
        revision,
        listing_complete,
        has_more,
        entries,
    })
}
