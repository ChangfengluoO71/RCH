//! 同步引擎（ADR-024/025）。
//!
//! 日常同步 = WebDAV Sync State + 三方合并；rchpkg 仅为备份格式。

pub mod actor;
pub mod apply;
pub mod base;
pub mod history;
pub mod identity;
pub mod merge;
pub mod snapshot;
pub mod state;
pub mod webdav;

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use rusqlite::Connection;

use crate::db;
use crate::source::webdav::WebDavClient;
use crate::sync::merge::{MergeCounts, SyncEntry, SyncPlanItem};

/// 一次同步结果（FRB 用）。
#[derive(Debug, Clone)]
pub struct SyncOutcome {
    pub initialized: bool,
    pub revision: i64,
    pub changed_entities: HashMap<String, usize>,
    pub plan: Vec<SyncPlanItem>,
    pub counts: MergeCounts,
    /// CAS 冲突重试次数（可观测性）。
    pub conflict_retries: i64,
}

fn parse_remote_entity(bytes: &[u8]) -> Result<HashMap<String, SyncEntry>> {
    let mut out = HashMap::new();
    for line in String::from_utf8_lossy(bytes).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let e: SyncEntry = serde_json::from_str(line).context("解析远端状态条目失败")?;
        out.insert(e.key.clone(), e);
    }
    Ok(out)
}

fn parse_remote_files(
    files: &HashMap<String, Vec<u8>>,
) -> Result<HashMap<String, HashMap<String, SyncEntry>>> {
    let mut out = HashMap::new();
    for (entity, bytes) in files {
        out.insert(entity.clone(), parse_remote_entity(bytes)?);
    }
    Ok(out)
}

fn serialize_entity(entries: &HashMap<String, SyncEntry>) -> Vec<u8> {
    let mut keys: Vec<&String> = entries.keys().collect();
    keys.sort();
    let mut out = String::new();
    for k in keys {
        if let Ok(s) = serde_json::to_string(&entries[k]) {
            out.push_str(&s);
            out.push('\n');
        }
    }
    out.into_bytes()
}

/// 合并本地与远端（不落库、不写远端），返回各实体 merged + plan。
pub fn plan_merge(
    conn: &Connection,
    remote_files: Option<&HashMap<String, Vec<u8>>>,
    with_plan: bool,
) -> Result<(
    HashMap<String, HashMap<String, SyncEntry>>,
    Vec<SyncPlanItem>,
    MergeCounts,
)> {
    let local = snapshot::load_local_snapshots(conn)?;
    let base = snapshot::load_base_snapshots(conn)?;
    let remote = remote_files
        .map(parse_remote_files)
        .transpose()?
        .unwrap_or_default();
    let mut merged = HashMap::new();
    let mut plans = Vec::new();
    let mut counts = MergeCounts::default();
    for entity in snapshot::ENTITIES {
        let b = base.get(entity).cloned().unwrap_or_default();
        let l = local.get(entity).cloned().unwrap_or_default();
        // ADR-028：manifest 未引用该实体 = 本轮未提交，沿用 base（远端未变化）。
        // 绝不能把"未引用"当成"远端为空"——那会把未变化实体误判为远端删除。
        let r = match remote.get(entity) {
            Some(entries) => entries.clone(),
            None => b.clone(),
        };
        let res = merge::merge_batch(entity, &b, &l, &r, with_plan);
        merged.insert(
            entity.to_string(),
            res.merged.into_iter().map(|e| (e.key.clone(), e)).collect(),
        );
        plans.extend(res.plan);
        counts.local += res.counts.local;
        counts.remote += res.counts.remote;
        counts.merged += res.counts.merged;
        counts.deleted += res.counts.deleted;
    }
    Ok((merged, plans, counts))
}

fn build_remote_files(
    merged: &HashMap<String, HashMap<String, SyncEntry>>,
) -> HashMap<String, Vec<u8>> {
    let mut files = HashMap::new();
    for entity in snapshot::ENTITIES {
        if let Some(entries) = merged.get(entity) {
            // ADR-028：未变化实体（空 map）禁止写空文件——拉取端会把它当成"远端删光"。
            // 实体真正清空必须由墓碑条目（deleted=true）表达。
            if entries.is_empty() {
                continue;
            }
            files.insert(entity.to_string(), serialize_entity(entries));
        }
    }
    files
}

fn changed_counts(merged: &HashMap<String, HashMap<String, SyncEntry>>) -> HashMap<String, usize> {
    merged.iter().map(|(e, m)| (e.clone(), m.len())).collect()
}

/// 一次同步的已就绪产物（DB 阶段输出，网络阶段消费）。
pub struct SyncPrepared {
    pub merged: HashMap<String, HashMap<String, SyncEntry>>,
    pub plans: Vec<SyncPlanItem>,
    pub counts: MergeCounts,
    pub files: HashMap<String, Vec<u8>>,
    pub manifest: state::Manifest,
    pub next_rev: i64,
}

