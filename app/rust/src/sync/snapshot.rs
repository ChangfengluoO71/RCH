//! 本地状态装载 / Sync Base 推进（ADR-024 §3/§7）。
//!
//! 本地快照 = 各实体 live 行；删除由"相对 base 的缺席"推断（三方语义），
//! 不需要为同步维护额外墓碑表。base 保存上次成功时的完整条目 JSON。

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json::json;

use crate::db;
use crate::sync::base::{self, SyncBaseRow};
use crate::sync::identity;
use crate::sync::merge::SyncEntry;
use crate::sync::state::entity_hash;

pub const ENTITIES: [&str; 7] = [
    base::ENTITY_SOURCES,
    base::ENTITY_METAS,
    base::ENTITY_RECORDS,
    base::ENTITY_TAGS,
    base::ENTITY_BOOK_TAGS,
    base::ENTITY_LIBRARY_INDEX,
    base::ENTITY_SETTINGS,
];

fn source_fp_map(conn: &Connection) -> HashMap<String, String> {
    let mut stmt = conn
        .prepare("SELECT id, fingerprint FROM book_sources WHERE fingerprint IS NOT NULL AND fingerprint != ''")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
}

/// 本地 key（`type|source_id|path`）→ 同步稳定 key（fingerprint + path）。
/// 源缺失/已删 → None（孤儿行不同步）。
fn local_book_key_to_sync(local_key: &str, fp_by_id: &HashMap<String, String>) -> Option<String> {
    let mut it = local_key.splitn(3, '|');
    let _ = it.next()?;
    let source_id = it.next()?;
    let path = it.next()?;
    let fp = fp_by_id.get(source_id)?;
    Some(identity::book_id(fp, path))
}

fn load_sources(conn: &Connection) -> Result<HashMap<String, SyncEntry>> {
    let mut stmt = conn.prepare(
        "SELECT fingerprint, type, name, path, url, username, port, note, capability_label,
                root_id, client_id, updated_at
         FROM book_sources WHERE deleted = 0 AND fingerprint IS NOT NULL AND fingerprint != ''",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                json!({
                    "type": r.get::<_, String>(1)?,
                    "name": r.get::<_, String>(2)?,
                    "path": r.get::<_, String>(3)?,
                    "url": r.get::<_, Option<String>>(4)?,
                    "username": r.get::<_, Option<String>>(5)?,
                    "port": r.get::<_, Option<i64>>(6)?,
                    "note": r.get::<_, String>(7)?,
                    "capabilityLabel": r.get::<_, String>(8)?,
                    "rootId": r.get::<_, Option<String>>(9)?,
                    "clientId": r.get::<_, Option<String>>(10)?,
                }),
                r.get::<_, i64>(11)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect::<Vec<_>>();
    Ok(rows
        .into_iter()
        .map(|(fp, data, updated_at)| {
            (
                fp.clone(),
                SyncEntry::live(&fp, updated_at, data),
            )
        })
        .collect())
}

