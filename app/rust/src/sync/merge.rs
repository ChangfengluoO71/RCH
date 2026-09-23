//! 三方合并引擎（ADR-024 §5/§7）。
//!
//! Base + Local + Remote → Sync Plan → Merged State。
//! 实体策略：
//! - metas：字段级三方合并（仅同字段双改才 LWW）
//! - tags / book_tags：集合级并集 + 墓碑（同 key 双改 LWW，平局删除优先防复活）
//! - records / sources / settings：LWW + 墓碑
//! - library_index：单端变化接受；双端变化确定性 LWW + 墓碑
//!
//! 本引擎与 rchpkg 导入完全解耦：rchpkg 恢复走 force 覆盖，不经过三方合并。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::sync::base;

/// 同步条目（引擎统一表示；key = 同步层稳定身份）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SyncEntry {
    pub key: String,
    /// 逻辑时钟（updated_at / revision）。
    pub updated_at: i64,
    /// 墓碑：删除传播。
    pub deleted: bool,
    /// 实体载荷（metas 为字段对象；其余实体为各自行字段）。
    pub data: Value,
}

impl SyncEntry {
    pub fn live(key: &str, updated_at: i64, data: Value) -> Self {
        Self {
            key: key.to_string(),
            updated_at,
            deleted: false,
            data,
        }
    }

    /// 墓碑保留原 data（路径等身份信息），保证应用层可反解本地行执行删除。
    pub fn tombstone(key: &str, updated_at: i64, data: Value) -> Self {
        Self {
            key: key.to_string(),
            updated_at,
            deleted: true,
            data,
        }
    }
}

/// 同步决策记录（可诊断；debug/日志/诊断页展示）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPlanItem {
    pub entity: String,
    pub key: String,
    pub base: Option<Value>,
    pub local: Option<Value>,
    pub remote: Option<Value>,
    /// 本机/远端条目逻辑时钟（updated_at），诊断来源时间。
    pub local_revision: Option<i64>,
    pub remote_revision: Option<i64>,
    pub decision: String,
    /// local / remote / merged / none / deleted。
    pub winner: String,
    /// 人类可读原因（诊断展示）。
    pub reason: String,
    pub result: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct MergeResult {
    pub merged: Vec<SyncEntry>,
    pub plan: Vec<SyncPlanItem>,
    pub counts: MergeCounts,
}

/// 轻量决策计数（同步路径用，避免为每个条目构建诊断 Plan）。
#[derive(Debug, Clone, Default)]
pub struct MergeCounts {
    pub local: usize,
    pub remote: usize,
    pub merged: usize,
    pub deleted: usize,
}

/// 诊断 Plan 封顶条数（同步路径默认不构建；诊断/调试时最多保留前 N 条）。
pub const PLAN_CAP: usize = 500;

/// 单条目合并决策（同步路径计数用，避免深比较）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeDecision {
    Unchanged,
    Local,
    Remote,
    Merged,
    Deleted,
}

/// 本地/远端是否相对 base 未变（数据 + 时钟一致）。
fn same(a: Option<&SyncEntry>, b: Option<&SyncEntry>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            x.deleted == y.deleted && x.updated_at == y.updated_at && x.data == y.data
        }
        _ => false,
    }
}

/// LWW：updated_at 大者胜；平局墓碑胜（防复活）；都存活平局取 local（确定性）。
fn lww(local: &SyncEntry, remote: &SyncEntry) -> SyncEntry {
    if local.updated_at > remote.updated_at {
        local.clone()
    } else if remote.updated_at > local.updated_at {
        remote.clone()
    } else if local.deleted && !remote.deleted {
        local.clone()
    } else if remote.deleted && !local.deleted {
        remote.clone()
    } else {
        local.clone()
    }
}

/// 卷/话"只增不减"：一侧为空、另一侧非空时，把非空值补给空的一侧。
///
/// 等价于"空串 = 没有信息"（而不是"清空"），与同步/整包两处落库守卫
/// （`CASE WHEN excluded.<col> = '' THEN ...`）保持同一语义。
fn adopt_sequence(
    target: &mut serde_json::Map<String, Value>,
    source: &serde_json::Map<String, Value>,
) {
    for key in ["volume", "chapter"] {
        let target_empty = target
            .get(key)
            .and_then(Value::as_str)
            .map(|v| v.trim().is_empty())
            .unwrap_or(true);
        if !target_empty {
            continue;
        }
        if let Some(value) = source.get(key).and_then(Value::as_str) {
            if !value.trim().is_empty() {
                target.insert(key.to_string(), Value::String(value.to_string()));
            }
        }
    }
}

