//! SQLite persistence for remote asset routes, cover variants and jobs.
//!
//! This module deliberately contains only short database operations.  Network
//! requests and image decoding belong to the service/worker layer and must not
//! run while a SQLite transaction is held.

use super::cover_model::{CoverJobKey, CoverJobState, RemoteCoverJob};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS remote_asset_route (
            source_id TEXT NOT NULL,
            asset_id TEXT NOT NULL,
            logical_path TEXT NOT NULL,
            parent_asset_id TEXT,
            provider_id TEXT NOT NULL,
            provider_file_id TEXT,
            source_fingerprint TEXT NOT NULL,
            generation INTEGER NOT NULL,
            session_epoch TEXT NOT NULL DEFAULT '',
            route_revision INTEGER NOT NULL,
            PRIMARY KEY(source_id,asset_id)
         );
         CREATE TABLE IF NOT EXISTS remote_directory_cover (
            source_id TEXT NOT NULL,
            directory_asset_id TEXT NOT NULL,
            representative_asset_id TEXT,
            selection_reason TEXT NOT NULL,
            revision INTEGER NOT NULL,
            completeness TEXT NOT NULL DEFAULT 'pending',
            PRIMARY KEY(source_id,directory_asset_id)
         );
         CREATE TABLE IF NOT EXISTS remote_cover_job (
            job_key TEXT PRIMARY KEY,
            source_id TEXT NOT NULL,
            asset_id TEXT NOT NULL,
            content_revision TEXT NOT NULL,
            selection_revision TEXT NOT NULL,
            profile TEXT NOT NULL,
            state TEXT NOT NULL,
            demand_kind TEXT NOT NULL,
            priority INTEGER NOT NULL DEFAULT 0,
            attempt INTEGER NOT NULL DEFAULT 0,
            next_attempt_at INTEGER,
            lease_owner TEXT,
            lease_until INTEGER,
            generation INTEGER NOT NULL,
            session_epoch TEXT NOT NULL DEFAULT '',
            error_code TEXT,
            updated_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS remote_cover_blob (
            blob_key TEXT PRIMARY KEY,
            relative_path TEXT NOT NULL,
            format TEXT NOT NULL,
            byte_size INTEGER NOT NULL,
            width INTEGER NOT NULL,
            height INTEGER NOT NULL,
            checksum TEXT NOT NULL,
            storage_version INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS remote_cover_variant (
            source_id TEXT NOT NULL,
            asset_id TEXT NOT NULL,
            content_revision TEXT NOT NULL,
            selection_revision TEXT NOT NULL,
            profile TEXT NOT NULL,
            blob_key TEXT,
            state TEXT NOT NULL,
            revision INTEGER NOT NULL,
            is_previous_revision INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY(source_id,asset_id,content_revision,selection_revision,profile)
         );
         CREATE TABLE IF NOT EXISTS remote_cover_ref (
            owner_key TEXT NOT NULL,
            blob_key TEXT NOT NULL,
            role TEXT NOT NULL,
            source_id TEXT NOT NULL,
            asset_id TEXT NOT NULL,
            dependency_revision TEXT NOT NULL,
            PRIMARY KEY(owner_key,blob_key,role)
         );
         CREATE TABLE IF NOT EXISTS remote_view_revision (
            source_id TEXT PRIMARY KEY,
            revision INTEGER NOT NULL,
            listing_generation INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
         );
         -- Preview rows are an additive, non-authoritative projection used
         -- while a generation is still being discovered. They are kept out
         -- of library_index so a cancelled/failed scan can never overwrite
         -- the last complete generation or its deletion proof.
         CREATE TABLE IF NOT EXISTS remote_scan_preview (
            source_id TEXT NOT NULL,
            generation INTEGER NOT NULL,
            asset_id TEXT NOT NULL,
            parent_asset_id TEXT NOT NULL,
            logical_path TEXT NOT NULL,
            name TEXT NOT NULL,
            entry_type TEXT NOT NULL,
            asset_kind TEXT NOT NULL,
            size INTEGER,
            modified_at INTEGER,
            content_fingerprint TEXT,
            provider_file_id TEXT,
            source_fingerprint TEXT NOT NULL,
            session_epoch TEXT NOT NULL,
            PRIMARY KEY(source_id,generation,asset_id)
         );
         CREATE INDEX IF NOT EXISTS idx_remote_cover_job_state
            ON remote_cover_job(state,next_attempt_at,priority);
         CREATE INDEX IF NOT EXISTS idx_remote_cover_route_parent
            ON remote_asset_route(source_id,parent_asset_id);
         CREATE INDEX IF NOT EXISTS idx_remote_cover_ref_asset
            ON remote_cover_ref(source_id,asset_id);
         CREATE INDEX IF NOT EXISTS idx_remote_cover_variant_blob
            ON remote_cover_variant(blob_key);
         CREATE INDEX IF NOT EXISTS idx_remote_scan_preview_parent
            ON remote_scan_preview(source_id,generation,parent_asset_id);
         CREATE INDEX IF NOT EXISTS idx_remote_scan_preview_path
            ON remote_scan_preview(source_id,generation,logical_path);",
    )?;
    // A few development builds created the additive tables before all
    // fields were finalized. `CREATE TABLE IF NOT EXISTS` alone would leave
    // those installs with an unusable partial schema, so make every column
    // upgrade additive and idempotent as well. Primary-key semantics belong
    // to the first table creation; newly added compatibility columns use a
    // safe default and never rewrite existing rows.
    for (table, columns) in [
        (
            "remote_asset_route",
            vec![
                ("source_id", "TEXT NOT NULL DEFAULT ''"),
                ("asset_id", "TEXT NOT NULL DEFAULT ''"),
                ("logical_path", "TEXT NOT NULL DEFAULT '/'"),
                ("parent_asset_id", "TEXT"),
                ("provider_id", "TEXT NOT NULL DEFAULT 'unknown'"),
                ("provider_file_id", "TEXT"),
                ("source_fingerprint", "TEXT NOT NULL DEFAULT ''"),
                ("generation", "INTEGER NOT NULL DEFAULT 0"),
                ("session_epoch", "TEXT NOT NULL DEFAULT ''"),
                ("route_revision", "INTEGER NOT NULL DEFAULT 0"),
            ],
        ),
        (
            "remote_directory_cover",
            vec![
                ("source_id", "TEXT NOT NULL DEFAULT ''"),
                ("directory_asset_id", "TEXT NOT NULL DEFAULT ''"),
                ("representative_asset_id", "TEXT"),
                ("selection_reason", "TEXT NOT NULL DEFAULT 'natural_order'"),
                ("revision", "INTEGER NOT NULL DEFAULT 0"),
                ("completeness", "TEXT NOT NULL DEFAULT 'pending'"),
            ],
        ),
        (
            "remote_cover_job",
            vec![
                ("job_key", "TEXT NOT NULL DEFAULT ''"),
                ("source_id", "TEXT NOT NULL DEFAULT ''"),
                ("asset_id", "TEXT NOT NULL DEFAULT ''"),
                ("content_revision", "TEXT NOT NULL DEFAULT ''"),
                ("selection_revision", "TEXT NOT NULL DEFAULT 'default'"),
                ("profile", "TEXT NOT NULL DEFAULT '340x480@1'"),
                ("state", "TEXT NOT NULL DEFAULT 'pending'"),
                ("demand_kind", "TEXT NOT NULL DEFAULT 'background'"),
                ("priority", "INTEGER NOT NULL DEFAULT 0"),
                ("attempt", "INTEGER NOT NULL DEFAULT 0"),
                ("next_attempt_at", "INTEGER"),
                ("lease_owner", "TEXT"),
                ("lease_until", "INTEGER"),
                ("generation", "INTEGER NOT NULL DEFAULT 0"),
                ("session_epoch", "TEXT NOT NULL DEFAULT ''"),
                ("error_code", "TEXT"),
                ("updated_at", "INTEGER NOT NULL DEFAULT 0"),
            ],
        ),
        (
            "remote_cover_blob",
            vec![
                ("blob_key", "TEXT NOT NULL DEFAULT ''"),
                ("relative_path", "TEXT NOT NULL DEFAULT ''"),
                ("format", "TEXT NOT NULL DEFAULT 'rgba'"),
                ("byte_size", "INTEGER NOT NULL DEFAULT 0"),
                ("width", "INTEGER NOT NULL DEFAULT 0"),
                ("height", "INTEGER NOT NULL DEFAULT 0"),
                ("checksum", "TEXT NOT NULL DEFAULT ''"),
                ("storage_version", "INTEGER NOT NULL DEFAULT 1"),
            ],
        ),
        (
            "remote_cover_variant",
            vec![
                ("source_id", "TEXT NOT NULL DEFAULT ''"),
                ("asset_id", "TEXT NOT NULL DEFAULT ''"),
                ("content_revision", "TEXT NOT NULL DEFAULT ''"),
                ("selection_revision", "TEXT NOT NULL DEFAULT 'default'"),
                ("profile", "TEXT NOT NULL DEFAULT '340x480@1'"),
                ("blob_key", "TEXT"),
                ("state", "TEXT NOT NULL DEFAULT 'pending'"),
                ("revision", "INTEGER NOT NULL DEFAULT 0"),
                ("is_previous_revision", "INTEGER NOT NULL DEFAULT 0"),
                ("updated_at", "INTEGER NOT NULL DEFAULT 0"),
            ],
        ),
        (
            "remote_cover_ref",
            vec![
                ("owner_key", "TEXT NOT NULL DEFAULT ''"),
                ("blob_key", "TEXT NOT NULL DEFAULT ''"),
                ("role", "TEXT NOT NULL DEFAULT 'cover'"),
                ("source_id", "TEXT NOT NULL DEFAULT ''"),
                ("asset_id", "TEXT NOT NULL DEFAULT ''"),
                ("dependency_revision", "TEXT NOT NULL DEFAULT ''"),
            ],
        ),
        (
            "remote_view_revision",
            vec![
                ("source_id", "TEXT NOT NULL DEFAULT ''"),
                ("revision", "INTEGER NOT NULL DEFAULT 0"),
                ("listing_generation", "INTEGER NOT NULL DEFAULT 0"),
                ("updated_at", "INTEGER NOT NULL DEFAULT 0"),
            ],
        ),
        (
            "remote_scan_preview",
            vec![
                ("source_id", "TEXT NOT NULL DEFAULT ''"),
                ("generation", "INTEGER NOT NULL DEFAULT 0"),
                ("asset_id", "TEXT NOT NULL DEFAULT ''"),
                ("parent_asset_id", "TEXT NOT NULL DEFAULT ''"),
                ("logical_path", "TEXT NOT NULL DEFAULT '/'"),
                ("name", "TEXT NOT NULL DEFAULT ''"),
                ("entry_type", "TEXT NOT NULL DEFAULT 'file'"),
                ("asset_kind", "TEXT NOT NULL DEFAULT 'Other'"),
                ("size", "INTEGER"),
                ("modified_at", "INTEGER"),
                ("content_fingerprint", "TEXT"),
                ("provider_file_id", "TEXT"),
                ("source_fingerprint", "TEXT NOT NULL DEFAULT ''"),
                ("session_epoch", "TEXT NOT NULL DEFAULT ''"),
            ],
        ),
    ] {
        for (name, definition) in columns {
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name=?2)",
                params![table, name],
                |row| row.get(0),
            )?;
            if !exists {
                conn.execute(
                    &format!("ALTER TABLE {table} ADD COLUMN {name} {definition}"),
                    [],
                )?;
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn upsert_route_on(
    conn: &Connection,
    source_id: &str,
    asset_id: &str,
    logical_path: &str,
    parent_asset_id: Option<&str>,
    provider_id: &str,
    provider_file_id: Option<&str>,
    source_fingerprint: &str,
    generation: i64,
    session_epoch: &str,
    route_revision: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO remote_asset_route(
             source_id,asset_id,logical_path,parent_asset_id,provider_id,
             provider_file_id,source_fingerprint,generation,session_epoch,route_revision)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
         ON CONFLICT(source_id,asset_id) DO UPDATE SET
             logical_path=excluded.logical_path,
             parent_asset_id=excluded.parent_asset_id,
             provider_id=excluded.provider_id,
             provider_file_id=excluded.provider_file_id,
             source_fingerprint=excluded.source_fingerprint,
             generation=excluded.generation,
             session_epoch=excluded.session_epoch,
             route_revision=excluded.route_revision
         WHERE excluded.generation >= remote_asset_route.generation",
        params![
            source_id,
            asset_id,
            logical_path,
            parent_asset_id,
            provider_id,
            provider_file_id,
            source_fingerprint,
            generation,
            session_epoch,
            route_revision
        ],
    )?;
    Ok(())
}

pub fn bump_view_revision_on(
    conn: &Connection,
    source_id: &str,
    listing_generation: i64,
    updated_at: i64,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO remote_view_revision(source_id,revision,listing_generation,updated_at)
         VALUES(?1,1,?2,?3)
         ON CONFLICT(source_id) DO UPDATE SET
             revision=remote_view_revision.revision+1,
             listing_generation=excluded.listing_generation,
             updated_at=excluded.updated_at",
        params![source_id, listing_generation, updated_at],
    )?;
    conn.query_row(
        "SELECT revision FROM remote_view_revision WHERE source_id=?1",
        [source_id],
        |row| row.get(0),
    )
}

