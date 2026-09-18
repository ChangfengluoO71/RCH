use super::cover_state::CoverJobUpsertCause;
use super::engine::{CoverTask, ScanDirectoryTask};
use super::model::{
    fingerprint as entry_fingerprint, RemoteAssetKind, RemoteEntry, RemoteScanState,
};
use crate::document::remote_folder::RemoteImageEntry;
use rusqlite::{params, Connection, OptionalExtension, Result};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRemoteTombstone {
    pub logical_path: String,
    pub dependency_paths: Vec<String>,
}

#[cfg(test)]
mod session_rebind_tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE book_sources(
               id TEXT PRIMARY KEY,type TEXT NOT NULL,fingerprint TEXT NOT NULL,
               path TEXT,root_id TEXT,deleted INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE library_index(
               id TEXT PRIMARY KEY,source_id TEXT,parent_id TEXT,name TEXT,path TEXT,
               entry_type TEXT,size INTEGER,modified_at INTEGER,asset_kind TEXT,
               content_fingerprint TEXT,scan_generation INTEGER,
               listing_complete INTEGER NOT NULL DEFAULT 0,
               deleted INTEGER NOT NULL DEFAULT 0,updated_at INTEGER);
             INSERT INTO book_sources(id,type,fingerprint,path,root_id)
               VALUES('source','115','fp','/','/');",
        )
        .unwrap();
        migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn completed_generation_rebinds_routes_and_pending_jobs_once() {
        let conn = db();
        bind_scan_epoch(&conn, "source", 2, "/", 11).unwrap();
        conn.execute(
            "INSERT INTO remote_scan_state(source_id,status,mode,generation)
             VALUES('source','Succeeded','Snapshot',2)",
            [],
        )
        .unwrap();
        let old_epoch: String = conn
            .query_row(
                "SELECT session_epoch FROM remote_scan_epoch
                 WHERE source_id='source' AND generation=2",
                [],
                |row| row.get(0),
            )
            .unwrap();
        super::super::cover_store::upsert_route_on(
            &conn,
            "source",
            "asset",
            "/book.cbz",
            None,
            "115",
            Some("pickcode"),
            "fp",
            2,
            &old_epoch,
            1,
        )
        .unwrap();
        let key = super::super::cover_model::CoverJobKey {
            source_id: "source".into(),
            asset_id: "asset".into(),
            content_revision: "v1".into(),
            selection_revision: "default".into(),
            profile: "340x480@1".into(),
        };
        super::super::cover_store::upsert_job_on(
            &conn,
            &key,
            super::super::cover_model::CoverJobState::Pending,
            "background",
            10,
            2,
            &old_epoch,
            1,
            CoverJobUpsertCause::Demand,
        )
        .unwrap();

        let new_epoch = rebind_completed_generation_session(&conn, "source", 2, 22).unwrap();
        assert!(new_epoch.is_some());
        let epoch_row: (String, i64) = conn
            .query_row(
                "SELECT session_epoch,session_token FROM remote_scan_epoch
                 WHERE source_id='source' AND generation=2",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(epoch_row.1, 22);
        assert_ne!(epoch_row.0, old_epoch);
        let route_epoch: String = conn
            .query_row(
                "SELECT session_epoch FROM remote_asset_route
                 WHERE source_id='source' AND asset_id='asset'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(route_epoch, epoch_row.0);
        let job_epoch: String = conn
            .query_row(
                "SELECT session_epoch FROM remote_cover_job WHERE job_key=?1",
                [key.encode()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(job_epoch, epoch_row.0);

        // Idempotent for the same runtime session and does not manufacture a
        // new generation or duplicate route/job rows.
        let second = rebind_completed_generation_session(&conn, "source", 2, 22).unwrap();
        assert_eq!(second, Some(epoch_row.0));
        let jobs: i64 = conn
            .query_row("SELECT COUNT(*) FROM remote_cover_job", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(jobs, 1);
    }

    #[test]
    fn running_generation_is_never_rebound() {
        let conn = db();
        bind_scan_epoch(&conn, "source", 2, "/", 11).unwrap();
        conn.execute(
            "INSERT INTO remote_scan_state(source_id,status,mode,generation)
             VALUES('source','Running','Snapshot',2)",
            [],
        )
        .unwrap();
        assert_eq!(
            rebind_completed_generation_session(&conn, "source", 2, 22).unwrap(),
            None
        );
    }
}

fn source_schema_has_proof_fields(conn: &Connection) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('book_sources') WHERE name='type')
          AND EXISTS(SELECT 1 FROM pragma_table_info('book_sources') WHERE name='path')
          AND EXISTS(SELECT 1 FROM pragma_table_info('book_sources') WHERE name='root_id')",
        [],
        |row| row.get(0),
    )
}

fn source_identity_on(conn: &Connection, source_id: &str) -> Result<(String, String)> {
    let fingerprint: Option<String> = conn.query_row(
        "SELECT fingerprint FROM book_sources WHERE id=?1",
        [source_id],
        |row| row.get(0),
    )?;
    let fingerprint = fingerprint
        .filter(|value| !value.trim().is_empty())
        .ok_or(rusqlite::Error::InvalidQuery)?;
    if !source_schema_has_proof_fields(conn)? {
        // Minimal pre-Task-1 test databases have only source id/fingerprint.
        // They can exercise staging mechanics, but cannot produce deletion
        // proof; production databases always have these columns after init.
        return Ok((fingerprint, "/".to_string()));
    }
    let (source_type, path, root_id): (String, String, Option<String>) = conn.query_row(
        "SELECT type,path,root_id FROM book_sources WHERE id=?1",
        [source_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let root = if matches!(source_type.as_str(), "115" | "quark") {
        root_id
            .filter(|value| !value.trim().is_empty())
            .or_else(|| (!path.trim().is_empty()).then_some(path.clone()))
            .unwrap_or_else(|| "0".to_string())
    } else {
        path
    };
    Ok((fingerprint, super::model::normalize_path(&root)))
}

fn source_epoch_matches_on(conn: &Connection, source_id: &str, generation: i64) -> Result<bool> {
    let (fingerprint, root) = source_identity_on(conn, source_id)?;
    conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM remote_scan_epoch
           WHERE source_id=?1 AND generation=?2
             AND source_fingerprint=?3 AND root_path=?4 AND session_epoch <> ''
         )",
        params![source_id, generation, fingerprint, root],
        |row| row.get(0),
    )
}

/// Check that a caller is using the source's authoritative effective root.
/// A root from an old source row or a legacy source without a fingerprint is
/// never accepted as proof material.
pub fn requested_root_matches_source(
    conn: &Connection,
    source_id: &str,
    requested_root: &str,
) -> Result<bool> {
    if !source_schema_has_proof_fields(conn)? {
        return Ok(false);
    }
    let (_, root) = source_identity_on(conn, source_id)?;
    Ok(root == super::model::normalize_path(requested_root))
}

pub(crate) fn current_generation_is_active(
    conn: &Connection,
    source_id: &str,
    generation: i64,
    status: &str,
) -> Result<bool> {
    if !source_schema_has_proof_fields(conn)? {
        let persisted: Option<(i64, String)> = conn
            .query_row(
                "SELECT generation,status FROM remote_scan_state WHERE source_id=?1",
                [source_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        return Ok(persisted == Some((generation, status.to_string())));
    }
    if !source_epoch_matches_on(conn, source_id, generation)? {
        return Ok(false);
    }
    let persisted: Option<(i64, String)> = conn
        .query_row(
            "SELECT generation,status FROM remote_scan_state WHERE source_id=?1",
            [source_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(persisted == Some((generation, status.to_string())))
}

pub(crate) fn current_generation_is_active_with_epoch(
    conn: &Connection,
    source_id: &str,
    generation: i64,
    status: &str,
    session_epoch: &str,
) -> Result<bool> {
    if session_epoch.is_empty() {
        return Ok(false);
    }
    if !source_schema_has_proof_fields(conn)? {
        return Ok(false);
    }
    if !source_epoch_matches_on(conn, source_id, generation)? {
        return Ok(false);
    }
    let persisted: Option<(i64, String)> = conn
        .query_row(
            "SELECT generation,status FROM remote_scan_state WHERE source_id=?1",
            [source_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if persisted != Some((generation, status.to_string())) {
        return Ok(false);
    }
    conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM remote_scan_epoch
           WHERE source_id=?1 AND generation=?2 AND session_epoch=?3
             AND session_epoch <> ''
         )",
        params![source_id, generation, session_epoch],
        |row| row.get(0),
    )
}

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
         CREATE TABLE IF NOT EXISTS remote_listing_state (source_id TEXT NOT NULL,logical_path TEXT NOT NULL,content_fingerprint TEXT,scan_generation INTEGER NOT NULL,listing_complete INTEGER NOT NULL DEFAULT 0,last_checked_at INTEGER,recheck_after INTEGER,PRIMARY KEY(source_id,logical_path));
         CREATE TABLE IF NOT EXISTS remote_cover_dependency (book_key TEXT NOT NULL,dependency_path TEXT NOT NULL,dependency_fingerprint TEXT NOT NULL,profile TEXT NOT NULL,status TEXT NOT NULL,PRIMARY KEY(book_key,dependency_path));
         CREATE TABLE IF NOT EXISTS remote_cover_stage (source_id TEXT NOT NULL,generation INTEGER NOT NULL,book_key TEXT NOT NULL,dependency_path TEXT NOT NULL,dependency_fingerprint TEXT NOT NULL,profile TEXT NOT NULL,session_epoch TEXT NOT NULL DEFAULT '',PRIMARY KEY(source_id,generation,book_key,dependency_path));
         CREATE TABLE IF NOT EXISTS remote_scan_listing_stage (source_id TEXT NOT NULL,generation INTEGER NOT NULL,logical_path TEXT NOT NULL,content_fingerprint TEXT NOT NULL,asset_kind TEXT NOT NULL,entries_json TEXT NOT NULL,incremental INTEGER NOT NULL,session_epoch TEXT NOT NULL DEFAULT '',PRIMARY KEY(source_id,generation,logical_path));
         CREATE TABLE IF NOT EXISTS remote_scan_pending (source_id TEXT NOT NULL,generation INTEGER NOT NULL,logical_path TEXT NOT NULL,incremental INTEGER NOT NULL,force_recheck INTEGER NOT NULL DEFAULT 0,session_epoch TEXT NOT NULL DEFAULT '',PRIMARY KEY(source_id,generation,logical_path));
        CREATE TABLE IF NOT EXISTS remote_scan_config (source_id TEXT PRIMARY KEY,source_type TEXT NOT NULL,root_path TEXT NOT NULL,mode TEXT NOT NULL,generation INTEGER NOT NULL,status TEXT NOT NULL,updated_at INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS remote_scan_baseline (source_id TEXT PRIMARY KEY,source_fingerprint TEXT NOT NULL,root_path TEXT NOT NULL,full_generation INTEGER NOT NULL,updated_at INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS remote_scan_epoch (source_id TEXT NOT NULL,generation INTEGER NOT NULL,source_fingerprint TEXT NOT NULL,root_path TEXT NOT NULL,session_epoch TEXT NOT NULL,session_token INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(source_id,generation));
         CREATE TABLE IF NOT EXISTS remote_cover_partial_cache (book_key TEXT PRIMARY KEY,dependency_fingerprint TEXT NOT NULL,bytes BLOB NOT NULL,updated_at INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS idx_remote_listing_source_path ON remote_listing_state(source_id,logical_path);
         CREATE INDEX IF NOT EXISTS idx_remote_cover_book_path ON remote_cover_dependency(book_key,dependency_path);
         CREATE INDEX IF NOT EXISTS idx_remote_stage_generation ON remote_scan_listing_stage(source_id,generation);
         CREATE INDEX IF NOT EXISTS idx_remote_pending_generation ON remote_scan_pending(source_id,generation);",
    )?;
    for (name, ty) in [("last_checked_at", "INTEGER"), ("recheck_after", "INTEGER")] {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('remote_listing_state') WHERE name=?1)",
            [name],
            |row| row.get(0),
        )?;
        if !exists {
            conn.execute(
                &format!("ALTER TABLE remote_listing_state ADD COLUMN {name} {ty}"),
                [],
            )?;
        }
    }
    super::cover_store::migrate(conn).map_err(|error| {
        rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(error.to_string())))
    })?;
    for table in [
        "remote_cover_stage",
        "remote_scan_listing_stage",
        "remote_scan_pending",
    ] {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name='session_epoch')",
            [table],
            |row| row.get(0),
        )?;
        if !exists {
            conn.execute(
                &format!("ALTER TABLE {table} ADD COLUMN session_epoch TEXT NOT NULL DEFAULT ''"),
                [],
            )?;
        }
    }
    let pending_has_force_recheck: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('remote_scan_pending') WHERE name='force_recheck')",
        [],
        |row| row.get(0),
    )?;
    if !pending_has_force_recheck {
        conn.execute(
            "ALTER TABLE remote_scan_pending ADD COLUMN force_recheck INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    let epoch_has_token: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('remote_scan_epoch') WHERE name='session_token')",
        [],
        |row| row.get(0),
    )?;
    if !epoch_has_token {
        conn.execute(
            "ALTER TABLE remote_scan_epoch ADD COLUMN session_token INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(())
}

/// Return whether this source has a successful full-scan baseline for its
/// current identity and authoritative root. The identity binding is
/// intentional: an old baseline must not authorize incremental scans after a
/// source is edited or a 115/Quark root folder changes.
pub fn has_full_scan_baseline(conn: &Connection, source_id: &str) -> Result<bool> {
    let (fingerprint, root) = source_identity_on(conn, source_id)?;
    conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM remote_scan_baseline
           WHERE source_id=?1 AND source_fingerprint=?2 AND root_path=?3
         )",
        params![source_id, fingerprint, root],
        |row| row.get(0),
    )
}