/// metas 字段级合并：逐字段 local/remote 相对 base 三态；
/// 仅同字段双改才 LWW（按条目 updated_at，平局取 local）。
fn merge_metas(
    base: Option<&SyncEntry>,
    local: Option<&SyncEntry>,
    remote: Option<&SyncEntry>,
) -> Option<SyncEntry> {
    let l = local?;
    let r = remote?;
    if l.deleted || r.deleted {
        return Some(lww(l, r));
    }
    // 没有 base（首次配对、或两端都已存在同一本书）时**不能整条丢弃**：
    // 被丢弃的条目既不会进入 `merged`，`advance_base` 也就永远不会为它建立 base
    // ⇒ 该 key 永久不收敛（实测：同一本书两端都有行时，title/author/series/卷/话
    // 一个都过不去。第132轮独立评审 Important）。退化为整条 LWW：
    // updated_at 大者胜、平局取 local；下一轮就有 base 可做字段级三方合并。
    let Some(b) = base else {
        return Some(lww(l, r));
    };
    let b_obj = b.data.as_object().cloned().unwrap_or_default();
    let l_obj = l.data.as_object().cloned().unwrap_or_default();
    let r_obj = r.data.as_object().cloned().unwrap_or_default();

    let mut keys: Vec<&String> = b_obj
        .keys()
        .chain(l_obj.keys())
        .chain(r_obj.keys())
        .collect();
    keys.sort();
    keys.dedup();

    let mut merged = l_obj.clone();
    for k in keys {
        let bv = b_obj.get(k);
        let lv = l_obj.get(k);
        let rv = r_obj.get(k);
        if lv == bv {
            // local 未改 → 采用 remote（若 remote 有值）
            if let Some(rv) = rv {
                merged.insert(k.clone(), rv.clone());
            }
        } else if rv == bv {
            // remote 未改 → 保持 local（merged 初始即为 local）
        } else if r.updated_at > l.updated_at {
            // 同字段双改 → LWW（平局保持 local）
            merged.insert(k.clone(), rv.cloned().unwrap_or(Value::Null));
        }
    }
    Some(SyncEntry {
        key: l.key.clone(),
        updated_at: r.updated_at.max(l.updated_at),
        deleted: false,
        data: Value::Object(merged),
    })
}

/// 双端都变化时的实体策略入口。
fn merge_both(
    entity: &str,
    base: Option<&SyncEntry>,
    local: Option<&SyncEntry>,
    remote: Option<&SyncEntry>,
) -> Option<SyncEntry> {
    match entity {
        base::ENTITY_METAS => merge_metas(base, local, remote),
        _ => {
            let l = local?;
            let r = remote?;
            Some(lww(l, r))
        }
    }
}

/// 单条目三方比较。
pub fn three_way(
    entity: &str,
    base: Option<&SyncEntry>,
    local: Option<&SyncEntry>,
    remote: Option<&SyncEntry>,
) -> (Option<SyncEntry>, MergeDecision) {
    // 卷/话在落库侧"空值不覆盖"⇒ 对端的空值在语义上**不构成变更**。若只在结果上对齐、
    // 不在这里对齐，每轮都会判"远端已改"而重推（revision 无谓增长）。
    let aligned_remote = align_incoming_sequence(entity, remote, local);
    let remote = aligned_remote.as_ref().or(remote);
    let local_changed = !same(base, local);
    let remote_changed = !same(base, remote);
    let (result, decision) = match (local_changed, remote_changed) {
        (false, false) => (None, MergeDecision::Unchanged),
        (true, false) => match local {
            Some(l) => (Some(l.clone()), MergeDecision::Local),
            // 本地删除：仅当远端仍存活才产出墓碑（远端已删/墓碑则无需动作）
            None => match remote {
                Some(r) if !r.deleted => (
                    Some(SyncEntry::tombstone(
                        &r.key,
                        base.map(|b| b.updated_at).unwrap_or(0),
                        r.data.clone(),
                    )),
                    MergeDecision::Deleted,
                ),
                _ => (None, MergeDecision::Unchanged),
            },
        },
        (false, true) => match remote {
            Some(r) => (Some(r.clone()), MergeDecision::Remote),
            // 远端删除：本地仍存活 → 产出生墓碑应用本地删除
            None => match local {
                Some(l) if !l.deleted => (
                    Some(SyncEntry::tombstone(
                        &l.key,
                        base.map(|b| b.updated_at).unwrap_or(0),
                        l.data.clone(),
                    )),
                    MergeDecision::Deleted,
                ),
                _ => (None, MergeDecision::Unchanged),
            },
        },
        (true, true) => {
            let result = merge_both(entity, base, local, remote);
            let decision = match &result {
                Some(e) if e.deleted => MergeDecision::Deleted,
                Some(_) => MergeDecision::Merged,
                None => MergeDecision::Deleted,
            };
            (result, decision)
        }
    };
    // 决策结果必须反映"实际会落库的状态"，否则 base 与库内值不一致会引发每轮重推。
    (
        align_persisted_sequence(entity, result, local),
        decision,
    )
}