/// DB 阶段：合并 + 应用本地（事务）+ 构建远端文件/manifest。
/// 返回 None = 无变化（不写远端、不推进 base）。
fn prepare_sync(
    conn: &mut Connection,
    library_id: &str,
    own: &actor::DeviceFile,
    remote: Option<&(state::Manifest, HashMap<String, Vec<u8>>)>,
) -> Result<Option<SyncPrepared>> {
    let remote_rev = remote.map(|(m, _)| m.revision);
    let (merged, plans, counts) = match remote {
        // ADR-028 §12.6：远端无 manifest = 初始化 / 远端被清空自愈。
        // 无论 base 是否有内容，都以本地全量作为提交状态——
        // 否则三方 diff 在"远端为空、base 已建立"时产出空集，会推一个空 manifest
        // 导致远端数据永久丢失。
        None => {
            let local = snapshot::load_local_snapshots(conn)?;
            let mut counts = MergeCounts::default();
            for (_, m) in &local {
                counts.local += m.len();
            }
            (local, Vec::new(), counts)
        }
        Some((m, files)) => {
            // 防误合并（ADR-024 §2）
            if m.library_id != library_id {
                bail!(
                    "同步目录属于另一个同步库（library_id 不一致），已拒绝合并；请检查 WebDAV 目录配置"
                );
            }
            plan_merge(conn, Some(files), false)?
        }
    };
    let changed = merged.values().any(|m| !m.is_empty());
    if !changed && remote_rev.is_some() {
        return Ok(None);
    }
    let next_rev = remote_rev.unwrap_or(0) + 1;
    let files = build_remote_files(&merged);
    // ADR-028 全量引用：只有变化实体写新文件；未变化实体沿用远端 manifest 的旧引用。
    let changed_files: HashMap<String, String> = files
        .keys()
        .map(|e| (e.clone(), state::state_file_name(e, next_rev, true)))
        .collect();
    let writer = Some(state::WriterInfo {
        device_id: own.device_id.clone(),
        device_name: own.name.clone(),
    });
    let manifest = state::Manifest::push(
        library_id,
        next_rev,
        changed_files,
        remote.map(|(m, _)| m),
        writer,
    );
    let tx = conn.transaction().context("开启本地事务失败")?;
    let writer_device = remote
        .and_then(|(m, _)| m.writer.as_ref())
        .map(|w| w.device_id.clone());
    for (entity, entries) in &merged {
        apply::apply_merged(&tx, entity, entries, writer_device.as_deref())?;
    }
    // ADR-028：新源加入后，把此前无法解析的 pending 条目落真实表。
    apply::reapply_pending(&tx)?;
    tx.commit().context("应用合并结果失败")?;
    Ok(Some(SyncPrepared {
        merged,
        plans,
        counts,
        files,
        manifest,
        next_rev,
    }))
}

/// 网络阶段：上传状态文件 + 校验 manifest（CAS，防 TOCTOU 覆盖）。
fn push_sync(
    client: &WebDavClient,
    dir: &str,
    prepared: &SyncPrepared,
    expected_rev: Option<i64>,
) -> Result<()> {
    webdav::upload_state(
        client,
        dir,
        &prepared.manifest,
        &prepared.files,
        expected_rev,
    )?;
    match webdav::read_manifest(client, dir)? {
        Some(m)
            if m.revision == prepared.manifest.revision
                && m.library_id == prepared.manifest.library_id =>
        {
            Ok(())
        }
        _ => bail!("远端状态已变化（revision 冲突），请重新合并后重试"),
    }
}

/// DB 阶段：设备注册 + 推进 base + 墓碑 GC（只在成功后调用）。
fn finalize_sync(
    conn: &Connection,
    prepared: &SyncPrepared,
    own: &actor::DeviceFile,
    remotes: &[actor::DeviceFile],
) -> Result<()> {
    actor::register_devices_on(conn, own, remotes, prepared.next_rev)?;
    snapshot::advance_base(conn, prepared.next_rev, &prepared.merged)?;
    // 墓碑 GC：软删超过 30 天的 library_index 行（足够各端传播删除）。
    let _ = db::gc_library_index_tombstones(conn, db::now_ms() - 30 * 24 * 3600 * 1000);
    Ok(())
}

fn is_revision_conflict(e: &anyhow::Error) -> bool {
    format!("{e:#}").contains("revision 冲突")
}

