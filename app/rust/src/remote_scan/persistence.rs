use super::engine::{CoverTask, ScanDirectoryTask};
use super::model::{
    fingerprint as entry_fingerprint, RemoteAssetKind, RemoteEntry, RemoteScanState,
};
use crate::document::remote_folder::RemoteImageEntry;
use rusqlite::{params, Connection, OptionalExtension, Result};

pub fn migrate(conn: &Connection) -> Result<()> {
    for (name, ty) in [
        ("asset_kind", "TEXT"),
        ("content_fingerprint", "TEXT"),
        ("scan_generation", "INTEGER"),
        ("listing_complete", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('library_index') WHERE name=?1)",
            [name],
            |row| row.get(0),
        )?;
        if !exists {
            conn.execute(
                &format!("ALTER TABLE library_index ADD COLUMN {name} {ty}"),
                [],
            )?;
        }
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS remote_scan_state (source_id TEXT PRIMARY KEY,status TEXT NOT NULL,mode TEXT NOT NULL,generation INTEGER NOT NULL,checkpoint TEXT,last_success_at INTEGER,error_code TEXT);
         CREATE TABLE IF NOT EXISTS remote_listing_state (source_id TEXT NOT NULL,logical_path TEXT NOT NULL,content_fingerprint TEXT,scan_generation INTEGER NOT NULL,listing_complete INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(source_id,logical_path));
         CREATE TABLE IF NOT EXISTS remote_cover_dependency (book_key TEXT NOT NULL,dependency_path TEXT NOT NULL,dependency_fingerprint TEXT NOT NULL,profile TEXT NOT NULL,status TEXT NOT NULL,PRIMARY KEY(book_key,dependency_path));
         CREATE TABLE IF NOT EXISTS remote_cover_stage (source_id TEXT NOT NULL,generation INTEGER NOT NULL,book_key TEXT NOT NULL,dependency_path TEXT NOT NULL,dependency_fingerprint TEXT NOT NULL,profile TEXT NOT NULL,PRIMARY KEY(source_id,generation,book_key,dependency_path));
         CREATE TABLE IF NOT EXISTS remote_scan_listing_stage (source_id TEXT NOT NULL,generation INTEGER NOT NULL,logical_path TEXT NOT NULL,content_fingerprint TEXT NOT NULL,asset_kind TEXT NOT NULL,entries_json TEXT NOT NULL,incremental INTEGER NOT NULL,PRIMARY KEY(source_id,generation,logical_path));
         CREATE TABLE IF NOT EXISTS remote_scan_pending (source_id TEXT NOT NULL,generation INTEGER NOT NULL,logical_path TEXT NOT NULL,incremental INTEGER NOT NULL,PRIMARY KEY(source_id,generation,logical_path));
         CREATE TABLE IF NOT EXISTS remote_scan_config (source_id TEXT PRIMARY KEY,source_type TEXT NOT NULL,root_path TEXT NOT NULL,mode TEXT NOT NULL,generation INTEGER NOT NULL,status TEXT NOT NULL,updated_at INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS remote_cover_partial_cache (book_key TEXT PRIMARY KEY,dependency_fingerprint TEXT NOT NULL,bytes BLOB NOT NULL,updated_at INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS idx_remote_listing_source_path ON remote_listing_state(source_id,logical_path);
         CREATE INDEX IF NOT EXISTS idx_remote_cover_book_path ON remote_cover_dependency(book_key,dependency_path);
         CREATE INDEX IF NOT EXISTS idx_remote_stage_generation ON remote_scan_listing_stage(source_id,generation);
         CREATE INDEX IF NOT EXISTS idx_remote_pending_generation ON remote_scan_pending(source_id,generation);",
    )
}

pub fn load_checkpoint(conn: &Connection, source_id: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT checkpoint FROM remote_scan_state WHERE source_id=?1",
        [source_id],
        |row| row.get::<_, Option<String>>(0),
    )
    .or_else(|error| {
        if matches!(error, rusqlite::Error::QueryReturnedNoRows) {
            Ok(None)
        } else {
            Err(error)
        }
    })
}

