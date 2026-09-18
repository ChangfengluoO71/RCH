//! SQLite persistence for remote asset routes, cover variants and jobs.
//!
//! This module deliberately contains only short database operations.  Network
//! requests and image decoding belong to the service/worker layer and must not
//! run while a SQLite transaction is held.

use super::cover_model::{CoverJobKey, CoverJobState, RemoteCoverJob};
use super::cover_state::{resolve_upsert_state, CoverJobUpsertCause};
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
            // P1-C 长期补偿（long compensation）：
            //   long_retry_not_before : NULL=无长期自动补偿资格；非空=retryable failure，值即 earliest eligibility。
            //   long_retry_consumed   : 当前 failure episode 的一次长期补偿是否已被真正 claim。
            //   long_retry_pending    : 该 pending 由长期补偿 reconciliation 产生、尚未 claim。
            "remote_cover_job",
            vec![
                ("long_retry_not_before", "INTEGER"),
                ("long_retry_consumed", "INTEGER NOT NULL DEFAULT 0"),
                ("long_retry_pending", "INTEGER NOT NULL DEFAULT 0"),
            ],
        ),
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

/// P1-E：在一次**真实**的 durable cover-state transition 之后推进 durable revision。
///
/// * 必须在与 state mutation **相同**的事务内调用（调用者持有 conn 或 tx）。
/// *  取该 job **自身**的 generation —— 绝不拿“当前最新 generation”猜，
///   否则旧 generation 的 worker 完成会污染新 generation 的 revision。
fn bump_cover_revision_for_job_on(
    conn: &Connection,
    key: &CoverJobKey,
    now: i64,
) -> rusqlite::Result<()> {
    let row = conn
        .query_row(
            "SELECT source_id,generation FROM remote_cover_job WHERE job_key=?1",
            [key.encode()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    if let Some((source_id, generation)) = row {
        bump_view_revision_on(conn, &source_id, generation, now)?;
    }
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
    // P1-E：state mutation 与 revision bump 必须**原子提交**（crash 不得漏 bump）。
    // 调用者已持有事务时复用当前 conn；否则本函数自持一个事务
    //（沿用 P1-A 的 is_autocommit 模式，避免 nested transaction 错误）。
    let owned_tx = if conn.is_autocommit() {
        Some(conn.unchecked_transaction()?)
    } else {
        None
    };
    let scope: &Connection = match owned_tx.as_ref() {
        Some(tx) => tx,
        None => conn,
    };
    let changed = scope.execute(
        "UPDATE remote_cover_job SET state=?1,error_code=?2,next_attempt_at=NULL,
                lease_owner=NULL,lease_until=NULL,updated_at=?3,
                long_retry_not_before=CASE WHEN ?1='ready' THEN NULL ELSE long_retry_not_before END,
                long_retry_consumed=CASE WHEN ?1='ready' THEN 0 ELSE long_retry_consumed END,
                long_retry_pending=CASE WHEN ?1='ready' THEN 0 ELSE long_retry_pending END
         WHERE job_key=?4 AND state='running'",
        params![state.as_str(), error_code, now, key.encode()],
    )?;
    if changed == 0 {
        if let Some(tx) = owned_tx {
            tx.rollback()?;
        }
        return Ok(());
    }
    publish_variant_on(scope, key, state, now)?;
    // P1-E：真实的 durable cover-state transition ⇒ 同一事务内推进 revision。
    bump_cover_revision_for_job_on(scope, key, now)?;
    if let Some(tx) = owned_tx {
        tx.commit()?;
        // P1-E：commit-after-emit（仅在自己拥有事务时才能确定已提交）。
        crate::remote_scan::cover_revision_stream::notify_cover_revision(
            &key.source_id,
            Some(&key.asset_id),
        );
    }
    Ok(())
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
    // P1-E：state mutation 与 revision bump 必须**原子提交**（crash 不得漏 bump）。
    // 调用者已持有事务时复用当前 conn；否则本函数自持一个事务
    //（沿用 P1-A 的 is_autocommit 模式，避免 nested transaction 错误）。
    let owned_tx = if conn.is_autocommit() {
        Some(conn.unchecked_transaction()?)
    } else {
        None
    };
    let scope: &Connection = match owned_tx.as_ref() {
        Some(tx) => tx,
        None => conn,
    };
    let changed = scope.execute(
        "UPDATE remote_cover_job SET state=?1,error_code=?2,next_attempt_at=NULL,
                lease_owner=NULL,lease_until=NULL,updated_at=?3,
                long_retry_not_before=CASE WHEN ?1='ready' THEN NULL ELSE long_retry_not_before END,
                long_retry_consumed=CASE WHEN ?1='ready' THEN 0 ELSE long_retry_consumed END,
                long_retry_pending=CASE WHEN ?1='ready' THEN 0 ELSE long_retry_pending END
         WHERE job_key=?4 AND state='running' AND lease_owner=?5",
        params![state.as_str(), error_code, now, key.encode(), owner],
    )?;
    if changed == 0 {
        if let Some(tx) = owned_tx {
            tx.rollback()?;
        }
        return Ok(false);
    }
    publish_variant_on(scope, key, state, now)?;
    // P1-E：真实的 durable cover-state transition ⇒ 同一事务内推进 revision。
    bump_cover_revision_for_job_on(scope, key, now)?;
    if let Some(tx) = owned_tx {
        tx.commit()?;
        // P1-E：commit-after-emit（仅在自己拥有事务时才能确定已提交）。
        crate::remote_scan::cover_revision_stream::notify_cover_revision(
            &key.source_id,
            Some(&key.asset_id),
        );
    }
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
        // 成功进入 ready 即结束当前 failure episode：清空长期补偿三列，使将来真正新形成的
        // failure episode 拥有自己独立的一次补偿额度。
        "UPDATE remote_cover_job SET state='ready',error_code=NULL,
                next_attempt_at=NULL,lease_owner=NULL,lease_until=NULL,updated_at=?1,
                long_retry_not_before=NULL,long_retry_consumed=0,long_retry_pending=0
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
    // P1-E/REV-3：running → ready 是真实 durable transition ⇒ 同一事务内推进 revision。
    bump_cover_revision_for_job_on(&tx, key, now)?;
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
    // P1-E：commit-after-emit —— running → ready 是 E-SCAN-TERMINAL-READY 的核心
    // 生产事件，必须在提交成功之后唤醒消费者。
    crate::remote_scan::cover_revision_stream::notify_cover_revision(
        &key.source_id,
        Some(&key.asset_id),
    );
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
    cause: CoverJobUpsertCause,
) -> Result<RemoteCoverJob> {
    let job_key = key.encode();
    // P1-A：状态由 `resolve_upsert_state` 的**唯一规则表**决定，不再无条件保留旧状态。
    // 改造前这里是 `state=remote_cover_job.state`，于是 failed / unsupported /
    // blocked / retry_wait 一旦进入就再没有任何路径能恢复，即使原因早已消失。
    //
    // 只在调用方**没有**事务时才自己开一个：本函数也会被 `publish_staged_generation`
    // 这类已持有事务的调用方复用，嵌套 BEGIN 会被 SQLite 拒绝。
    let owned_tx = if conn.is_autocommit() {
        Some(conn.unchecked_transaction()?)
    } else {
        None
    };
    let scope: &Connection = match owned_tx.as_ref() {
        Some(tx) => tx,
        None => conn,
    };
    let existing = load_job_on(scope, &job_key)?.map(|job| job.state);
    let resolved = resolve_upsert_state(existing, state, cause);
    // 被显式拉回候选队列的任务获得全新的短重试预算（attempt / 退避 / lease）。
    // 注意：长期补偿的额度**不在这里重置**（见 mark_job_failure_owned_on 的 episode 语义）。
    let revival = resolved == CoverJobState::Pending
        && existing.is_some_and(|previous| previous != CoverJobState::Pending);
    scope.execute(
        "INSERT INTO remote_cover_job(
             job_key,source_id,asset_id,content_revision,selection_revision,profile,
             state,demand_kind,priority,attempt,generation,session_epoch,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,0,?10,?11,?12)
         ON CONFLICT(job_key) DO UPDATE SET
             demand_kind=CASE WHEN remote_cover_job.demand_kind='background' THEN excluded.demand_kind ELSE remote_cover_job.demand_kind END,
             priority=MAX(remote_cover_job.priority,excluded.priority),
             generation=MAX(remote_cover_job.generation,excluded.generation),
             session_epoch=CASE WHEN excluded.session_epoch <> '' THEN excluded.session_epoch ELSE remote_cover_job.session_epoch END,
             state=excluded.state,
             attempt=CASE WHEN ?13=1 THEN 0 ELSE remote_cover_job.attempt END,
             next_attempt_at=CASE WHEN ?13=1 THEN NULL ELSE remote_cover_job.next_attempt_at END,
             lease_owner=CASE WHEN ?13=1 THEN NULL ELSE remote_cover_job.lease_owner END,
             lease_until=CASE WHEN ?13=1 THEN NULL ELSE remote_cover_job.lease_until END,
             error_code=CASE WHEN ?13=1 THEN NULL ELSE remote_cover_job.error_code END,
             updated_at=excluded.updated_at",
        params![
            job_key,
            key.source_id,
            key.asset_id,
            key.content_revision,
            key.selection_revision,
            key.profile,
            resolved.as_str(),
            demand_kind,
            priority,
            generation,
            session_epoch,
            now,
            i64::from(revival)
        ],
    )?;
    let job =
        load_job_on(scope, &job_key)?.ok_or_else(|| anyhow::anyhow!("cover job disappeared"))?;
    // P1-E/REV-1 + U-α：revision 的**粒度所有权**属于"拥有该事务的那一层"。
    //  • 自己拥有事务（autocommit ⇒ owned_tx = Some）：一个逻辑 transition = 一次 bump。
    //  • 借用外层事务（owned_tx = None）：**只 mutate，不 bump 也不 emit** —— 否则
    //    一个批次创建 N 个 job 会把 revision 变成 changed-row counter，违反
    //    "remote_view_revision is a monotonic source-generation token for an atomic
    //     durable view change"。
    if let Some(tx) = owned_tx.as_ref() {
        bump_view_revision_on(tx, &key.source_id, generation, now)?;
    }
    if let Some(tx) = owned_tx {
        tx.commit()?;
        // P1-E：commit-after-emit —— 仅在自己拥有事务时才能确定已提交；
        // 调用者持有事务时不在此提前 emit（由其外层 transaction owner 负责）。
        crate::remote_scan::cover_revision_stream::notify_cover_revision(
            &key.source_id,
            Some(&key.asset_id),
        );
    }
    Ok(job)
}

/// 解析"真正能 claim 该 source 持久工作"的运行时 session token（P1-B）。
///
/// 只返回**已经存在**的 token：`remote_scan_epoch.session_token` 与待处理 job 的
/// `(generation, session_epoch)` 三元组匹配且非 0。没有任何可用 session 时返回
/// `None` —— 调用方据此**不伪造 session、不远程访问**，pending 保持持久化。
pub fn resolve_cover_wake_session(
    conn: &Connection,
    source_id: &str,
    now: i64,
) -> Result<Option<u64>> {
    let source_id = source_id.trim();
    if source_id.is_empty() {
        return Ok(None);
    }
    let token: Option<i64> = conn
        .query_row(
            "SELECT MAX(epoch.session_token)
               FROM remote_cover_job job
               JOIN remote_scan_epoch epoch
                 ON epoch.source_id=job.source_id
                AND epoch.generation=job.generation
                AND epoch.session_epoch=job.session_epoch
              WHERE job.source_id=?1 AND epoch.session_token<>0
                AND (job.state='pending' OR (job.state='retry_wait' AND
                     (job.next_attempt_at IS NULL OR job.next_attempt_at<=?2)))",
            params![source_id, now],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()?
        .flatten();
    Ok(token
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value != 0))
}

/// 默认封面选择 / 画质档位：必须与扫描与卡片请求路径逐字一致，否则 job key 不同、去重失效。
pub const DEFAULT_SELECTION_REVISION: &str = "default";
pub const DEFAULT_COVER_PROFILE: &str = "340x480@1";

/// 只有这些 blocker 属于"auth/session 类"——新的有效 session 可以解除它们。
///
/// 这是对**代码自身写入的明确 blocker 码**做类型判断（`error_code()` 中
/// `Unauthorized → "authExpired"`），不是从字符串反推 retryability。
/// 其余 blocker（`forbidden` 权限、网络关闭、provider cooldown）一律不因 session-ready 解除。
pub const SESSION_BLOCKER_CODES: &[&str] = &["authExpired"];

/// 长期补偿的最短等待：`failed` 后满 6 小时才获得自动补偿**资格**。
///
/// 这是 **earliest eligibility**，不是 exact timer deadline：补偿由 source-session
/// lifecycle 事件驱动，而不是 wall-clock 驱动。本仓库不存在 timer / scheduler。
pub const LONG_RETRY_DELAY_MS: i64 = 6 * 60 * 60 * 1_000;

/// 一次 source-scoped reconciliation 的预算（必须 bounded）。
#[derive(Debug, Clone, Copy)]
pub struct ReconcileBudget {
    /// 本次最多推进/创建的 job 数。
    pub max_jobs: usize,
    /// 本次最多占用的墙上时间（毫秒）。reconciler 只碰 durable state，**不做 provider 请求**。
    pub max_wall_time_ms: i64,
}

impl Default for ReconcileBudget {
    fn default() -> Self {
        Self {
            max_jobs: 64,
            max_wall_time_ms: 250,
        }
    }
}

/// 一次 reconciliation 的结果（供日志与测试断言；不含任何 provider 细节）。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileReport {
    /// 被推进为可 claim 的长期补偿 job 数。
    pub compensation_promoted: usize,
    /// 因阻塞原因解除（auth/session 恢复）而回到候选队列的 job 数。
    pub blocker_cleared: usize,
    /// 因 library 缺口而新建的 pending job 数。
    pub jobs_created: usize,
    /// 本次是否产生了可 claim 的工作（调用方据此决定是否 wake worker）。
    pub claimable_work: bool,
    /// 是否因预算上限提前结束（剩余工作留给后续事件）。
    pub truncated: bool,
}