pub fn view_revision(conn: &Connection, source_id: &str) -> rusqlite::Result<i64> {
    Ok(conn
        .query_row(
            "SELECT revision FROM remote_view_revision WHERE source_id=?1",
            [source_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

pub fn mark_job_state_on(
    conn: &Connection,
    key: &CoverJobKey,
    state: CoverJobState,
    error_code: Option<&str>,
    now: i64,
) -> rusqlite::Result<()> {
    if !job_session_is_current(conn, key)? {
        return Ok(());
    }
    let changed = conn.execute(
        "UPDATE remote_cover_job SET state=?1,error_code=?2,next_attempt_at=NULL,
                lease_owner=NULL,lease_until=NULL,updated_at=?3
         WHERE job_key=?4 AND state='running'",
        params![state.as_str(), error_code, now, key.encode()],
    )?;
    if changed == 0 {
        return Ok(());
    }
    publish_variant_on(conn, key, state, now)
}

/// Publish a result only when the same worker still owns the running lease.
/// A verified deletion or a lease takeover therefore rejects a late result.
pub fn mark_job_state_owned_on(
    conn: &Connection,
    key: &CoverJobKey,
    owner: &str,
    state: CoverJobState,
    error_code: Option<&str>,
    now: i64,
) -> rusqlite::Result<bool> {
    if !job_session_is_current(conn, key)? || !owner_session_is_current(conn, key, owner)? {
        return Ok(false);
    }
    let changed = conn.execute(
        "UPDATE remote_cover_job SET state=?1,error_code=?2,next_attempt_at=NULL,
                lease_owner=NULL,lease_until=NULL,updated_at=?3
         WHERE job_key=?4 AND state='running' AND lease_owner=?5",
        params![state.as_str(), error_code, now, key.encode(), owner],
    )?;
    if changed == 0 {
        return Ok(false);
    }
    publish_variant_on(conn, key, state, now)?;
    Ok(true)
}

/// Atomically publish a ready cover together with its blob metadata and
/// reference. The worker calls this after the versioned payload has been
/// written; the ownership predicate still protects against a verified remote
/// deletion or a lease takeover racing with that write.
#[allow(clippy::too_many_arguments)]
pub fn mark_job_ready_owned_on(
    conn: &Connection,
    key: &CoverJobKey,
    owner: &str,
    now: i64,
    width: u32,
    height: u32,
    byte_size: u64,
    checksum: &str,
) -> rusqlite::Result<bool> {
    if width == 0 || height == 0 || checksum.trim().is_empty() {
        return Ok(false);
    }
    if !job_session_is_current(conn, key)? || !owner_session_is_current(conn, key, owner)? {
        return Ok(false);
    }
    let tx = conn.unchecked_transaction()?;
    let changed = tx.execute(
        "UPDATE remote_cover_job SET state='ready',error_code=NULL,
                next_attempt_at=NULL,lease_owner=NULL,lease_until=NULL,updated_at=?1
         WHERE job_key=?2 AND state='running' AND lease_owner=?3",
        params![now, key.encode(), owner],
    )?;
    if changed == 0 {
        tx.rollback()?;
        return Ok(false);
    }
    let blob_key = key.encode();
    let relative_path = crate::cache::remote_cover_cache_relative_path(
        &key.source_id,
        &key.asset_id,
        &key.content_revision,
        &key.selection_revision,
        &key.profile,
    );
    tx.execute(
        "INSERT INTO remote_cover_blob(
             blob_key,relative_path,format,byte_size,width,height,checksum,storage_version)
         VALUES(?1,?2,'rgba',?3,?4,?5,?6,2)
         ON CONFLICT(blob_key) DO UPDATE SET
             relative_path=excluded.relative_path,format=excluded.format,
             byte_size=excluded.byte_size,width=excluded.width,height=excluded.height,
             checksum=excluded.checksum,storage_version=excluded.storage_version",
        params![
            blob_key,
            relative_path,
            byte_size as i64,
            width,
            height,
            checksum
        ],
    )?;
    publish_variant_on(&tx, key, CoverJobState::Ready, now)?;
    tx.execute(
        "INSERT INTO remote_cover_ref(
             owner_key,blob_key,role,source_id,asset_id,dependency_revision)
         VALUES(?1,?2,'variant',?3,?4,?5)
         ON CONFLICT(owner_key,blob_key,role) DO UPDATE SET
             source_id=excluded.source_id,asset_id=excluded.asset_id,
             dependency_revision=excluded.dependency_revision",
        params![
            key.encode(),
            key.encode(),
            key.source_id,
            key.asset_id,
            key.content_revision
        ],
    )?;
    tx.commit()?;
    Ok(true)
}

/// A cover result must belong to the epoch that produced the route.  The
/// helper is tolerant of isolated queue fixtures created before the scan
/// epoch table existed, but production databases always have that table and
/// therefore reject missing/mismatched epochs.
fn job_session_is_current(conn: &Connection, key: &CoverJobKey) -> rusqlite::Result<bool> {
    let has_epoch: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='remote_scan_epoch')",
        [],
        |row| row.get(0),
    )?;
    if !has_epoch {
        return Ok(true);
    }
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM remote_cover_job job
             JOIN remote_scan_epoch epoch
               ON epoch.source_id=job.source_id
              AND epoch.generation=job.generation
              AND epoch.session_epoch=job.session_epoch
              AND epoch.session_epoch <> ''
             WHERE job.job_key=?1)",
        [key.encode()],
        |row| row.get(0),
    )
}

