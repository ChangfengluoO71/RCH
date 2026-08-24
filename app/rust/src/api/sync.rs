//! 同步引擎 FRB 桥接（ADR-024 Phase 4）。

use std::collections::HashMap;

use anyhow::Result;

use crate::api::source;
use crate::{db, sync};

/// 各实体合并变化数。
pub struct EntityCountDto {
    pub entity: String,
    pub count: i64,
}

/// 一次同步结果。
pub struct SyncOutcomeDto {
    pub initialized: bool,
    pub revision: i64,
    pub changed_entities: Vec<EntityCountDto>,
    pub pull_count: i64,
    pub push_count: i64,
    pub merge_count: i64,
    pub deleted_count: i64,
    /// Sync Plan 的 JSON 文本（debug/诊断；正常流程不解析）。
    pub plan_json: String,
}

/// 同步状态（设置页展示）。
pub struct SyncStatusDto {
    pub initialized: bool,
    pub revision: i64,
    pub last_sync_at: i64,
    pub last_error: String,
    pub library_id: String,
}

fn outcome_to_dto(outcome: &sync::SyncOutcome) -> SyncOutcomeDto {
    let mut changed: Vec<EntityCountDto> = outcome
        .changed_entities
        .iter()
        .map(|(e, n)| EntityCountDto {
            entity: e.clone(),
            count: *n as i64,
        })
        .collect();
    changed.sort_by(|a, b| a.entity.cmp(&b.entity));
    SyncOutcomeDto {
        initialized: outcome.initialized,
        revision: outcome.revision,
        changed_entities: changed,
        pull_count: outcome.counts.remote as i64,
        push_count: (outcome.counts.local + outcome.counts.merged + outcome.counts.deleted) as i64,
        merge_count: outcome.counts.merged as i64,
        deleted_count: outcome.counts.deleted as i64,
        plan_json: serde_json::to_string(&outcome.plan).unwrap_or_else(|_| "[]".into()),
    }
}

/// 执行一次完整同步（WebDAV session + 同步目录 + 本机平台标识）。
pub fn sync_now(session: u64, dir: String, platform: String) -> Result<SyncOutcomeDto, String> {
    let client = source::get_session(session).map_err(|e| e.to_string())?;
    // P1-6：生产路径走锁外网络（网络阶段不持有 DB 锁）。
    let outcome =
        sync::sync_with_webdav_global(&client, &dir, &platform).map_err(|e| e.to_string())?;
    Ok(outcome_to_dto(&outcome))
}

/// 轻量轮询：只读远端 manifest revision（无 manifest → 0）。
/// ADR-028 §12.6：定时轮询先比较 revision，未变化则不触发全量同步
/// （避免每 60 秒全量下载状态文件 + 写历史，坚果云被打到 503 的根因之一）。
pub fn sync_remote_revision(session: u64, dir: String) -> Result<i64, String> {
    let client = source::get_session(session).map_err(|e| e.to_string())?;
    let rev = sync::webdav::read_manifest(&client, &dir)
        .map_err(|e| e.to_string())?
        .map(|m| m.revision)
        .unwrap_or(0);
    Ok(rev)
}

/// 读取同步状态。
pub fn sync_status() -> Result<SyncStatusDto, String> {
    let conn = db::get().lock().unwrap();
    let last_rev = sync::base::get_meta_on(&conn, sync::base::META_LAST_REVISION)
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let last_sync_at = sync::base::get_meta_on(&conn, sync::base::META_LAST_SYNC_AT)
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    Ok(SyncStatusDto {
        initialized: last_rev > 0,
        revision: last_rev,
        last_sync_at,
        last_error: sync::base::get_meta_on(&conn, sync::base::META_LAST_ERROR).unwrap_or_default(),
        library_id: sync::base::get_meta_on(&conn, sync::base::META_LIBRARY_ID).unwrap_or_default(),
    })
}

/// 记录最近一次同步错误（设置页展示）。
pub fn sync_set_last_error(message: String) -> Result<(), String> {
    let conn = db::get().lock().unwrap();
    sync::base::set_meta_on(&conn, sync::base::META_LAST_ERROR, &message).map_err(|e| e.to_string())
}

/// 清除同步错误标记。
pub fn sync_clear_last_error() -> Result<(), String> {
    let conn = db::get().lock().unwrap();
    sync::base::set_meta_on(&conn, sync::base::META_LAST_ERROR, "").map_err(|e| e.to_string())
}

/// 供诊断/调试：各实体本地快照规模。
pub fn sync_local_counts() -> Result<HashMap<String, i64>, String> {
    let conn = db::get().lock().unwrap();
    let snap = sync::snapshot::load_local_snapshots(&conn).map_err(|e| e.to_string())?;
    Ok(snap.into_iter().map(|(e, m)| (e, m.len() as i64)).collect())
}

/// 同步历史条目（可观测性）。
pub struct SyncHistoryDto {
    pub id: i64,
    pub start_time: i64,
    pub end_time: i64,
    pub revision_before: i64,
    pub revision_after: i64,
    pub pull_count: i64,
    pub push_count: i64,
    pub merge_count: i64,
    pub conflict_count: i64,
    pub error: String,
    pub summary: String,
}

/// 最近 N 条同步历史（倒序）。
pub fn sync_history_recent(limit: i64) -> Result<Vec<SyncHistoryDto>, String> {
    Ok(sync::history::recent(limit)
        .into_iter()
        .map(|r| SyncHistoryDto {
            id: r.id,
            start_time: r.start_time,
            end_time: r.end_time,
            revision_before: r.revision_before,
            revision_after: r.revision_after,
            pull_count: r.pull_count,
            push_count: r.push_count,
            merge_count: r.merge_count,
            conflict_count: r.conflict_count,
            error: r.error,
            summary: r.summary,
        })
        .collect())
}

/// 同步参与者列表（Phase 6 设备分组 UI 用）。
pub struct SyncDeviceDto {
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub last_seen_at: i64,
    pub last_revision: i64,
}

pub fn sync_devices_list() -> Result<Vec<SyncDeviceDto>, String> {
    Ok(sync::actor::list_devices()
        .into_iter()
        .map(|r| SyncDeviceDto {
            device_id: r.device_id,
            device_name: r.device_name,
            platform: r.platform,
            last_seen_at: r.last_seen_at,
            last_revision: r.last_revision,
        })
        .collect())
}