pub fn mark_scan_status(conn: &Connection, state: &RemoteScanState) -> Result<()> {
    conn.execute(
        "INSERT INTO remote_scan_state(source_id,status,mode,generation,checkpoint,last_success_at,error_code) VALUES(?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(source_id) DO UPDATE SET status=excluded.status,mode=excluded.mode,generation=excluded.generation,checkpoint=excluded.checkpoint,last_success_at=excluded.last_success_at,error_code=excluded.error_code",
        params![state.source_id, format!("{:?}", state.status), format!("{:?}", state.mode), state.generation, state.checkpoint, state.last_success_at, state.error_code],
    )?;
    Ok(())
}

fn upsert_listing_on(
    conn: &Connection,
    source_id: &str,
    path: &str,
    entries: &[RemoteEntry],
    generation: i64,
    fingerprint: &str,
    complete: bool,
) -> Result<()> {
    let source_fp: String = conn.query_row("SELECT fingerprint FROM book_sources WHERE id=?1 AND fingerprint IS NOT NULL AND fingerprint <> ''", [source_id], |row| row.get(0))?;
    let now = crate::db::now_ms();
    for entry in entries {
        let logical_path = super::model::normalize_path(&entry.logical_path);
        let id = crate::db::library_index_id(&source_fp, &logical_path);
        let parent = logical_path.rfind('/').map(|index| {
            if index == 0 {
                "/"
            } else {
                &logical_path[..index]
            }
        });
        let parent_id = parent.map(|value| crate::db::library_index_id(&source_fp, value));
        let child_fingerprint = entry_fingerprint(std::slice::from_ref(entry));
        conn.execute(
            "INSERT INTO library_index(id,source_id,parent_id,name,path,entry_type,size,modified_at,asset_kind,content_fingerprint,scan_generation,listing_complete,deleted,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,0,?13)
             ON CONFLICT(id) DO UPDATE SET parent_id=excluded.parent_id,name=excluded.name,size=excluded.size,modified_at=excluded.modified_at,asset_kind=excluded.asset_kind,content_fingerprint=excluded.content_fingerprint,scan_generation=excluded.scan_generation,listing_complete=excluded.listing_complete,deleted=0,updated_at=excluded.updated_at",
            params![id, source_id, parent_id, entry.name, logical_path, if entry.is_dir { "dir" } else { "file" }, entry.size.map(|value| value as i64), entry.mtime, format!("{:?}", entry.asset_kind), child_fingerprint, generation, complete as i64, now],
        )?;
    }
    conn.execute(
        "INSERT INTO remote_listing_state VALUES(?1,?2,?3,?4,?5) ON CONFLICT(source_id,logical_path) DO UPDATE SET content_fingerprint=excluded.content_fingerprint,scan_generation=excluded.scan_generation,listing_complete=excluded.listing_complete",
        params![source_id, super::model::normalize_path(path), fingerprint, generation, complete as i64],
    )?;
    Ok(())
}

pub fn upsert_complete_listing(
    conn: &Connection,
    source_id: &str,
    path: &str,
    entries: &[RemoteEntry],
    generation: i64,
    fingerprint: &str,
    complete: bool,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    upsert_listing_on(
        &tx,
        source_id,
        path,
        entries,
        generation,
        fingerprint,
        complete,
    )?;
    tx.commit()
}

pub fn stage_complete_listing(
    conn: &Connection,
    source_id: &str,
    path: &str,
    entries: &[RemoteEntry],
    generation: i64,
    fingerprint: &str,
    asset_kind: RemoteAssetKind,
    incremental: bool,
) -> Result<()> {
    let json = serde_json::to_string(entries)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    conn.execute(
        "INSERT INTO remote_scan_listing_stage(source_id,generation,logical_path,content_fingerprint,asset_kind,entries_json,incremental) VALUES(?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(source_id,generation,logical_path) DO UPDATE SET content_fingerprint=excluded.content_fingerprint,asset_kind=excluded.asset_kind,entries_json=excluded.entries_json,incremental=excluded.incremental",
        params![source_id, generation, super::model::normalize_path(path), fingerprint, format!("{:?}", asset_kind), json, incremental as i64],
    )?;
    Ok(())
}