fn owner_session_is_current(
    conn: &Connection,
    key: &CoverJobKey,
    owner: &str,
) -> rusqlite::Result<bool> {
    // Scanner compatibility owners do not carry a runtime session token;
    // they are only accepted by the legacy path when the job epoch itself is
    // current.  Live cover workers use `cover:<source>:<session>` and must
    // match the epoch token to prevent an old session publishing late.
    if !owner.starts_with("cover:") {
        return Ok(true);
    }
    let Some(token) = owner.rsplit(':').next().and_then(|v| v.parse::<i64>().ok()) else {
        return Ok(false);
    };
    let has_epoch: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='remote_scan_epoch')",
        [],
        |row| row.get(0),
    )?;
    if !has_epoch {
        return Ok(true);
    }
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM remote_cover_job job
             JOIN remote_scan_epoch epoch
               ON epoch.source_id=job.source_id
              AND epoch.generation=job.generation
              AND epoch.session_epoch=job.session_epoch
             WHERE job.job_key=?1 AND epoch.session_token=?2)",
        params![key.encode(), token],
        |row| row.get(0),
    )
}

fn publish_variant_on(
    conn: &Connection,
    key: &CoverJobKey,
    state: CoverJobState,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO remote_cover_variant(source_id,asset_id,content_revision,selection_revision,profile,blob_key,state,revision,is_previous_revision,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0,?8)
         ON CONFLICT(source_id,asset_id,content_revision,selection_revision,profile) DO UPDATE SET
            blob_key=excluded.blob_key,state=excluded.state,revision=excluded.revision,
            is_previous_revision=CASE WHEN remote_cover_variant.state='ready' AND excluded.state <> 'ready' THEN 1 ELSE 0 END,
            updated_at=excluded.updated_at",
        params![
            key.source_id,
            key.asset_id,
            key.content_revision,
            key.selection_revision,
            key.profile,
            (state == CoverJobState::Ready).then(|| key.encode()),
            state.as_str(),
            now
        ],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn upsert_job_on(
    conn: &Connection,
    key: &CoverJobKey,
    state: CoverJobState,
    demand_kind: &str,
    priority: i64,
    generation: i64,
    session_epoch: &str,
    now: i64,
) -> Result<RemoteCoverJob> {
    let job_key = key.encode();
    conn.execute(
        "INSERT INTO remote_cover_job(
             job_key,source_id,asset_id,content_revision,selection_revision,profile,
             state,demand_kind,priority,attempt,generation,session_epoch,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,0,?10,?11,?12)
         ON CONFLICT(job_key) DO UPDATE SET
             demand_kind=CASE WHEN remote_cover_job.demand_kind='background' THEN excluded.demand_kind ELSE remote_cover_job.demand_kind END,
             priority=MAX(remote_cover_job.priority,excluded.priority),
             generation=MAX(remote_cover_job.generation,excluded.generation),
             session_epoch=CASE WHEN excluded.session_epoch <> '' THEN excluded.session_epoch ELSE remote_cover_job.session_epoch END,
             state=remote_cover_job.state,
             updated_at=excluded.updated_at",
        params![
            job_key,
            key.source_id,
            key.asset_id,
            key.content_revision,
            key.selection_revision,
            key.profile,
            state.as_str(),
            demand_kind,
            priority,
            generation,
            session_epoch,
            now
        ],
    )?;
    load_job_on(conn, &job_key)?.ok_or_else(|| anyhow::anyhow!("cover job disappeared"))
}

