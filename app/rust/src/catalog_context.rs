//! Persisted catalog context normalization for offline scraping.
//!
//! This module is intentionally limited to `library_index` rows.  It never
//! opens a file, calls a source adapter, refreshes a directory, or receives a
//! `ByteSource`.  Its job is to turn the three catalog shapes used by RCH
//! (local paths, Quark entries, and 115 entries) into one parser contract.

use std::collections::{HashMap, HashSet};

use crate::{db, scraper};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogAssetContext {
    pub asset_key: String,
    pub book_key: String,
    pub source_type: String,
    pub source_id: String,
    pub path: String,
    pub filename: String,
    /// Nearest ancestor first, matching `CatalogSnapshot` semantics.
    pub ancestor_dirs: Vec<String>,
    /// Sibling names in the same persisted parent directory.  The current
    /// asset is always excluded here, even if the catalog row is duplicated.
    pub parent_siblings: Vec<String>,
}

/// Normalize one persisted catalog file into the parser's zero-I/O context.
/// `by_id` must come from the local `library_index` snapshot already loaded by
/// the caller; this function never looks anything up outside that map.
pub fn normalize_entry(
    entry: &db::LibraryIndexRow,
    source_type: &str,
    by_id: &HashMap<String, db::LibraryIndexRow>,
    ancestor_depth: usize,
) -> CatalogAssetContext {
    let filename = basename(&entry.name, &entry.path);
    let current_filename = basename(&entry.name, &entry.path);
    let ancestors = persisted_ancestors(entry, &current_filename, by_id, ancestor_depth)
        .into_iter()
        .map(|name| basename(&name, &name))
        .filter(|name| !name.is_empty())
        .filter(|name| !is_catalog_bucket(name, source_type))
        .collect::<Vec<_>>();
    let siblings = persisted_siblings(entry, by_id)
        .into_iter()
        .map(|name| basename(&name, &name))
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();

    CatalogAssetContext {
        asset_key: db::asset_key_of(source_type, &entry.source_id, &entry.id),
        book_key: db::book_key_of(source_type, &entry.source_id, &entry.path),
        source_type: source_type.to_owned(),
        source_id: entry.source_id.clone(),
        path: entry.path.clone(),
        filename,
        ancestor_dirs: ancestors,
        parent_siblings: siblings,
    }
}

fn persisted_ancestors(
    entry: &db::LibraryIndexRow,
    current_filename: &str,
    by_id: &HashMap<String, db::LibraryIndexRow>,
    max_depth: usize,
) -> Vec<String> {
    let mut output = Vec::new();
    let mut next = entry.parent_id.clone();
    let mut seen = HashSet::new();
    while output.len() < max_depth {
        let Some(id) = next else { break };
        if !seen.insert(id.clone()) {
            break;
        }
        let Some(parent) = by_id.get(&id) else { break };
        if parent.deleted || parent.entry_type != "dir" {
            break;
        }
        let parent_name = basename(&parent.name, &parent.path);
        // Some flattened cloud catalogs persist a synthetic directory node
        // whose name is the file itself (including `.zip`). It is not work
        // context and must never outrank the filename parser.
        if parent_name.eq_ignore_ascii_case(current_filename) || has_archive_extension(&parent_name)
        {
            next = parent.parent_id.clone();
            continue;
        }
        output.push(parent.name.clone());
        next = parent.parent_id.clone();
    }
    output
}

fn persisted_siblings(
    entry: &db::LibraryIndexRow,
    by_id: &HashMap<String, db::LibraryIndexRow>,
) -> Vec<String> {
    let Some(parent_id) = entry.parent_id.as_deref() else {
        return Vec::new();
    };
    let current_filename = basename(&entry.name, &entry.path);
    let mut names = by_id
        .values()
        .filter(|candidate| {
            !candidate.deleted
                && candidate.entry_type == "file"
                && candidate.parent_id.as_deref() == Some(parent_id)
                && candidate.id != entry.id
        })
        .map(|candidate| basename(&candidate.name, &candidate.path))
        .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case(&current_filename))
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

