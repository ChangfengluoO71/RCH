//! 资料库语义层（Phase 6.0）。
//!
//! 为 UI 提供明确的"可用性/归属"DTO，Flutter 只做 DTO → 展示，
//! 不在 Dart 侧重新判断 remote_only / fingerprint 等语义（避免 UI 与 Rust 模型漂移）。
//!
//! 三状态（由 status 字段表达，UI 映射图标）：
//! - read（🟢）：本机可直接阅读（本地资源存在）
//! - needs_network（🟡）：本机有书源/凭据，但需连接或资源暂不可直接读取
//! - index_only（⚪）：只有同步索引，本机没有对应资源（不是"不可用"）

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{params, OptionalExtension};

use crate::{db, sync};

/// 书源可用性（设备树节点）。
pub struct SourceAvailabilityDto {
    pub source_id: String,
    pub fingerprint: String,
    pub name: String,
    pub r#type: String,
    /// 书源根路径（离线浏览/在线浏览的初始目录；ADR-028 §12.5 兜底不再用 '/'）。
    pub path: String,
    /// 本机配置的书源（非同步进来的远端行）。
    pub has_local_source: bool,
    /// 本地/SMB 路径在本机存在。
    pub has_local_resource: bool,
    /// 本机具备该书源所需凭据（云端）。
    pub has_credentials: bool,
    pub device_id: String,
    pub device_name: String,
    pub is_remote: bool,
    /// 离线索引条目数（可离线浏览）。
    pub offline_index_count: i64,
    pub can_browse_offline: bool,
    pub requires_network: bool,
    /// "read" | "needs_network" | "index_only"
    pub status: String,
}

/// 设备 → 书源树节点。
pub struct SourceTreeNodeDto {
    pub device_id: String,
    pub device_name: String,
    pub sources: Vec<SourceAvailabilityDto>,
}

/// 漫画搜索结果（跨设备资料库检索）。
pub struct BookSearchDto {
    pub book_id: String,
    pub source_id: String,
    pub source_name: String,
    pub source_type: String,
    pub path: String,
    pub title: String,
    pub device_id: String,
    pub device_name: String,
    pub is_remote: bool,
    pub status: String,
    pub last_read_at: i64,
    pub tags: String,
}

/// One deduplicated, currently indexed candidate for random reading.
pub struct RandomBookCandidateDto {
    pub book_key: String,
    pub source_id: String,
    pub source_type: String,
    pub path: String,
    pub title: String,
}

/// library_index 目录条目（书源内离线浏览）。
pub struct LibEntryDto {
    pub id: String,
    pub name: String,
    pub path: String,
    pub entry_type: String,
    pub size: Option<i64>,
    pub modified_at: Option<i64>,
    pub cover_path: Option<String>,
    pub hash: Option<String>,
}

/// 某书源离线索引总数（浏览模式判定：有索引 → 离线优先）。
pub fn db_source_index_count(source_id: String) -> Result<i64, String> {
    let conn = db::get().lock().unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM library_index WHERE source_id = ?1 AND deleted = 0",
        rusqlite::params![source_id],
        |r| r.get::<_, i64>(0),
    )
    .map_err(|e| e.to_string())
}

