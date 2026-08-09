//! 合并结果应用回本地 SQLite（ADR-024 §7 步骤 7）。
//!
//! 身份映射：远端稳定 key（book_id = sha256(fingerprint|path)）不可逆，
//! 因此条目 data 携带 `path`（及必要的源字段），通过"遍历本机 fingerprint 计算候选 book_id"
//! 反查本地 source；凭据字段（password/refresh_token/client_secret/cookie）绝不被远端覆盖。
//!
//! ADR-028 §12.3：resolve 失败**禁止静默跳过**——live 条目写入 sync_pending_apply
//! （参与本地快照，使三方合并不产生伪墓碑）；新源加入后由 `reapply_pending` 落真实表。

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::db;
use crate::sync::base;
use crate::sync::identity;
use crate::sync::merge::SyncEntry;

/// 本地书源身份索引（一次性装载，避免逐条目全表 SQL 查询）。
pub(crate) struct LocalSources {
    rows: Vec<(String, String, String)>, // (id, type, fingerprint)
}

impl LocalSources {
    pub(crate) fn load(conn: &Connection) -> LocalSources {
        let mut stmt = match conn.prepare(
            "SELECT id, type, fingerprint FROM book_sources
             WHERE deleted = 0 AND fingerprint IS NOT NULL AND fingerprint != ''",
        ) {
            Ok(s) => s,
            Err(_) => return LocalSources { rows: Vec::new() },
        };
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        });
        let rows = rows
            .map(|iter| iter.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();
        LocalSources { rows }
    }

    /// 按 (book_id, path) 反查本地 (source_type, source_id)。
    fn resolve(&self, book_id: &str, path: &str) -> Option<(String, String)> {
        for (id, r#type, fp) in &self.rows {
            if identity::book_id(fp, path) == book_id {
                return Some((r#type.clone(), id.clone()));
            }
        }
        None
    }
}

/// ADR-028 §12.3：resolve 失败时把 live 条目写入 pending（待绑定），
/// 墓碑条目清除 pending（本地无真实行可删）。绝不 silent continue。
fn defer_unresolved(conn: &Connection, entity: &str, e: &SyncEntry) -> Result<()> {
    if e.deleted {
        base::delete_pending_on(conn, entity, &e.key)?;
    } else {
        let payload = serde_json::to_string(e).unwrap_or_else(|_| "{}".into());
        base::upsert_pending_on(conn, entity, &e.key, "unresolved_source", &payload)?;
    }
    Ok(())
}

fn apply_sources(
    conn: &Connection,
    writer_device_id: Option<&str>,
    entries: &HashMap<String, SyncEntry>,
) -> Result<()> {
    for (fp, e) in entries {
        if e.deleted {
            if let Some(id) = db::find_source_id_by_fingerprint_on(conn, fp) {
                db::delete_source_on(conn, &id)?;
            }
            continue;
        }
        let d = &e.data;
        let r#type = d["type"].as_str().unwrap_or("").to_string();
        let name = d["name"].as_str().unwrap_or("").to_string();
        let path = d["path"].as_str().unwrap_or("").to_string();
        let url = d["url"].as_str().map(|s| s.to_string());
        let username = d["username"].as_str().map(|s| s.to_string());
        let port = d["port"].as_i64();
        let note = d["note"].as_str().unwrap_or("").to_string();
        let capability = d["capabilityLabel"].as_str().unwrap_or("").to_string();
        let root_id = d["rootId"].as_str().map(|s| s.to_string());
        let client_id = d["clientId"].as_str().map(|s| s.to_string());

        if let Some(existing_id) = db::find_source_id_by_fingerprint_on(conn, fp) {
            // ADR-028 §12.4：本地已有（含本机源/同 fingerprint 合并）→ 只更新配置字段，
            // **不覆盖 remote_only/origin_device_id**：远端负载不再携带这两个字段，
            // 标志由本机 + writer（manifest）派生，避免设备间翻转导致 LWW 重推抖动。
            conn.execute(
                "UPDATE book_sources SET
                    type=?2, name=?3, path=?4, url=?5, username=?6, port=?7, note=?8,
                    capability_label=?9, root_id=?10, client_id=?11, updated_at=?12
                 WHERE id=?1",
                params![existing_id, r#type, name, path, url, username, port, note, capability, root_id, client_id, e.updated_at],
            )?;
        } else {
            let id = format!("sync_{}_{}", &fp[..fp.len().min(8)], e.updated_at);
            // 新 fingerprint → 一律标记为"远端"（remote_only=true + 来源设备=writer），
            // 供 UI 设备分组使用；凭据为空 → ⚪ 仅索引。
            let origin = writer_device_id.map(|s| s.to_string());
            conn.execute(
                "INSERT INTO book_sources
                 (id, type, name, path, url, username, port, note, capability_label,
                  fingerprint, remote_only, origin_device_id, root_id, client_id, updated_at, deleted)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 0)",
                params![id, r#type, name, path, url, username, port, note, capability, fp, true, origin, root_id, client_id, e.updated_at],
            )?;
        }
    }
    Ok(())
}

fn apply_metas(
    conn: &Connection,
    sources: &LocalSources,
    entries: &HashMap<String, SyncEntry>,
) -> Result<()> {
    for (book_id, e) in entries {
        let path = e.data["path"].as_str().unwrap_or("");
        if path.is_empty() {
            continue;
        }
        let Some((source_type, source_id)) = sources.resolve(book_id, path) else {
            // ADR-028：本机无对应书源 → 待绑定，禁止静默跳过（否则下一轮合并会伪删除）
            defer_unresolved(conn, base::ENTITY_METAS, e)?;
            continue;
        };
        if e.deleted {
            base::delete_pending_on(conn, base::ENTITY_METAS, book_id)?;
            let local_key = identity::local_book_key(&source_type, &source_id, path);
            conn.execute("DELETE FROM book_metas WHERE key = ?1", params![local_key])?;
            continue;
        }
        let d = &e.data;
        let local_key = identity::local_book_key(&source_type, &source_id, path);
        conn.execute(
            "INSERT INTO book_metas
             (key, stable_id, cover_page, crop_x, crop_y, crop_w, crop_h,
              author, genre, series, title, chinese_title, summary, comment, rotations, updated_at, deleted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 0)
             ON CONFLICT(key) DO UPDATE SET
                stable_id=excluded.stable_id, cover_page=excluded.cover_page,
                crop_x=excluded.crop_x, crop_y=excluded.crop_y,
                crop_w=excluded.crop_w, crop_h=excluded.crop_h,
                author=excluded.author, genre=excluded.genre, series=excluded.series,
                title=excluded.title, chinese_title=excluded.chinese_title,
                summary=excluded.summary, comment=excluded.comment,
                rotations=excluded.rotations, updated_at=excluded.updated_at",
            params![
                local_key,
                d["stableId"].as_str(),
                d["coverPage"].as_i64().unwrap_or(0),
                d["cropX"].as_f64(),
                d["cropY"].as_f64(),
                d["cropW"].as_f64(),
                d["cropH"].as_f64(),
                d["author"].as_str().unwrap_or(""),
                d["genre"].as_str().unwrap_or(""),
                d["series"].as_str().unwrap_or(""),
                d["title"].as_str().unwrap_or(""),
                d["chineseTitle"].as_str().unwrap_or(""),
                d["summary"].as_str().unwrap_or(""),
                d["comment"].as_str().unwrap_or(""),
                d["rotations"].as_str().unwrap_or("{}"),
                e.updated_at,
            ],
        )?;
        base::delete_pending_on(conn, base::ENTITY_METAS, book_id)?;
    }
    Ok(())
}

fn apply_records(
    conn: &Connection,
    sources: &LocalSources,
    entries: &HashMap<String, SyncEntry>,
) -> Result<()> {
    for (book_id, e) in entries {
        let path = e.data["path"].as_str().unwrap_or("");
        if path.is_empty() {
            continue;
        }
        let Some((source_type, source_id)) = sources.resolve(book_id, path) else {
            defer_unresolved(conn, base::ENTITY_RECORDS, e)?;
            continue;
        };
        let local_key = identity::local_book_key(&source_type, &source_id, path);
        if e.deleted {
            base::delete_pending_on(conn, base::ENTITY_RECORDS, book_id)?;
            conn.execute("DELETE FROM read_records WHERE key = ?1", params![local_key])?;
            continue;
        }
        let d = &e.data;
        conn.execute(
            "INSERT INTO read_records
             (key, stable_id, source_id, source_type, path, title, last_page, read_count, last_read_at, updated_at, deleted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)
             ON CONFLICT(key) DO UPDATE SET
                stable_id=excluded.stable_id, source_id=excluded.source_id,
                source_type=excluded.source_type, path=excluded.path, title=excluded.title,
                last_page=excluded.last_page, read_count=excluded.read_count,
                last_read_at=excluded.last_read_at, updated_at=excluded.updated_at",
            params![
                local_key,
                d["stableId"].as_str(),
                source_id,
                source_type,
                path,
                d["title"].as_str().unwrap_or(""),
                d["lastPage"].as_i64().unwrap_or(0),
                d["readCount"].as_i64().unwrap_or(0),
                d["lastReadAt"].as_i64().unwrap_or(0),
                e.updated_at,
            ],
        )?;
        base::delete_pending_on(conn, base::ENTITY_RECORDS, book_id)?;
    }
    Ok(())
}

fn apply_tags(conn: &Connection, entries: &HashMap<String, SyncEntry>) -> Result<()> {
    for (id, e) in entries {
        if e.deleted {
            conn.execute("DELETE FROM book_tags WHERE tag_id = ?1", params![id])?;
            conn.execute("DELETE FROM tags WHERE id = ?1", params![id])?;
            continue;
        }
        let name = e.data["name"].as_str().unwrap_or(id);
        conn.execute(
            "INSERT INTO tags (id, name, created_at, updated_at, deleted)
             VALUES (?1, ?2, ?3, ?4, 0)
             ON CONFLICT(id) DO UPDATE SET name=excluded.name, updated_at=excluded.updated_at",
            params![id, name, e.data["createdAt"].as_i64().unwrap_or(e.updated_at), e.updated_at],
        )?;
    }
    Ok(())
}

fn apply_book_tags(
    conn: &Connection,
    sources: &LocalSources,
    entries: &HashMap<String, SyncEntry>,
) -> Result<()> {
    for (sync_key, e) in entries {
        let Some((book_id, tag_id)) = sync_key.rsplit_once('|') else {
            continue;
        };
        let path = e.data["path"].as_str().unwrap_or("");
        if path.is_empty() {
            continue;
        }
        let Some((source_type, source_id)) = sources.resolve(book_id, path) else {
            defer_unresolved(conn, base::ENTITY_BOOK_TAGS, e)?;
            continue;
        };
        let local_book_key = identity::local_book_key(&source_type, &source_id, path);
        if e.deleted {
            base::delete_pending_on(conn, base::ENTITY_BOOK_TAGS, sync_key)?;
            conn.execute(
                "DELETE FROM book_tags WHERE book_key = ?1 AND tag_id = ?2",
                params![local_book_key, tag_id],
            )?;
            continue;
        }
        conn.execute(
            "INSERT INTO book_tags (book_key, tag_id, updated_at, deleted)
             VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(book_key, tag_id) DO UPDATE SET updated_at=excluded.updated_at",
            params![local_book_key, tag_id, e.updated_at],
        )?;
        base::delete_pending_on(conn, base::ENTITY_BOOK_TAGS, sync_key)?;
    }
    Ok(())
}

fn apply_library_index(
    conn: &Connection,
    sources: &LocalSources,
    entries: &HashMap<String, SyncEntry>,
) -> Result<()> {
    for (book_id, e) in entries {
        let path = e.data["path"].as_str().unwrap_or("");
        if path.is_empty() {
            continue;
        }
        if e.deleted {
            base::delete_pending_on(conn, base::ENTITY_LIBRARY_INDEX, book_id)?;
            // Phase 5.2：删除不立即 DELETE，保留 deleted=true 墓碑（传播 + 后续 GC）。
            conn.execute(
                "UPDATE library_index SET deleted = 1, updated_at = ?2 WHERE id = ?1",
                params![book_id, e.updated_at],
            )?;
            continue;
        }
        let Some((_st, source_id)) = sources.resolve(book_id, path) else {
            defer_unresolved(conn, base::ENTITY_LIBRARY_INDEX, e)?;
            continue;
        };
        let d = &e.data;
        conn.execute(
            "INSERT INTO library_index
             (id, source_id, parent_id, name, path, entry_type, size, modified_at, cover_path, hash, updated_at, deleted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0)
             ON CONFLICT(id) DO UPDATE SET
                source_id=excluded.source_id, parent_id=excluded.parent_id, name=excluded.name,
                path=excluded.path, entry_type=excluded.entry_type, size=excluded.size,
                modified_at=excluded.modified_at, cover_path=excluded.cover_path, hash=excluded.hash,
                deleted=0, updated_at=excluded.updated_at",
            params![
                book_id,
                source_id,
                d["parentId"].as_str(),
                d["name"].as_str().unwrap_or(""),
                path,
                d["entryType"].as_str().unwrap_or("file"),
                d["size"].as_i64(),
                d["modifiedAt"].as_i64(),
                d["coverPath"].as_str(),
                d["hash"].as_str(),
                e.updated_at,
            ],
        )?;
        base::delete_pending_on(conn, base::ENTITY_LIBRARY_INDEX, book_id)?;
    }
    Ok(())
}

fn apply_settings(conn: &Connection, entries: &HashMap<String, SyncEntry>) -> Result<()> {
    for (key, e) in entries {
        if !db::is_syncable_setting(key) {
            continue;
        }
        if e.deleted {
            conn.execute("DELETE FROM app_settings WHERE key = ?1", params![key])?;
            continue;
        }
        let value = e.data["value"].as_str().unwrap_or("");
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at, deleted)
             VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
            params![key, value, e.updated_at],
        )?;
    }
    Ok(())
}

