//! WebDAV Sync State 模型（ADR-024 §2）。
//!
//! manifest 是唯一提交点；状态文件按 revision 版本化（`state/<entity>-<rev>.jsonl|json`），
//! 读取端只信任 manifest 引用的文件。history 记录旧版本文件名，供修剪。

use std::collections::HashMap;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::db;

/// 同步协议版本（v3：新增 writer 参与者身份，ADR-026）。
pub const SYNC_SCHEMA_VERSION: i64 = 3;
pub const MANIFEST_FILE: &str = "manifest.json";
pub const STATE_DIR: &str = "state";
pub const DEVICES_DIR: &str = "devices";
/// 保留的旧版本数（异常恢复用）。
pub const KEEP_REVISIONS: usize = 3;
/// history 上限（KEEP_REVISIONS × 每版本最大实体文件数，防膨胀）。
pub const HISTORY_CAP: usize = KEEP_REVISIONS * 16;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriterInfo {
    pub device_id: String,
    pub device_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema_version: i64,
    pub library_id: String,
    pub revision: i64,
    pub updated_at: i64,
    /// entity -> 该实体**最新**的版本化文件名（如 state/metas-12.jsonl）。
    /// ADR-028：未变化的实体沿用旧文件引用（全量引用语义），
    /// 绝不为未变化实体生成空文件。
    pub files: HashMap<String, String>,
    /// 已被替换的旧版本文件名（按写入顺序），供修剪；不含当前 files 引用的文件。
    pub history: Vec<String>,
    /// 写者身份（revision 元数据，不参与合并；v1/v2 旧包读取时为 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer: Option<WriterInfo>,
}

impl Manifest {
    pub fn new(library_id: &str, revision: i64) -> Self {
        Self {
            schema_version: SYNC_SCHEMA_VERSION,
            library_id: library_id.to_string(),
            revision,
            updated_at: db::now_ms(),
            files: HashMap::new(),
            history: Vec::new(),
            writer: None,
        }
    }

    /// 生成下一 revision 的 manifest（files 为本次新写的版本化文件）。
    pub fn bump(&self, files: HashMap<String, String>, writer: Option<WriterInfo>) -> Self {
        let mut history: Vec<String> = self.files.values().cloned().collect();
        history.extend(self.history.iter().cloned());
        if history.len() > HISTORY_CAP {
            history.drain(0..history.len() - HISTORY_CAP);
        }
        Self {
            schema_version: SYNC_SCHEMA_VERSION,
            library_id: self.library_id.clone(),
            revision: self.revision + 1,
            updated_at: db::now_ms(),
            files,
            history,
            writer,
        }
    }

    /// 生成新 revision 的 manifest（ADR-028 全量引用）。
    ///
    /// - `changed`：本轮有变化的实体 -> 新文件名；未变化实体沿用 `prev.files` 的引用。
    /// - 只有被替换的旧文件名进入 history（未变化实体的文件永不进入 history，
    ///   因此也不会被修剪）。
    pub fn push(
        library_id: &str,
        revision: i64,
        changed: HashMap<String, String>,
        prev: Option<&Manifest>,
        writer: Option<WriterInfo>,
    ) -> Self {
        let mut files = prev.map(|m| m.files.clone()).unwrap_or_default();
        let mut history: Vec<String> = prev.map(|m| m.history.clone()).unwrap_or_default();
        for (entity, name) in changed {
            if let Some(old) = files.insert(entity, name) {
                history.push(old);
            }
        }
        if history.len() > HISTORY_CAP {
            history.drain(0..history.len() - HISTORY_CAP);
        }
        Self {
            schema_version: SYNC_SCHEMA_VERSION,
            library_id: library_id.to_string(),
            revision,
            updated_at: db::now_ms(),
            files,
            history,
            writer,
        }
    }

    /// 应删除的旧版本文件名（history 中超出保留数量的部分，从最旧开始）。
    ///
    /// ADR-028：**当前 files 仍引用的文件绝不修剪**（未变化实体的全量引用
    /// 可能指向很旧的 revision 文件，一旦被剪掉，拉取端将 404）。
    pub fn prune_targets(&self) -> Vec<String> {
        let keep = KEEP_REVISIONS * 16;
        if self.history.len() <= keep {
            return Vec::new();
        }
        self.history[..self.history.len() - keep]
            .iter()
            .filter(|n| !self.files.values().any(|f| f == *n))
            .cloned()
            .collect()
    }
}