fn load_metas(conn: &Connection) -> Result<HashMap<String, SyncEntry>> {
    let fp_by_id = source_fp_map(conn);
    let mut stmt = conn.prepare(
        "SELECT key, stable_id, cover_page, crop_x, crop_y, crop_w, crop_h,
                author, genre, series, title, chinese_title, summary, comment, rotations, updated_at
         FROM book_metas WHERE deleted = 0",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let key: String = r.get(0)?;
            Ok((
                key,
                json!({
                    "stableId": r.get::<_, Option<String>>(1)?,
                    "coverPage": r.get::<_, i64>(2)?,
                    "cropX": r.get::<_, Option<f64>>(3)?,
                    "cropY": r.get::<_, Option<f64>>(4)?,
                    "cropW": r.get::<_, Option<f64>>(5)?,
                    "cropH": r.get::<_, Option<f64>>(6)?,
                    "author": r.get::<_, String>(7)?,
                    "genre": r.get::<_, String>(8)?,
                    "series": r.get::<_, String>(9)?,
                    "title": r.get::<_, String>(10)?,
                    "chineseTitle": r.get::<_, String>(11)?,
                    "summary": r.get::<_, String>(12)?,
                    "comment": r.get::<_, String>(13)?,
                    "rotations": r.get::<_, String>(14)?,
                }),
                r.get::<_, i64>(15)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect::<Vec<_>>();
    let mut out = HashMap::new();
    for (key, mut data, updated_at) in rows {
        if let Some(sync_key) = local_book_key_to_sync(&key, &fp_by_id) {
            // 反查用：应用层需要 path 才能从 book_id 反解本地 source
            if let Some(path) = key.splitn(3, '|').nth(2) {
                if let Some(obj) = data.as_object_mut() {
                    obj.insert("path".into(), json!(path));
                }
            }
            out.insert(sync_key.clone(), SyncEntry::live(&sync_key, updated_at, data));
        }
    }
    merge_pending_entries(conn, base::ENTITY_METAS, &mut out);
    Ok(out)
}

fn load_records(conn: &Connection) -> Result<HashMap<String, SyncEntry>> {
    let fp_by_id = source_fp_map(conn);
    let mut stmt = conn.prepare(
        "SELECT key, stable_id, path, title, last_page, read_count, last_read_at, updated_at
         FROM read_records WHERE deleted = 0",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let key: String = r.get(0)?;
            Ok((
                key,
                json!({
                    "stableId": r.get::<_, Option<String>>(1)?,
                    "path": r.get::<_, String>(2)?,
                    "title": r.get::<_, String>(3)?,
                    "lastPage": r.get::<_, i64>(4)?,
                    "readCount": r.get::<_, i64>(5)?,
                    "lastReadAt": r.get::<_, i64>(6)?,
                }),
                r.get::<_, i64>(7)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect::<Vec<_>>();
    let mut out = HashMap::new();
    for (key, data, updated_at) in rows {
        if let Some(sync_key) = local_book_key_to_sync(&key, &fp_by_id) {
            out.insert(sync_key.clone(), SyncEntry::live(&sync_key, updated_at, data));
        }
    }
    merge_pending_entries(conn, base::ENTITY_RECORDS, &mut out);
    Ok(out)
}

fn load_tags(conn: &Connection) -> Result<HashMap<String, SyncEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, created_at, updated_at FROM tags WHERE deleted = 0",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let id: String = r.get(0)?;
            Ok((
                id.clone(),
                json!({ "id": id, "name": r.get::<_, String>(1)?, "createdAt": r.get::<_, i64>(2)? }),
                r.get::<_, i64>(3)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect::<Vec<_>>();
    Ok(rows
        .into_iter()
        .map(|(id, data, updated_at)| (id.clone(), SyncEntry::live(&id, updated_at, data)))
        .collect())
}

fn load_book_tags(conn: &Connection) -> Result<HashMap<String, SyncEntry>> {
    let fp_by_id = source_fp_map(conn);
    let mut stmt = conn.prepare(
        "SELECT book_key, tag_id, updated_at FROM book_tags WHERE deleted = 0",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect::<Vec<_>>();
    let mut out = HashMap::new();
    for (book_key, tag_id, updated_at) in rows {
        if let Some(book_id) = local_book_key_to_sync(&book_key, &fp_by_id) {
            let sync_key = format!("{book_id}|{tag_id}");
            let path = book_key.splitn(3, '|').nth(2).unwrap_or("").to_string();
            out.insert(
                sync_key.clone(),
                SyncEntry::live(
                    &sync_key,
                    updated_at,
                    json!({ "path": path, "tagId": tag_id }),
                ),
            );
        }
    }
    merge_pending_entries(conn, base::ENTITY_BOOK_TAGS, &mut out);
    Ok(out)
}

fn load_library_index(conn: &Connection) -> Result<HashMap<String, SyncEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, source_id, parent_id, name, path, entry_type, size, modified_at, cover_path, hash, updated_at
         FROM library_index WHERE deleted = 0",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<i64>>(6)?,
                r.get::<_, Option<i64>>(7)?,
                r.get::<_, Option<String>>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, i64>(10)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect::<Vec<_>>();
    let fp_by_id = source_fp_map(conn);
    let mut out = HashMap::new();
    for (id, source_id, parent_id, name, path, entry_type, size, modified_at, cover_path, hash, updated_at) in rows {
        if let Some(fp) = fp_by_id.get(&source_id) {
            let sync_key = identity::book_id(fp, &path);
            // 兼容旧 id（若与现 fingerprint 不一致，以新计算为准）
            let _ = id;
            out.insert(
                sync_key.clone(),
                SyncEntry::live(
                    &sync_key,
                    updated_at,
                    json!({
                        "name": name,
                        "path": path,
                        "entryType": entry_type,
                        "size": size,
                        "modifiedAt": modified_at,
                        "coverPath": cover_path,
                        "hash": hash,
                        "parentId": parent_id,
                    }),
                ),
            );
        }
    }
    merge_pending_entries(conn, base::ENTITY_LIBRARY_INDEX, &mut out);
    Ok(out)
}

fn load_settings(conn: &Connection) -> Result<HashMap<String, SyncEntry>> {
    let mut stmt = conn.prepare(
        "SELECT key, value, updated_at FROM app_settings WHERE deleted = 0",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect::<Vec<_>>();
    Ok(rows
        .into_iter()
        .filter(|(k, _, _)| db::is_syncable_setting(k))
        .map(|(k, v, updated_at)| {
            let sync_key = k.clone();
            (
                sync_key.clone(),
                SyncEntry::live(&sync_key, updated_at, json!({ "value": v })),
            )
        })
        .collect())
}

/// ADR-028 §12.3：pending 条目视为"存在"（live）并入本地快照。
///
/// 三方合并要求 base / local / remote 三态一致：apply 不再静默跳过，
/// resolve 失败的条目存于 sync_pending_apply 并参与快照，
/// 使"本机无法托管该条目"不会被误判为"本机删除了该条目"（伪墓碑根因）。
fn merge_pending_entries(
    conn: &Connection,
    entity: &str,
    out: &mut HashMap<String, SyncEntry>,
) {
    let Ok(pending) = base::load_pending_on(conn) else {
        return;
    };
    for row in pending {
        if row.entity_type != entity {
            continue;
        }
        if out.contains_key(&row.entity_key) {
            continue; // 真实行优先（reapply 成功时会清除 pending）
        }
        if let Ok(e) = serde_json::from_str::<SyncEntry>(&row.payload) {
            if !e.deleted {
                out.insert(
                    row.entity_key.clone(),
                    // 逻辑时钟取条目自身 updated_at（与远端/ base 一致），
                    // 避免落库时间戳导致三方比较误判"已修改"而反复重推。
                    SyncEntry::live(&row.entity_key, e.updated_at, e.data),
                );
            }
        }
    }
}

/// 装载本地 7 类实体快照。
pub fn load_local_snapshots(
    conn: &Connection,
) -> Result<HashMap<String, HashMap<String, SyncEntry>>> {
    let mut out = HashMap::new();
    out.insert(base::ENTITY_SOURCES.into(), load_sources(conn)?);
    out.insert(base::ENTITY_METAS.into(), load_metas(conn)?);
    out.insert(base::ENTITY_RECORDS.into(), load_records(conn)?);
    out.insert(base::ENTITY_TAGS.into(), load_tags(conn)?);
    out.insert(base::ENTITY_BOOK_TAGS.into(), load_book_tags(conn)?);
    out.insert(base::ENTITY_LIBRARY_INDEX.into(), load_library_index(conn)?);
    out.insert(base::ENTITY_SETTINGS.into(), load_settings(conn)?);
    Ok(out)
}

/// 装载 sync_base 快照（state_json → SyncEntry）。
pub fn load_base_snapshots(
    conn: &Connection,
) -> Result<HashMap<String, HashMap<String, SyncEntry>>> {
    let mut stmt = conn.prepare(
        "SELECT entity_type, entity_key, state_json FROM sync_base WHERE state_json IS NOT NULL",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect::<Vec<_>>();
    let mut out: HashMap<String, HashMap<String, SyncEntry>> = HashMap::new();
    for (entity, key, json) in rows {
        if let Ok(entry) = serde_json::from_str::<SyncEntry>(&json) {
            out.entry(entity).or_default().insert(key, entry);
        }
    }
    Ok(out)
}

fn serialize_entry(e: &SyncEntry) -> String {
    serde_json::to_string(e).unwrap_or_else(|_| "{}".into())
}

/// 推进 Sync Base（同步成功后调用）：Base = 本次提交后的**全量镜像**（ADR-028 §12.1）。
///
/// - merged 只含本轮变化条目；**未变化条目必须保留在 base 中**，禁止按 merged 差集剪枝。
///   剪枝会让 base 退化为 change log，下一轮三方合并把未变化条目误判为
///   "本地新增/远端删除"，产生伪墓碑（双设备实测数据清空根因之一）。
/// - 墓碑条目（deleted=true）同样保留，保证"已删除"与"从未存在"可区分。
pub fn advance_base(
    conn: &Connection,
    revision: i64,
    merged: &HashMap<String, HashMap<String, SyncEntry>>,
) -> Result<()> {
    let now = db::now_ms();
    for entity in ENTITIES {
        let entries = merged.get(entity).cloned().unwrap_or_default();
        for (key, entry) in &entries {
            let json = serialize_entry(entry);
            let hash = entity_hash(json.as_bytes());
            let row = SyncBaseRow {
                entity_type: entity.to_string(),
                entity_key: key.clone(),
                state_hash: hash,
                state_json: Some(json),
                revision,
                updated_at: now,
            };
            base::upsert_base_on(conn, &row)?;
        }
        // 未变化条目保持原样，仅统一推进 revision/updated_at（全量镜像语义）。
        conn.execute(
            "UPDATE sync_base SET revision = ?1, updated_at = ?2 WHERE entity_type = ?3",
            params![revision, now, entity],
        )?;
    }
    base::set_meta_on(conn, base::META_LAST_REVISION, &revision.to_string())?;
    base::set_meta_on(conn, base::META_LAST_SYNC_AT, &now.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn schema_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init_tables(&conn).unwrap();
        conn
    }

    fn insert_source(conn: &Connection, id: &str, r#type: &str, path: &str, url: Option<&str>) -> String {
        let fp = db::compute_source_fingerprint(r#type, url, path, None);
        conn.execute(
            "INSERT INTO book_sources (id, type, name, path, url, fingerprint, updated_at, deleted)
             VALUES (?1, ?2, 'NAS', ?3, ?4, ?5, 1000, 0)",
            params![id, r#type, path, url, fp],
        )
        .unwrap();
        fp
    }

    #[test]
    fn local_snapshot_maps_keys_to_sync_identity() {
        let conn = schema_conn();
        let fp = insert_source(&conn, "s1", "webdav", "/books", Some("https://dav.example.com/dav"));
        conn.execute(
            "INSERT INTO book_metas (key, title, rotations, updated_at, deleted)
             VALUES ('webdav|s1|/books/a.cbz', 'A', '{}', 100, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO read_records (key, source_id, source_type, path, title, updated_at, deleted)
             VALUES ('webdav|s1|/books/a.cbz', 's1', 'webdav', '/books/a.cbz', 'A', 200, 0)",
            [],
        )
        .unwrap();
        let snap = load_local_snapshots(&conn).unwrap();
        let metas = snap.get(base::ENTITY_METAS).unwrap();
        let expect_key = identity::book_id(&fp, "/books/a.cbz");
        assert!(metas.contains_key(&expect_key));
        assert_eq!(metas[&expect_key].data["title"], json!("A"));
        assert!(snap[base::ENTITY_SOURCES].contains_key(&fp));
        assert!(snap[base::ENTITY_RECORDS].contains_key(&expect_key));
    }

    #[test]
    fn orphan_rows_without_source_are_skipped() {
        let conn = schema_conn();
        conn.execute(
            "INSERT INTO book_metas (key, title, rotations, updated_at, deleted)
             VALUES ('webdav|ghost|/x.cbz', 'X', '{}', 100, 0)",
            [],
        )
        .unwrap();
        let snap = load_local_snapshots(&conn).unwrap();
        assert!(snap[base::ENTITY_METAS].is_empty());
    }

    #[test]
    fn advance_base_keeps_unchanged_keys_and_tombstones() {
        let conn = schema_conn();
        let fp = insert_source(&conn, "s1", "webdav", "/books", Some("https://dav.example.com/dav"));
        let key = identity::book_id(&fp, "/books/a.cbz");
        let mut merged = HashMap::new();
        let mut metas = HashMap::new();
        metas.insert(key.clone(), SyncEntry::live(&key, 1, json!({"title": "A"})));
        merged.insert(base::ENTITY_METAS.into(), metas);
        advance_base(&conn, 1, &merged).unwrap();
        assert_eq!(base::get_meta_on(&conn, base::META_LAST_REVISION).as_deref(), Some("1"));
        let b = base::get_base_on(&conn, base::ENTITY_METAS, &key).unwrap();
        assert_eq!(b.revision, 1);
        assert!(b.state_json.is_some());

        // ADR-028：第二次推进时该 key 未变化（不在 merged 中）→ base 必须保留（全量镜像）
        let empty_merged = HashMap::new();
        advance_base(&conn, 2, &empty_merged).unwrap();
        let b2 = base::get_base_on(&conn, base::ENTITY_METAS, &key).unwrap();
        assert_eq!(b2.revision, 2);

        // 墓碑也保留：'已删除' 与 '从未存在' 可区分
        let mut tomb_merged = HashMap::new();
        let mut tomb_metas = HashMap::new();
        tomb_metas.insert(
            key.clone(),
            SyncEntry::tombstone(&key, 3, json!({"title": "A"})),
        );
        tomb_merged.insert(base::ENTITY_METAS.into(), tomb_metas);
        advance_base(&conn, 3, &tomb_merged).unwrap();
        let b3 = base::get_base_on(&conn, base::ENTITY_METAS, &key).unwrap();
        assert!(
            serde_json::from_str::<SyncEntry>(&b3.state_json.unwrap())
                .unwrap()
                .deleted
        );
    }
}
