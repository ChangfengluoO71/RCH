//! WebDAV Sync State 传输（ADR-024 §2/§17）。
//!
//! 复用 `source::webdav::WebDavClient`（书源与同步共用同一会话模型）。
//! 协议要点：
//! - manifest.json 是唯一提交点：先写**本轮变化实体**的版本化状态文件，再直接 PUT manifest。
//! - 版本化文件（`state/<entity>-<rev>.*`）新文件名直接 PUT，天然避免半覆盖；
//!   未变化实体不写文件，沿用旧引用（ADR-028）。
//! - CAS：写入前校验远端 revision（expected）；冲突返回错误，由编排层重拉重并。

use std::collections::HashMap;

use anyhow::{anyhow, bail, Context, Result};

use crate::source::webdav::WebDavClient;
use crate::sync::actor::DeviceFile;
use crate::sync::state::{self, verify_manifest, Manifest};

fn is_not_found_err(e: &anyhow::Error) -> bool {
    let s = format!("{e:#}");
    s.contains("404") || s.contains("文件不存在") || s.contains("路径不存在")
}

fn join(dir: &str, name: &str) -> String {
    let d = dir.trim_matches('/');
    if d.is_empty() {
        name.to_string()
    } else {
        format!("{d}/{name}")
    }
}

/// 确保远端同步目录结构存在（base / state / devices；MKCOL 幂等）。
/// 坚果云等服务器在父目录不存在时 PUT 返回 409，必须先建目录。
fn ensure_dirs(client: &WebDavClient, dir: &str) -> Result<()> {
    client.make_dir(dir)?;
    client.make_dir(&join(dir, state::STATE_DIR))?;
    client.make_dir(&join(dir, state::DEVICES_DIR))?;
    Ok(())
}

/// 读取远端 manifest；未初始化（404）返回 None。
pub fn read_manifest(client: &WebDavClient, dir: &str) -> Result<Option<Manifest>> {
    let path = join(dir, state::MANIFEST_FILE);
    match client.download_file(&path) {
        Ok(bytes) => {
            let m: Manifest = serde_json::from_slice(&bytes).context("解析远端 manifest 失败")?;
            verify_manifest(&m)?;
            Ok(Some(m))
        }
        Err(e) if is_not_found_err(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

/// 读取远端完整状态：(manifest, entity -> bytes)。
pub fn download_state(
    client: &WebDavClient,
    dir: &str,
) -> Result<Option<(Manifest, HashMap<String, Vec<u8>>)>> {
    let Some(manifest) = read_manifest(client, dir)? else {
        return Ok(None);
    };
    let mut files = HashMap::new();
    for (entity, name) in &manifest.files {
        let data = client
            .download_file(&join(dir, name))
            .with_context(|| format!("下载远端状态文件 {name} 失败"))?;
        // ADR-028：新协议下被引用的状态文件绝不可能是空的（实体清空 = 墓碑条目）。
        // 空文件只可能来自旧版 bug（把未变化实体写成 0 字节）——直接报错阻止
        // "远端为空 → 全量墓碑"灾难，提示用户重置同步目录，而不是静默清库。
        if data.is_empty() {
            bail!(
                "远端状态文件 {name} 为空（疑似旧版残留或损坏）；请先重置同步目录（换 library_id 或清空远端），再重新同步"
            );
        }
        files.insert(entity.clone(), data);
    }
    Ok(Some((manifest, files)))
}

/// 写入远端状态（CAS）。
///
/// - `expected_revision`：Some(r) 要求远端当前 revision == r；None 表示初始化（远端不存在）。
/// - 顺序：版本化文件（仅 `files` 中的变化实体）→ manifest 直接 PUT → 修剪旧文件（尽力而为）。
/// - 冲突时返回错误（消息含 "revision 冲突"），编排层应重新拉取合并再试。
pub fn upload_state(
    client: &WebDavClient,
    dir: &str,
    manifest: &Manifest,
    files: &HashMap<String, Vec<u8>>,
    expected_revision: Option<i64>,
) -> Result<()> {
    ensure_dirs(client, dir)?;
    match read_manifest(client, dir)? {
        None if expected_revision.is_none() => {}
        Some(m) if expected_revision == Some(m.revision) => {}
        _ => bail!("远端状态已变化（revision 冲突），请重新合并后重试"),
    }

    for (entity, data) in files {
        let name = manifest
            .files
            .get(entity)
            .ok_or_else(|| anyhow!("manifest 缺少实体 {entity} 的文件引用"))?;
        client
            .upload_file(&join(dir, name), data)
            .with_context(|| format!("上传状态文件 {name} 失败"))?;
    }

    let manifest_bytes = serde_json::to_vec(manifest).context("序列化 manifest 失败")?;
    // 提交点：所有状态文件写完之后，manifest 直接 PUT 覆盖。
    // 坚果云等对 MOVE 的 Overwrite 支持不可靠（目标存在时 409 DuplicateName），
    // 且 manifest 体积小，原地覆盖的"半写窗口"可接受（解析失败会走重试）。
    client.upload_file(&join(dir, state::MANIFEST_FILE), &manifest_bytes)?;
    // 清理旧版本遗留的半写入 tmp 文件（尽力而为）。
    let _ = client.delete_file(&join(dir, "manifest.json.tmp"));

    for old in manifest.prune_targets() {
        let _ = client.delete_file(&join(dir, &old));
    }
    Ok(())
}

/// 读取远端 devices 目录（不存在 → 空）。
pub fn read_devices(client: &WebDavClient, dir: &str) -> Result<Vec<DeviceFile>> {
    let devices_dir = join(dir, state::DEVICES_DIR);
    let entries = match client.list(&devices_dir) {
        Ok(e) => e,
        Err(e) if is_not_found_err(&e) => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for e in entries {
        if e.is_dir || !e.name.ends_with(".json") {
            continue;
        }
        if let Ok(bytes) = client.download_file(&e.path) {
            if let Ok(f) = serde_json::from_slice::<DeviceFile>(&bytes) {
                out.push(f);
            }
        }
    }
    Ok(out)
}

/// 上传本机设备文件（每台设备只写自己的文件，直接 PUT 覆盖安全）。
pub fn upload_device_file(client: &WebDavClient, dir: &str, device: &DeviceFile) -> Result<()> {
    ensure_dirs(client, dir)?;
    let devices_dir = join(dir, state::DEVICES_DIR);
    let path = join(&devices_dir, &format!("{}.json", device.device_id));
    let bytes = serde_json::to_vec(device)?;
    client.upload_file(&path, &bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_normalizes_dir_slashes() {
        assert_eq!(join("RCH/sync", "manifest.json"), "RCH/sync/manifest.json");
        assert_eq!(
            join("/RCH/sync/", "state/x.jsonl"),
            "RCH/sync/state/x.jsonl"
        );
        assert_eq!(join("", "manifest.json"), "manifest.json");
    }

    #[test]
    fn not_found_detection() {
        let e = anyhow::anyhow!("下载失败:HTTP 404 文件不存在（可能尚未推送）");
        assert!(is_not_found_err(&e));
        let e2 = anyhow::anyhow!("下载失败:HTTP 500 服务器错误");
        assert!(!is_not_found_err(&e2));
    }
}