/// 判定前把对端的卷/话对齐到"本机非空值"；无需对齐时返回 `None`（调用方沿用原引用）。
///
/// 与 `align_persisted_sequence` 的区别：那个保证**结果**等于落库后的状态；这个保证
/// **变更判定**不会把"对端空值"当成一次远端修改，否则 base 每轮都对不上、周期重推。
fn align_incoming_sequence(
    entity: &str,
    remote: Option<&SyncEntry>,
    local: Option<&SyncEntry>,
) -> Option<SyncEntry> {
    if entity != base::ENTITY_METAS {
        return None;
    }
    let (remote, local) = (remote?, local?);
    if remote.deleted || local.deleted {
        return None;
    }
    let mut aligned = remote.clone();
    let local_obj = local.data.as_object()?;
    let obj = aligned.data.as_object_mut()?;
    let before = obj.clone();
    adopt_sequence(obj, local_obj);
    if *obj == before {
        return None;
    }
    Some(aligned)
}

/// 把合并结果对齐到**实际落库后的状态**。
///
/// 卷/话在两条落库路径上都有"空值不覆盖"守卫（对端为空时保留本机值），所以当合并结果
/// （可能来自"整条采用远端"分支）里这两列为空、而本机非空时，必须回填成本机值：
/// 否则 `advance_base` 会把空串记进 base，与本机落库值不一致 ⇒ 下一轮判"本地已改"，
/// 每轮重推该条目、revision 无谓增长（第132轮独立评审 Important 2）。
fn align_persisted_sequence(
    entity: &str,
    result: Option<SyncEntry>,
    local: Option<&SyncEntry>,
) -> Option<SyncEntry> {
    if entity != base::ENTITY_METAS {
        return result;
    }
    match (result, local) {
        (Some(mut merged), Some(local)) if !merged.deleted => {
            if let (Some(obj), Some(local_obj)) = (
                merged.data.as_object_mut(),
                local.data.as_object(),
            ) {
                adopt_sequence(obj, local_obj);
            }
            Some(merged)
        }
        (other, _) => other,
    }
}

fn plan_meta(decision: MergeDecision) -> (String, String, String) {
    let (decision, winner, reason) = match decision {
        MergeDecision::Unchanged => ("unchanged", "none", "双方均无变化"),
        MergeDecision::Deleted => ("deleted", "none", "删除传播"),
        MergeDecision::Local => ("local", "local", "本机修改，远端未动"),
        MergeDecision::Remote => ("remote", "remote", "远端修改，本机未动"),
        MergeDecision::Merged => ("merged", "merged", "双方修改，已合并"),
    };
    (decision.into(), winner.into(), reason.into())
}

fn plan_revision(e: Option<&SyncEntry>) -> Option<i64> {
    e.map(|x| x.updated_at)
}

fn entry_value(e: &SyncEntry) -> Value {
    serde_json::to_value(e).unwrap_or_else(|_| json!({}))
}

pub fn plan_item(
    entity: &str,
    key: &str,
    base: Option<&SyncEntry>,
    local: Option<&SyncEntry>,
    remote: Option<&SyncEntry>,
    result: &Option<SyncEntry>,
    decision: MergeDecision,
) -> SyncPlanItem {
    let (decision, winner, reason) = plan_meta(decision);
    SyncPlanItem {
        entity: entity.to_string(),
        key: key.to_string(),
        base: base.map(entry_value),
        local: local.map(entry_value),
        remote: remote.map(entry_value),
        local_revision: plan_revision(local),
        remote_revision: plan_revision(remote),
        decision,
        winner,
        reason,
        result: result.as_ref().map(entry_value),
    }
}

