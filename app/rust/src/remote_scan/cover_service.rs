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

/// RG-B 性能修复（方案 B）：**统计用**的廉价可用性判据（`metadata` + `len > 0`）。
///
/// 与 `cover_material_available` 的差异是**语义强度**：
/// * 本函数回答"缓存文件是否存在且非空" —— 与 raw-cache 权威判据一致，成本为一次 `metadata`；
/// * `cover_material_available` 回答"字节是否可解析" —— 会整文件读取 + 头校验，成本高。
///
/// `available_books` 统计使用本函数（它可能在轮询路径上被反复调用）；需要字节级确认时
/// （P1-B 对账、真正的读取链路）继续使用 `cover_material_available`。
pub fn cover_material_present(
    source_id: &str,
    asset_id: &str,
    content_revision: &str,
    selection_revision: &str,
    profile: &str,
) -> bool {
    cache::remote_cover_cache_present(
        source_id,
        asset_id,
        content_revision,
        selection_revision,
        profile,
    )
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
    // 第 82 轮补（D4）：**只读回退的候选**。换档 purge 只删 `remote_cover_job`/`variant`，
    // 磁盘上的 `.cover-v2` 与 `remote_cover_ref`(role='variant') 都还在（真机实测：
    // `variant=0` 而 `blob=ref=1061`、1.2 GB）⇒ 全库封面在卡片上退化成"等待/获取失败"，
    // 尽管字节就在本地。`owner_key` 是 `CoverJobKey::encode()`（`源|资产|content_revision|
    // selection|profile`，每段 `len:value`），缓存文件名又正是由这 5 段派生
    // （`cache::remote_cover_cache_filename`）⇒ 不需要 variant 行也能读到同一份字节。
    // 这里只**在锁内取候选**，文件读取放到锁外（与既有设计一致：不持锁做 I/O）。
    let ref_backed = ref_backed_revisions_on(&conn, source_id, asset_id, selection_revision, profile);
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
    // 第 82 轮补（D4）：durable 行没了但字节还在 ⇒ 纯只读地把它读出来（不写行、不 bump、不发事件）。
    for content_revision in ref_backed {
        if let Some(image) = crate::cache::remote_cover_cache_read(
            source_id,
            asset_id,
            &content_revision,
            selection_revision,
            profile,
        ) {
            return Ok(Some(image));
        }
    }
    Ok(None)
}

/// 用 `remote_cover_ref`(role='variant') 反推**候选 content_revision**（第 82 轮补/D4）。
///
/// 纯只读；只返回与调用方 `(selection, profile)` 完全一致的那些 ref —— 跨档位/跨选择
/// 的字节由 Dart 侧既有的 `_readAnyCachedCover` 负责，这里不越权。
fn ref_backed_revisions_on(
    conn: &rusqlite::Connection,
    source_id: &str,
    asset_id: &str,
    selection_revision: &str,
    profile: &str,
) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT owner_key FROM remote_cover_ref
          WHERE source_id=?1 AND asset_id=?2 AND role='variant'",
    ) else {
        return Vec::new();
    };
    let keys: Vec<String> = match stmt.query_map(params![source_id, asset_id], |row| {
        row.get::<_, String>(0)
    }) {
        Ok(rows) => rows.filter_map(Result::ok).collect(),
        Err(_) => return Vec::new(),
    };
    keys.iter()
        .filter_map(|owner_key| cover_owner_tail(owner_key))
        .filter(|(_, selection, owner_profile)| {
            selection == selection_revision && owner_profile == profile
        })
        .map(|(content_revision, _, _)| content_revision)
        .collect()
}

/// 解析 `CoverJobKey::encode()` 的形状，取回 `(content_revision, selection_revision, profile)`。
///
/// 每段都是 `len:value`（`cover_model.rs` 的 `encode`）；这里**按声明长度校验**而不是
/// 朴素按 `:` 切，值里含 `:` 时也会被判为不合法而不是误解析。
fn cover_owner_tail(owner_key: &str) -> Option<(String, String, String)> {
    let parts: Vec<&str> = owner_key.split('|').collect();
    if parts.len() != 5 {
        return None;
    }
    let field = |raw: &str| -> Option<String> {
        let (len_text, value) = raw.split_once(':')?;
        let len: usize = len_text.parse().ok()?;
        (value.len() == len).then(|| value.to_string())
    };
    Some((field(parts[2])?, field(parts[3])?, field(parts[4])?))
}