/// 记录一次终态失败，并在需要时开启一个**新的 failure episode**。
///
/// `long_retry_not_before`：retryable failure 传 `Some(now + 6h)`；永久失败传 `None`。
/// **同一 episode 内不会刷新** eligibility，也不会复位 `long_retry_consumed` ——
/// 这就从结构上杜绝了"每 6 小时无限自动重试"。episode 只在成功进入 `ready` 时结束。
pub fn mark_job_failure_owned_on(
    conn: &Connection,
    key: &CoverJobKey,
    owner: &str,
    state: CoverJobState,
    error_code: Option<&str>,
    now: i64,
    long_retry_not_before: Option<i64>,
) -> rusqlite::Result<bool> {
    // P1-E：state mutation 与 revision bump 必须**原子提交**（crash 不得漏 bump）。
    // 调用者已持有事务时复用当前 conn；否则本函数自持一个事务
    //（沿用 P1-A 的 is_autocommit 模式，避免 nested transaction 错误）。
    let owned_tx = if conn.is_autocommit() {
        Some(conn.unchecked_transaction()?)
    } else {
        None
    };
    let scope: &Connection = match owned_tx.as_ref() {
        Some(tx) => tx,
        None => conn,
    };
    let changed = scope.execute(
        "UPDATE remote_cover_job SET
             state=?1,error_code=?2,lease_owner=NULL,lease_until=NULL,updated_at=?3,
             long_retry_not_before=CASE
                 WHEN long_retry_not_before IS NULL THEN ?4
                 ELSE long_retry_not_before END,
             long_retry_consumed=CASE
                 WHEN long_retry_not_before IS NULL THEN 0
                 ELSE long_retry_consumed END,
             long_retry_pending=0
         WHERE job_key=?5 AND state='running' AND lease_owner=?6",
        params![
            state.as_str(),
            error_code,
            now,
            long_retry_not_before,
            key.encode(),
            owner
        ],
    )?;
    if changed == 0 {
        if let Some(tx) = owned_tx {
            tx.rollback()?;
        }
        return Ok(false);
    }
    publish_variant_on(scope, key, state, now)?;
    // P1-E：真实的 durable cover-state transition ⇒ 同一事务内推进 revision。
    bump_cover_revision_for_job_on(scope, key, now)?;
    if let Some(tx) = owned_tx {
        tx.commit()?;
        // P1-E：commit-after-emit（仅在自己拥有事务时才能确定已提交）。
        crate::remote_scan::cover_revision_stream::notify_cover_revision(
            &key.source_id,
            Some(&key.asset_id),
        );
    }
    Ok(true)
}