pub fn stage_cover_task(
    conn: &Connection,
    generation: i64,
    book_key: &str,
    task: &CoverTask,
) -> Result<()> {
    conn.execute(
        "INSERT INTO remote_cover_stage(source_id,generation,book_key,dependency_path,dependency_fingerprint,profile) VALUES(?1,?2,?3,?4,?5,?6)
         ON CONFLICT(source_id,generation,book_key,dependency_path) DO UPDATE SET dependency_fingerprint=excluded.dependency_fingerprint,profile=excluded.profile",
        params![task.source_id, generation, book_key, super::model::normalize_path(&task.logical_path), task.fingerprint, task.profile],
    )?;
    Ok(())
}

fn replace_verified_children_on(
    conn: &Connection,
    source_id: &str,
    parent: &str,
    paths: &[String],
    generation: i64,
    complete: bool,
) -> Result<()> {
    let now = crate::db::now_ms();
    for path in paths {
        conn.execute("UPDATE library_index SET deleted=0,scan_generation=?1,updated_at=?4 WHERE source_id=?2 AND path=?3", params![generation, source_id, path, now])?;
    }
    let parent = super::model::normalize_path(parent);
    let proven: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM remote_listing_state WHERE source_id=?1 AND logical_path=?2 AND scan_generation=?3 AND listing_complete=1)",
        params![source_id, parent, generation], |row| row.get(0),
    )?;
    if complete && proven {
        let source_fp: String = conn.query_row(
            "SELECT fingerprint FROM book_sources WHERE id=?1",
            [source_id],
            |row| row.get(0),
        )?;
        let parent_id = crate::db::library_index_id(&source_fp, &parent);
        conn.execute(
            "UPDATE library_index SET deleted=1,updated_at=?4 WHERE source_id=?1 AND parent_id=?2 AND scan_generation < ?3 AND listing_complete=1",
            params![source_id, parent_id, generation, now],
        )?;
    }
    Ok(())
}

pub fn publish_staged_generation(
    conn: &Connection,
    source_id: &str,
    generation: i64,
) -> Result<()> {
    let latest: Option<i64> = conn
        .query_row(
            "SELECT generation FROM remote_scan_state WHERE source_id=?1",
            [source_id],
            |row| row.get(0),
        )
        .optional()?;
    if latest.is_some_and(|value| value != generation) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let tx = conn.unchecked_transaction()?;
    loop {
        let staged: Option<(String, String, String, String)> = tx.query_row(
            "SELECT logical_path,content_fingerprint,asset_kind,entries_json FROM remote_scan_listing_stage WHERE source_id=?1 AND generation=?2 ORDER BY logical_path LIMIT 1",
            params![source_id, generation],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).optional()?;
        let Some((path, directory_fingerprint, asset_kind, json)) = staged else {
            break;
        };
        let entries: Vec<RemoteEntry> = serde_json::from_str(&json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        upsert_listing_on(
            &tx,
            source_id,
            &path,
            &entries,
            generation,
            &directory_fingerprint,
            true,
        )?;
        let paths = entries
            .iter()
            .map(|entry| super::model::normalize_path(&entry.logical_path))
            .collect::<Vec<_>>();
        replace_verified_children_on(&tx, source_id, &path, &paths, generation, true)?;
        tx.execute("UPDATE library_index SET asset_kind=?1,content_fingerprint=?2,scan_generation=?3,listing_complete=1,updated_at=?4 WHERE source_id=?5 AND path=?6", params![asset_kind, directory_fingerprint, generation, crate::db::now_ms(), source_id, path])?;
        tx.execute(
            "DELETE FROM remote_scan_listing_stage WHERE source_id=?1 AND generation=?2 AND logical_path=?3",
            params![source_id, generation, path],
        )?;
    }
    tx.execute(
        "INSERT INTO remote_cover_dependency(book_key,dependency_path,dependency_fingerprint,profile,status)
         SELECT book_key,dependency_path,dependency_fingerprint,profile,'queued' FROM remote_cover_stage WHERE source_id=?1 AND generation=?2
         ON CONFLICT(book_key,dependency_path) DO UPDATE SET dependency_fingerprint=excluded.dependency_fingerprint,profile=excluded.profile,status='queued'",
        params![source_id, generation],
    )?;
    tx.execute(
        "DELETE FROM remote_scan_pending WHERE source_id=?1 AND generation=?2",
        params![source_id, generation],
    )?;
    tx.commit()
}

pub fn discard_staged_generation(
    conn: &Connection,
    source_id: &str,
    generation: i64,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM remote_scan_listing_stage WHERE source_id=?1 AND generation=?2",
        params![source_id, generation],
    )?;
    tx.execute(
        "DELETE FROM remote_scan_pending WHERE source_id=?1 AND generation=?2",
        params![source_id, generation],
    )?;
    tx.execute(
        "DELETE FROM remote_cover_stage WHERE source_id=?1 AND generation=?2",
        params![source_id, generation],
    )?;
    tx.commit()
}