/// 某书源目录的直接子条目（离线浏览；parent_id = book_id(fingerprint, dir)）。
pub fn db_source_dir_entries(
    source_id: String,
    dir_path: String,
) -> Result<Vec<LibEntryDto>, String> {
    let conn = db::get().lock().unwrap();
    let fp: Option<String> = conn
        .query_row(
            "SELECT fingerprint FROM book_sources WHERE id = ?1",
            rusqlite::params![source_id],
            |r| r.get(0),
        )
        .ok();
    let Some(fp) = fp else {
        return Ok(Vec::new());
    };
    let parent_id = db::library_index_id(&fp, &dir_path);
    let mut stmt = conn
        .prepare(
            "SELECT id, name, path, entry_type, size, modified_at, cover_path, hash
             FROM library_index WHERE source_id = ?1 AND parent_id = ?2 AND deleted = 0
             ORDER BY entry_type = 'file', name",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![source_id, parent_id], |r| {
            Ok(LibEntryDto {
                id: r.get(0)?,
                name: r.get(1)?,
                path: r.get(2)?,
                entry_type: r.get(3)?,
                size: r.get(4)?,
                modified_at: r.get(5)?,
                cover_path: r.get(6)?,
                hash: r.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn cloud_has_credentials(
    r#type: &str,
    row: &(
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ),
) -> bool {
    let (username, password, refresh_token, cookie, _client_secret) = row;
    match r#type {
        "webdav" | "sftp" => {
            username.as_deref().map(|u| !u.is_empty()).unwrap_or(false)
                && password.as_deref().map(|p| !p.is_empty()).unwrap_or(false)
        }
        "baidu" => refresh_token
            .as_deref()
            .map(|t| !t.is_empty())
            .unwrap_or(false),
        "115" | "quark" => cookie.as_deref().map(|c| !c.is_empty()).unwrap_or(false),
        _ => false,
    }
}

fn source_status(
    r#type: &str,
    has_local_source: bool,
    has_local_resource: bool,
    has_credentials: bool,
) -> &'static str {
    let is_cloud = r#type != "local" && r#type != "smb";
    if !is_cloud && has_local_resource {
        "read"
    } else if has_local_source && (has_credentials || !is_cloud) {
        "needs_network"
    } else {
        "index_only"
    }
}

/// 设备 → 书源树（本机逻辑书源 + 远端书源按设备分组）。
pub fn db_source_tree() -> Result<Vec<SourceTreeNodeDto>, String> {
    let conn = db::get().lock().unwrap();
    let own_id = db::get_or_create_device_id_on(&conn).map_err(|e| e.to_string())?;
    let own_name = sync::actor::list_devices_on(&conn)
        .into_iter()
        .find(|d| d.device_id == own_id)
        .map(|d| d.device_name)
        .unwrap_or_else(|| "本机".to_string());

    // 离线索引计数（一次分组查询）
    let mut index_counts: HashMap<String, i64> = HashMap::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT source_id, COUNT(*) FROM library_index WHERE deleted = 0 GROUP BY source_id",
    ) {
        if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        {
            for r in rows.flatten() {
                index_counts.insert(r.0, r.1);
            }
        }
    }

    let mut stmt = conn
        .prepare(
            "SELECT id, type, name, path, url, username, password, refresh_token, client_secret, cookie,
                    root_id, client_id, capability_label, fingerprint, remote_only, origin_device_id
             FROM book_sources WHERE deleted = 0",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<_> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,          // id
                r.get::<_, String>(1)?,          // type
                r.get::<_, String>(2)?,          // name
                r.get::<_, String>(3)?,          // path
                r.get::<_, Option<String>>(4)?,  // url
                r.get::<_, Option<String>>(5)?,  // username
                r.get::<_, Option<String>>(6)?,  // password
                r.get::<_, Option<String>>(7)?,  // refresh_token
                r.get::<_, Option<String>>(8)?,  // client_secret
                r.get::<_, Option<String>>(9)?,  // cookie
                r.get::<_, Option<String>>(10)?, // root_id
                r.get::<_, Option<String>>(11)?, // client_id
                r.get::<_, String>(12)?,         // capability_label
                r.get::<_, String>(13)?,         // fingerprint
                r.get::<_, i64>(14)?,            // remote_only
                r.get::<_, Option<String>>(15)?, // origin_device_id
            ))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let mut devices: Vec<SourceTreeNodeDto> = Vec::new();
    let mut by_device: HashMap<String, usize> = HashMap::new();
    let push = |devices: &mut Vec<SourceTreeNodeDto>,
                by_device: &mut HashMap<String, usize>,
                id: String,
                name: String|
     -> usize {
        if let Some(&i) = by_device.get(&id) {
            return i;
        }
        devices.push(SourceTreeNodeDto {
            device_id: id.clone(),
            device_name: name,
            sources: Vec::new(),
        });
        let i = devices.len() - 1;
        by_device.insert(id, i);
        i
    };

    for (
        id,
        r#type,
        name,
        path,
        _url,
        username,
        password,
        refresh_token,
        client_secret,
        cookie,
        _root_id,
        _client_id,
        _capability,
        fingerprint,
        remote_only,
        origin_device_id,
    ) in rows
    {
        let is_remote = remote_only != 0;
        let has_credentials = cloud_has_credentials(
            &r#type,
            &(
                username.clone(),
                password.clone(),
                refresh_token.clone(),
                cookie.clone(),
                client_secret.clone(),
            ),
        );
        let has_local_resource =
            (r#type == "local" || r#type == "smb") && std::path::Path::new(&path).exists();
        let device_id = if is_remote {
            origin_device_id.clone().unwrap_or_else(|| own_id.clone())
        } else {
            own_id.clone()
        };
        let device_name = if device_id == own_id {
            own_name.clone()
        } else {
            sync::actor::list_devices_on(&conn)
                .into_iter()
                .find(|d| d.device_id == device_id)
                .map(|d| d.device_name)
                .unwrap_or_else(|| "其他设备".to_string())
        };
        let idx = push(&mut devices, &mut by_device, device_id, device_name);
        let count = index_counts.get(&id).copied().unwrap_or(0);
        let device_id_out = devices[idx].device_id.clone();
        let device_name_out = devices[idx].device_name.clone();
        let status =
            source_status(&r#type, !is_remote, has_local_resource, has_credentials).to_string();
        let requires_network = r#type != "local" && r#type != "smb" && has_credentials;
        devices[idx].sources.push(SourceAvailabilityDto {
            source_id: id,
            fingerprint,
            name,
            r#type,
            path,
            has_local_source: !is_remote,
            has_local_resource,
            has_credentials,
            device_id: device_id_out,
            device_name: device_name_out,
            is_remote,
            offline_index_count: count,
            can_browse_offline: count > 0,
            requires_network,
            status,
        });
    }

    // 本机设备排最前
    if let Some(i) = by_device.remove(&own_id) {
        let node = devices.remove(i);
        devices.insert(0, node);
    }
    Ok(devices)
}

fn normalized_library_path_sql(path_expr: &str) -> String {
    format!(
        "CASE
            WHEN lower({path_expr}) LIKE '%.cbz' THEN substr({path_expr}, 1, length({path_expr}) - 4)
            WHEN lower({path_expr}) LIKE '%.zip' THEN substr({path_expr}, 1, length({path_expr}) - 4)
            WHEN lower({path_expr}) LIKE '%.cbr' THEN substr({path_expr}, 1, length({path_expr}) - 4)
            WHEN lower({path_expr}) LIKE '%.rar' THEN substr({path_expr}, 1, length({path_expr}) - 4)
            WHEN lower({path_expr}) LIKE '%.cb7' THEN substr({path_expr}, 1, length({path_expr}) - 4)
            WHEN lower({path_expr}) LIKE '%.7z' THEN substr({path_expr}, 1, length({path_expr}) - 3)
            WHEN lower({path_expr}) LIKE '%.cbt' THEN substr({path_expr}, 1, length({path_expr}) - 4)
            WHEN lower({path_expr}) LIKE '%.tar' THEN substr({path_expr}, 1, length({path_expr}) - 4)
            WHEN lower({path_expr}) LIKE '%.azw3' THEN substr({path_expr}, 1, length({path_expr}) - 5) || '.mobi'
            WHEN lower({path_expr}) LIKE '%.azw' THEN substr({path_expr}, 1, length({path_expr}) - 4) || '.mobi'
            ELSE {path_expr}
         END"
    )
}

fn random_book_key_sql(path_expr: &str) -> String {
    let slash_normalized = format!("rtrim(trim(replace({path_expr}, char(92), '/')), '/')");
    let drive_normalized = format!(
        "CASE WHEN substr({slash_normalized}, 2, 1) = ':' \
         THEN lower(substr({slash_normalized}, 1, 1)) || substr({slash_normalized}, 2) \
         ELSE {slash_normalized} END"
    );
    format!(
        "CASE
            WHEN lower({drive_normalized}) LIKE '%.cbz' THEN substr({drive_normalized}, 1, length({drive_normalized}) - 4)
            WHEN lower({drive_normalized}) LIKE '%.zip' THEN substr({drive_normalized}, 1, length({drive_normalized}) - 4)
            WHEN lower({drive_normalized}) LIKE '%.cbr' THEN substr({drive_normalized}, 1, length({drive_normalized}) - 4)
            WHEN lower({drive_normalized}) LIKE '%.rar' THEN substr({drive_normalized}, 1, length({drive_normalized}) - 4)
            WHEN lower({drive_normalized}) LIKE '%.cb7' THEN substr({drive_normalized}, 1, length({drive_normalized}) - 4)
            WHEN lower({drive_normalized}) LIKE '%.7z' THEN substr({drive_normalized}, 1, length({drive_normalized}) - 3)
            WHEN lower({drive_normalized}) LIKE '%.cbt' THEN substr({drive_normalized}, 1, length({drive_normalized}) - 4)
            WHEN lower({drive_normalized}) LIKE '%.tar' THEN substr({drive_normalized}, 1, length({drive_normalized}) - 4)
            WHEN lower({drive_normalized}) LIKE '%.azw3' THEN substr({drive_normalized}, 1, length({drive_normalized}) - 5) || '.mobi'
            WHEN lower({drive_normalized}) LIKE '%.azw' THEN substr({drive_normalized}, 1, length({drive_normalized}) - 4) || '.mobi'
            ELSE {drive_normalized}
         END"
    )
}

fn local_archive_path_predicate_sql() -> String {
    ["li.path", "li.name"]
        .into_iter()
        .flat_map(|expr| {
            [
                ".cbz", ".zip", ".epub", ".cb7", ".7z", ".cbt", ".tar", ".pdf", ".cbr", ".rar",
                ".mobi", ".azw", ".azw3",
            ]
            .into_iter()
            .map(move |ext| format!("lower({expr}) LIKE '%{ext}'"))
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// Resolve an indexed logical path to the current provider-facing identifier.
///
/// Opaque providers (Baidu, 115 and Quark) expose stable catalog paths to the
/// UI, but their open APIs require the file identifier stored in the remote
/// route table. A path that is not a live catalog entry is left untouched so
/// source-browser callers can continue passing provider identifiers directly.
pub fn db_resolve_remote_provider_path(
    source_id: String,
    logical_path: String,
) -> Result<Option<String>, String> {
    let conn = db::get().lock().map_err(|error| error.to_string())?;
    let source: Option<(String, String)> = conn
        .query_row(
            "SELECT type,fingerprint FROM book_sources
             WHERE id=?1 AND deleted=0 AND remote_only=0",
            params![source_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((source_type, fingerprint)) = source else {
        return Ok(None);
    };
    if !matches!(source_type.as_str(), "baidu" | "115" | "quark") {
        return Ok(None);
    }

    let preview_path: Option<String> = conn
        .query_row(
            "SELECT preview.provider_file_id
             FROM remote_scan_preview preview
             JOIN remote_scan_state state
               ON state.source_id=preview.source_id
              AND state.generation=preview.generation
              AND state.status='Running'
             WHERE preview.source_id=?1 AND preview.logical_path=?2
               AND preview.session_epoch<>''
               AND preview.source_fingerprint=?3
               AND preview.provider_file_id IS NOT NULL
               AND preview.provider_file_id<>''
             ORDER BY preview.generation DESC
             LIMIT 1",
            params![source_id, logical_path, fingerprint],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if preview_path.is_some() {
        return Ok(preview_path);
    }

    let provider_path: Option<String> = conn
        .query_row(
            "SELECT route.provider_file_id
             FROM remote_asset_route route
             JOIN library_index item
               ON item.source_id=route.source_id AND item.id=route.asset_id
              AND item.path=route.logical_path AND item.deleted=0
             WHERE route.source_id=?1 AND route.logical_path=?2
               AND route.source_fingerprint=?3
               AND route.provider_file_id IS NOT NULL AND route.provider_file_id<>''
             ORDER BY route.generation DESC,route.route_revision DESC
             LIMIT 1",
            params![source_id, logical_path, fingerprint],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if provider_path.is_some() {
        return Ok(provider_path);
    }

    let is_indexed_book: bool = conn
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM library_index
                 WHERE source_id=?1 AND path=?2 AND deleted=0
                   AND entry_type='file' AND asset_kind='ArchiveFile'
             )",
            params![source_id, logical_path],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if is_indexed_book {
        return Err("远程书籍索引缺少有效的文件映射，请刷新书源后重试".to_string());
    }

    Ok(None)
}

/// Return a random batch from complete catalog data only. Remote candidates
/// require a published listing proof; this API never starts a source session
/// or scans the network.
pub fn db_random_library_candidates(
    excluded_book_keys: Vec<String>,
    unavailable_candidate_keys: Vec<String>,
    limit: i64,
) -> Result<Vec<RandomBookCandidateDto>, String> {
    let conn = db::get().lock().map_err(|error| error.to_string())?;
    let book_key = format!(
        "(s.type || '|' || s.id || '|' || {})",
        random_book_key_sql("li.path")
    );
    let local_archive = local_archive_path_predicate_sql();
    let sql = format!(
        "WITH eligible AS (
            SELECT {book_key} AS book_key, s.id AS source_id, s.type AS source_type,
                   li.path AS path, COALESCE(NULLIF(m.title, ''), li.name) AS title
            FROM library_index li
            JOIN book_sources s ON s.id = li.source_id AND s.deleted = 0 AND s.remote_only = 0
            LEFT JOIN book_metas m ON m.key = {book_key} AND m.deleted = 0
            WHERE li.deleted = 0
              AND {book_key} NOT IN (SELECT value FROM json_each(?1))
              AND {book_key} NOT IN (SELECT value FROM json_each(?2))
              AND (
                (s.type IN ('local', 'smb') AND (
                    (li.entry_type = 'file' AND (li.asset_kind = 'ArchiveFile' OR ({local_archive})))
                    OR (li.entry_type = 'dir' AND li.asset_kind = 'ImageFolder')
                ))
                OR
                (s.type NOT IN ('local', 'smb') AND (
                    (li.entry_type = 'file' AND li.asset_kind = 'ArchiveFile' AND EXISTS (
                        SELECT 1
                        FROM library_index parent
                        JOIN remote_listing_state listing
                          ON listing.source_id = parent.source_id
                         AND listing.logical_path = parent.path
                         AND listing.scan_generation = li.scan_generation
                         AND listing.listing_complete = 1
                        WHERE parent.id = li.parent_id
                          AND parent.source_id = li.source_id
                          AND parent.entry_type = 'dir'
                          AND parent.deleted = 0
                    ))
                    OR (li.entry_type = 'dir' AND li.asset_kind = 'ImageFolder'
                        AND li.listing_complete = 1 AND EXISTS (
                            SELECT 1 FROM remote_listing_state listing
                            WHERE listing.source_id = li.source_id
                              AND listing.logical_path = li.path
                              AND listing.scan_generation = li.scan_generation
                              AND listing.listing_complete = 1
                        ))
                ))
              )
        ), unique_candidates AS (
            SELECT book_key, MIN(source_id) AS source_id, MIN(source_type) AS source_type,
                   MIN(path) AS path, MIN(title) AS title
            FROM eligible
            GROUP BY book_key
        )
        SELECT book_key, source_id, source_type, path, title
        FROM unique_candidates
        ORDER BY RANDOM()
        LIMIT ?3"
    );
    let excluded_json = serde_json::to_string(&excluded_book_keys).map_err(|e| e.to_string())?;
    let unavailable_json =
        serde_json::to_string(&unavailable_candidate_keys).map_err(|e| e.to_string())?;
    let limit = limit.clamp(1, 32);
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(
            rusqlite::params![excluded_json, unavailable_json, limit],
            |row| {
                Ok(RandomBookCandidateDto {
                    book_key: row.get(0)?,
                    source_id: row.get(1)?,
                    source_type: row.get(2)?,
                    path: row.get(3)?,
                    title: row.get(4)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())
}

fn build_book_query(
    source_id: Option<&str>,
    own_id: &str,
    query: &str,
    tags: &[String],
    include_remote: bool,
) -> String {
    let normalized_path = normalized_library_path_sql("li.path");
    let book_key_expr = format!("(s.type || '|' || s.id || '|' || {normalized_path})");
    let mut sql = format!(
        "SELECT li.id, s.id, s.name, s.type, li.path,
                COALESCE(m.title, li.name),
                s.remote_only, s.origin_device_id,
                COALESCE(r.last_read_at, 0),
                s.username, s.password, s.refresh_token, s.cookie, s.client_secret,
                (SELECT GROUP_CONCAT(t.name, ',') FROM book_tags bt
                 JOIN tags t ON t.id = bt.tag_id AND t.deleted = 0
                 WHERE bt.deleted = 0
                   AND bt.book_key = {book_key_expr})
         FROM library_index li
         JOIN book_sources s ON s.id = li.source_id AND s.deleted = 0
         LEFT JOIN book_metas m ON m.key = {book_key_expr} AND m.deleted = 0
         LEFT JOIN read_records r ON r.key = {book_key_expr} AND r.deleted = 0
         WHERE li.deleted = 0",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(sid) = source_id {
        sql.push_str(" AND s.id = ?1");
        params.push(Box::new(sid.to_string()));
    }
    if !query.trim().is_empty() {
        let q = format!("%{}%", query.trim());
        let q_idx = params.len() + 1;
        params.push(Box::new(q));
        let own_idx = params.len() + 1;
        params.push(Box::new(own_id.to_string()));
        sql.push_str(&format!(
            " AND (li.name LIKE ?{q_idx} OR li.path LIKE ?{q_idx} OR m.title LIKE ?{q_idx} OR m.chinese_title LIKE ?{q_idx} OR m.series LIKE ?{q_idx} OR m.author LIKE ?{q_idx} OR s.name LIKE ?{q_idx} OR EXISTS (SELECT 1 FROM sync_devices sd WHERE sd.device_id = COALESCE(s.origin_device_id, ?{own_idx}) AND sd.device_name LIKE ?{q_idx}))"
        ));
    }
    if include_remote {
        // 无过滤
    } else {
        sql.push_str(" AND s.remote_only = 0");
    }
    if !tags.is_empty() {
        sql.push_str(&format!(
            " AND EXISTS (
            SELECT 1 FROM book_tags bt JOIN tags t ON t.id = bt.tag_id AND t.deleted = 0
            WHERE bt.deleted = 0
              AND bt.book_key = {book_key_expr}
              AND t.name IN (",
        ));
        for tag in tags {
            sql.push_str(&format!("?{}", params.len() + 1));
            params.push(Box::new(tag.clone()));
            sql.push(',');
        }
        sql.pop();
        sql.push_str("))");
    }
    sql.push_str(&format!(
        " ORDER BY title COLLATE NOCASE LIMIT ?{} OFFSET ?{}",
        params.len() + 1,
        params.len() + 2
    ));
    sql
}

fn run_book_query(
    conn: &rusqlite::Connection,
    sql: &str,
    params: &[Box<dyn rusqlite::ToSql>],
) -> Result<Vec<BookSearchDto>, String> {
    let own_id = db::get_or_create_device_id_on(&conn).map_err(|e| e.to_string())?;
    let own_name = sync::actor::list_devices_on(&conn)
        .into_iter()
        .find(|d| d.device_id == own_id)
        .map(|d| d.device_name)
        .unwrap_or_else(|| "本机".to_string());

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt
        .query_map(param_refs.as_slice(), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, i64>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, Option<String>>(10)?,
                r.get::<_, Option<String>>(11)?,
                r.get::<_, Option<String>>(12)?,
                r.get::<_, Option<String>>(13)?,
                r.get::<_, Option<String>>(14)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for r in rows.flatten() {
        let (
            book_id,
            source_id,
            source_name,
            source_type,
            path,
            title,
            remote_only,
            origin,
            last_read_at,
            username,
            password,
            refresh_token,
            cookie,
            client_secret,
            tags_str,
        ) = r;
        let is_remote = remote_only != 0;
        let has_credentials = cloud_has_credentials(
            &source_type,
            &(username, password, refresh_token, cookie, client_secret),
        );
        let has_local_resource = (source_type == "local" || source_type == "smb")
            && std::path::Path::new(&path).exists();
        let status = source_status(
            &source_type,
            !is_remote,
            has_local_resource,
            has_credentials,
        )
        .to_string();
        let device_id = if is_remote {
            origin.unwrap_or_else(|| own_id.clone())
        } else {
            own_id.clone()
        };
        let device_name = if device_id == own_id {
            own_name.clone()
        } else {
            sync::actor::list_devices_on(&conn)
                .into_iter()
                .find(|d| d.device_id == device_id)
                .map(|d| d.device_name)
                .unwrap_or_else(|| "其他设备".to_string())
        };
        out.push(BookSearchDto {
            book_id,
            source_id,
            source_name,
            source_type,
            path,
            title,
            device_id,
            device_name,
            is_remote,
            status,
            last_read_at,
            tags: tags_str.unwrap_or_default(),
        });
    }
    Ok(out)
}

/// 跨设备资料库搜索（分页；直接走 SQL，不整载 library_index）。
pub fn db_search_books(
    query: String,
    tags: Vec<String>,
    include_remote: bool,
    limit: i64,
    offset: i64,
) -> Result<Vec<BookSearchDto>, String> {
    let conn = db::get().lock().unwrap();
    let own_id = db::get_or_create_device_id_on(&conn).map_err(|e| e.to_string())?;
    let sql = build_book_query(None, &own_id, &query, &tags, include_remote);
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if !query.trim().is_empty() {
        params.push(Box::new(format!("%{}%", query.trim())));
        params.push(Box::new(own_id));
    }
    for t in &tags {
        params.push(Box::new(t.clone()));
    }
    params.push(Box::new(limit));
    params.push(Box::new(offset));
    run_book_query(&conn, &sql, &params)
}

/// 某书源下的漫画（分页；三级树书源节点展开时懒加载）。
pub fn db_source_books(
    source_id: String,
    query: String,
    tags: Vec<String>,
    limit: i64,
    offset: i64,
) -> Result<Vec<BookSearchDto>, String> {
    let conn = db::get().lock().unwrap();
    let own_id = db::get_or_create_device_id_on(&conn).map_err(|e| e.to_string())?;
    let sql = build_book_query(Some(&source_id), &own_id, &query, &tags, true);
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(source_id)];
    if !query.trim().is_empty() {
        params.push(Box::new(format!("%{}%", query.trim())));
        params.push(Box::new(own_id));
    }
    for t in &tags {
        params.push(Box::new(t.clone()));
    }
    params.push(Box::new(limit));
    params.push(Box::new(offset));
    run_book_query(&conn, &sql, &params)
}