/// 应用某实体的合并结果。
pub fn apply_merged(
    conn: &Connection,
    entity: &str,
    entries: &HashMap<String, SyncEntry>,
    writer_device_id: Option<&str>,
) -> Result<()> {
    let sources = LocalSources::load(conn);
    match entity {
        base::ENTITY_SOURCES => apply_sources(conn, writer_device_id, entries),
        base::ENTITY_METAS => apply_metas(conn, &sources, entries),
        base::ENTITY_RECORDS => apply_records(conn, &sources, entries),
        base::ENTITY_TAGS => apply_tags(conn, entries),
        base::ENTITY_BOOK_TAGS => apply_book_tags(conn, &sources, entries),
        base::ENTITY_LIBRARY_INDEX => apply_library_index(conn, &sources, entries),
        base::ENTITY_SETTINGS => apply_settings(conn, entries),
        _ => Ok(()),
    }
}

/// ADR-028 §12.3：新源加入/更新后，把此前无法解析的 pending 条目落真实表。
/// 调用时机：`prepare_sync` 事务内、apply_merged 全部实体之后（sources 已更新）。
pub fn reapply_pending(conn: &Connection) -> Result<()> {
    let sources = LocalSources::load(conn);
    let pending = base::load_pending_on(conn)?;
    for row in pending {
        let e: SyncEntry = match serde_json::from_str(&row.payload) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if e.deleted {
            base::delete_pending_on(conn, &row.entity_type, &row.entity_key)?;
            continue;
        }
        let path = e.data["path"].as_str().unwrap_or("").to_string();
        if path.is_empty() {
            continue;
        }
        // 仅当现在可解析才落真实表；否则保留 pending 等待绑定。
        let resolvable = match row.entity_type.as_str() {
            base::ENTITY_BOOK_TAGS => {
                let (book_id, _tag) = row.entity_key.rsplit_once('|').unwrap_or(("", ""));
                sources.resolve(book_id, &path).is_some()
            }
            _ => sources.resolve(&row.entity_key, &path).is_some(),
        };
        if !resolvable {
            continue;
        }
        let mut m = HashMap::new();
        m.insert(row.entity_key.clone(), e);
        apply_merged(conn, &row.entity_type, &m, None)?;
        base::delete_pending_on(conn, &row.entity_type, &row.entity_key)?;
    }
    Ok(())
}