pub fn next_staged_cover_task(
    conn: &Connection,
    source_id: &str,
    generation: i64,
) -> Result<Option<(String, CoverTask)>> {
    conn.query_row(
        "SELECT book_key,dependency_path,dependency_fingerprint,profile FROM remote_cover_stage WHERE source_id=?1 AND generation=?2 ORDER BY dependency_path LIMIT 1",
        params![source_id, generation],
        |row| {
            Ok((
                row.get(0)?,
                CoverTask {
                    source_id: source_id.to_string(),
                    logical_path: row.get(1)?,
                    fingerprint: row.get(2)?,
                    profile: row.get(3)?,
                },
            ))
        },
    )
    .optional()
}

pub fn finish_cover_task(
    conn: &Connection,
    source_id: &str,
    generation: i64,
    book_key: &str,
    dependency_path: &str,
    status: &str,
    bytes: Option<&[u8]>,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let dependency_path = super::model::normalize_path(dependency_path);
    if let Some(bytes) = bytes {
        tx.execute(
            "INSERT INTO remote_cover_partial_cache(book_key,dependency_fingerprint,bytes,updated_at)
             SELECT book_key,dependency_fingerprint,?2,?3 FROM remote_cover_stage WHERE source_id=?1 AND generation=?4 AND book_key=?5 AND dependency_path=?6
             ON CONFLICT(book_key) DO UPDATE SET dependency_fingerprint=excluded.dependency_fingerprint,bytes=excluded.bytes,updated_at=excluded.updated_at",
            params![source_id, bytes, crate::db::now_ms(), generation, book_key, dependency_path],
        )?;
    }
    tx.execute(
        "UPDATE remote_cover_dependency SET status=?1 WHERE book_key=?2 AND dependency_path=?3",
        params![status, book_key, dependency_path],
    )?;
    tx.execute(
        "DELETE FROM remote_cover_stage WHERE source_id=?1 AND generation=?2 AND book_key=?3 AND dependency_path=?4",
        params![source_id, generation, book_key, dependency_path],
    )?;
    tx.commit()
}

pub fn replace_verified_children(
    conn: &Connection,
    source_id: &str,
    parent: &str,
    paths: &[String],
    generation: i64,
    complete: bool,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    replace_verified_children_on(&tx, source_id, parent, paths, generation, complete)?;
    tx.commit()
}

pub fn store_pending_task(conn: &Connection, task: &ScanDirectoryTask) -> Result<()> {
    conn.execute("INSERT OR IGNORE INTO remote_scan_pending(source_id,generation,logical_path,incremental) VALUES(?1,?2,?3,?4)", params![task.source_id, task.generation, super::model::normalize_path(&task.logical_path), task.incremental as i64])?;
    Ok(())
}