/// 该 source / 该 session 当前是否确实有可 claim 的工作（与 claim 谓词逐字一致）。
fn has_claimable_work_on(
    conn: &Connection,
    source_id: &str,
    session_token: i64,
    now: i64,
) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM remote_cover_job job
             JOIN remote_scan_epoch epoch
               ON epoch.source_id=job.source_id
              AND epoch.generation=job.generation
              AND epoch.session_epoch=job.session_epoch
              AND epoch.session_token=?2
             WHERE job.source_id=?1
               AND (job.state='pending' OR (job.state='retry_wait'
                    AND (job.next_attempt_at IS NULL OR job.next_attempt_at<=?3))))",
        params![source_id, session_token, now],
        |row| row.get(0),
    )
}

/// source-scoped 长期补偿 reconciliation。
///
/// 只做两件事，全部针对**单个 source**、全部只碰 durable state：
/// 1. 把已过期、本 episode 未消耗、且该 session 真能 claim 的 `failed` job 推进为
///    `pending` + `long_retry_pending=1`（**不消耗额度**，额度在 claim 时才消耗）；
/// 2. 把 blocker 属于 [`SESSION_BLOCKER_CODES`] 的 `blocked` job 解回候选队列。
///
/// 不触碰 `unsupported`（只有真实 capability change 才能重估），也不触碰非 session 类 blocker。
pub fn reconcile_cover_compensation_for_source_on(
    conn: &Connection,
    source_id: &str,
    session: u64,
    now: i64,
    budget: ReconcileBudget,
) -> Result<ReconcileReport> {
    let source_id = source_id.trim();
    let mut report = ReconcileReport::default();
    if source_id.is_empty() || budget.max_jobs == 0 {
        return Ok(report);
    }
    let Ok(session_token) = i64::try_from(session) else {
        return Ok(report);
    };
    if session_token == 0 {
        return Ok(report);
    }
    let started = std::time::Instant::now();
    let tx = conn.unchecked_transaction()?;
    let mut spent = 0usize;

    // (1) 长期补偿：episode 已过期且未被消耗；且该 session 真的能 claim（否则只会把 job
    //     变成没人能领的 pending，并错误占用 crash-safety 标记）。
    let remaining = budget.max_jobs - spent;
    let mut candidates: Vec<String> = tx
        .prepare(
            "SELECT job.job_key FROM remote_cover_job job
              WHERE job.source_id=?1 AND job.state='failed'
                AND job.long_retry_not_before IS NOT NULL
                AND job.long_retry_not_before<=?2
                AND job.long_retry_consumed=0
                AND job.long_retry_pending=0
                AND EXISTS (SELECT 1 FROM remote_scan_epoch epoch
                             WHERE epoch.source_id=job.source_id
                               AND epoch.generation=job.generation
                               AND epoch.session_token=?3)
              ORDER BY job.long_retry_not_before,job.job_key
              LIMIT ?4",
        )?
        .query_map(
            params![source_id, now, session_token, (remaining + 1) as i64],
            |row| row.get(0),
        )?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    if candidates.len() > remaining {
        report.truncated = true;
        candidates.truncate(remaining);
    }
    for job_key in candidates {
        let changed = tx.execute(
            "UPDATE remote_cover_job
                SET state='pending',long_retry_pending=1,updated_at=?1,
                    session_epoch=(SELECT epoch.session_epoch FROM remote_scan_epoch epoch
                                    WHERE epoch.source_id=remote_cover_job.source_id
                                      AND epoch.generation=remote_cover_job.generation
                                      AND epoch.session_token=?3)
              WHERE job_key=?2 AND state='failed'
                AND long_retry_consumed=0 AND long_retry_pending=0",
            params![now, job_key, session_token],
        )?;
        if changed == 1 {
            report.compensation_promoted += 1;
            spent += 1;
        }
        if started.elapsed().as_millis() as i64 > budget.max_wall_time_ms {
            report.truncated = true;
            break;
        }
    }

    // (2) 阻塞解除：仅限 auth/session 类 blocker。码值来自本模块常量（非外部输入），
    //     因此直接内联进 IN 列表，避免动态绑定参数。
    if spent < budget.max_jobs && started.elapsed().as_millis() as i64 <= budget.max_wall_time_ms {
        let codes = SESSION_BLOCKER_CODES
            .iter()
            .map(|code| format!("'{code}'"))
            .collect::<Vec<_>>()
            .join(",");
        let remaining = budget.max_jobs - spent;
        let sql = format!(
            "SELECT job.job_key FROM remote_cover_job job
              WHERE job.source_id=?1 AND job.state='blocked'
                AND job.error_code IN ({codes})
                AND EXISTS (SELECT 1 FROM remote_scan_epoch epoch
                             WHERE epoch.source_id=job.source_id
                               AND epoch.generation=job.generation
                               AND epoch.session_token=?2)
              ORDER BY job.updated_at,job.job_key
              LIMIT ?3"
        );
        let mut blocked: Vec<String> = tx
            .prepare(&sql)?
            .query_map(
                params![source_id, session_token, (remaining + 1) as i64],
                |row| row.get(0),
            )?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        if blocked.len() > remaining {
            report.truncated = true;
            blocked.truncate(remaining);
        }
        for job_key in blocked {
            let changed = tx.execute(
                "UPDATE remote_cover_job SET state='pending',updated_at=?1,
                        session_epoch=(SELECT epoch.session_epoch FROM remote_scan_epoch epoch
                                        WHERE epoch.source_id=remote_cover_job.source_id
                                          AND epoch.generation=remote_cover_job.generation
                                          AND epoch.session_token=?3)
                  WHERE job_key=?2 AND state='blocked'",
                params![now, job_key, session_token],
            )?;
            if changed == 1 {
                report.blocker_cleared += 1;
            }
        }
    }

    report.claimable_work = has_claimable_work_on(&tx, source_id, session_token, now)?;
    // P1-E/REV-8：只有本批次真正改变了 durable cover truth 才推进 revision。
    // 粒度按 **source/generation 的成功事务**：一次 batch 最多 +1（不要求 +N，
    // 避免 batch size 与 revision 数值语义绑定；stream 只发一个 source wake）。
    // generation 取本批次所针对的 epoch generation（与上面 EXISTS 谓词同源），
    // 不拿“当前最新 generation”猜。
    if report.compensation_promoted > 0 || report.blocker_cleared > 0 {
        let generation: Option<i64> = tx
            .query_row(
                "SELECT generation FROM remote_scan_epoch WHERE source_id=?1 AND session_token=?2",
                params![source_id, session_token],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(generation) = generation {
            bump_view_revision_on(&tx, source_id, generation, now)?;
        }
    }
    tx.commit()?;

    // P1-E：一次 compensation batch 只发**一次** source-level wake（asset = None）——
    // durable revision 是 source-generation change token（不是 row counter）。
    // mounted cards 收到后按自己的 assetId reread durable state。
    crate::remote_scan::cover_revision_stream::notify_cover_revision(source_id, None);
    Ok(report)
}

/// source-scoped 缺口补齐：只用 durable `library_index` 信息，**不遍历远端目录树**。
///
/// 只补"完全没有对应 cover job 行"的资产。已有 `pending`/`running`/`retry_wait`/`ready`
/// 一律不动；`failed`/`blocked`/`unsupported` 等终态也不在这里复活 —— 它们只由各自的
/// 显式原因（见 P1-A 的统一矩阵）解除。`ready` 但字节缺失的情形走 P1-B 已验证的对账路径。
pub fn reconcile_missing_covers_for_source_on(
    conn: &Connection,
    source_id: &str,
    session: u64,
    now: i64,
    budget: ReconcileBudget,
) -> Result<ReconcileReport> {
    // P1-E/U-α：整批补齐必须发生**同一个**事务里，这样 inner upsert 不自行 bump，
    // revision 由本函数按批次粒度推进一次（N=1 与 N=100 都是 +1）。
    // 沿用 P1-A 的 is_autocommit 模式：调用者已持有事务时直接复用。
    let owned_tx = if conn.is_autocommit() {
        Some(conn.unchecked_transaction()?)
    } else {
        None
    };
    let scope: &Connection = match owned_tx.as_ref() {
        Some(tx) => tx,
        None => conn,
    };
    let source_id = source_id.trim();
    let mut report = ReconcileReport::default();
    if source_id.is_empty() || budget.max_jobs == 0 {
        return Ok(report);
    }
    let Ok(session_token) = i64::try_from(session) else {
        return Ok(report);
    };
    if session_token == 0 {
        return Ok(report);
    }
    // 必须知道该 session 当前绑定的 (generation, session_epoch)：否则新建的 job 无法被
    // 这个 session claim（claim 要求三元组匹配）。
    let binding: Option<(i64, String)> = conn
        .query_row(
            "SELECT generation,session_epoch FROM remote_scan_epoch
              WHERE source_id=?1 AND session_token=?2
              ORDER BY generation DESC LIMIT 1",
            params![source_id, session_token],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((generation, session_epoch)) = binding else {
        return Ok(report);
    };
    let started = std::time::Instant::now();
    let mut gaps: Vec<(String, Option<String>)> = conn
        .prepare(
            "SELECT li.id,li.content_fingerprint FROM library_index li
              WHERE li.source_id=?1 AND li.deleted=0
                AND ((li.entry_type='file' AND li.asset_kind='ArchiveFile')
                  OR (li.entry_type='dir' AND li.asset_kind='ImageFolder'))
                AND NOT EXISTS (SELECT 1 FROM remote_cover_job job
                                 WHERE job.source_id=li.source_id AND job.asset_id=li.id
                                   AND job.selection_revision=?2 AND job.profile=?3)
              ORDER BY li.path
              LIMIT ?4",
        )?
        .query_map(
            params![
                source_id,
                DEFAULT_SELECTION_REVISION,
                DEFAULT_COVER_PROFILE,
                (budget.max_jobs + 1) as i64
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
        .collect::<rusqlite::Result<Vec<(String, Option<String>)>>>()?;
    if gaps.len() > budget.max_jobs {
        report.truncated = true;
        gaps.truncate(budget.max_jobs);
    }
    for (asset_id, content_fingerprint) in gaps {
        if started.elapsed().as_millis() as i64 > budget.max_wall_time_ms {
            report.truncated = true;
            break;
        }
        // content_revision 必须与卡片请求路径逐字一致（library_index → preview → epoch），
        // 否则会为该资产生成第二个 job key，去重失效。
        let content_revision = content_fingerprint
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
            source_id: source_id.to_string(),
            asset_id,
            content_revision,
            selection_revision: DEFAULT_SELECTION_REVISION.to_string(),
            profile: DEFAULT_COVER_PROFILE.to_string(),
        };
        upsert_job_on(
            scope,
            &key,
            CoverJobState::Pending,
            "background",
            10,
            generation,
            &session_epoch,
            now,
            CoverJobUpsertCause::Demand,
        )?;
        report.jobs_created += 1;
    }
    if report.jobs_created > 0 {
        report.claimable_work = true;
    }
    // P1-E/U-α：本函数只对"完全没有 job 行"的 asset 调用 upsert，因此
    // `jobs_created > 0` 就是它自己的 changed gate（不外推到别的 batch owner）。
    if report.jobs_created > 0 {
        bump_view_revision_on(scope, source_id, generation, now)?;
    }
    if let Some(tx) = owned_tx {
        tx.commit()?;
        if report.jobs_created > 0 {
            // 仅在**自己拥有事务**时才能确定已提交；借用外层事务时不得 emit
            // （否则外层 rollback 后 wake 已经发出）。
            crate::remote_scan::cover_revision_stream::notify_cover_revision(source_id, None);
        }
    }
    Ok(report)
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
             long_retry_consumed=CASE WHEN long_retry_pending=1 THEN 1 ELSE long_retry_consumed END,
             long_retry_pending=0,
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
    if let Some(ref claimed) = job {
        // P1-E/REV-2：claim 是真实 durable transition
        //（pending/retry_wait → running）⇒ 同一事务内推进 revision。
        // 竞争失败（job = None）**不** bump，禁止制造假 revision。
        bump_view_revision_on(&tx, &claimed.key.source_id, claimed.generation, now)?;
    }
    tx.commit()?;
    if let Some(ref claimed) = job {
        // P1-E：commit-after-emit —— wake-up 只能在提交成功之后发出。
        crate::remote_scan::cover_revision_stream::notify_cover_revision(
            &claimed.key.source_id,
            Some(&claimed.key.asset_id),
        );
    }
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
             long_retry_consumed=CASE WHEN long_retry_pending=1 THEN 1 ELSE long_retry_consumed END,
             long_retry_pending=0,
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
    if let Some(ref claimed) = job {
        // P1-E/REV-2：claim 是真实 durable transition
        //（pending/retry_wait → running）⇒ 同一事务内推进 revision。
        // 竞争失败（job = None）**不** bump，禁止制造假 revision。
        bump_view_revision_on(&tx, &claimed.key.source_id, claimed.generation, now)?;
    }
    tx.commit()?;
    if let Some(ref claimed) = job {
        // P1-E：commit-after-emit —— wake-up 只能在提交成功之后发出。
        crate::remote_scan::cover_revision_stream::notify_cover_revision(
            &claimed.key.source_id,
            Some(&claimed.key.asset_id),
        );
    }
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
             long_retry_consumed=CASE WHEN long_retry_pending=1 THEN 1 ELSE long_retry_consumed END,
             long_retry_pending=0,
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
    if let Some(ref claimed) = job {
        // P1-E/REV-2：claim 是真实 durable transition
        //（pending/retry_wait → running）⇒ 同一事务内推进 revision。
        // 竞争失败（job = None）**不** bump，禁止制造假 revision。
        bump_view_revision_on(&tx, &claimed.key.source_id, claimed.generation, now)?;
    }
    tx.commit()?;
    if let Some(ref claimed) = job {
        // P1-E：commit-after-emit —— wake-up 只能在提交成功之后发出。
        crate::remote_scan::cover_revision_stream::notify_cover_revision(
            &claimed.key.source_id,
            Some(&claimed.key.asset_id),
        );
    }
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
             lease_until=?2,attempt=attempt+1,next_attempt_at=NULL,
             long_retry_consumed=CASE WHEN long_retry_pending=1 THEN 1 ELSE long_retry_consumed END,
             long_retry_pending=0,updated_at=?3
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
