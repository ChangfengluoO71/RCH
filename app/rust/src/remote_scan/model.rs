use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteAssetKind { ArchiveFile, ImageFile, ImageFolder, ContainerDir, PlainDir, Other }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteEntry {
    pub name: String, pub logical_path: String, pub is_dir: bool,
    pub size: Option<u64>, pub mtime: Option<i64>, pub asset_kind: RemoteAssetKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteScanStatus { Idle, Running, Succeeded, Failed }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteScanMode { Snapshot, Incremental }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteScanState { pub source_id: String, pub status: RemoteScanStatus, pub mode: RemoteScanMode, pub generation: i64, pub checkpoint: Option<String>, pub last_success_at: Option<i64>, pub error_code: Option<String> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteCoverDependency { pub book_key: String, pub dependency_path: String, pub dependency_fingerprint: String, pub profile: String, pub status: String }

pub fn normalize_path(path: &str) -> String { let mut s = path.trim().replace('\\', "/"); while s.contains("//") { s = s.replace("//", "/"); } if s.len() > 1 { s = s.trim_end_matches('/').to_string(); } s }
pub fn classify(name: &str, is_dir: bool) -> RemoteAssetKind {
    if name.starts_with('.') || name.starts_with("~$") { return RemoteAssetKind::Other; }
    let n = name.to_ascii_lowercase();
    if is_dir { return RemoteAssetKind::PlainDir; }
    if ["cbz","zip","cbr","rar","cb7","7z","cbt","tar","epub","pdf","mobi","azw","azw3"].iter().any(|x| n.ends_with(&format!(".{x}"))) { RemoteAssetKind::ArchiveFile }
    else if ["jpg","jpeg","png","webp","gif","bmp"].iter().any(|x| n.ends_with(&format!(".{x}"))) { RemoteAssetKind::ImageFile } else { RemoteAssetKind::Other }
}
pub fn classify_directory(children: &[RemoteEntry]) -> RemoteAssetKind {
    if children.iter().any(|e| matches!(e.asset_kind, RemoteAssetKind::ArchiveFile)) { RemoteAssetKind::ContainerDir }
    else if children.iter().any(|e| matches!(e.asset_kind, RemoteAssetKind::ImageFile)) { RemoteAssetKind::ImageFolder }
    else { RemoteAssetKind::PlainDir }
}
pub fn natural_sort_key(name: &str) -> Vec<String> {
    let mut out=Vec::new(); let mut cur=String::new(); let mut digit=false;
    for c in name.chars() { let d=c.is_ascii_digit(); if d != digit && !cur.is_empty() { out.push(if digit { format!("#{:020}",cur.parse::<u64>().unwrap_or(0)) } else { cur.to_ascii_lowercase() }); cur.clear(); } digit=d; cur.push(c); }
    if !cur.is_empty() { out.push(if digit { format!("#{:020}",cur.parse::<u64>().unwrap_or(0)) } else { cur.to_ascii_lowercase() }); } out
}
pub fn fingerprint(entries: &[RemoteEntry]) -> String {
    let mut rows: Vec<String> = entries.iter().filter(|e| !e.name.starts_with('.') && !e.name.starts_with("~$")).map(|e| format!("{}|{}|{}|{}|{:?}", normalize_path(&e.logical_path), e.name, e.is_dir, e.size.unwrap_or(0), e.mtime.unwrap_or(0))).collect(); rows.sort(); let mut h=Sha256::new(); h.update(rows.join("\n")); format!("{:x}",h.finalize())
}
#[cfg(test)] mod tests { use super::*; #[test] fn normalized_fingerprint_stable(){ let a=RemoteEntry{name:"x.jpg".into(),logical_path:"/a/".into(),is_dir:false,size:Some(1),mtime:Some(2),asset_kind:RemoteAssetKind::ImageFile}; let mut b=a.clone(); b.logical_path="\\a".into(); assert_eq!(fingerprint(&[a]),fingerprint(&[b])); } }