pub fn take_pending_task(
    conn: &Connection,
    source_id: &str,
    generation: i64,
) -> Result<Option<ScanDirectoryTask>> {
    let tx = conn.unchecked_transaction()?;
    let row: Option<(String, bool)> = tx.query_row("SELECT logical_path,incremental FROM remote_scan_pending WHERE source_id=?1 AND generation=?2 ORDER BY rowid DESC LIMIT 1", params![source_id, generation], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
    if let Some((path, incremental)) = row {
        tx.execute("DELETE FROM remote_scan_pending WHERE source_id=?1 AND generation=?2 AND logical_path=?3", params![source_id, generation, path])?;
        tx.commit()?;
        let task = ScanDirectoryTask::new(source_id, path, generation);
        Ok(Some(if incremental {
            task.incremental()
        } else {
            task
        }))
    } else {
        tx.commit()?;
        Ok(None)
    }
}

pub fn load_complete_image_folder_manifest(
    conn: &Connection,
    source_id: &str,
    logical_path: &str,
) -> Result<Vec<RemoteImageEntry>> {
    let logical_path = super::model::normalize_path(logical_path);
    let proof: Option<(i64, String)> = conn
        .query_row(
            "SELECT li.scan_generation, li.content_fingerprint
             FROM library_index li
             JOIN remote_listing_state rs
               ON rs.source_id=li.source_id AND rs.logical_path=li.path
              AND rs.scan_generation=li.scan_generation
              AND rs.content_fingerprint=li.content_fingerprint
             WHERE li.source_id=?1 AND li.path=?2 AND li.entry_type='dir'
               AND li.asset_kind='ImageFolder' AND li.listing_complete=1 AND li.deleted=0
               AND rs.listing_complete=1",
            params![source_id, logical_path],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((generation, _folder_fingerprint)) = proof else {
        return Ok(Vec::new());
    };

    let prefix = if logical_path == "/" {
        "/".to_string()
    } else {
        format!("{logical_path}/")
    };
    let mut stmt = conn.prepare(
        "SELECT path,name,size,modified_at,content_fingerprint
         FROM library_index
         WHERE source_id=?1 AND path LIKE ?2 AND entry_type='file'
           AND asset_kind='ImageFile' AND scan_generation=?3
           AND listing_complete=1 AND deleted=0
         ORDER BY path",
    )?;
    let like = format!("{prefix}%");
    let rows = stmt.query_map(params![source_id, like, generation], |row| {
        Ok(RemoteImageEntry {
            logical_path: row.get(0)?,
            name: row.get(1)?,
            size: row.get::<_, Option<i64>>(2)?.map(|value| value.max(0) as u64),
            mtime: row.get(3)?,
            fingerprint: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        })
    })?;
    let mut entries = Vec::new();
    for row in rows {
        let entry = row?;
        let remainder = entry.logical_path.strip_prefix(&prefix).unwrap_or("");
        if !remainder.is_empty() && !remainder.contains('/') {
            entries.push(entry);
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod remote_folder_manifest_tests {
    use super::*;

    fn manifest_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE library_index (
                source_id TEXT NOT NULL, parent_id TEXT, name TEXT NOT NULL, path TEXT NOT NULL,
                entry_type TEXT NOT NULL, size INTEGER, modified_at INTEGER, asset_kind TEXT,
                content_fingerprint TEXT, scan_generation INTEGER, listing_complete INTEGER NOT NULL,
                deleted INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE remote_listing_state (
                source_id TEXT NOT NULL, logical_path TEXT NOT NULL, content_fingerprint TEXT,
                scan_generation INTEGER NOT NULL, listing_complete INTEGER NOT NULL
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn complete_image_folder_manifest_loads_only_committed_image_children() {
        let conn = manifest_db();
        conn.execute(
            "INSERT INTO library_index VALUES('s','root','Book','/Book','dir',NULL,NULL,'ImageFolder','folder-fp',4,1,0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO remote_listing_state VALUES('s','/Book','folder-fp',4,1)",
            [],
        ).unwrap();
        for (name, path, kind) in [
            ("2.jpg", "/Book/2.jpg", "ImageFile"),
            ("note.txt", "/Book/note.txt", "Other"),
        ] {
            conn.execute(
                "INSERT INTO library_index VALUES('s',NULL,?1,?2,'file',3,7,?3,'child-fp',4,1,0)",
                params![name, path, kind],
            ).unwrap();
        }

        let entries = load_complete_image_folder_manifest(&conn, "s", "/Book").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].logical_path, "/Book/2.jpg");
        assert_eq!(entries[0].fingerprint, "child-fp");
    }

    #[test]
    fn incomplete_or_generation_mismatched_folder_manifest_is_rejected() {
        let conn = manifest_db();
        conn.execute(
            "INSERT INTO library_index VALUES('s','root','Book','/Book','dir',NULL,NULL,'ImageFolder','folder-fp',4,1,0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO remote_listing_state VALUES('s','/Book','folder-fp',5,1)",
            [],
        ).unwrap();

        assert!(load_complete_image_folder_manifest(&conn, "s", "/Book")
            .unwrap()
            .is_empty());
    }
}