/// Return a stable filename even when a flattened provider persisted the
/// whole path in `name`.  The path is only a local catalog string; it is not
/// inspected through a source API.
fn basename(name: &str, fallback_path: &str) -> String {
    let candidate = name.trim();
    let candidate = candidate
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(candidate)
        .trim();
    if !candidate.is_empty() {
        return candidate.to_owned();
    }
    fallback_path
        .trim()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn has_archive_extension(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    [
        ".cbz", ".zip", ".cbr", ".rar", ".cb7", ".7z", ".cbt", ".tar",
    ]
    .iter()
    .any(|extension| lower.ends_with(extension))
}

/// Category/source buckets are not work titles.  In particular 115 commonly
/// persists `日漫` as the first ancestor, while Quark may persist `漫画` or a
/// provider root.  Filtering these here prevents a weak bucket from
/// overriding a strong filename title.
fn is_catalog_bucket(value: &str, source_type: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return true;
    }
    if matches!(
        normalized.as_str(),
        "epub"
            | "pdf"
            | "cbz"
            | "cbr"
            | "zip"
            | "rar"
            | "mobi"
            | "azw"
            | "azw3"
            | "7z"
            | "漫画"
            | "日漫"
            | "国漫"
            | "韩漫"
            | "韓漫"
            | "欧美"
            | "manga"
            | "comic"
            | "comics"
            | "小说"
            | "轻小说"
            | "杂志"
            | "图集"
            | "画集"
            | "分类"
            | "分类目录"
            | "書架"
            | "书架"
            | "library"
            | "downloads"
            | "download"
            | "全部"
            | "全部文件"
            | "根目录"
            | "root"
            | "collection"
            | "collections"
            | "合集"
            | "全集"
            | "单行本"
            | "连载"
            | "番外"
            | "tankoubon"
            | "serial"
    ) {
        return true;
    }
    // Provider roots are category nodes, not release/provider attribution.
    matches!(source_type, "115" | "quark")
        && matches!(
            normalized.as_str(),
            "115" | "115网盘" | "夸克" | "夸克网盘" | "quark" | "cloud" | "云盘"
        )
}

/// Build a parser snapshot from a normalized persisted asset without exposing
/// the source-specific context object to the grammar layer.
pub fn to_snapshot(context: &CatalogAssetContext) -> scraper::CatalogSnapshot {
    scraper::CatalogSnapshot {
        book_key: context.book_key.clone(),
        filename: context.filename.clone(),
        ancestor_dirs: context.ancestor_dirs.clone(),
        parent_siblings: context.parent_siblings.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        id: &str,
        parent_id: Option<&str>,
        name: &str,
        path: &str,
        entry_type: &str,
    ) -> db::LibraryIndexRow {
        db::LibraryIndexRow {
            id: id.into(),
            source_id: "s115".into(),
            parent_id: parent_id.map(str::to_owned),
            name: name.into(),
            path: path.into(),
            entry_type: entry_type.into(),
            size: None,
            modified_at: None,
            cover_path: None,
            hash: None,
            updated_at: 1,
            deleted: false,
        }
    }

    #[test]
    fn flattened_provider_name_becomes_filename_and_bucket_is_removed() {
        let root = row("root", None, "日漫", "日漫", "dir");
        let work = row("work", Some("root"), "作品名", "日漫/作品名", "dir");
        let entry = row(
            "file",
            Some("work"),
            "日漫/作品名/[Alice Crazy] Title.zip",
            "日漫/作品名/[Alice Crazy] Title.zip",
            "file",
        );
        let siblings = row("sib", Some("work"), "10.zip", "日漫/作品名/10.zip", "file");
        let by_id = [root, work, entry.clone(), siblings]
            .into_iter()
            .map(|item| (item.id.clone(), item))
            .collect();
        let context = normalize_entry(&entry, "115", &by_id, 8);
        assert_eq!(context.filename, "[Alice Crazy] Title.zip");
        assert_eq!(context.ancestor_dirs, vec!["作品名"]);
        assert_eq!(context.parent_siblings, vec!["10.zip"]);
    }

    #[test]
    fn current_file_is_not_sibling_even_when_name_is_duplicated() {
        let parent = row("p", None, "作品", "作品", "dir");
        let entry = row("a", Some("p"), "same.zip", "作品/same.zip", "file");
        let duplicate = row("b", Some("p"), "same.zip", "作品/same.zip", "file");
        let by_id = [parent, entry.clone(), duplicate]
            .into_iter()
            .map(|item| (item.id.clone(), item))
            .collect();
        let context = normalize_entry(&entry, "local", &by_id, 8);
        assert!(context.parent_siblings.is_empty());
    }
}