pub fn load_job_on(conn: &Connection, job_key: &str) -> Result<Option<RemoteCoverJob>> {
    conn.query_row(
        "SELECT source_id,asset_id,content_revision,selection_revision,profile,state,
                demand_kind,priority,attempt,next_attempt_at,lease_owner,lease_until,
                generation,session_epoch,error_code,updated_at
         FROM remote_cover_job WHERE job_key=?1",
        [job_key],
        |row| {
            let state: String = row.get(5)?;
            Ok(RemoteCoverJob {
                key: CoverJobKey {
                    source_id: row.get(0)?,
                    asset_id: row.get(1)?,
                    content_revision: row.get(2)?,
                    selection_revision: row.get(3)?,
                    profile: row.get(4)?,
                },
                state: CoverJobState::parse(&state).unwrap_or(CoverJobState::Failed),
                demand_kind: row.get(6)?,
                priority: row.get(7)?,
                attempt: row.get(8)?,
                next_attempt_at: row.get(9)?,
                lease_owner: row.get(10)?,
                lease_until: row.get(11)?,
                generation: row.get(12)?,
                session_epoch: row.get(13)?,
                error_code: row.get(14)?,
                updated_at: row.get(15)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

/// Atomically claim one due job. The transaction only updates the durable
/// lease; callers must drop the SQLite lock before doing provider I/O.
pub fn claim_next_job_on(
    conn: &Connection,
    owner: &str,
    now: i64,
    lease_ms: i64,
) -> Result<Option<RemoteCoverJob>> {
    let owner = owner.trim();
    if owner.is_empty() || lease_ms <= 0 {
        return Ok(None);
    }
    let tx = conn.unchecked_transaction()?;
    let candidate: Option<String> = tx
        .query_row(
            "SELECT job_key FROM remote_cover_job
             WHERE (state='pending' OR (state='retry_wait' AND
                    (next_attempt_at IS NULL OR next_attempt_at<=?1)))
             ORDER BY priority DESC,updated_at,job_key LIMIT 1",
            params![now],
            |row| row.get(0),
        )
        .optional()?;
    let Some(job_key) = candidate else {
        tx.commit()?;
        return Ok(None);
    };
    let changed = tx.execute(
        "UPDATE remote_cover_job SET state='running',lease_owner=?1,
             lease_until=?2,attempt=attempt+1,next_attempt_at=NULL,
             updated_at=?3
         WHERE job_key=?4 AND
               (state='pending' OR (state='retry_wait' AND
                 (next_attempt_at IS NULL OR next_attempt_at<=?3)))",
        params![owner, now.saturating_add(lease_ms), now, job_key],
    )?;
    let job = if changed == 1 {
        load_job_on(&tx, &job_key)?
    } else {
        None
    };
    tx.commit()?;
    Ok(job)
}

pub fn claim_next_job_for_source_on(
    conn: &Connection,
    source_id: &str,
    owner: &str,
    now: i64,
    lease_ms: i64,
) -> Result<Option<RemoteCoverJob>> {
    let owner = owner.trim();
    if source_id.trim().is_empty() || owner.is_empty() || lease_ms <= 0 {
        return Ok(None);
    }
    let tx = conn.unchecked_transaction()?;
    let candidate: Option<String> = tx
        .query_row(
            "SELECT job_key FROM remote_cover_job
             WHERE source_id=?1 AND (state='pending' OR (state='retry_wait' AND
                    (next_attempt_at IS NULL OR next_attempt_at<=?2)))
             ORDER BY priority DESC,updated_at,job_key LIMIT 1",
            params![source_id, now],
            |row| row.get(0),
        )
        .optional()?;
    let Some(job_key) = candidate else {
        tx.commit()?;
        return Ok(None);
    };
    let changed = tx.execute(
        "UPDATE remote_cover_job SET state='running',lease_owner=?1,
             lease_until=?2,attempt=attempt+1,next_attempt_at=NULL,
             updated_at=?3 WHERE job_key=?4 AND source_id=?5 AND
             (state='pending' OR (state='retry_wait' AND
               (next_attempt_at IS NULL OR next_attempt_at<=?3)))",
        params![owner, now.saturating_add(lease_ms), now, job_key, source_id],
    )?;
    let job = if changed == 1 {
        load_job_on(&tx, &job_key)?
    } else {
        None
    };
    tx.commit()?;
    Ok(job)
}

/// Claim a job only when its persisted scan epoch belongs to the runtime
/// session that is about to perform provider I/O.  This prevents a worker
/// left behind by an expired login from consuming the new session's queue and
/// then losing its result at publish time.  The unfiltered function above is
/// retained for old queue fixtures and non-session maintenance callers.
pub fn claim_next_job_for_source_session_on(
    conn: &Connection,
    source_id: &str,
    owner: &str,
    now: i64,
    lease_ms: i64,
    session: u64,
) -> Result<Option<RemoteCoverJob>> {
    let session = i64::try_from(session).map_err(|_| anyhow::anyhow!("session token overflow"))?;
    let owner = owner.trim();
    if source_id.trim().is_empty() || owner.is_empty() || lease_ms <= 0 {
        return Ok(None);
    }
    let tx = conn.unchecked_transaction()?;
    let candidate: Option<String> = tx
        .query_row(
            "SELECT job.job_key FROM remote_cover_job job
             JOIN remote_scan_epoch epoch
               ON epoch.source_id=job.source_id
              AND epoch.generation=job.generation
              AND epoch.session_epoch=job.session_epoch
              AND epoch.session_token=?3
             WHERE job.source_id=?1 AND
               (job.state='pending' OR (job.state='retry_wait' AND
                 (job.next_attempt_at IS NULL OR job.next_attempt_at<=?2)))
             ORDER BY job.priority DESC,job.updated_at,job.job_key LIMIT 1",
            params![source_id, now, session],
            |row| row.get(0),
        )
        .optional()?;
    let Some(job_key) = candidate else {
        tx.commit()?;
        return Ok(None);
    };
    let changed = tx.execute(
        "UPDATE remote_cover_job SET state='running',lease_owner=?1,
             lease_until=?2,attempt=attempt+1,next_attempt_at=NULL,
             updated_at=?3
         WHERE job_key=?4 AND source_id=?5 AND
           EXISTS(SELECT 1 FROM remote_scan_epoch epoch
                  WHERE epoch.source_id=remote_cover_job.source_id
                    AND epoch.generation=remote_cover_job.generation
                    AND epoch.session_epoch=remote_cover_job.session_epoch
                    AND epoch.session_token=?6) AND
           (state='pending' OR (state='retry_wait' AND
             (next_attempt_at IS NULL OR next_attempt_at<=?3)))",
        params![
            owner,
            now.saturating_add(lease_ms),
            now,
            job_key,
            source_id,
            session
        ],
    )?;
    let job = if changed == 1 {
        load_job_on(&tx, &job_key)?
    } else {
        None
    };
    tx.commit()?;
    Ok(job)
}

/// Lease a known job key for a caller that already selected the matching
/// legacy stage row. This is used by the compatibility scanner so a demand
/// worker cannot fetch the same asset concurrently.
pub fn lease_job_on(
    conn: &Connection,
    key: &CoverJobKey,
    owner: &str,
    now: i64,
    lease_ms: i64,
) -> rusqlite::Result<bool> {
    if owner.trim().is_empty() || lease_ms <= 0 {
        return Ok(false);
    }
    let changed = conn.execute(
        "UPDATE remote_cover_job SET state='running',lease_owner=?1,
             lease_until=?2,attempt=attempt+1,next_attempt_at=NULL,updated_at=?3
         WHERE job_key=?4 AND state IN ('pending','retry_wait')",
        params![owner, now.saturating_add(lease_ms), now, key.encode()],
    )?;
    Ok(changed == 1)
}

/// Requeue workers whose process died or whose lease expired. Keeping the
/// attempt count makes the automatic retry budget durable across restarts.
pub fn recover_expired_leases_on(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE remote_cover_job SET state='pending',lease_owner=NULL,
             lease_until=NULL,next_attempt_at=NULL,updated_at=?1
         WHERE state='running' AND (lease_until IS NULL OR lease_until<=?1)",
        params![now],
    )
}

/// Return a claimed job to the durable queue without consuming a retry. This
/// is used when a provider/account budget says the request is not runnable
/// yet; the worker can sleep outside SQLite and claim it again later.
pub fn release_job_lease_on(
    conn: &Connection,
    key: &CoverJobKey,
    owner: &str,
    now: i64,
) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE remote_cover_job SET state='pending',lease_owner=NULL,
             lease_until=NULL,next_attempt_at=NULL,updated_at=?1
         WHERE job_key=?2 AND state='running' AND lease_owner=?3",
        params![now, key.encode(), owner],
    )?;
    Ok(changed == 1)
}

pub fn mark_retry_wait_on(
    conn: &Connection,
    key: &CoverJobKey,
    now: i64,
    retry_after_ms: Option<u64>,
    error_code: Option<&str>,
) -> rusqlite::Result<()> {
    let delay = retry_after_ms.unwrap_or(1_000).min(15 * 60 * 1_000);
    conn.execute(
        "UPDATE remote_cover_job SET state='retry_wait',error_code=?1,
             next_attempt_at=?2,lease_owner=NULL,lease_until=NULL,updated_at=?3
         WHERE job_key=?4",
        params![
            error_code,
            now.saturating_add(i64::try_from(delay).unwrap_or(i64::MAX)),
            now,
            key.encode()
        ],
    )?;
    Ok(())
}

pub fn mark_retry_wait_owned_on(
    conn: &Connection,
    key: &CoverJobKey,
    owner: &str,
    now: i64,
    retry_after_ms: Option<u64>,
    error_code: Option<&str>,
) -> rusqlite::Result<bool> {
    if !job_session_is_current(conn, key)? || !owner_session_is_current(conn, key, owner)? {
        return Ok(false);
    }
    let delay = retry_after_ms.unwrap_or(1_000).min(15 * 60 * 1_000);
    let changed = conn.execute(
        "UPDATE remote_cover_job SET state='retry_wait',error_code=?1,
             next_attempt_at=?2,lease_owner=NULL,lease_until=NULL,updated_at=?3
         WHERE job_key=?4 AND state='running' AND lease_owner=?5",
        params![
            error_code,
            now.saturating_add(i64::try_from(delay).unwrap_or(i64::MAX)),
            now,
            key.encode(),
            owner
        ],
    )?;
    Ok(changed == 1)
}
