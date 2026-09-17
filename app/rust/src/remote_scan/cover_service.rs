//! Cover extraction service.  The persistence and queue primitives live in
//! [`super::cover_store`]; this module is the single owner of demand tracking
//! and will also host provider I/O workers as the pipeline is enabled.

use super::cover_model::CoverJobKey;
use crate::cache;
use crate::db;
use rusqlite::params;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

/// In-memory consumer ownership is intentionally not persisted.  A process
/// restart must not resurrect viewport cards that no longer exist, while the
/// SQLite job remains available for background demand/retry.
#[derive(Debug, Default)]
pub struct ConsumerRegistry {
    by_consumer: Mutex<HashMap<String, HashSet<String>>>,
}

impl ConsumerRegistry {
    pub fn attach(&self, consumer_id: &str, key: &CoverJobKey) {
        self.by_consumer
            .lock()
            .unwrap()
            .entry(consumer_id.to_string())
            .or_default()
            .insert(key.encode());
    }

    pub fn release(&self, consumer_id: &str) {
        self.by_consumer.lock().unwrap().remove(consumer_id);
    }

    pub fn consumer_count(&self, key: &CoverJobKey) -> usize {
        let encoded = key.encode();
        self.by_consumer
            .lock()
            .unwrap()
            .values()
            .filter(|keys| keys.contains(&encoded))
            .count()
    }

    pub fn is_attached(&self, consumer_id: &str, key: &CoverJobKey) -> bool {
        self.by_consumer
            .lock()
            .unwrap()
            .get(consumer_id)
            .is_some_and(|keys| keys.contains(&key.encode()))
    }
}

pub fn consumers() -> &'static ConsumerRegistry {
    static REGISTRY: OnceLock<ConsumerRegistry> = OnceLock::new();
    REGISTRY.get_or_init(ConsumerRegistry::default)
}

/// Read only a published, versioned cover.  No provider session is created
/// and no network call is allowed on this path.
pub fn read_cached_cover(
    source_id: &str,
    asset_id: &str,
    selection_revision: &str,
    profile: &str,
) -> Result<Option<(Vec<u8>, u32, u32)>, String> {
    let conn = db::get().lock().map_err(|error| error.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT content_revision,updated_at FROM remote_cover_variant
             WHERE source_id=?1 AND asset_id=?2 AND selection_revision=?3
               AND profile=?4 AND state='ready'
             UNION
             SELECT content_revision,updated_at FROM remote_cover_job
             WHERE source_id=?1 AND asset_id=?2 AND selection_revision=?3
               AND profile=?4 AND state='ready'
             ORDER BY updated_at DESC",
        )
        .map_err(|error| error.to_string())?;
    let revisions = stmt
        .query_map(
            params![source_id, asset_id, selection_revision, profile],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<(String, i64)>>>()
        .map_err(|error| error.to_string())?;
    drop(stmt);
    drop(conn);
    for (revision, observed_at) in revisions {
        if let Some(image) = cache::remote_cover_cache_read(
            source_id,
            asset_id,
            &revision,
            selection_revision,
            profile,
        ) {
            return Ok(Some(image));
        }
        // Missing/corrupt bytes are not a completed cover. Reconcile only the
        // observed ready revision; a late worker, deletion or cancellation
        // must not be overwritten. No file I/O runs under the database lock.
        let conn = db::get().lock().map_err(|error| error.to_string())?;
        let tx = conn.unchecked_transaction().map_err(|error| error.to_string())?;
        for table in ["remote_cover_variant", "remote_cover_job"] {
            tx.execute(
                &format!("UPDATE {table} SET state='pending',updated_at=?7
                 WHERE source_id=?1 AND asset_id=?2 AND content_revision=?3
                   AND selection_revision=?4 AND profile=?5
                   AND state='ready' AND updated_at=?6"),
                params![source_id, asset_id, revision, selection_revision, profile, observed_at, db::now_ms()],
            ).map_err(|error| error.to_string())?;
        }
        tx.commit().map_err(|error| error.to_string())?;
    }
    Ok(None)
}