/// 同步库身份协调（首接采纳）：
/// - 本机无库身份：远端已有 manifest → 采纳远端 library_id（同一同步库）；远端为空 → 新建。
/// - 本机与远端不一致：若本机**从未成功同步**（last_revision=0）→ 采纳远端（首次接入）；
///   否则 → 拒绝（两个不同同步库误指向同一目录）。
fn resolve_library_id(
    conn: &Connection,
    remote: Option<&(state::Manifest, HashMap<String, Vec<u8>>)>,
) -> Result<String> {
    let local = base::get_meta_on(conn, base::META_LIBRARY_ID);
    let remote_id = remote.map(|(m, _)| m.library_id.clone());
    match (local, remote_id) {
        (Some(l), Some(r)) if l != r => {
            let synced = base::get_meta_on(conn, base::META_LAST_REVISION)
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0)
                > 0;
            if synced {
                bail!(
                    "同步目录属于另一个同步库（library_id 不一致），已拒绝合并；请检查 WebDAV 目录配置"
                );
            }
            // 本地从未成功同步：采纳远端库身份（首次接入，避免误拒绝）。
            base::set_meta_on(conn, base::META_LIBRARY_ID, &r)?;
            Ok(r)
        }
        (Some(l), Some(_)) => Ok(l),
        (None, Some(r)) => {
            base::set_meta_on(conn, base::META_LIBRARY_ID, &r)?;
            Ok(r)
        }
        (Some(l), None) => Ok(l),
        (None, None) => {
            let id = base::new_library_id();
            base::set_meta_on(conn, base::META_LIBRARY_ID, &id)?;
            Ok(id)
        }
    }
}

fn record_outcome_history(
    conn: &Connection,
    start: i64,
    rev_before: i64,
    result: &Result<SyncOutcome>,
) {
    let end = db::now_ms();
    match result {
        Ok(out) => {
            let pull = out.counts.remote as i64;
            let push = (out.counts.local + out.counts.merged + out.counts.deleted) as i64;
            let merge = out.counts.merged as i64;
            let changed: Vec<(String, usize)> = out
                .changed_entities
                .iter()
                .map(|(e, n)| (e.clone(), *n))
                .collect();
            let _ = history::record_on(
                conn,
                start,
                end,
                rev_before,
                out.revision,
                pull,
                push,
                merge,
                out.conflict_retries,
                "",
                &changed,
            );
            let _ = base::set_meta_on(conn, base::META_LAST_ERROR, "");
        }
        Err(e) => {
            let msg = format!("{e:#}");
            let _ = history::record_on(
                conn,
                start,
                end,
                rev_before,
                rev_before,
                0,
                0,
                0,
                0,
                &msg,
                &[],
            );
            let _ = base::set_meta_on(conn, base::META_LAST_ERROR, &msg);
        }
    }
}

/// 执行一次完整同步（传入连接，测试/持锁路径；网络阶段仍持调用方连接）。
pub fn sync_with_webdav(
    conn: &mut Connection,
    client: &WebDavClient,
    dir: &str,
    platform: &str,
) -> Result<SyncOutcome> {
    let start = db::now_ms();
    let rev_before = base::get_meta_on(conn, base::META_LAST_REVISION)
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let result = run_sync_loop(conn, client, dir, platform);
    record_outcome_history(conn, start, rev_before, &result);
    result
}