/// 实体文件名：`state/<entity>-<rev>.json|jsonl`。
pub fn state_file_name(entity: &str, revision: i64, jsonl: bool) -> String {
    let ext = if jsonl { "jsonl" } else { "json" };
    format!("{STATE_DIR}/{entity}-{revision}.{ext}")
}

/// 实体内容 hash（sha256 hex；远端状态完整性/变化判断）。
pub fn entity_hash(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    let out = h.finalize();
    let mut s = String::with_capacity(64);
    for b in out {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 校验 manifest：schema 不识别安全拒绝；revision 合法。
pub fn verify_manifest(m: &Manifest) -> Result<()> {
    if m.schema_version > SYNC_SCHEMA_VERSION {
        bail!(
            "同步协议版本 {} 高于当前支持版本 {}，请升级应用",
            m.schema_version,
            SYNC_SCHEMA_VERSION
        );
    }
    if m.revision < 0 {
        bail!("manifest revision 非法: {}", m.revision);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_with(rev: i64) -> Manifest {
        Manifest::new("lib-1", rev)
    }

    #[test]
    fn bump_increments_revision_and_tracks_history() {
        let mut m = manifest_with(1);
        m.files
            .insert("metas".into(), state_file_name("metas", 1, true));
        let m2 = m.bump(
            HashMap::from([("metas".into(), state_file_name("metas", 2, true))]),
            None,
        );
        assert_eq!(m2.revision, 2);
        assert_eq!(m2.files["metas"], state_file_name("metas", 2, true));
        assert!(m2.history.contains(&state_file_name("metas", 1, true)));
        assert_eq!(m2.library_id, "lib-1");
    }

    #[test]
    fn history_is_capped() {
        let mut m = manifest_with(0);
        for i in 0..(HISTORY_CAP + 10) {
            let files = HashMap::from([("metas".into(), state_file_name("metas", i as i64, true))]);
            m = m.bump(files, None);
        }
        assert!(m.history.len() <= HISTORY_CAP);
    }

    #[test]
    fn prune_targets_drops_oldest_beyond_keep() {
        let mut m = manifest_with(100);
        m.history = (0..(KEEP_REVISIONS * 16 + 5) as i64)
            .map(|i| state_file_name("metas", i, true))
            .collect();
        let targets = m.prune_targets();
        assert_eq!(targets.len(), 5);
        assert!(targets[0].ends_with("-0.jsonl"));
    }

    #[test]
    fn push_preserves_unchanged_references() {
        let mut prev = manifest_with(21);
        prev.files
            .insert("sources".into(), state_file_name("sources", 19, true));
        prev.files
            .insert("metas".into(), state_file_name("metas", 21, true));

        // 本轮只改了 metas：sources 沿用旧引用，不产生新文件。
        let changed = HashMap::from([("metas".into(), state_file_name("metas", 22, true))]);
        let m = Manifest::push("lib-1", 22, changed, Some(&prev), None);

        assert_eq!(m.files["sources"], state_file_name("sources", 19, true));
        assert_eq!(m.files["metas"], state_file_name("metas", 22, true));
        assert!(m.history.contains(&state_file_name("metas", 21, true)));
        assert!(!m.history.contains(&state_file_name("sources", 19, true)));
        assert_eq!(m.revision, 22);
    }

    #[test]
    fn prune_never_drops_referenced_files() {
        // sources 长时间未变：其文件一直留在 history 中且被当前 manifest 引用。
        let mut m = manifest_with(100);
        let referenced = state_file_name("sources", 1, true);
        m.files.insert("sources".into(), referenced.clone());
        m.history = (0..(KEEP_REVISIONS * 16 + 10) as i64)
            .map(|i| state_file_name("metas", i, true))
            .collect();
        m.history.push(referenced.clone());
        let targets = m.prune_targets();
        assert!(!targets.contains(&referenced));
        // 被引用的文件即使处于"最旧"区间也不能删。
        m.history.insert(0, referenced.clone());
        let targets2 = m.prune_targets();
        assert!(!targets2.contains(&referenced));
    }

    #[test]
    fn entity_hash_stable_and_distinct() {
        assert_eq!(entity_hash(b"abc"), entity_hash(b"abc"));
        assert_ne!(entity_hash(b"abc"), entity_hash(b"abd"));
    }

    #[test]
    fn verify_rejects_future_schema() {
        let m = Manifest {
            schema_version: SYNC_SCHEMA_VERSION + 1,
            writer: None,
            ..manifest_with(1)
        };
        assert!(verify_manifest(&m).is_err());
        assert!(verify_manifest(&manifest_with(1)).is_ok());
    }
}