/// 整实体批量合并（key 并集，排序保证确定性）。
pub fn merge_batch(
    entity: &str,
    base: &HashMap<String, SyncEntry>,
    local: &HashMap<String, SyncEntry>,
    remote: &HashMap<String, SyncEntry>,
    with_plan: bool,
) -> MergeResult {
    let mut keys: Vec<&String> = base
        .keys()
        .chain(local.keys())
        .chain(remote.keys())
        .collect();
    keys.sort();
    keys.dedup();

    let mut merged = Vec::new();
    let mut plan = Vec::new();
    let mut counts = MergeCounts::default();
    for k in keys {
        let b = base.get(k);
        let l = local.get(k);
        let r = remote.get(k);
        let (merged_entry, decision) = three_way(entity, b, l, r);
        match decision {
            MergeDecision::Unchanged => {}
            MergeDecision::Deleted => counts.deleted += 1,
            MergeDecision::Local => counts.local += 1,
            MergeDecision::Remote => counts.remote += 1,
            MergeDecision::Merged => counts.merged += 1,
        }
        if with_plan && plan.len() < PLAN_CAP {
            plan.push(plan_item(entity, k, b, l, r, &merged_entry, decision));
        }
        if let Some(e) = merged_entry {
            merged.push(e);
        }
    }
    MergeResult {
        merged,
        plan,
        counts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn metas(key: &str, rev: i64, fields: Value) -> SyncEntry {
        SyncEntry::live(key, rev, fields)
    }

    fn map(entries: Vec<SyncEntry>) -> HashMap<String, SyncEntry> {
        entries.into_iter().map(|e| (e.key.clone(), e)).collect()
    }

    #[test]
    fn unchanged_both_sides_yields_nothing() {
        let b = metas("k", 1, json!({"title": "A"}));
        assert!(three_way(base::ENTITY_METAS, Some(&b), Some(&b), Some(&b))
            .0
            .is_none());
        assert!(three_way(base::ENTITY_METAS, None, None, None).0.is_none());
    }

    #[test]
    fn local_only_and_remote_only() {
        let l = metas("k", 1, json!({"title": "A"}));
        let r = metas("k", 1, json!({"title": "B"}));
        assert_eq!(
            three_way(base::ENTITY_METAS, None, Some(&l), None).0,
            Some(l.clone())
        );
        assert_eq!(
            three_way(base::ENTITY_METAS, None, None, Some(&r)).0,
            Some(r.clone())
        );
    }

    #[test]
    fn metas_different_fields_merge() {
        let b = metas("k", 10, json!({"read": false, "rating": 4}));
        let l = metas("k", 11, json!({"read": true, "rating": 4}));
        let r = metas("k", 12, json!({"read": false, "rating": 5}));
        let merged = three_way(base::ENTITY_METAS, Some(&b), Some(&l), Some(&r))
            .0
            .unwrap();
        assert_eq!(merged.data, json!({"read": true, "rating": 5}));
    }

    #[test]
    fn metas_sequence_fields_merge_like_any_other_field() {
        // 卷/话是载荷里的普通字段 ⇒ 远端改了号码、本地未改时必须采用远端，
        // 否则"号码能跨设备显示"就不成立（第132轮把两列接进载荷后的关键一步）。
        let b = metas("k", 10, json!({"title": "A", "volume": "1", "chapter": "7"}));
        let l = metas("k", 10, json!({"title": "A", "volume": "1", "chapter": "7"}));
        let r = metas("k", 20, json!({"title": "A", "volume": "2", "chapter": "8"}));
        let merged = three_way(base::ENTITY_METAS, Some(&b), Some(&l), Some(&r))
            .0
            .unwrap();
        assert_eq!(merged.data["volume"], json!("2"));
        assert_eq!(merged.data["chapter"], json!("8"));
    }

    #[test]
    fn metas_without_base_converge_by_lww_instead_of_being_dropped() {
        // 首次配对：两端都有同一本书、尚无 base。旧实现整条返回 None（判 Deleted）
        // ⇒ 条目既不进 merged、`advance_base` 也永远建不起 base ⇒ 该 key 永久不收敛
        // （title/author/series/卷/话全都过不去）。第132轮评审 Important。
        let l = metas("k", 10, json!({"title": "本地", "volume": "1", "chapter": "7"}));
        let r = metas("k", 20, json!({"title": "远端", "volume": "2", "chapter": "8"}));
        let merged = three_way(base::ENTITY_METAS, None, Some(&l), Some(&r))
            .0
            .expect("无 base 时也必须产出条目，否则永远建不起 base");
        assert_eq!(merged.data["title"], json!("远端"));
        assert_eq!(merged.data["chapter"], json!("8"));

        // 平局取 local（确定性）
        let l2 = metas("k", 30, json!({"title": "本地"}));
        let r2 = metas("k", 30, json!({"title": "远端"}));
        let merged2 = three_way(base::ENTITY_METAS, None, Some(&l2), Some(&r2))
            .0
            .unwrap();
        assert_eq!(merged2.data["title"], json!("本地"));
    }

    #[test]
    fn metas_sequence_empty_side_is_not_a_clear() {
        // 对端（旧版本）载荷没有卷/话 ⇒ 空串不得清掉本机值；且结果必须仍是"本机值"，
        // 否则 base 会记成空串、每轮重推（跨版本对端 revision 无谓增长）。
        let b = metas("k", 10, json!({"title": "A", "volume": "1", "chapter": "7"}));
        let l = metas("k", 10, json!({"title": "A", "volume": "1", "chapter": "7"}));
        let r = metas("k", 20, json!({"title": "A"}));
        let merged = three_way(base::ENTITY_METAS, Some(&b), Some(&l), Some(&r))
            .0
            .unwrap();
        assert_eq!(merged.data["volume"], json!("1"));
        assert_eq!(merged.data["chapter"], json!("7"));

        // 反向：本机空、对端有值 ⇒ 采纳对端（号码能过来）
        let b2 = metas("k", 10, json!({"title": "A", "volume": "", "chapter": ""}));
        let l2 = metas("k", 10, json!({"title": "A", "volume": "", "chapter": ""}));
        let r2 = metas("k", 20, json!({"title": "A", "volume": "2", "chapter": "8"}));
        let merged2 = three_way(base::ENTITY_METAS, Some(&b2), Some(&l2), Some(&r2))
            .0
            .unwrap();
        assert_eq!(merged2.data["volume"], json!("2"));
        assert_eq!(merged2.data["chapter"], json!("8"));
    }

    #[test]
    fn metas_same_field_conflict_lww() {
        let b = metas("k", 10, json!({"title": "A"}));
        let l = metas("k", 11, json!({"title": "Local"}));
        let r = metas("k", 12, json!({"title": "Remote"}));
        let merged = three_way(base::ENTITY_METAS, Some(&b), Some(&l), Some(&r))
            .0
            .unwrap();
        assert_eq!(merged.data["title"], json!("Remote"));
        let l2 = metas("k", 12, json!({"title": "Local"}));
        let merged2 = three_way(base::ENTITY_METAS, Some(&b), Some(&l2), Some(&r))
            .0
            .unwrap();
        assert_eq!(merged2.data["title"], json!("Local"));
    }

    #[test]
    fn tags_union_and_tombstone() {
        let base = HashMap::new();
        let local = map(vec![SyncEntry::live("收藏", 1, json!({"name": "收藏"}))]);
        let remote = map(vec![SyncEntry::live("神作", 1, json!({"name": "神作"}))]);
        let res = merge_batch(base::ENTITY_TAGS, &base, &local, &remote, true);
        let keys: Vec<&str> = res.merged.iter().map(|e| e.key.as_str()).collect();
        assert!(keys.contains(&"收藏") && keys.contains(&"神作"));

        let b = SyncEntry::live("日漫", 10, json!({"name": "日漫"}));
        let del = SyncEntry::tombstone("日漫", 11, json!({"name": "日漫"}));
        let upd = SyncEntry::live("日漫", 11, json!({"name": "日漫"}));
        let m = three_way(base::ENTITY_TAGS, Some(&b), Some(&del), Some(&upd))
            .0
            .unwrap();
        assert!(m.deleted);
    }

    #[test]
    fn records_lww_and_delete() {
        let b = SyncEntry::live("bk", 10, json!({"lastPage": 5}));
        let l = SyncEntry::live("bk", 11, json!({"lastPage": 20}));
        let r = SyncEntry::live("bk", 9, json!({"lastPage": 99}));
        let m = three_way(base::ENTITY_RECORDS, Some(&b), Some(&l), Some(&r))
            .0
            .unwrap();
        assert_eq!(m.data["lastPage"], json!(20));

        let del = SyncEntry::tombstone("bk", 10, json!({"path": "/a.cbz"}));
        let upd = SyncEntry::live("bk", 12, json!({"lastPage": 30}));
        let m2 = three_way(base::ENTITY_RECORDS, Some(&b), Some(&del), Some(&upd))
            .0
            .unwrap();
        assert!(!m2.deleted);
        assert_eq!(m2.data["lastPage"], json!(30));

        let del2 = SyncEntry::tombstone("bk", 13, json!({"path": "/a.cbz"}));
        let m3 = three_way(base::ENTITY_RECORDS, Some(&b), Some(&del), Some(&del2))
            .0
            .unwrap();
        assert!(m3.deleted);
    }

    #[test]
    fn remote_deletion_propagates_tombstone() {
        let b = SyncEntry::live("bk", 10, json!({"path": "/a.cbz", "title": "A"}));
        let l = SyncEntry::live("bk", 10, json!({"path": "/a.cbz", "title": "A"}));
        // remote 删除（缺席），本地未变 → 产出墓碑供本地删除
        let m = three_way(base::ENTITY_RECORDS, Some(&b), Some(&l), None)
            .0
            .unwrap();
        assert!(m.deleted);
        assert_eq!(m.data["path"], json!("/a.cbz"));
        // 远端已删且本地也已删 → 无动作
        assert!(three_way(base::ENTITY_RECORDS, Some(&b), None, None)
            .0
            .is_none());
    }

    #[test]
    fn local_deletion_pushes_tombstone_when_remote_live() {
        let b = SyncEntry::live("bk", 10, json!({"path": "/a.cbz"}));
        let r = SyncEntry::live("bk", 10, json!({"path": "/a.cbz"}));
        let m = three_way(base::ENTITY_RECORDS, Some(&b), None, Some(&r))
            .0
            .unwrap();
        assert!(m.deleted);
    }

    #[test]
    fn library_index_single_side_and_both_lww() {
        let b = SyncEntry::live("idx", 10, json!({"name": "a.cbz", "size": 100}));
        let l = SyncEntry::live("idx", 11, json!({"name": "a.cbz", "size": 101}));
        let r = SyncEntry::live("idx", 9, json!({"name": "a.cbz", "size": 999}));
        let m = three_way(base::ENTITY_LIBRARY_INDEX, Some(&b), Some(&l), Some(&r))
            .0
            .unwrap();
        assert_eq!(m.data["size"], json!(101));
        let m2 = three_way(base::ENTITY_LIBRARY_INDEX, Some(&b), Some(&l), Some(&b))
            .0
            .unwrap();
        assert_eq!(m2.data["size"], json!(101));
    }

    #[test]
    fn settings_lww() {
        let b = SyncEntry::live("themeMode", 10, json!({"value": "dark"}));
        let l = SyncEntry::live("themeMode", 11, json!({"value": "light"}));
        let r = SyncEntry::live("themeMode", 12, json!({"value": "dark"}));
        let m = three_way(base::ENTITY_SETTINGS, Some(&b), Some(&l), Some(&r))
            .0
            .unwrap();
        assert_eq!(m.data["value"], json!("dark"));
    }

    #[test]
    fn plan_records_decisions() {
        let base = map(vec![SyncEntry::live("k", 1, json!({"v": 1}))]);
        let local = map(vec![SyncEntry::live("k", 2, json!({"v": 2}))]);
        // 远端保留 k（与 base 一致）并新增 k2 → k 为 local 胜，k2 为 remote 胜
        let remote = map(vec![
            SyncEntry::live("k", 1, json!({"v": 1})),
            SyncEntry::live("k2", 2, json!({"v": 9})),
        ]);
        let res = merge_batch(base::ENTITY_RECORDS, &base, &local, &remote, true);
        assert_eq!(res.plan.len(), 2);
        let by_key: HashMap<&str, &SyncPlanItem> =
            res.plan.iter().map(|p| (p.key.as_str(), p)).collect();
        assert_eq!(by_key["k"].decision, "local");
        assert_eq!(by_key["k2"].decision, "remote");
    }
}
