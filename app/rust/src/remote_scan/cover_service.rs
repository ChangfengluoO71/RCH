//! Cover extraction service.  The persistence and queue primitives live in
//! [`super::cover_store`]; this module is the single owner of demand tracking
//! and will also host provider I/O workers as the pipeline is enabled.

use super::cover_model::CoverJobKey;
use crate::cache;
use crate::db;
use rusqlite::params;
use rusqlite::OptionalExtension;
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
/// P1-F：**纯**判断某 asset 的 cover 字节是否真的可用（filesystem / cache only）。
///
/// 硬契约：
/// * 只做缓存/文件系统读取 —— **无** DB mutation、**无** revision bump、**无** wake、
///   **无** worker、**无** session / provider 调用；
/// * P1-B 的 ready-missing 对账与 F 的 `available_books` 统计**共用**这一份判断，
///   避免"两份真理"（禁止在统计层再写一套判定）。
/// * 因此调用它绝**不**改变任何 durable state —— 统计层只读。
pub fn cover_material_available(
    source_id: &str,
    asset_id: &str,
    content_revision: &str,
    selection_revision: &str,
    profile: &str,
) -> bool {
    cache::remote_cover_cache_read(
        source_id,
        asset_id,
        content_revision,
        selection_revision,
        profile,
    )
    .is_some()
}

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
        let tx = conn
            .unchecked_transaction()
            .map_err(|error| error.to_string())?;
        // P1-E/REV-7：只有 job 侧的 ready → pending 真正发生（affected > 0）才推进
        // durable revision；variant 行的变化不单独 bump（同一逻辑 transition）。
        let mut job_changed = 0_usize;
        for table in ["remote_cover_variant", "remote_cover_job"] {
            let changed = tx
                .execute(
                    &format!(
                        "UPDATE {table} SET state='pending',updated_at=?7
                 WHERE source_id=?1 AND asset_id=?2 AND content_revision=?3
                   AND selection_revision=?4 AND profile=?5
                   AND state='ready' AND updated_at=?6"
                    ),
                    params![
                        source_id,
                        asset_id,
                        revision,
                        selection_revision,
                        profile,
                        observed_at,
                        db::now_ms()
                    ],
                )
                .map_err(|error| error.to_string())?;
            if table == "remote_cover_job" {
                job_changed = changed;
            }
        }
        if job_changed > 0 {
            // generation 取该 job **自身**的值（同一 tx 内读），不猜“当前最新”。
            let generation: Option<i64> = tx
                .query_row(
                    "SELECT generation FROM remote_cover_job
                      WHERE source_id=?1 AND asset_id=?2 AND content_revision=?3
                        AND selection_revision=?4 AND profile=?5",
                    params![source_id, asset_id, revision, selection_revision, profile],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| error.to_string())?;
            if let Some(generation) = generation {
                crate::remote_scan::cover_store::bump_view_revision_on(
                    &tx,
                    source_id,
                    generation,
                    db::now_ms(),
                )
                .map_err(|error| error.to_string())?;
            }
        }
        tx.commit().map_err(|error| error.to_string())?;
        // P1-E：commit-after-emit —— P1-B ready-missing 对账是一次真实的 durable
        // transition，必须唤醒消费者；此处 asset 唯一确定。
        crate::remote_scan::cover_revision_stream::notify_cover_revision(source_id, Some(asset_id));
    }
    Ok(None)
}