fn run_sync_loop(
    conn: &mut Connection,
    client: &WebDavClient,
    dir: &str,
    platform: &str,
) -> Result<SyncOutcome> {
    let own = actor::own_device(conn, platform)?;
    let mut conflict_retries = 0i64;

    for _attempt in 0..3 {
        let remote = webdav::download_state(client, dir)?;
        let library_id = resolve_library_id(conn, remote.as_ref())?;
        let remote_rev = remote.as_ref().map(|(m, _)| m.revision);
        let Some(prepared) = prepare_sync(conn, &library_id, &own, remote.as_ref())? else {
            // 无变化：不写远端、不推进 base；仍刷新设备注册
            let remotes = webdav::read_devices(client, dir)?;
            actor::register_devices_on(conn, &own, &remotes, remote_rev.unwrap_or(0))?;
            return Ok(SyncOutcome {
                initialized: remote_rev.is_some(),
                revision: remote_rev.unwrap_or(0),
                changed_entities: HashMap::new(),
                plan: Vec::new(),
                counts: MergeCounts::default(),
                conflict_retries,
            });
        };
        match push_sync(client, dir, &prepared, remote_rev) {
            Ok(()) => {
                webdav::upload_device_file(client, dir, &own)?;
                let remotes = webdav::read_devices(client, dir)?;
                finalize_sync(conn, &prepared, &own, &remotes)?;
                return Ok(SyncOutcome {
                    initialized: true,
                    revision: prepared.next_rev,
                    changed_entities: changed_counts(&prepared.merged),
                    plan: prepared.plans,
                    counts: prepared.counts,
                    conflict_retries,
                });
            }
            Err(e) if is_revision_conflict(&e) => {
                conflict_retries += 1;
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    bail!("同步冲突重试次数过多，请稍后再试")
}

/// 生产入口（P1-6 锁外网络）：网络阶段不持有 DB 锁，DB 阶段短锁。
pub fn sync_with_webdav_global(
    client: &WebDavClient,
    dir: &str,
    platform: &str,
) -> Result<SyncOutcome> {
    let start = db::now_ms();
    let rev_before = {
        let conn = db::get().lock().unwrap();
        base::get_meta_on(&conn, base::META_LAST_REVISION)
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0)
    };
    let result = (|| -> Result<SyncOutcome> {
        let own = {
            let conn = db::get().lock().unwrap();
            actor::own_device(&conn, platform)?
        };
        let mut conflict_retries = 0i64;
        for _attempt in 0..3 {
            // 网络（无锁）
            let remote = webdav::download_state(client, dir)?;
            let remote_rev = remote.as_ref().map(|(m, _)| m.revision);
            // DB 短锁：库身份首接协调
            let library_id = {
                let conn = db::get().lock().unwrap();
                resolve_library_id(&conn, remote.as_ref())?
            };
            // DB 短锁：合并 + 应用 + 构建
            let prepared = {
                let mut conn = db::get().lock().unwrap();
                prepare_sync(&mut conn, &library_id, &own, remote.as_ref())?
            };
            let Some(prepared) = prepared else {
                let remotes = webdav::read_devices(client, dir)?;
                {
                    let conn = db::get().lock().unwrap();
                    actor::register_devices_on(&conn, &own, &remotes, remote_rev.unwrap_or(0))?;
                }
                return Ok(SyncOutcome {
                    initialized: remote_rev.is_some(),
                    revision: remote_rev.unwrap_or(0),
                    changed_entities: HashMap::new(),
                    plan: Vec::new(),
                    counts: MergeCounts::default(),
                    conflict_retries,
                });
            };
            // 网络（无锁）：上传 + 设备文件 + 读远端设备
            match push_sync(client, dir, &prepared, remote_rev) {
                Ok(()) => {
                    webdav::upload_device_file(client, dir, &own)?;
                    let remotes = webdav::read_devices(client, dir)?;
                    {
                        let conn = db::get().lock().unwrap();
                        finalize_sync(&conn, &prepared, &own, &remotes)?;
                    }
                    return Ok(SyncOutcome {
                        initialized: true,
                        revision: prepared.next_rev,
                        changed_entities: changed_counts(&prepared.merged),
                        plan: prepared.plans,
                        counts: prepared.counts,
                        conflict_retries,
                    });
                }
                Err(e) if is_revision_conflict(&e) => {
                    conflict_retries += 1;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        bail!("同步冲突重试次数过多，请稍后再试")
    })();
    {
        let conn = db::get().lock().unwrap();
        record_outcome_history(&conn, start, rev_before, &result);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use serde_json::json;

    fn schema_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_tables(&conn).unwrap();
        conn
    }

    fn insert_source(
        conn: &Connection,
        id: &str,
        r#type: &str,
        path: &str,
        url: Option<&str>,
    ) -> String {
        let fp = crate::db::compute_source_fingerprint(r#type, url, path, None);
        conn.execute(
            "INSERT INTO book_sources (id, type, name, path, url, fingerprint, updated_at, deleted)
             VALUES (?1, ?2, 'NAS', ?3, ?4, ?5, 1000, 0)",
            rusqlite::params![id, r#type, path, url, fp],
        )
        .unwrap();
        fp
    }

    fn remote_files(entity: &str, entries: Vec<SyncEntry>) -> HashMap<String, Vec<u8>> {
        let mut m = HashMap::new();
        let mut map = HashMap::new();
        for e in entries {
            map.insert(e.key.clone(), e);
        }
        m.insert(entity.to_string(), serialize_entity(&map));
        m
    }

    #[test]
    fn initial_push_uploads_local_as_revision_1() {
        // 编排核心（无 WebDAV 客户端）：直接测 plan_merge + build_remote_files
        let conn = schema_conn();
        let fp = insert_source(
            &conn,
            "s1",
            "webdav",
            "/books",
            Some("https://dav.example.com/dav"),
        );
        let key = crate::sync::identity::book_id(&fp, "/books/a.cbz");
        conn.execute(
            "INSERT INTO book_metas (key, title, rotations, updated_at, deleted)
             VALUES ('webdav|s1|/books/a.cbz', 'A', '{}', 100, 0)",
            [],
        )
        .unwrap();

        let (merged, plan, _) = plan_merge(&conn, None, true).unwrap();
        assert!(merged[base::ENTITY_METAS].contains_key(&key));
        assert_eq!(merged[base::ENTITY_SOURCES].len(), 1);
        assert!(!plan.is_empty());

        let files = build_remote_files(&merged);
        assert!(files.contains_key(base::ENTITY_METAS));
        let parsed = parse_remote_entity(&files[base::ENTITY_METAS]).unwrap();
        assert!(parsed.contains_key(&key));
    }

    #[test]
    fn pull_merges_remote_meta_and_applies() {
        let conn = schema_conn();
        let fp = insert_source(
            &conn,
            "s1",
            "webdav",
            "/books",
            Some("https://dav.example.com/dav"),
        );
        let key = crate::sync::identity::book_id(&fp, "/books/a.cbz");
        // 远端有 meta（remote only）
        let remote = remote_files(
            base::ENTITY_METAS,
            vec![SyncEntry::live(
                &key,
                200,
                json!({
                    "path": "/books/a.cbz", "title": "远端标题", "author": "作者", "rotations": "{}"
                }),
            )],
        );
        let (merged, _, _) = plan_merge(&conn, Some(&remote), true).unwrap();
        apply::apply_merged(&conn, base::ENTITY_METAS, &merged[base::ENTITY_METAS], None).unwrap();
        let title: String = conn
            .query_row(
                "SELECT title FROM book_metas WHERE key='webdav|s1|/books/a.cbz'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title, "远端标题");
    }

    #[test]
    fn remote_deletion_applies_locally() {
        let conn = schema_conn();
        let fp = insert_source(
            &conn,
            "s1",
            "webdav",
            "/books",
            Some("https://dav.example.com/dav"),
        );
        let key = crate::sync::identity::book_id(&fp, "/books/a.cbz");
        conn.execute(
            "INSERT INTO book_metas (key, title, rotations, updated_at, deleted)
             VALUES ('webdav|s1|/books/a.cbz', 'A', '{}', 100, 0)",
            [],
        )
        .unwrap();
        // 上次同步已建立 base（= 本地快照，远端有该 meta）
        snapshot::advance_base(&conn, 1, &snapshot::load_local_snapshots(&conn).unwrap()).unwrap();

        // ADR-028：远端删除 = 文件内含墓碑条目；"未引用实体" ≠ "远端为空"。
        // 因此删除传播必须由墓碑表达，而不是空文件。
        let tomb = SyncEntry::tombstone(&key, 100, json!({"path": "/books/a.cbz"}));
        let remote = remote_files(base::ENTITY_METAS, vec![tomb]);
        let (merged, _, _) = plan_merge(&conn, Some(&remote), true).unwrap();
        let metas = &merged[base::ENTITY_METAS];
        assert!(metas[&key].deleted);
        apply::apply_merged(&conn, base::ENTITY_METAS, metas, None).unwrap();
        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM book_metas WHERE key='webdav|s1|/books/a.cbz'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 0);
    }

    #[test]
    fn plan_merge_absent_entity_uses_base_not_empty() {
        let conn = schema_conn();
        let fp = insert_source(
            &conn,
            "s1",
            "webdav",
            "/books",
            Some("https://dav.example.com/dav"),
        );
        let key = crate::sync::identity::book_id(&fp, "/books/a.cbz");
        conn.execute(
            "INSERT INTO book_metas (key, title, rotations, updated_at, deleted)
             VALUES ('webdav|s1|/books/a.cbz', 'A', '{}', 100, 0)",
            [],
        )
        .unwrap();
        // 上次提交已建立 base（全量镜像）
        snapshot::advance_base(&conn, 1, &snapshot::load_local_snapshots(&conn).unwrap()).unwrap();

        // 远端 manifest 只引用了 metas；sources 未引用 = 本轮未提交
        let remote = remote_files(
            base::ENTITY_METAS,
            vec![SyncEntry::live(
                &key,
                100,
                json!({"path": "/books/a.cbz", "title": "A", "rotations": "{}"}),
            )],
        );
        let (merged, _, _) = plan_merge(&conn, Some(&remote), true).unwrap();
        // 未引用的实体不得被误判为"远端删除"：merged 中不得出现任何墓碑/条目
        assert!(merged[base::ENTITY_SOURCES].is_empty());
        assert!(merged[base::ENTITY_LIBRARY_INDEX].is_empty());
    }

    #[test]
    fn unresolved_entries_go_pending_and_never_tombstone() {
        let conn = schema_conn();
        // 远端条目来自另一台设备的源（本机无该 fingerprint 源）→ apply 无法解析
        let remote_fp = db::compute_source_fingerprint(
            "webdav",
            Some("https://remote.example.com/dav"),
            "/books",
            None,
        );
        let key = crate::sync::identity::book_id(&remote_fp, "/books/a.cbz");
        let remote = remote_files(
            base::ENTITY_LIBRARY_INDEX,
            vec![SyncEntry::live(
                &key,
                200,
                json!({
                    "name": "a.cbz", "path": "/books/a.cbz", "entryType": "file",
                    "size": null, "modifiedAt": null, "coverPath": null,
                    "hash": null, "parentId": null,
                }),
            )],
        );
        let (merged, _, _) = plan_merge(&conn, Some(&remote), true).unwrap();
        apply::apply_merged(
            &conn,
            base::ENTITY_LIBRARY_INDEX,
            &merged[base::ENTITY_LIBRARY_INDEX],
            Some("dev-R"),
        )
        .unwrap();

        // 未解析条目进入 pending（不被静默丢弃），且快照视为存在
        let pending = base::load_pending_on(&conn).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].entity_key, key);
        assert!(
            snapshot::load_local_snapshots(&conn).unwrap()[base::ENTITY_LIBRARY_INDEX]
                .contains_key(&key)
        );

        // 建立 base（上次提交状态）后，三方 base/local/remote 一致 → 不产生伪墓碑
        snapshot::advance_base(&conn, 1, &snapshot::load_local_snapshots(&conn).unwrap()).unwrap();
        let (merged2, _, _) = plan_merge(&conn, Some(&remote), true).unwrap();
        assert!(merged2[base::ENTITY_LIBRARY_INDEX].is_empty());

        // 本机加入同 fingerprint 源 → reapply 落真实表并清除 pending
        let own_id = "local-id";
        conn.execute(
            "INSERT INTO book_sources (id, type, name, path, url, fingerprint, updated_at, deleted)
             VALUES (?1, 'webdav', 'NAS', '/books', 'https://remote.example.com/dav', ?2, 300, 0)",
            rusqlite::params![own_id, remote_fp],
        )
        .unwrap();
        apply::reapply_pending(&conn).unwrap();
        assert!(base::load_pending_on(&conn).unwrap().is_empty());
        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM library_index WHERE id=?1 AND deleted=0",
                rusqlite::params![key],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 1);
    }

    #[test]
    fn base_advance_records_revision_after_success() {
        let conn = schema_conn();
        let fp = insert_source(
            &conn,
            "s1",
            "webdav",
            "/books",
            Some("https://dav.example.com/dav"),
        );
        let key = crate::sync::identity::book_id(&fp, "/books/a.cbz");
        let mut merged = HashMap::new();
        let mut metas = HashMap::new();
        metas.insert(
            key.clone(),
            SyncEntry::live(&key, 1, json!({"path": "/books/a.cbz", "title": "A"})),
        );
        merged.insert(base::ENTITY_METAS.into(), metas);
        snapshot::advance_base(&conn, 5, &merged).unwrap();
        assert_eq!(
            base::get_meta_on(&conn, base::META_LAST_REVISION).as_deref(),
            Some("5")
        );
        assert!(base::get_base_on(&conn, base::ENTITY_METAS, &key).is_some());
    }

    #[test]
    fn plan_merge_includes_library_index_local_only() {
        let conn = schema_conn();
        let fp = insert_source(
            &conn,
            "s1",
            "webdav",
            "/books",
            Some("https://dav.example.com/dav"),
        );
        let key = crate::sync::identity::book_id(&fp, "/books/a.cbz");
        conn.execute(
            "INSERT INTO library_index (id, source_id, parent_id, name, path, entry_type, updated_at, deleted)
             VALUES (?1, 's1', NULL, 'a.cbz', '/books/a.cbz', 'file', 1, 0)",
            rusqlite::params![key],
        )
        .unwrap();
        let (merged, _, _) = plan_merge(&conn, None, true).unwrap();
        assert!(merged[base::ENTITY_LIBRARY_INDEX].contains_key(&key));
        assert_eq!(
            merged[base::ENTITY_LIBRARY_INDEX][&key].data["entryType"],
            json!("file")
        );
    }

    #[test]
    fn library_index_apply_tombstone_soft_deletes() {
        let conn = schema_conn();
        let fp = insert_source(
            &conn,
            "s1",
            "webdav",
            "/books",
            Some("https://dav.example.com/dav"),
        );
        let key = crate::sync::identity::book_id(&fp, "/books/a.cbz");
        conn.execute(
            "INSERT INTO library_index (id, source_id, name, path, entry_type, updated_at, deleted)
             VALUES (?1, 's1', 'a.cbz', '/books/a.cbz', 'file', 1, 0)",
            rusqlite::params![key],
        )
        .unwrap();
        let tomb =
            crate::sync::merge::SyncEntry::tombstone(&key, 2, json!({"path": "/books/a.cbz"}));
        let mut m = HashMap::new();
        m.insert(key.clone(), tomb);
        apply::apply_merged(&conn, base::ENTITY_LIBRARY_INDEX, &m, None).unwrap();
        let deleted: i64 = conn
            .query_row(
                "SELECT deleted FROM library_index WHERE id=?1",
                rusqlite::params![key],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(deleted, 1);
        // 离线浏览不再显示软删条目
        assert!(db::load_library_index_for_source_on(&conn, "s1").is_empty());
    }

    #[test]
    fn prepare_sync_detects_changes_and_revision() {
        let mut conn = schema_conn();
        let fp = insert_source(
            &conn,
            "s1",
            "webdav",
            "/books",
            Some("https://dav.example.com/dav"),
        );
        let key = crate::sync::identity::book_id(&fp, "/books/a.cbz");
        conn.execute(
            "INSERT INTO book_metas (key, title, rotations, updated_at, deleted)
             VALUES ('webdav|s1|/books/a.cbz', 'A', '{}', 100, 0)",
            [],
        )
        .unwrap();
        // 上次同步 = 本地快照：先取快照作为远端文件，再推进 base
        let local_snap = snapshot::load_local_snapshots(&conn).unwrap();
        let files = build_remote_files(&local_snap);
        snapshot::advance_base(&conn, 1, &local_snap).unwrap();

        let own = actor::own_device(&conn, "test").unwrap();
        let library_id = "lib-t";
        let remote = (state::Manifest::new(library_id, 1), files);

        // 本地未变 → 无动作
        assert!(prepare_sync(&mut conn, library_id, &own, Some(&remote))
            .unwrap()
            .is_none());

        // 本地改动 → 产出 prepared，next_rev = 远端 rev + 1
        conn.execute(
            "UPDATE book_metas SET title='A2', updated_at=200 WHERE key='webdav|s1|/books/a.cbz'",
            [],
        )
        .unwrap();
        let prepared = prepare_sync(&mut conn, library_id, &own, Some(&remote))
            .unwrap()
            .unwrap();
        assert_eq!(prepared.next_rev, 2);
        assert!(prepared.merged[base::ENTITY_METAS].contains_key(&key));
        assert!(prepared.manifest.writer.is_some());
    }

    #[test]
    fn prepare_sync_preserves_unchanged_entity_references() {
        let mut conn = schema_conn();
        let fp = insert_source(
            &conn,
            "s1",
            "webdav",
            "/books",
            Some("https://dav.example.com/dav"),
        );
        let key = crate::sync::identity::book_id(&fp, "/books/a.cbz");
        conn.execute(
            "INSERT INTO book_metas (key, title, rotations, updated_at, deleted)
             VALUES ('webdav|s1|/books/a.cbz', 'A', '{}', 100, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO library_index (id, source_id, parent_id, name, path, entry_type, updated_at, deleted)
             VALUES (?1, 's1', NULL, 'a.cbz', '/books/a.cbz', 'file', 1, 0)",
            rusqlite::params![key],
        )
        .unwrap();

        let own = actor::own_device(&conn, "test").unwrap();
        let library_id = "lib-t";
        // 首次同步：全量推（所有有内容的实体都写文件）
        let p1 = prepare_sync(&mut conn, library_id, &own, None)
            .unwrap()
            .unwrap();
        assert!(p1.files.contains_key(base::ENTITY_SOURCES));
        assert!(p1.files.contains_key(base::ENTITY_LIBRARY_INDEX));
        snapshot::advance_base(&conn, 1, &p1.merged).unwrap();
        let remote = (p1.manifest.clone(), p1.files.clone());

        // 第二次只改 metas：sources/library_index 不得写空文件，必须沿用旧引用
        conn.execute(
            "UPDATE book_metas SET title='A2', updated_at=200 WHERE key='webdav|s1|/books/a.cbz'",
            [],
        )
        .unwrap();
        let p2 = prepare_sync(&mut conn, library_id, &own, Some(&remote))
            .unwrap()
            .unwrap();
        assert!(!p2.files.contains_key(base::ENTITY_SOURCES));
        assert!(!p2.files.contains_key(base::ENTITY_LIBRARY_INDEX));
        assert!(p2.files.contains_key(base::ENTITY_METAS));
        assert_eq!(
            p2.manifest.files[base::ENTITY_SOURCES],
            p1.manifest.files[base::ENTITY_SOURCES]
        );
        assert_eq!(
            p2.manifest.files[base::ENTITY_LIBRARY_INDEX],
            p1.manifest.files[base::ENTITY_LIBRARY_INDEX]
        );
    }

    #[test]
    fn prepare_sync_remote_cleared_repushes_full_local() {
        let mut conn = schema_conn();
        let fp = insert_source(
            &conn,
            "s1",
            "webdav",
            "/books",
            Some("https://dav.example.com/dav"),
        );
        let key = crate::sync::identity::book_id(&fp, "/books/a.cbz");
        conn.execute(
            "INSERT INTO book_metas (key, title, rotations, updated_at, deleted)
             VALUES ('webdav|s1|/books/a.cbz', 'A', '{}', 100, 0)",
            [],
        )
        .unwrap();

        let own = actor::own_device(&conn, "test").unwrap();
        let library_id = "lib-t";
        // 已成功同步过一次（base 已建立）
        let p1 = prepare_sync(&mut conn, library_id, &own, None)
            .unwrap()
            .unwrap();
        snapshot::advance_base(&conn, 1, &p1.merged).unwrap();

        // 远端被清空（manifest 消失）→ 必须本地全量重推，而不是推空 manifest
        let p2 = prepare_sync(&mut conn, library_id, &own, None)
            .unwrap()
            .unwrap();
        assert_eq!(p2.next_rev, 1);
        assert!(p2.files.contains_key(base::ENTITY_SOURCES));
        assert!(p2.files.contains_key(base::ENTITY_METAS));
        assert_eq!(p2.merged[base::ENTITY_SOURCES].len(), 1);
        assert!(p2.merged[base::ENTITY_METAS].contains_key(&key));
    }

    #[test]
    fn apply_remote_source_marks_device_and_remote_only() {
        let conn = schema_conn();
        let fp = db::compute_source_fingerprint(
            "webdav",
            Some("https://dav.example.com/dav"),
            "/books",
            None,
        );
        let e = crate::sync::merge::SyncEntry::live(
            &fp,
            100,
            json!({
                "type": "webdav", "name": "远端NAS", "path": "/books",
                "url": "https://dav.example.com/dav", "username": null, "port": null,
                "note": "", "capabilityLabel": "webdav", "rootId": null, "clientId": null,
                "remoteOnly": false, "originDeviceId": null,
            }),
        );
        let mut m = HashMap::new();
        m.insert(fp.clone(), e);
        apply::apply_merged(&conn, base::ENTITY_SOURCES, &m, Some("dev-A")).unwrap();
        let (remote_only, origin): (i64, Option<String>) = conn
            .query_row(
                "SELECT remote_only, origin_device_id FROM book_sources WHERE fingerprint=?1",
                rusqlite::params![fp],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(remote_only, 1);
        assert_eq!(origin.as_deref(), Some("dev-A"));
    }

    fn seed_library_index(conn: &mut Connection, n: usize) -> String {
        let fp = insert_source(
            &conn,
            "s1",
            "webdav",
            "/books",
            Some("https://dav.example.com/dav"),
        );
        let tx = conn.transaction().unwrap();
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO library_index
                     (id, source_id, name, path, entry_type, size, modified_at, updated_at, deleted)
                     VALUES (?1, 's1', ?2, ?3, 'file', 1000, 1000, 1, 0)",
                )
                .unwrap();
            for i in 0..n {
                let path = format!("/books/book{i:05}.cbz");
                stmt.execute(rusqlite::params![
                    crate::sync::identity::book_id(&fp, &path),
                    format!("book{i:05}.cbz"),
                    path,
                ])
                .unwrap();
            }
        }
        tx.commit().unwrap();
        fp
    }

    fn bench_pipeline(conn: &mut Connection, n: usize, budget_ms: u128) {
        seed_library_index(conn, n);
        let t0 = std::time::Instant::now();
        let local = snapshot::load_local_snapshots(conn).unwrap();
        let t1 = std::time::Instant::now();
        assert_eq!(local[base::ENTITY_LIBRARY_INDEX].len(), n);
        let (merged, _, _) = plan_merge(conn, None, false).unwrap();
        let t2 = std::time::Instant::now();
        let files = build_remote_files(&merged);
        let t3 = std::time::Instant::now();
        let parsed = parse_remote_entity(&files[base::ENTITY_LIBRARY_INDEX]).unwrap();
        let t4 = std::time::Instant::now();
        assert_eq!(parsed.len(), n);
        let total = t4.duration_since(t0).as_millis();
        eprintln!(
            "bench n={n}: snapshot={}ms merge={}ms serialize={}ms parse={}ms total={}ms (budget={budget_ms}ms)",
            t1.duration_since(t0).as_millis(),
            t2.duration_since(t1).as_millis(),
            t3.duration_since(t2).as_millis(),
            t4.duration_since(t3).as_millis(),
            total,
        );
        assert!(
            total < budget_ms,
            "n={n} 本地管线耗时 {total}ms 超过预算 {budget_ms}ms"
        );
    }

    #[test]
    fn pipeline_scale_5k_within_budget() {
        let mut conn = schema_conn();
        bench_pipeline(&mut conn, 5_000, 3000);
    }

    #[test]
    #[ignore]
    fn bench_pipeline_100k() {
        let mut conn = schema_conn();
        // 目标：10 万条目本地管线（快照+合并+序列化+解析）< 5s；内存约 100-200MB（测量见日志）。
        bench_pipeline(&mut conn, 100_000, 5000);
    }
}