/// Persist proof that a full scan completed for the current source identity.
/// Callers must invoke this only after the staged listing has been published
/// and all cover tasks have reached their terminal state.
pub fn mark_full_scan_succeeded(conn: &Connection, source_id: &str, generation: i64) -> Result<()> {
    let (fingerprint, root) = source_identity_on(conn, source_id)?;
    conn.execute(
        "INSERT INTO remote_scan_baseline(source_id,source_fingerprint,root_path,full_generation,updated_at)
         VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(source_id) DO UPDATE SET
           source_fingerprint=excluded.source_fingerprint,
           root_path=excluded.root_path,
           full_generation=excluded.full_generation,
           updated_at=excluded.updated_at
         WHERE excluded.full_generation >= remote_scan_baseline.full_generation",
        params![source_id, fingerprint, root, generation, crate::db::now_ms()],
    )?;
    Ok(())
}

/// Bind one scan generation to the concrete source/account session that
/// produced it.  Legacy scans have no row here and therefore cannot prove a
/// deletion after upgrading.
pub fn bind_scan_epoch(
    conn: &Connection,
    source_id: &str,
    generation: i64,
    root_path: &str,
    session: u64,
) -> Result<String> {
    let (fingerprint, authoritative_root) = source_identity_on(conn, source_id)?;
    let root_path = super::model::normalize_path(root_path);
    if root_path != authoritative_root {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let mut digest = Sha256::new();
    digest.update(fingerprint.as_bytes());
    digest.update([0]);
    digest.update(root_path.as_bytes());
    digest.update([0]);
    digest.update(session.to_le_bytes());
    let session_epoch = format!("{:x}", digest.finalize());
    let session_token = i64::try_from(session).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let previous: Option<(String, String, String, i64)> = conn
        .query_row(
            "SELECT source_fingerprint,root_path,session_epoch,session_token FROM remote_scan_epoch WHERE source_id=?1 AND generation=?2",
            params![source_id, generation],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    if let Some(previous) = previous {
        if previous
            != (
                fingerprint.clone(),
                root_path.clone(),
                session_epoch.clone(),
                session_token,
            )
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        return Ok(session_epoch);
    }
    conn.execute(
        "INSERT INTO remote_scan_epoch(source_id,generation,source_fingerprint,root_path,session_epoch,session_token) VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            source_id,
            generation,
            fingerprint,
            root_path,
            session_epoch,
            session_token
        ],
    )?;
    Ok(session_epoch)
}

/// Rebind a completed authoritative generation to the current in-memory
/// provider session after an application restart. Session handles are not
/// persisted, but route/job identities are; without this one-time rebinding
/// a visible cover request would enqueue work against the expired epoch and
/// the new worker would (correctly) refuse to claim it.
///
/// Only a terminal Succeeded generation with an unchanged source identity
/// may be rebound. Running/failed generations remain tied to their original
/// epoch so a stale worker cannot publish into a new session accidentally.
pub fn rebind_completed_generation_session(
    conn: &Connection,
    source_id: &str,
    generation: i64,
    session: u64,
) -> Result<Option<String>> {
    if session == 0 || source_id.trim().is_empty() {
        return Ok(None);
    }
    let (fingerprint, root) = source_identity_on(conn, source_id)?;
    let status: Option<String> = conn
        .query_row(
            "SELECT status FROM remote_scan_state WHERE source_id=?1 AND generation=?2",
            params![source_id, generation],
            |row| row.get(0),
        )
        .optional()?;
    if status.as_deref() != Some("Succeeded") {
        return Ok(None);
    }
    let previous: Option<(String, String, String, i64)> = conn
        .query_row(
            "SELECT source_fingerprint,root_path,session_epoch,session_token
             FROM remote_scan_epoch WHERE source_id=?1 AND generation=?2",
            params![source_id, generation],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((previous_fingerprint, previous_root, previous_epoch, previous_token)) = previous
    else {
        return Ok(None);
    };
    if previous_fingerprint != fingerprint || previous_root != root {
        return Ok(None);
    }
    let token = i64::try_from(session).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let mut digest = Sha256::new();
    digest.update(fingerprint.as_bytes());
    digest.update([0]);
    digest.update(root.as_bytes());
    digest.update([0]);
    digest.update(session.to_le_bytes());
    let new_epoch = format!("{:x}", digest.finalize());
    if previous_epoch == new_epoch && previous_token == token {
        return Ok(Some(new_epoch));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE remote_scan_epoch SET session_epoch=?1,session_token=?2
         WHERE source_id=?3 AND generation=?4",
        params![new_epoch, token, source_id, generation],
    )?;
    tx.execute(
        "UPDATE remote_asset_route SET session_epoch=?1,route_revision=?2
         WHERE source_id=?3 AND generation=?4",
        params![new_epoch, crate::db::now_ms(), source_id, generation],
    )?;
    // Pending/retry work is safe to hand to the new authenticated worker.
    // Running work is requeued after clearing the old lease; any in-flight
    // result from the old session fails the epoch/owner predicate at publish.
    tx.execute(
        "UPDATE remote_cover_job SET session_epoch=?1,
                state=CASE WHEN state='running' THEN 'pending' ELSE state END,
                lease_owner=NULL,lease_until=NULL,updated_at=?2
         WHERE source_id=?3 AND generation=?4
           AND state IN ('pending','running','retry_wait')",
        params![new_epoch, crate::db::now_ms(), source_id, generation],
    )?;
    tx.execute(
        "UPDATE remote_cover_stage SET session_epoch=?1
         WHERE source_id=?2 AND generation=?3",
        params![new_epoch, source_id, generation],
    )?;
    tx.commit()?;
    Ok(Some(new_epoch))
}

pub(crate) fn invalidate_source_proof_on(conn: &Connection, source_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM remote_scan_epoch WHERE source_id=?1",
        [source_id],
    )?;
    // A source edit changes the identity/root against which a full scan was
    // proven. Remove the marker eagerly so an old baseline can never enable
    // an incremental scan for the new connection.
    conn.execute(
        "DELETE FROM remote_scan_baseline WHERE source_id=?1",
        [source_id],
    )?;
    conn.execute(
        "UPDATE remote_scan_state SET status='Failed',error_code='sourceChanged' WHERE source_id=?1",
        [source_id],
    )?;
    Ok(())
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
    if !source_epoch_matches_on(conn, &state.source_id, state.generation)? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let existing: Option<(i64, String)> = conn
        .query_row(
            "SELECT generation,status FROM remote_scan_state WHERE source_id=?1",
            [&state.source_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((generation, status)) = existing {
        if generation > state.generation
            || (generation == state.generation
                && matches!(status.as_str(), "Cancelled" | "Succeeded" | "Failed"))
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    let changed = conn.execute(
        "INSERT INTO remote_scan_state(source_id,status,mode,generation,checkpoint,last_success_at,error_code) VALUES(?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(source_id) DO UPDATE SET status=excluded.status,mode=excluded.mode,generation=excluded.generation,checkpoint=excluded.checkpoint,last_success_at=excluded.last_success_at,error_code=excluded.error_code
         WHERE excluded.generation > remote_scan_state.generation
            OR (excluded.generation = remote_scan_state.generation AND remote_scan_state.status NOT IN ('Cancelled','Succeeded','Failed'))",
        params![state.source_id, format!("{:?}", state.status), format!("{:?}", state.mode), state.generation, state.checkpoint, state.last_success_at, state.error_code],
    )?;
    if changed == 1 {
        Ok(())
    } else {
        Err(rusqlite::Error::InvalidQuery)
    }
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
    let source_fp: String = conn.query_row(
        "SELECT fingerprint FROM book_sources WHERE id=?1 AND fingerprint IS NOT NULL AND fingerprint <> ''",
        [source_id],
        |row| row.get(0),
    )?;
    // A few legacy fixtures (and databases created by pre-source-type builds)
    // do not have the `type` column.  Route identity remains valid; use an
    // explicit neutral provider id until the next real listing supplies it.
    let has_type: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('book_sources') WHERE name='type')",
        [],
        |row| row.get(0),
    )?;
    let provider_id: String = if has_type {
        conn.query_row(
            "SELECT type FROM book_sources WHERE id=?1",
            [source_id],
            |row| row.get(0),
        )?
    } else {
        "unknown".to_string()
    };
    let session_epoch: String = conn
        .query_row(
            "SELECT COALESCE(session_epoch,'') FROM remote_scan_epoch
             WHERE source_id=?1 AND generation=?2",
            params![source_id, generation],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or_default();
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
        super::cover_store::upsert_route_on(
            conn,
            source_id,
            &id,
            &logical_path,
            parent
                .map(|value| crate::db::library_index_id(&source_fp, value))
                .as_deref(),
            &provider_id,
            entry.provider_path.as_deref(),
            &source_fp,
            generation,
            &session_epoch,
            now,
        )?;
    }
    conn.execute(
        "INSERT INTO remote_listing_state(source_id,logical_path,content_fingerprint,scan_generation,listing_complete,last_checked_at,recheck_after)
         VALUES(?1,?2,?3,?4,?5,?6,?6+?7)
         ON CONFLICT(source_id,logical_path) DO UPDATE SET
           content_fingerprint=excluded.content_fingerprint,
           scan_generation=excluded.scan_generation,
           listing_complete=excluded.listing_complete,
           last_checked_at=excluded.last_checked_at,
           recheck_after=excluded.recheck_after",
        params![
            source_id,
            super::model::normalize_path(path),
            fingerprint,
            generation,
            complete as i64,
            now,
            15_i64 * 60 * 1000
        ],
    )?;
    let view_revision =
        super::cover_store::bump_view_revision_on(conn, source_id, generation, now)?;
    let directory_kind = super::model::classify_directory(entries);
    upsert_directory_cover_on(
        conn,
        source_id,
        &super::model::normalize_path(path),
        entries,
        directory_kind,
        view_revision,
    )?;
    Ok(())
}

/// Persist the direct representative discovered while a directory listing is
/// published.  The catalog query remains read-only and can still derive a
/// descendant representative when only nested directories are available.
/// Explicit user selections are never overwritten by a later natural-order
/// refresh while the selected asset is still live.
fn upsert_directory_cover_on(
    conn: &Connection,
    source_id: &str,
    logical_path: &str,
    entries: &[RemoteEntry],
    asset_kind: RemoteAssetKind,
    revision: i64,
) -> Result<()> {
    let source_fp: Option<String> = conn
        .query_row(
            "SELECT fingerprint FROM book_sources WHERE id=?1",
            [source_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(source_fp) = source_fp.filter(|value| !value.trim().is_empty()) else {
        return Ok(());
    };
    let directory_asset_id = crate::db::library_index_id(&source_fp, logical_path);
    let is_directory: bool = conn
        .query_row(
            "SELECT entry_type='dir' FROM library_index WHERE source_id=?1 AND id=?2",
            params![source_id, directory_asset_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(false);
    if !is_directory {
        // The configured root may not have a synthetic library_index row.
        return Ok(());
    }
    let selected: Option<String> = conn
        .query_row(
            "SELECT representative_asset_id FROM remote_directory_cover
             WHERE source_id=?1 AND directory_asset_id=?2 AND selection_reason='user_explicit'",
            params![source_id, directory_asset_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    if selected.is_some() {
        return Ok(());
    }
    let preferred = match asset_kind {
        RemoteAssetKind::ImageFolder => Some(RemoteAssetKind::ImageFile),
        RemoteAssetKind::ContainerDir => Some(RemoteAssetKind::ArchiveFile),
        _ => None,
    };
    let mut candidates = entries
        .iter()
        .filter(|entry| preferred.is_some_and(|kind| entry.asset_kind == kind && !entry.is_dir))
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        crate::util::natural_cmp(&a.name, &b.name).then_with(|| a.logical_path.cmp(&b.logical_path))
    });
    let representative = candidates.first().map(|entry| {
        crate::db::library_index_id(
            &source_fp,
            &super::model::normalize_path(&entry.logical_path),
        )
    });
    conn.execute(
        "INSERT INTO remote_directory_cover(
             source_id,directory_asset_id,representative_asset_id,
             selection_reason,revision,completeness)
         VALUES(?1,?2,?3,'natural_order',?4,?5)
         ON CONFLICT(source_id,directory_asset_id) DO UPDATE SET
             representative_asset_id=excluded.representative_asset_id,
             selection_reason=excluded.selection_reason,
             revision=excluded.revision,
             completeness=excluded.completeness
         WHERE remote_directory_cover.selection_reason <> 'user_explicit'",
        params![
            source_id,
            directory_asset_id,
            representative,
            revision,
            if representative.is_some() {
                "complete"
            } else {
                "pending"
            },
        ],
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

/// Return whether an incremental walk should re-list a directory whose
/// parent fingerprint did not change.  A missing/incomplete row is always
/// due; otherwise the durable TTL controls the next check.  The caller may
/// use `now_ms` from its injected clock in tests, while production passes the
/// shared database clock.
pub fn directory_recheck_due(
    conn: &Connection,
    source_id: &str,
    logical_path: &str,
    now_ms: i64,
) -> Result<bool> {
    let path = super::model::normalize_path(logical_path);
    let row: Option<(bool, Option<i64>)> = conn
        .query_row(
            "SELECT listing_complete,recheck_after
             FROM remote_listing_state
             WHERE source_id=?1 AND logical_path=?2",
            params![source_id, path],
            |row| Ok((row.get::<_, i64>(0)? != 0, row.get(1)?)),
        )
        .optional()?;
    Ok(match row {
        Some((true, Some(recheck_after))) => recheck_after <= now_ms,
        _ => true,
    })
}

#[cfg(test)]
mod directory_recheck_tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE remote_listing_state(
                source_id TEXT NOT NULL,
                logical_path TEXT NOT NULL,
                listing_complete INTEGER NOT NULL,
                recheck_after INTEGER,
                PRIMARY KEY(source_id, logical_path)
            );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn missing_or_incomplete_directory_is_due() {
        let conn = db();
        assert!(directory_recheck_due(&conn, "s", "/new", 0).unwrap());
        conn.execute(
            "INSERT INTO remote_listing_state VALUES('s','/partial',0,NULL)",
            [],
        )
        .unwrap();
        assert!(directory_recheck_due(&conn, "s", "/partial", 0).unwrap());
    }

    #[test]
    fn complete_directory_obeys_recheck_after() {
        let conn = db();
        conn.execute(
            "INSERT INTO remote_listing_state VALUES('s','/fresh',1,100)",
            [],
        )
        .unwrap();
        assert!(!directory_recheck_due(&conn, "s", "/fresh", 99).unwrap());
        assert!(directory_recheck_due(&conn, "s", "/fresh", 100).unwrap());
    }
}

#[allow(clippy::too_many_arguments)]
pub fn stage_complete_listing(
    conn: &Connection,
    source_id: &str,
    path: &str,
    entries: &[RemoteEntry],
    generation: i64,
    fingerprint: &str,
    asset_kind: RemoteAssetKind,
    incremental: bool,
    session_epoch: &str,
) -> Result<()> {
    if source_schema_has_proof_fields(conn)?
        && !current_generation_is_active_with_epoch(
            conn,
            source_id,
            generation,
            "Running",
            session_epoch,
        )?
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let session_epoch = if source_schema_has_proof_fields(conn)? {
        session_epoch.to_string()
    } else {
        String::new()
    };
    let json = serde_json::to_string(entries)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    conn.execute(
        "INSERT INTO remote_scan_listing_stage(source_id,generation,logical_path,content_fingerprint,asset_kind,entries_json,incremental,session_epoch) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(source_id,generation,logical_path) DO UPDATE SET content_fingerprint=excluded.content_fingerprint,asset_kind=excluded.asset_kind,entries_json=excluded.entries_json,incremental=excluded.incremental,session_epoch=excluded.session_epoch",
        params![source_id, generation, super::model::normalize_path(path), fingerprint, format!("{:?}", asset_kind), json, incremental as i64, session_epoch],
    )?;
    Ok(())
}

/// Materialize one listing page into the non-authoritative preview projection.
///
/// Preview rows are deliberately separate from `library_index`: while a
/// generation is running they let the catalog and cover queue make progress,
/// but a cancelled/failed generation can be discarded without touching the
/// last complete listing or its deletion proof. The caller must have already
/// staged the page and verified the current source/session epoch.
pub fn materialize_preview_listing(
    conn: &Connection,
    source_id: &str,
    path: &str,
    entries: &[RemoteEntry],
    generation: i64,
    session_epoch: &str,
    directory_fingerprint: &str,
    directory_kind: RemoteAssetKind,
) -> Result<()> {
    if !current_generation_is_active_with_epoch(
        conn,
        source_id,
        generation,
        "Running",
        session_epoch,
    )? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let source_fp: String = conn.query_row(
        "SELECT fingerprint FROM book_sources WHERE id=?1 AND fingerprint IS NOT NULL AND fingerprint <> ''",
        [source_id],
        |row| row.get(0),
    )?;
    let normalized_path = super::model::normalize_path(path);
    let parent_id = crate::db::library_index_id(&source_fp, &normalized_path);
    let tx = conn.unchecked_transaction()?;
    // A recheck may replace an already staged page. Remove this parent's
    // direct children and descendants in this generation; otherwise a second
    // listing of an emptied/changed directory could leave stale preview
    // grandchildren visible. Preserve the row representing the directory
    // itself because its parent page still needs that card.
    let descendant_prefix = if normalized_path == "/" {
        "/%".to_string()
    } else {
        format!("{normalized_path}/%")
    };
    tx.execute(
        "DELETE FROM remote_scan_preview
         WHERE source_id=?1 AND generation=?2
           AND (parent_asset_id=?3
                OR (logical_path LIKE ?4 AND logical_path<>?5))",
        params![
            source_id,
            generation,
            &parent_id,
            descendant_prefix,
            &normalized_path
        ],
    )?;
    let now = crate::db::now_ms();
    for entry in entries {
        let logical_path = super::model::normalize_path(&entry.logical_path);
        let asset_id = crate::db::library_index_id(&source_fp, &logical_path);
        let parent_path = parent_path(&logical_path);
        let child_parent_id = crate::db::library_index_id(&source_fp, &parent_path);
        tx.execute(
            "INSERT INTO remote_scan_preview(
                source_id,generation,asset_id,parent_asset_id,logical_path,name,
                entry_type,asset_kind,size,modified_at,content_fingerprint,
                provider_file_id,source_fingerprint,session_epoch)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
             ON CONFLICT(source_id,generation,asset_id) DO UPDATE SET
                parent_asset_id=excluded.parent_asset_id,
                logical_path=excluded.logical_path,name=excluded.name,
                entry_type=excluded.entry_type,asset_kind=excluded.asset_kind,
                size=excluded.size,modified_at=excluded.modified_at,
                content_fingerprint=excluded.content_fingerprint,
                provider_file_id=excluded.provider_file_id,
                source_fingerprint=excluded.source_fingerprint,
                session_epoch=excluded.session_epoch",
            params![
                source_id,
                generation,
                asset_id,
                child_parent_id,
                logical_path,
                entry.name,
                if entry.is_dir { "dir" } else { "file" },
                format!("{:?}", entry.asset_kind),
                entry.size.map(|value| value as i64),
                entry.mtime,
                entry_fingerprint(std::slice::from_ref(entry)),
                entry.provider_path.as_deref(),
                &source_fp,
                session_epoch,
            ],
        )?;
    }
    // The parent directory is represented by the entry from its parent page;
    // only this completed page can classify it as an image folder/container.
    // Root itself has no library asset row, so the update is naturally a
    // no-op for the root page.
    tx.execute(
        "UPDATE remote_scan_preview
         SET asset_kind=?1,content_fingerprint=?2
         WHERE source_id=?3 AND generation=?4 AND asset_id=?5",
        params![
            format!("{:?}", directory_kind),
            directory_fingerprint,
            source_id,
            generation,
            parent_id,
        ],
    )?;
    // Revision changes are intentionally batched per listing page, not per
    // entry, so a large root does not amplify Flutter rebuilds or AXTree work.
    super::cover_store::bump_view_revision_on(&tx, source_id, generation, now)?;
    tx.commit()
}

pub fn stage_cover_task(
    conn: &Connection,
    generation: i64,
    book_key: &str,
    task: &CoverTask,
) -> Result<()> {
    if task.generation != generation {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if source_schema_has_proof_fields(conn)?
        && !current_generation_is_active_with_epoch(
            conn,
            &task.source_id,
            generation,
            "Running",
            &task.session_epoch,
        )?
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let session_epoch = if source_schema_has_proof_fields(conn)? {
        task.session_epoch.clone()
    } else {
        String::new()
    };
    conn.execute(
        "INSERT INTO remote_cover_stage(source_id,generation,book_key,dependency_path,dependency_fingerprint,profile,session_epoch) VALUES(?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(source_id,generation,book_key,dependency_path) DO UPDATE SET dependency_fingerprint=excluded.dependency_fingerprint,profile=excluded.profile,session_epoch=excluded.session_epoch",
        params![task.source_id, generation, book_key, super::model::normalize_path(&task.logical_path), task.fingerprint, task.profile, session_epoch],
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
    let parent = super::model::normalize_path(parent);
    if source_schema_has_proof_fields(conn)?
        && !current_generation_is_active(conn, source_id, generation, "Running")?
    {
        return Ok(());
    }
    let proven: bool = if source_schema_has_proof_fields(conn)? {
        conn.query_row(
            "SELECT EXISTS( \
               SELECT 1 FROM remote_listing_state listing \
               JOIN remote_scan_state scan ON scan.source_id=listing.source_id \
               JOIN remote_scan_epoch epoch ON epoch.source_id=scan.source_id AND epoch.generation=scan.generation \
               WHERE listing.source_id=?1 AND listing.logical_path=?2 \
                 AND listing.scan_generation=?3 AND listing.listing_complete=1 \
                 AND scan.generation=?3 AND scan.status='Running' \
                 AND epoch.session_epoch <> '')",
            params![source_id, parent, generation],
            |row| row.get(0),
        )?
    } else {
        conn.query_row(
            "SELECT EXISTS( \
               SELECT 1 FROM remote_listing_state listing \
               JOIN remote_scan_state scan ON scan.source_id=listing.source_id \
               WHERE listing.source_id=?1 AND listing.logical_path=?2 \
                 AND listing.scan_generation=?3 AND listing.listing_complete=1 \
                 AND scan.generation=?3 AND scan.status='Running')",
            params![source_id, parent, generation],
            |row| row.get(0),
        )?
    };
    if complete && proven {
        let now = crate::db::now_ms();
        let source_fp: String = conn.query_row(
            "SELECT fingerprint FROM book_sources WHERE id=?1 AND fingerprint IS NOT NULL AND fingerprint <> ''",
            [source_id],
            |row| row.get(0),
        )?;
        let parent_id = crate::db::library_index_id(&source_fp, &parent);
        for path in paths {
            let path = super::model::normalize_path(path);
            conn.execute(
                "UPDATE library_index SET deleted=0,scan_generation=?1,updated_at=?5 \
                 WHERE source_id=?2 AND parent_id=?3 AND path=?4",
                params![generation, source_id, parent_id, path, now],
            )?;
        }
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
    if !current_generation_is_active(conn, source_id, generation, "Running")? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    // P1-E/U-α（§6 防叠加）：记录进入本事务前的 durable revision。若同一事务内的
    // listing/view publication 已经 bump 过，则在下面**不再**为 cover 变更二次 bump ——
    // revision 表示"原子 durable view changed"，不是"listing +1 再加 cover +1"。
    let revision_before_tx = super::cover_store::view_revision(conn, source_id)?;
    let tx = conn.unchecked_transaction()?;
    loop {
        let staged: Option<(String, String, String, String)> = tx.query_row(
            "SELECT logical_path,content_fingerprint,asset_kind,entries_json FROM remote_scan_listing_stage WHERE source_id=?1 AND generation=?2 AND session_epoch=COALESCE((SELECT session_epoch FROM remote_scan_epoch WHERE source_id=?1 AND generation=?2),'') ORDER BY logical_path LIMIT 1",
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
         SELECT book_key,dependency_path,dependency_fingerprint,profile,'queued' FROM remote_cover_stage WHERE source_id=?1 AND generation=?2 AND session_epoch=COALESCE((SELECT session_epoch FROM remote_scan_epoch WHERE source_id=?1 AND generation=?2),'')
         ON CONFLICT(book_key,dependency_path) DO UPDATE SET dependency_fingerprint=excluded.dependency_fingerprint,profile=excluded.profile,status='queued'",
        params![source_id, generation],
    )?;
    // The legacy dependency rows remain the cleanup compatibility layer.  A
    // published generation also materializes one deduplicated job per asset so
    // visible cards and background scanning can share the same queue key.
    // Older/minimal fixtures may stage a generation before a corresponding
    // `book_sources` row exists.  The legacy dependency publication was
    // intentionally independent of that row, so keep the additive unified
    // projection equally tolerant: a missing/empty fingerprint falls back to
    // the source id as a deterministic, source-scoped namespace.  Production
    // sessions still provide the real fingerprint through `book_sources`.
    let source_fp: String = tx
        .query_row(
            "SELECT fingerprint FROM book_sources WHERE id=?1",
            [source_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| source_id.to_string());
    // P1-E/U-α：本批次是否发生过 observable cover upsert。
    // U-B 已冻结：即使 lifecycle 字段等价，upsert 也刷新 updated_at ⇒ 属 observable
    // change，因此"发生过成功 upsert"就是当前最小可用的 changed gate
    //（不能只看新建数 jobs_created）。
    let mut cover_changed_any = false;
    let session_epoch: String = tx
        .query_row(
            "SELECT COALESCE(session_epoch,'') FROM remote_scan_epoch WHERE source_id=?1 AND generation=?2",
            params![source_id, generation],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or_default();
    let mut staged_jobs = tx.prepare(
        "SELECT dependency_path,dependency_fingerprint FROM remote_cover_stage
         WHERE source_id=?1 AND generation=?2
           AND session_epoch=COALESCE((SELECT session_epoch FROM remote_scan_epoch WHERE source_id=?1 AND generation=?2),'')
         GROUP BY dependency_path,dependency_fingerprint",
    )?;
    let staged_rows = staged_jobs
        .query_map(params![source_id, generation], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(staged_jobs);
    for (dependency_path, content_revision) in staged_rows {
        let asset_id = crate::db::library_index_id(&source_fp, &dependency_path);
        let key = super::cover_model::CoverJobKey {
            source_id: source_id.to_string(),
            asset_id,
            content_revision,
            selection_revision: "default".into(),
            profile: "340x480@1".into(),
        };
        cover_changed_any = true;
        super::cover_store::upsert_job_on(
            &tx,
            &key,
            super::cover_model::CoverJobState::Pending,
            "background",
            10,
            generation,
            &session_epoch,
            crate::db::now_ms(),
            CoverJobUpsertCause::Demand,
        )
        .map_err(|error| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(
                error.to_string(),
            )))
        })?;
    }
    tx.execute(
        "DELETE FROM remote_scan_pending WHERE source_id=?1 AND generation=?2",
        params![source_id, generation],
    )?;
    // P1-E/U-α：一次原子事务只 bump 一次（N 本仍 +1）；若 listing publication 在同
    // 一事务内已经 bump 过，则此处**不再** bump，避免 revision +2。
    if cover_changed_any && super::cover_store::view_revision(&tx, source_id)? == revision_before_tx
    {
        super::cover_store::bump_view_revision_on(&tx, source_id, generation, crate::db::now_ms())?;
    }
    // Once the staged generation is authoritative, its preview rows are no
    // longer needed. The materialized library/route rows now carry the same
    // identities and cover jobs continue against that generation.
    tx.execute(
        "DELETE FROM remote_scan_preview WHERE source_id=?1 AND generation=?2",
        params![source_id, generation],
    )?;
    tx.commit()?;
    if cover_changed_any {
        // P1-E：revision token 可以共用，但 transport wake 仍需 cover 语义 ——
        // 即使本事务已因 listing publication bump 过 revision，也要发这一次 cover wake。
        crate::remote_scan::cover_revision_stream::notify_cover_revision(source_id, None);
    }
    Ok(())
}

pub fn discard_staged_generation(
    conn: &Connection,
    source_id: &str,
    generation: i64,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    // Stop preview cover jobs before removing their stage/route context. A
    // worker may already hold a lease; changing the durable state to
    // cancelled makes its late publish fail the running-lease predicate.
    tx.execute(
        "UPDATE remote_cover_job SET state='cancelled',error_code='generationDiscarded',
                lease_owner=NULL,lease_until=NULL,next_attempt_at=NULL,updated_at=?3
         WHERE source_id=?1 AND generation=?2
           AND state IN ('pending','running','retry_wait')",
        params![source_id, generation, crate::db::now_ms()],
    )?;
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
    tx.execute(
        "DELETE FROM remote_scan_preview WHERE source_id=?1 AND generation=?2",
        params![source_id, generation],
    )?;
    tx.commit()
}

pub fn next_staged_cover_task(
    conn: &Connection,
    source_id: &str,
    generation: i64,
) -> Result<Option<(String, CoverTask)>> {
    if !current_generation_is_active(conn, source_id, generation, "Running")? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    conn.query_row(
        "SELECT book_key,dependency_path,dependency_fingerprint,profile,session_epoch FROM remote_cover_stage WHERE source_id=?1 AND generation=?2 AND session_epoch=(SELECT session_epoch FROM remote_scan_epoch WHERE source_id=?1 AND generation=?2) ORDER BY dependency_path LIMIT 1",
        params![source_id, generation],
        |row| {
            Ok((
                row.get(0)?,
                CoverTask {
                    source_id: source_id.to_string(),
                    logical_path: row.get(1)?,
                    fingerprint: row.get(2)?,
                    profile: row.get(3)?,
                    generation,
                    session_epoch: row.get(4)?,
                },
            ))
        },
    )
    .optional()
}

/// Number of distinct comic cover tasks discovered in a running generation.
/// This is deliberately based on staged cover dependencies rather than
/// directory queue length: a directory task is an implementation detail and
/// must never be presented as a comic count in the UI.
pub fn count_staged_cover_tasks(
    conn: &Connection,
    source_id: &str,
    generation: i64,
) -> Result<u64> {
    conn.query_row(
        "SELECT COUNT(DISTINCT book_key) FROM remote_cover_stage
         WHERE source_id=?1 AND generation=?2
           AND session_epoch=COALESCE((SELECT session_epoch FROM remote_scan_epoch WHERE source_id=?1 AND generation=?2),'')",
        params![source_id, generation],
        |row| row.get::<_, i64>(0).map(|value| value.max(0) as u64),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn finish_cover_task(
    conn: &Connection,
    source_id: &str,
    generation: i64,
    book_key: &str,
    dependency_path: &str,
    session_epoch: &str,
    status: &str,
    bytes: Option<&[u8]>,
) -> Result<()> {
    finish_cover_task_with_aliases(
        conn,
        source_id,
        generation,
        book_key,
        dependency_path,
        session_epoch,
        status,
        &[],
        bytes,
    )
}

/// Finish a staged cover task and persist any provider-facing cache aliases
/// alongside the canonical dependency. Opaque cloud providers expose a file
/// id to the existing cover API while the scanner indexes a logical path; the
/// alias row makes that cache entry removable after a later verified deletion,
/// even after the provider session (and its in-memory id map) is gone.
#[allow(clippy::too_many_arguments)]
pub fn finish_cover_task_with_aliases(
    conn: &Connection,
    source_id: &str,
    generation: i64,
    book_key: &str,
    dependency_path: &str,
    session_epoch: &str,
    status: &str,
    cache_aliases: &[String],
    bytes: Option<&[u8]>,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let dependency_path = super::model::normalize_path(dependency_path);
    if source_schema_has_proof_fields(&tx)?
        && !current_generation_is_active_with_epoch(
            &tx,
            source_id,
            generation,
            "Running",
            session_epoch,
        )?
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let staged: bool = if source_schema_has_proof_fields(&tx)? {
        tx.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM remote_cover_stage stage
               JOIN remote_scan_epoch epoch ON epoch.source_id=stage.source_id AND epoch.generation=stage.generation AND epoch.session_epoch=stage.session_epoch
               WHERE stage.source_id=?1 AND stage.generation=?2 AND stage.book_key=?3
                 AND stage.dependency_path=?4 AND stage.session_epoch=?5 AND stage.session_epoch <> '')",
            params![source_id, generation, book_key, dependency_path, session_epoch],
            |row| row.get(0),
        )?
    } else {
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM remote_cover_stage WHERE source_id=?1 AND generation=?2 AND book_key=?3 AND dependency_path=?4)",
            params![source_id, generation, book_key, dependency_path],
            |row| row.get(0),
        )?
    };
    if !staged {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if let Some(bytes) = bytes {
        tx.execute(
            "INSERT INTO remote_cover_partial_cache(book_key,dependency_fingerprint,bytes,updated_at)
             SELECT book_key,dependency_fingerprint,?2,?3 FROM remote_cover_stage WHERE source_id=?1 AND generation=?4 AND book_key=?5 AND dependency_path=?6 AND session_epoch=?7
             ON CONFLICT(book_key) DO UPDATE SET dependency_fingerprint=excluded.dependency_fingerprint,bytes=excluded.bytes,updated_at=excluded.updated_at",
            params![source_id, bytes, crate::db::now_ms(), generation, book_key, dependency_path, session_epoch],
        )?;
    }
    tx.execute(
        "UPDATE remote_cover_dependency SET status=?1 WHERE book_key=?2 AND dependency_path=?3",
        params![status, book_key, dependency_path],
    )?;
    if status == "partial_ready" {
        for alias in cache_aliases {
            // Opaque provider paths are also cache keys. Preserve the exact
            // spelling (for example `fid` rather than `/fid`) so later
            // verified cleanup hashes the same key that was written.
            let alias = alias.trim();
            if alias.is_empty() || alias == dependency_path {
                continue;
            }
            tx.execute(
                "INSERT INTO remote_cover_dependency(book_key,dependency_path,dependency_fingerprint,profile,status)
                 SELECT book_key,?2,dependency_fingerprint,profile,'cache_alias'
                 FROM remote_cover_dependency
                 WHERE book_key=?1 AND dependency_path=?3
                 ON CONFLICT(book_key,dependency_path) DO UPDATE SET
                   dependency_fingerprint=excluded.dependency_fingerprint,
                   profile=excluded.profile,
                   status='cache_alias'",
                params![book_key, alias, dependency_path],
            )?;
        }
    }
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

fn parent_path(path: &str) -> String {
    let path = super::model::normalize_path(path);
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(index) => path[..index].to_string(),
    }
}

pub fn verify_remote_tombstone(
    conn: &Connection,
    source_id: &str,
    logical_path: &str,
) -> Result<bool> {
    if !source_schema_has_proof_fields(conn)? {
        return Ok(false);
    }
    let logical_path = super::model::normalize_path(logical_path);
    let parent = parent_path(&logical_path);
    let source_fp: Option<String> = conn
        .query_row(
            "SELECT fingerprint FROM book_sources \
             WHERE id=?1 AND fingerprint IS NOT NULL AND fingerprint <> ''",
            [source_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(source_fp) = source_fp else {
        return Ok(false);
    };
    let listing_generation: Option<i64> = conn
        .query_row(
            "SELECT scan_generation FROM remote_listing_state WHERE source_id=?1 AND logical_path=?2 AND listing_complete=1",
            params![source_id, parent],
            |row| row.get(0),
        )
        .optional()?;
    let Some(listing_generation) = listing_generation else {
        return Ok(false);
    };
    if !current_generation_is_active(conn, source_id, listing_generation, "Succeeded")? {
        return Ok(false);
    }
    let parent_id = crate::db::library_index_id(&source_fp, &parent);
    conn.query_row(
        "SELECT EXISTS( \
           SELECT 1 FROM library_index child \
           JOIN remote_listing_state listing \
             ON listing.source_id=child.source_id AND listing.logical_path=?4 \
           JOIN remote_scan_state scan ON scan.source_id=child.source_id \
           JOIN remote_scan_epoch epoch ON epoch.source_id=scan.source_id AND epoch.generation=scan.generation AND epoch.source_fingerprint=?5 \
           WHERE child.source_id=?1 AND child.path=?2 AND child.parent_id=?3 \
             AND child.deleted=1 AND child.listing_complete=1 \
             AND child.scan_generation < listing.scan_generation \
             AND listing.listing_complete=1 \
             AND listing.scan_generation=scan.generation \
             AND scan.status='Succeeded' AND epoch.session_epoch <> '')",
        params![source_id, logical_path, parent_id, parent, source_fp],
        |row| row.get(0),
    )
}

pub fn load_verified_remote_tombstones(
    conn: &Connection,
    source_id: &str,
) -> Result<Vec<VerifiedRemoteTombstone>> {
    let source_type: Option<String> = conn
        .query_row(
            "SELECT type FROM book_sources WHERE id=?1",
            [source_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(source_type) = source_type else {
        return Ok(Vec::new());
    };
    let paths = conn
        .prepare(
            "SELECT path FROM library_index \
             WHERE source_id=?1 AND deleted=1 ORDER BY path",
        )?
        .query_map([source_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>>>()?;
    let dependency_prefix = format!("{source_type}|{source_id}|");
    let dependencies = conn
        .prepare(
            "SELECT book_key,dependency_path,status FROM remote_cover_dependency \
             WHERE substr(book_key,1,length(?1))=?1 ORDER BY book_key,dependency_path",
        )?
        .query_map([dependency_prefix], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>>>()?;

    let mut verified = Vec::new();
    for path in paths {
        let path = super::model::normalize_path(&path);
        if !verify_remote_tombstone(conn, source_id, &path)? {
            continue;
        }
        let path_prefix = format!("{path}/");
        let logical_book = crate::db::book_key_of(&source_type, source_id, &path);
        let logical_book_prefix = format!("{logical_book}/");
        let dependency_paths = dependencies
            .iter()
            .filter(|(book_key, dependency_path, _status)| {
                let normalized_dependency_path = super::model::normalize_path(dependency_path);
                book_key == &logical_book
                    || book_key.starts_with(&logical_book_prefix)
                    || normalized_dependency_path == path
                    || normalized_dependency_path.starts_with(&path_prefix)
            })
            .map(|(_, dependency_path, status)| {
                // Provider cache aliases are opaque IDs. Keep their exact
                // spelling because cover/page/raw cache keys hash the raw
                // provider path (which may not start with '/'). Canonical
                // logical paths continue to use the normalized form.
                if status == "cache_alias" {
                    dependency_path.clone()
                } else {
                    super::model::normalize_path(dependency_path)
                }
            })
            .collect();
        verified.push(VerifiedRemoteTombstone {
            logical_path: path,
            dependency_paths,
        });
    }
    Ok(verified)
}

pub fn store_pending_task(conn: &Connection, task: &ScanDirectoryTask) -> Result<()> {
    let session_epoch = if source_schema_has_proof_fields(conn)? {
        if !current_generation_is_active_with_epoch(
            conn,
            &task.source_id,
            task.generation,
            "Running",
            &task.session_epoch,
        )? {
            return Err(rusqlite::Error::InvalidQuery);
        }
        task.session_epoch.clone()
    } else {
        String::new()
    };
    conn.execute("INSERT OR IGNORE INTO remote_scan_pending(source_id,generation,logical_path,incremental,force_recheck,session_epoch) VALUES(?1,?2,?3,?4,?5,?6)", params![task.source_id, task.generation, super::model::normalize_path(&task.logical_path), task.incremental as i64, task.force_recheck as i64, session_epoch])?;
    Ok(())
}

pub fn take_pending_task(
    conn: &Connection,
    source_id: &str,
    generation: i64,
) -> Result<Option<ScanDirectoryTask>> {
    let tx = conn.unchecked_transaction()?;
    let row: Option<(String, bool, bool, String)> = tx.query_row("SELECT logical_path,incremental,force_recheck,session_epoch FROM remote_scan_pending WHERE source_id=?1 AND generation=?2 AND session_epoch=COALESCE((SELECT session_epoch FROM remote_scan_epoch WHERE source_id=?1 AND generation=?2),'') ORDER BY rowid DESC LIMIT 1", params![source_id, generation], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).optional()?;
    if let Some((path, incremental, force_recheck, session_epoch)) = row {
        tx.execute("DELETE FROM remote_scan_pending WHERE source_id=?1 AND generation=?2 AND logical_path=?3", params![source_id, generation, path])?;
        tx.commit()?;
        let task =
            ScanDirectoryTask::new(source_id, path, generation).with_session_epoch(session_epoch);
        Ok(Some(if incremental {
            let task = task.incremental();
            if force_recheck {
                task.force_recheck()
            } else {
                task
            }
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
            size: row
                .get::<_, Option<i64>>(2)?
                .map(|value| value.max(0) as u64),
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
            "INSERT INTO remote_listing_state(source_id,logical_path,content_fingerprint,scan_generation,listing_complete) VALUES('s','/Book','folder-fp',4,1)",
            [],
        )
        .unwrap();
        for (name, path, kind) in [
            ("2.jpg", "/Book/2.jpg", "ImageFile"),
            ("note.txt", "/Book/note.txt", "Other"),
        ] {
            conn.execute(
                "INSERT INTO library_index VALUES('s',NULL,?1,?2,'file',3,7,?3,'child-fp',4,1,0)",
                params![name, path, kind],
            )
            .unwrap();
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
            "INSERT INTO remote_listing_state(source_id,logical_path,content_fingerprint,scan_generation,listing_complete) VALUES('s','/Book','folder-fp',5,1)",
            [],
        )
        .unwrap();

        assert!(load_complete_image_folder_manifest(&conn, "s", "/Book")
            .unwrap()
            .is_empty());
    }
}

#[cfg(test)]
mod verified_remote_deletion_tests {
    use super::*;

    fn deletion_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE book_sources(id TEXT PRIMARY KEY,type TEXT NOT NULL,fingerprint TEXT NOT NULL,path TEXT,root_id TEXT);\
             CREATE TABLE library_index(\
               id TEXT PRIMARY KEY,source_id TEXT NOT NULL,parent_id TEXT,name TEXT,path TEXT,entry_type TEXT,\
               size INTEGER,modified_at INTEGER,asset_kind TEXT,content_fingerprint TEXT,\
               scan_generation INTEGER,listing_complete INTEGER NOT NULL DEFAULT 0,deleted INTEGER NOT NULL DEFAULT 0,updated_at INTEGER);",
        )
        .unwrap();
        migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO book_sources VALUES('source','webdav','canonical-source','/',NULL)",
            [],
        )
        .unwrap();
        conn
    }

    fn insert_child(conn: &Connection, source_id: &str, source_fp: &str, path: &str) {
        let parent = path
            .rsplit_once('/')
            .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
            .unwrap();
        conn.execute(
            "INSERT INTO library_index(id,source_id,parent_id,name,path,entry_type,scan_generation,listing_complete,deleted,updated_at)\
             VALUES(?1,?2,?3,?4,?5,'file',1,1,0,1)",
            params![
                crate::db::library_index_id(source_fp, path),
                source_id,
                crate::db::library_index_id(source_fp, parent),
                path.rsplit('/').next().unwrap(),
                path,
            ],
        )
        .unwrap();
    }

    fn set_scan_state(conn: &Connection, generation: i64, status: &str) {
        bind_scan_epoch(conn, "source", generation, "/", generation as u64).unwrap();
        conn.execute(
            "INSERT INTO remote_scan_state(source_id,status,mode,generation) VALUES('source',?1,'Snapshot',?2)\
             ON CONFLICT(source_id) DO UPDATE SET status=excluded.status,generation=excluded.generation",
            params![status, generation],
        )
        .unwrap();
    }

    #[test]
    fn legacy_proof_without_session_epoch_is_rejected() {
        let conn = deletion_db();
        insert_child(&conn, "source", "canonical-source", "/gone.cbz");
        conn.execute(
            "UPDATE library_index SET deleted=1 WHERE source_id='source' AND path='/gone.cbz'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO remote_listing_state(source_id,logical_path,content_fingerprint,scan_generation,listing_complete) VALUES('source','/','root-v2',2,1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO remote_scan_state(source_id,status,mode,generation) VALUES('source','Succeeded','Snapshot',2)",
            [],
        )
        .unwrap();

        assert!(!verify_remote_tombstone(&conn, "source", "/gone.cbz").unwrap());
    }

    #[test]
    fn stale_terminal_write_cannot_regress_newer_generation() {
        let conn = deletion_db();
        let state = |generation, status| RemoteScanState {
            source_id: "source".into(),
            status,
            mode: super::super::model::RemoteScanMode::Snapshot,
            generation,
            checkpoint: None,
            last_success_at: None,
            error_code: None,
        };
        bind_scan_epoch(&conn, "source", 2, "/", 2).unwrap();
        mark_scan_status(
            &conn,
            &state(2, super::super::model::RemoteScanStatus::Running),
        )
        .unwrap();
        bind_scan_epoch(&conn, "source", 3, "/", 3).unwrap();
        mark_scan_status(
            &conn,
            &state(3, super::super::model::RemoteScanStatus::Running),
        )
        .unwrap();

        assert!(mark_scan_status(
            &conn,
            &state(2, super::super::model::RemoteScanStatus::Failed)
        )
        .is_err());
        let current: (i64, String) = conn
            .query_row(
                "SELECT generation,status FROM remote_scan_state WHERE source_id='source'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(current, (3, "Running".into()));
    }

    #[test]
    fn complete_current_root_listing_tombstones_exact_children_without_root_row() {
        let conn = deletion_db();
        insert_child(&conn, "source", "canonical-source", "/gone.cbz");
        insert_child(&conn, "source", "canonical-source", "/kept.cbz");
        conn.execute(
            "INSERT INTO remote_listing_state(source_id,logical_path,content_fingerprint,scan_generation,listing_complete) VALUES('source','/','root-v2',2,1)",
            [],
        )
        .unwrap();
        set_scan_state(&conn, 2, "Running");

        replace_verified_children(&conn, "source", "///", &["/kept.cbz/".to_string()], 2, true)
            .unwrap();

        let states: (i64, i64) = conn
            .query_row(
                "SELECT SUM(path='/gone.cbz' AND deleted=1),SUM(path='/kept.cbz' AND deleted=0) FROM library_index WHERE source_id='source'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(states, (1, 1));
    }

    #[test]
    fn verified_empty_non_root_listing_tombstones_only_direct_children_without_parent_row() {
        let conn = deletion_db();
        insert_child(&conn, "source", "canonical-source", "/Shelf/gone.cbz");
        insert_child(
            &conn,
            "source",
            "canonical-source",
            "/Shelf/Nested/keep.cbz",
        );
        conn.execute(
            "INSERT INTO remote_listing_state(source_id,logical_path,content_fingerprint,scan_generation,listing_complete) VALUES('source','/Shelf','shelf-v2',2,1)",
            [],
        )
        .unwrap();
        set_scan_state(&conn, 2, "Running");

        replace_verified_children(&conn, "source", "/Shelf/", &[], 2, true).unwrap();

        let states: (i64, i64) = conn
            .query_row(
                "SELECT SUM(path='/Shelf/gone.cbz' AND deleted=1),\
                        SUM(path='/Shelf/Nested/keep.cbz' AND deleted=0)\
                 FROM library_index WHERE source_id='source'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(states, (1, 1));
    }

    #[test]
    fn incomplete_failed_or_stale_proof_never_changes_existing_rows() {
        for (complete, proof_generation, current_generation, status) in [
            (false, 2, 2, "Running"),
            (true, 1, 2, "Running"),
            (true, 2, 2, "Failed"),
        ] {
            let conn = deletion_db();
            insert_child(&conn, "source", "canonical-source", "/kept.cbz");
            insert_child(&conn, "source", "canonical-source", "/gone.cbz");
            conn.execute(
                "INSERT INTO remote_listing_state(source_id,logical_path,content_fingerprint,scan_generation,listing_complete) VALUES('source','/','root',?1,?2)",
                params![proof_generation, complete],
            )
            .unwrap();
            set_scan_state(&conn, current_generation, status);

            replace_verified_children(
                &conn,
                "source",
                "/",
                &["/kept.cbz".to_string()],
                2,
                complete,
            )
            .unwrap();

            let deleted: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM library_index WHERE source_id='source' AND deleted=1",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                deleted, 0,
                "case {complete}/{proof_generation}/{current_generation}/{status}"
            );
        }
    }

    #[test]
    fn verified_tombstones_require_succeeded_current_parent_proof_and_keep_siblings() {
        let conn = deletion_db();
        conn.execute(
            "INSERT INTO book_sources VALUES('sibling','webdav','canonical-sibling','/',NULL)",
            [],
        )
        .unwrap();
        for (source_id, fp, path) in [
            ("source", "canonical-source", "/gone.cbz"),
            ("sibling", "canonical-sibling", "/gone.cbz"),
        ] {
            insert_child(&conn, source_id, fp, path);
            conn.execute(
                "UPDATE library_index SET deleted=1 WHERE source_id=?1 AND path=?2",
                params![source_id, path],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO remote_listing_state(source_id,logical_path,content_fingerprint,scan_generation,listing_complete) VALUES('source','/','root-v2',2,1)",
            [],
        )
        .unwrap();
        set_scan_state(&conn, 2, "Succeeded");

        let tombstones = load_verified_remote_tombstones(&conn, "source").unwrap();
        assert_eq!(tombstones.len(), 1);
        assert_eq!(tombstones[0].logical_path, "/gone.cbz");
        assert!(tombstones[0].dependency_paths.is_empty());
    }

    #[test]
    fn deleted_row_from_current_generation_is_not_a_verified_missing_child() {
        let conn = deletion_db();
        insert_child(&conn, "source", "canonical-source", "/gone.cbz");
        conn.execute(
            "UPDATE library_index SET deleted=1,scan_generation=2 WHERE source_id='source' AND path='/gone.cbz'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO remote_listing_state(source_id,logical_path,content_fingerprint,scan_generation,listing_complete) VALUES('source','/','root-v2',2,1)",
            [],
        )
        .unwrap();
        set_scan_state(&conn, 2, "Succeeded");

        assert!(!verify_remote_tombstone(&conn, "source", "/gone.cbz").unwrap());
        assert!(load_verified_remote_tombstones(&conn, "source")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn verified_tombstone_dependencies_match_source_id_literally() {
        let conn = deletion_db();
        conn.execute("UPDATE book_sources SET id='source%' WHERE id='source'", [])
            .unwrap();
        conn.execute(
            "INSERT INTO book_sources VALUES('sourceX','webdav','canonical-sibling','/',NULL)",
            [],
        )
        .unwrap();
        insert_child(&conn, "source%", "canonical-source", "/gone.cbz");
        conn.execute(
            "UPDATE library_index SET deleted=1 WHERE source_id='source%' AND path='/gone.cbz'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO remote_listing_state(source_id,logical_path,content_fingerprint,scan_generation,listing_complete) VALUES('source%','/','root-v2',2,1)",
            [],
        )
        .unwrap();
        bind_scan_epoch(&conn, "source%", 2, "/", 2).unwrap();
        conn.execute(
            "INSERT INTO remote_scan_state(source_id,status,mode,generation) VALUES('source%','Succeeded','Snapshot',2)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO remote_cover_dependency VALUES('webdav|sourceX|/other','/gone.cbz','other-v1','default','partial_ready')",
            [],
        )
        .unwrap();

        let tombstones = load_verified_remote_tombstones(&conn, "source%").unwrap();

        assert_eq!(tombstones.len(), 1);
        assert!(tombstones[0].dependency_paths.is_empty());
    }

    #[test]
    fn same_generation_terminal_state_absorbs_delayed_running_checkpoint() {
        let conn = deletion_db();
        bind_scan_epoch(&conn, "source", 2, "/", 2).unwrap();
        let state = |status| RemoteScanState {
            source_id: "source".into(),
            status,
            mode: super::super::model::RemoteScanMode::Snapshot,
            generation: 2,
            checkpoint: Some("/late".into()),
            last_success_at: None,
            error_code: None,
        };
        mark_scan_status(
            &conn,
            &state(super::super::model::RemoteScanStatus::Running),
        )
        .unwrap();
        mark_scan_status(&conn, &state(super::super::model::RemoteScanStatus::Failed)).unwrap();
        assert!(mark_scan_status(
            &conn,
            &state(super::super::model::RemoteScanStatus::Running)
        )
        .is_err());
        let current: String = conn
            .query_row(
                "SELECT status FROM remote_scan_state WHERE source_id='source'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(current, "Failed");
        let delayed_cover = CoverTask {
            source_id: "source".into(),
            logical_path: "/book.cbz".into(),
            fingerprint: "book-v1".into(),
            profile: "default".into(),
            generation: 2,
            session_epoch: String::new(),
        };
        assert!(stage_cover_task(&conn, 2, "book-key", &delayed_cover).is_err());
    }

    #[test]
    fn mismatched_requested_root_cannot_bind_current_source_epoch() {
        let conn = deletion_db();
        assert!(!requested_root_matches_source(&conn, "source", "/other").unwrap());
        assert!(requested_root_matches_source(&conn, "source", "///").unwrap());
    }

    #[test]
    fn delayed_cover_completion_after_epoch_replacement_is_rejected() {
        let conn = deletion_db();
        bind_scan_epoch(&conn, "source", 2, "/", 2).unwrap();
        conn.execute(
            "INSERT INTO remote_scan_state(source_id,status,mode,generation) VALUES('source','Running','Snapshot',2)",
            [],
        )
        .unwrap();
        let task = CoverTask {
            source_id: "source".into(),
            logical_path: "/book.cbz".into(),
            fingerprint: "book-v1".into(),
            profile: "default".into(),
            generation: 2,
            session_epoch: conn
                .query_row(
                    "SELECT session_epoch FROM remote_scan_epoch WHERE source_id='source' AND generation=2",
                    [],
                    |row| row.get(0),
                )
                .unwrap(),
        };
        stage_cover_task(&conn, 2, "book-key", &task).unwrap();
        conn.execute("DELETE FROM remote_scan_epoch WHERE source_id='source'", [])
            .unwrap();
        assert!(finish_cover_task(
            &conn,
            "source",
            2,
            "book-key",
            "/book.cbz",
            &task.session_epoch,
            "partial_ready",
            Some(&[1, 2, 3]),
        )
        .is_err());
        let partial_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM remote_cover_partial_cache WHERE book_key='book-key'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(partial_count, 0);
    }

    #[test]
    fn old_session_generation_cannot_stage_after_new_session_binds() {
        let conn = deletion_db();
        bind_scan_epoch(&conn, "source", 2, "/", 11).unwrap();
        conn.execute(
            "INSERT INTO remote_scan_state(source_id,status,mode,generation) VALUES('source','Running','Snapshot',2)",
            [],
        )
        .unwrap();
        let old_task = CoverTask {
            source_id: "source".into(),
            logical_path: "/book.cbz".into(),
            fingerprint: "book-v1".into(),
            profile: "default".into(),
            generation: 2,
            session_epoch: conn
                .query_row(
                    "SELECT session_epoch FROM remote_scan_epoch WHERE source_id='source' AND generation=2",
                    [],
                    |row| row.get(0),
                )
                .unwrap(),
        };
        bind_scan_epoch(&conn, "source", 3, "/", 22).unwrap();
        conn.execute(
            "UPDATE remote_scan_state SET generation=3 WHERE source_id='source'",
            [],
        )
        .unwrap();
        assert!(stage_cover_task(&conn, 2, "book-key", &old_task).is_err());
    }

    #[test]
    fn staged_cover_task_retains_origin_generation_and_session_epoch() {
        let conn = deletion_db();
        bind_scan_epoch(&conn, "source", 2, "/", 11).unwrap();
        conn.execute(
            "INSERT INTO remote_scan_state(source_id,status,mode,generation) VALUES('source','Running','Snapshot',2)",
            [],
        )
        .unwrap();
        let session_epoch: String = conn
            .query_row(
                "SELECT session_epoch FROM remote_scan_epoch WHERE source_id='source' AND generation=2",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let task = CoverTask {
            source_id: "source".into(),
            logical_path: "/book.cbz".into(),
            fingerprint: "book-v1".into(),
            profile: "default".into(),
            generation: 2,
            session_epoch: session_epoch.clone(),
        };
        stage_cover_task(&conn, 2, "book-key", &task).unwrap();
        let (_, queued) = next_staged_cover_task(&conn, "source", 2).unwrap().unwrap();
        assert_eq!(queued.generation, 2);
        assert_eq!(queued.session_epoch, session_epoch);
    }

    #[test]
    fn finished_cover_task_persists_opaque_cache_alias_for_later_cleanup() {
        let conn = deletion_db();
        bind_scan_epoch(&conn, "source", 2, "/", 11).unwrap();
        conn.execute(
            "INSERT INTO remote_scan_state(source_id,status,mode,generation) VALUES('source','Running','Snapshot',2)",
            [],
        )
        .unwrap();
        let session_epoch: String = conn
            .query_row(
                "SELECT session_epoch FROM remote_scan_epoch WHERE source_id='source' AND generation=2",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let task = CoverTask {
            source_id: "source".into(),
            logical_path: "/book.cbz".into(),
            fingerprint: "book-v1".into(),
            profile: "default".into(),
            generation: 2,
            session_epoch,
        };
        stage_cover_task(&conn, 2, "book-key", &task).unwrap();
        publish_staged_generation(&conn, "source", 2).unwrap();
        finish_cover_task_with_aliases(
            &conn,
            "source",
            2,
            "book-key",
            "/book.cbz",
            &task.session_epoch,
            "partial_ready",
            &["fid-opaque".into()],
            Some(&[1, 2, 3]),
        )
        .unwrap();

        let dependencies: Vec<(String, String)> = conn
            .prepare(
                "SELECT dependency_path,status FROM remote_cover_dependency WHERE book_key='book-key' ORDER BY dependency_path",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            dependencies,
            vec![
                ("/book.cbz".into(), "partial_ready".into()),
                ("fid-opaque".into(), "cache_alias".into()),
            ]
        );
    }

    #[test]
    fn full_scan_baseline_is_bound_to_source_identity_and_invalidated_on_edit() {
        let conn = deletion_db();
        assert!(!has_full_scan_baseline(&conn, "source").unwrap());

        mark_full_scan_succeeded(&conn, "source", 2).unwrap();
        assert!(has_full_scan_baseline(&conn, "source").unwrap());

        // A source fingerprint change makes the old marker ineligible even
        // before the source-edit transaction removes it.
        conn.execute(
            "UPDATE book_sources SET fingerprint='changed-fingerprint' WHERE id='source'",
            [],
        )
        .unwrap();
        assert!(!has_full_scan_baseline(&conn, "source").unwrap());

        // The normal source mutation path eagerly removes the stale marker.
        invalidate_source_proof_on(&conn, "source").unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM remote_scan_baseline WHERE source_id='source'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}
