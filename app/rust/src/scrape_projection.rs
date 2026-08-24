//! Local proposal -> canonical metadata/tag projection.
//!
//! This module is deliberately independent from the catalog parser's input
//! capabilities. It consumes persisted proposal JSON and writes only local
//! SQLite metadata/tags. It never receives a source session, ByteSource,
//! Downloader or sync transport handle.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use crate::db;

#[derive(Debug, Clone)]
pub struct MaterializeResult {
    pub asset_key: String,
    pub book_key: String,
    pub status: String,
    pub changed_fields: Vec<String>,
    pub added_tags: Vec<String>,
    pub skipped_fields: Vec<String>,
    pub sync_dirty: bool,
}

pub fn materialize_ready_proposal(
    asset_key: &str,
    expected_revision: &str,
) -> Result<MaterializeResult> {
    let mut conn = db::get().lock().unwrap();
    let tx = conn.transaction()?;
    let result = materialize_ready_proposal_on(&tx, asset_key, expected_revision);
    match result {
        Ok(value) => {
            tx.commit()?;
            Ok(value)
        }
        Err(error) => {
            let _ = tx.rollback();
            Err(error)
        }
    }
}

pub(crate) fn materialize_ready_proposal_on(
    conn: &Connection,
    asset_key: &str,
    expected_revision: &str,
) -> Result<MaterializeResult> {
    let proposal = db::load_scrape_proposal_on(conn, asset_key)
        .ok_or_else(|| anyhow!("scrape proposal not found: {asset_key}"))?;
    let mut result = MaterializeResult {
        asset_key: asset_key.to_string(),
        book_key: proposal.book_key.clone(),
        status: "rejected".into(),
        changed_fields: Vec::new(),
        added_tags: Vec::new(),
        skipped_fields: Vec::new(),
        sync_dirty: false,
    };

    if proposal.materialization_status == "applied" && proposal.input_revision == expected_revision
    {
        // Applied proposals from the pre-v2 projection may have no live
        // resource tags anymore: the startup migration intentionally removes
        // obsolete namespaced tags, but it cannot reconstruct their semantic
        // values. Re-enter the normal local transaction when any current
        // canonical resource tag is missing; otherwise preserve idempotence.
        let semantic = serde_json::from_str::<Value>(&proposal.semantic_json).ok();
        let target_book_key = canonical_book_key(&proposal.book_key);
        let needs_reprojection = semantic
            .as_ref()
            .map(|value| {
                let expected_tags = canonical_tag_names(value);
                !canonical_tags_present(conn, &target_book_key, &expected_tags)
            })
            .unwrap_or(false);
        if !needs_reprojection {
            result.status = "skipped".into();
            return Ok(result);
        }
    }

    if proposal.state != "ready" {
        result.status = "review-required".into();
        result.skipped_fields.push("state".into());
        record_materialization(conn, &proposal, &result, None)?;
        return Ok(result);
    }

    if !json_array_is_empty(&proposal.conflicts_json) {
        result.status = "review-required".into();
        result.skipped_fields.push("conflicts".into());
        record_materialization(conn, &proposal, &result, None)?;
        return Ok(result);
    }

    if proposal.input_revision.is_empty() || proposal.input_revision != expected_revision {
        result.status = "stale".into();
        result.skipped_fields.push("input_revision".into());
        record_materialization(conn, &proposal, &result, None)?;
        return Ok(result);
    }

    // A newer persisted catalog revision supersedes this proposal even when a
    // caller still holds the old DTO. The queue is local catalog state, so this
    // check does not refresh or inspect any remote source.
    if let Some(current_revision) =
        db::latest_scrape_queue_revision_on(conn, &proposal.asset_key, &proposal.rule_version)
    {
        if current_revision != expected_revision {
            result.status = "stale".into();
            result.skipped_fields.push("input_revision".into());
            record_materialization(conn, &proposal, &result, None)?;
            return Ok(result);
        }
    }

    let target_book_key = canonical_book_key(&proposal.book_key);
    let (mut meta, legacy_keys_migrated) =
        load_and_migrate_meta_aliases(conn, &target_book_key)?;
    if legacy_keys_migrated {
        conn.execute(
            "UPDATE scrape_proposals SET book_key = ?2, updated_at = ?3 WHERE asset_key = ?1",
            params![proposal.asset_key, target_book_key, db::now_ms()],
        )?;
    }

    let semantic: Value = serde_json::from_str(&proposal.semantic_json)
        .map_err(|error| anyhow!("invalid proposal semantic_json: {error}"))?;
    if meta.is_none() {
        meta = Some(db::BookMetaRow {
        key: target_book_key.clone(),
        cover_page: 0,
        crop_x: None,
        crop_y: None,
        crop_w: None,
        crop_h: None,
        author: String::new(),
        genre: String::new(),
        series: String::new(),
        title: String::new(),
        chinese_title: String::new(),
        summary: String::new(),
        comment: String::new(),
        rotations: "{}".into(),
        });
    }
    let mut meta = meta.expect("metadata initialized above");
    let mut meta_changed = false;

    if let Some(title) = semantic_string(&semantic, "work_title").or(proposal.title.clone()) {
        apply_empty_field(
            &mut meta.title,
            &title,
            "title",
            &mut result,
            &mut meta_changed,
        );
    }

    let authors = creator_names(&semantic).unwrap_or_else(|| {
        serde_json::from_str::<Vec<String>>(&proposal.authors_json).unwrap_or_default()
    });
    if !authors.is_empty() {
        apply_empty_field(
            &mut meta.author,
            &authors.join(", "),
            "author",
            &mut result,
            &mut meta_changed,
        );
    }

    let series = string_array(&semantic, "source_series");
    if let Some(first) = series.first() {
        apply_empty_field(
            &mut meta.series,
            first,
            "series",
            &mut result,
            &mut meta_changed,
        );
    }

    let tag_names = canonical_tag_names(&semantic);
    for tag_name in tag_names {
        let id = db::tag_id(&tag_name);
        let already_linked: bool = conn
            .query_row(
                "SELECT deleted = 0 FROM book_tags WHERE book_key = ?1 AND tag_id = ?2",
                params![target_book_key, id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false);
        let now = db::now_ms();
        conn.execute(
            "INSERT INTO tags (id, name, created_at, updated_at, deleted)
             VALUES (?1, ?2, ?3, ?3, 0)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name,
                 updated_at = excluded.updated_at, deleted = 0",
            params![id, tag_name, now],
        )?;
        let inserted = conn.execute(
            "INSERT INTO book_tags (book_key, tag_id, updated_at, deleted)
             VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(book_key, tag_id) DO UPDATE SET
                 updated_at = excluded.updated_at, deleted = 0",
            params![target_book_key, id, now],
        )?;
        if inserted > 0 && !already_linked {
            result.added_tags.push(tag_name);
            result.sync_dirty = true;
        }
    }

    if meta_changed || legacy_keys_migrated {
        db::upsert_meta_on(conn, &meta)?;
        result.sync_dirty = true;
    }
    result.book_key = target_book_key;
    result.status = "applied".into();
    let mut recorded_proposal = proposal.clone();
    recorded_proposal.book_key = result.book_key.clone();
    record_materialization(conn, &recorded_proposal, &result, Some(db::now_ms()))?;
    Ok(result)
}

/// Rebuild the logical key with the same path spelling rules used by the
/// catalog/index layer. Persisted installs may still contain legacy keys such
/// as `F:\\...zip`; those rows are folded into the canonical key before a
/// proposal can create a second metadata projection.
fn canonical_book_key(key: &str) -> String {
    let Some(first) = key.find('|') else { return key.to_string() };
    let rest = &key[first + 1..];
    let Some(second_rel) = rest.find('|') else { return key.to_string() };
    let second = first + 1 + second_rel;
    db::book_key_of(&key[..first], &key[first + 1..second], &key[second + 1..])
}

fn canonical_tags_present(conn: &Connection, book_key: &str, expected: &[String]) -> bool {
    expected.iter().all(|tag_name| {
        conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM book_tags
                JOIN tags ON tags.id = book_tags.tag_id
                WHERE book_tags.book_key = ?1
                  AND book_tags.deleted = 0
                  AND tags.deleted = 0
                  AND tags.name = ?2
            )",
            params![book_key, tag_name],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
            != 0
    })
}

fn load_and_migrate_meta_aliases(
    conn: &Connection,
    canonical_key: &str,
) -> Result<(Option<db::BookMetaRow>, bool)> {
    let Some(first) = canonical_key.find('|') else {
        return Ok((db::load_meta_on(conn, canonical_key), false));
    };
    let rest = &canonical_key[first + 1..];
    let Some(second_rel) = rest.find('|') else {
        return Ok((db::load_meta_on(conn, canonical_key), false));
    };
    let second = first + 1 + second_rel;
    let source_type = &canonical_key[..first];
    let source_id = &canonical_key[first + 1..second];
    let prefix = format!("{source_type}|{source_id}|");
    let mut stmt = conn.prepare(
        "SELECT key, cover_page, crop_x, crop_y, crop_w, crop_h,
                author, genre, series, title, chinese_title, summary, comment, rotations
         FROM book_metas
         WHERE deleted = 0 AND key LIKE ?1
         ORDER BY updated_at ASC, key ASC",
    )?;
    let rows: Vec<db::BookMetaRow> = stmt
        .query_map([format!("{prefix}%")], |row| {
            Ok(db::BookMetaRow {
                key: row.get(0)?,
                cover_page: row.get(1)?,
                crop_x: row.get(2)?,
                crop_y: row.get(3)?,
                crop_w: row.get(4)?,
                crop_h: row.get(5)?,
                author: row.get(6)?,
                genre: row.get(7)?,
                series: row.get(8)?,
                title: row.get(9)?,
                chinese_title: row.get(10)?,
                summary: row.get(11)?,
                comment: row.get(12)?,
                rotations: row.get(13)?,
            })
        })?
        .filter_map(|row| row.ok())
        .filter(|row| canonical_book_key(&row.key) == canonical_key)
        .collect();

    let mut aliases = rows
        .iter()
        .filter(|row| row.key != canonical_key)
        .cloned()
        .collect::<Vec<_>>();
    let canonical = rows.iter().find(|row| row.key == canonical_key).cloned();
    if aliases.is_empty() {
        return Ok((canonical, false));
    }

    // Legacy aliases are the previous user-facing projection. Start with the
    // oldest alias so user-entered title/author/series values win over a new
    // proposal row; canonical-row fields only fill genuinely empty fields.
    let mut merged = aliases.remove(0);
    for alias in aliases {
        merge_non_empty_meta(&mut merged, &alias);
    }
    if let Some(canonical) = canonical {
        merge_non_empty_meta(&mut merged, &canonical);
    }
    merged.key = canonical_key.to_string();

    for alias in rows.iter().filter(|row| row.key != canonical_key) {
        migrate_book_tag_links(conn, &alias.key, canonical_key)?;
        conn.execute("DELETE FROM book_metas WHERE key = ?1", params![alias.key])?;
    }
    Ok((Some(merged), true))
}

fn merge_non_empty_meta(target: &mut db::BookMetaRow, source: &db::BookMetaRow) {
    if target.author.trim().is_empty() { target.author = source.author.clone(); }
    if target.genre.trim().is_empty() { target.genre = source.genre.clone(); }
    if target.series.trim().is_empty() { target.series = source.series.clone(); }
    if target.title.trim().is_empty() { target.title = source.title.clone(); }
    if target.chinese_title.trim().is_empty() { target.chinese_title = source.chinese_title.clone(); }
    if target.summary.trim().is_empty() { target.summary = source.summary.clone(); }
    if target.comment.trim().is_empty() { target.comment = source.comment.clone(); }
    if target.rotations.trim().is_empty() || target.rotations == "{}" {
        if !source.rotations.trim().is_empty() && source.rotations != "{}" {
            target.rotations = source.rotations.clone();
        }
    }
    if target.cover_page == 0 && source.cover_page != 0 { target.cover_page = source.cover_page; }
    if target.crop_x.is_none() { target.crop_x = source.crop_x; }
    if target.crop_y.is_none() { target.crop_y = source.crop_y; }
    if target.crop_w.is_none() { target.crop_w = source.crop_w; }
    if target.crop_h.is_none() { target.crop_h = source.crop_h; }
}

fn migrate_book_tag_links(conn: &Connection, old_key: &str, new_key: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO book_tags (book_key, tag_id, updated_at, deleted)
         SELECT ?1, tag_id, updated_at, 0 FROM book_tags
         WHERE book_key = ?2 AND deleted = 0
         ON CONFLICT(book_key, tag_id) DO UPDATE SET
             deleted = 0, updated_at = MAX(book_tags.updated_at, excluded.updated_at)",
        params![new_key, old_key],
    )?;
    conn.execute("DELETE FROM book_tags WHERE book_key = ?1", params![old_key])?;
    Ok(())
}

fn apply_empty_field(
    current: &mut String,
    candidate: &str,
    field: &str,
    result: &mut MaterializeResult,
    changed: &mut bool,
) {
    let candidate = candidate.trim();
    if candidate.is_empty() {
        return;
    }
    if current.trim().is_empty() {
        *current = candidate.to_string();
        result.changed_fields.push(field.to_string());
        *changed = true;
    } else if current.trim() != candidate {
        result.skipped_fields.push(field.to_string());
    }
}

fn semantic_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn string_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Convert parser vocabulary into the small, user-facing tag vocabulary that
/// is safe to project into the canonical tag table.  Parser/provider fields
/// intentionally stay out of the tag UI: they remain available in proposal
/// JSON and provenance, while only stable resource semantics are exposed as
/// tags.
fn canonical_tag_names(value: &Value) -> Vec<String> {
    let mut tags = Vec::new();

    let mut add = |tag: &str| {
        let tag = tag.trim();
        if !tag.is_empty() && !tags.iter().any(|existing| existing == tag) {
            tags.push(tag.to_string());
        }
    };

    let normalized = |raw: Option<String>| {
        raw.unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .replace(['-', ' ', '／', '/'], "_")
    };

    let translation_state = normalized(semantic_string(value, "translation_state"));
    let language = normalized(semantic_string(value, "resource_language"));
    match translation_state.as_str() {
        "zh_translated" => add("Chinese"),
        "translated" | "translation"
            if language == "zh" || language == "cn" || language == "chinese" =>
        {
            add("Chinese")
        }
        "translated" | "translation" => add("已翻译"),
        "untranslated" | "original" => add("未翻译"),
        _ if language == "zh" || language == "cn" || language == "chinese" => {
            // The user-facing vocabulary intentionally merges a Chinese
            // language marker and an explicit Chinese translation marker.
            add("Chinese");
        }
        _ => {}
    }

    match normalized(semantic_string(value, "translation_method")).as_str() {
        "machine" | "mtl" | "machine_translation" | "ai" | "ai_translation" => add("机翻"),
        "human" | "manual" | "human_translation" => add("人工翻译"),
        _ => {}
    }

    // A legacy proposal may still carry resource_edition=digital, but that
    // value is obsolete and must never be projected into a user-facing tag.
    match normalized(semantic_string(value, "resource_edition")).as_str() {
        "print" | "paper" => add("实体版"),
        _ => {}
    }

    match normalized(semantic_string(value, "censorship")).as_str() {
        "uncensored" | "unmodified" | "無修正" => add("无修正"),
        "censored" | "modified" => add("有修正"),
        _ => {}
    }

    match normalized(semantic_string(value, "color_state")).as_str() {
        "full_color" | "colorized" | "colourized" | "color" | "fullcolour" => add("彩漫"),
        "color_pages" | "colored_pages" | "colour_pages" => add("彩页"),
        "monochrome" | "black_and_white" => add("黑白漫"),
        _ => {}
    }

    let completeness = normalized(semantic_string(value, "resource_completeness"));
    match completeness.as_str() {
        "complete" | "completed" | "collection" | "全集" => add("合集"),
        "incomplete" | "partial" => add("未完结"),
        _ => {}
    }

    match normalized(semantic_string(value, "source_medium")).as_str() {
        "tankoubon" | "volume" | "single_volume" => add("单行本"),
        "serial" | "serialization" => add("连载"),
        "collection" => add("合集"),
        _ => {}
    }

    match normalized(semantic_string(value, "scan_completeness")).as_str() {
        "complete" | "cover_to_cover" | "full" => add("全本扫描"),
        "no_ads" | "clean" => add("无广告"),
        _ => {}
    }

    for raw in string_array(value, "release_groups") {
        let name = raw.trim();
        if !name.is_empty() {
            add(&format!("汉化组：{name}"));
        }
    }

    // Older proposals stored the resource flags as free-form tags.  Consume
    // only values with stable user-facing semantics; unknown/internal values
    // are deliberately dropped instead of becoming opaque English tags.
    for raw in string_array(value, "resource_tags") {
        let normalized = raw
            .trim()
            .to_ascii_lowercase()
            .replace(['-', ' ', '／', '/'], "_");
        match normalized.as_str() {
            "uncensored" | "unmodified" => add("无修正"),
            "censored" | "modified" => add("有修正"),
            "full_color" | "colorized" | "colourized" | "color" => add("彩漫"),
            "color_pages" | "colored_pages" | "colour_pages" => add("彩页"),
            "complete" | "completed" | "collection" | "全集" => add("合集"),
            "incomplete" | "partial" => add("未完结"),
            "translated" | "translation" | "chinese_translation" => add("Chinese"),
            "machine" | "mtl" | "machine_translation" | "ai_translation" => add("机翻"),
            "human_translation" | "manual_translation" => add("人工翻译"),
            "high_quality" | "hd" | "high_definition" | "dl" => add("高清"),
            "full_scan" | "cover_to_cover" => add("全本扫描"),
            "no_ads" | "clean" => add("无广告"),
            "monochrome" | "black_and_white" => add("黑白漫"),
            _ => {}
        }
    }

    tags.sort();
    tags
}

fn creator_names(value: &Value) -> Option<Vec<String>> {
    let mut names = Vec::new();
    for creator in value.get("creators")?.as_array()? {
        let role = creator
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !matches!(role, "artist" | "author" | "writer") {
            continue;
        }
        if let Some(name) = creator.get("name").and_then(Value::as_str) {
            let name = name.trim();
            if !name.is_empty() && !names.iter().any(|item| item == name) {
                names.push(name.to_string());
            }
        }
    }
    Some(names)
}

fn json_array_is_empty(raw: &str) -> bool {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|value| value.as_array().map(|items| items.is_empty()))
        .unwrap_or(false)
}

fn record_materialization(
    conn: &Connection,
    proposal: &db::ScrapeProposalRow,
    result: &MaterializeResult,
    applied_at: Option<i64>,
) -> Result<()> {
    let now = db::now_ms();
    let status = result.status.as_str();
    let applied_fields_json = serde_json::to_string(&result.changed_fields)?;
    let added_tags_json = serde_json::to_string(&result.added_tags)?;
    let skipped_fields_json = serde_json::to_string(&result.skipped_fields)?;
    let error = result.skipped_fields.join(", ");
    db::upsert_scrape_materialization_on(
        conn,
        &db::ScrapeMaterializationRow {
            asset_key: proposal.asset_key.clone(),
            book_key: proposal.book_key.clone(),
            proposal_revision: proposal.input_revision.clone(),
            rule_version: proposal.rule_version.clone(),
            status: status.to_string(),
            applied_fields_json,
            added_tags_json,
            skipped_fields_json,
            error: error.clone(),
            applied_at,
            updated_at: now,
        },
    )?;
    conn.execute(
        "UPDATE scrape_proposals
         SET materialization_status = ?2, materialization_error = ?3,
             materialized_at = ?4, updated_at = ?5
         WHERE asset_key = ?1",
        params![proposal.asset_key, status, error, applied_at, now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn fixture_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::init_tables(&conn).unwrap();
        conn
    }

    fn proposal(state: &str, conflicts: &str, revision: &str) -> db::ScrapeProposalRow {
        db::ScrapeProposalRow {
            asset_key: "asset|local|s1|a".into(),
            book_key: "local|s1|/books/a".into(),
            source_id: "s1".into(),
            path: "/books/a.zip".into(),
            filename: "a.zip".into(),
            title: Some("Work".into()),
            authors_json: r#"["Artist"]"#.into(),
            provider: None,
            volume: None,
            chapter: None,
            state: state.into(),
            evidence_json: "[]".into(),
            conflicts_json: conflicts.into(),
            semantic_json: r#"{
                "work_title":"Work",
                "creators":[{"name":"Circle","role":"circle"},{"name":"Artist","role":"artist"}],
                "resource_language":"zh",
                "translation_state":"translated",
                "resource_edition":"digital",
                "censorship":"uncensored",
                "release_groups":["Group"],
                "resource_tags":["complete"]
            }"#
            .into(),
            rule_version: "catalog-rules-v3".into(),
            input_revision: revision.into(),
            materialization_status: "pending".into(),
            materialization_error: String::new(),
            materialized_at: None,
            updated_at: db::now_ms(),
        }
    }

    #[test]
    fn safe_auto_projects_only_person_creator_and_canonical_tags() {
        let conn = fixture_conn();
        let row = proposal("ready", "[]", "r1");
        conn.execute(
            "INSERT INTO scrape_proposals
             (asset_key, book_key, source_id, path, filename, title, authors_json, state,
              conflicts_json, semantic_json, rule_version, input_revision, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                row.asset_key,
                row.book_key,
                row.source_id,
                row.path,
                row.filename,
                row.title,
                row.authors_json,
                row.state,
                row.conflicts_json,
                row.semantic_json,
                row.rule_version,
                row.input_revision,
                row.updated_at,
            ],
        )
        .unwrap();
        let result = materialize_ready_proposal_on(&conn, "asset|local|s1|a", "r1").unwrap();
        assert_eq!(result.status, "applied");
        assert!(result.added_tags.iter().any(|tag| tag == "汉化组：Group"));
        let author: String = conn
            .query_row(
                "SELECT author FROM book_metas WHERE key = 'local|s1|/books/a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(author, "Artist");
        let tags: Vec<String> = conn
            .prepare(
                "SELECT tags.name FROM tags JOIN book_tags ON book_tags.tag_id = tags.id
                 WHERE book_tags.book_key = 'local|s1|/books/a' ORDER BY tags.name",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(tags.iter().all(|tag| !tag.contains("Circle")));
        assert!(tags.iter().any(|tag| tag == "Chinese"));
        assert!(tags.iter().any(|tag| tag == "无修正"));
        assert!(!tags.iter().any(|tag| tag == "数字版"));
        assert!(tags.iter().any(|tag| tag == "合集"));
        assert!(tags.iter().all(|tag| {
            !tag.starts_with("resource:")
                && !tag.starts_with("sequence:")
                && !tag.starts_with("publication:")
                && !tag.starts_with("release:")
        }));
    }

    #[test]
    fn canonical_tag_projection_merges_synonyms_and_drops_internal_fields() {
        let semantic: Value = serde_json::json!({
            "resource_language": "zh",
            "translation_state": "translated",
            "translation_method": "machine",
            "resource_edition": "digital",
            "censorship": "uncensored",
            "color_state": "colorized",
            "resource_completeness": "complete",
            "sequence_kind": "chapter",
            "chapter": "10",
            "publication_source": "COMIC X-EROS",
            "distribution_platform": "kakao",
            "release_groups": ["组A"],
            "resource_tags": ["uncensored", "collection", "colorized", "high_quality", "unknown-tag"]
        });
        let tags = canonical_tag_names(&semantic);
        let mut expected = vec![
            "Chinese".to_string(),
            "合集".to_string(),
            "无修正".to_string(),
            "机翻".to_string(),
            "高清".to_string(),
            "彩漫".to_string(),
            "汉化组：组A".to_string(),
        ];
        expected.sort();
        assert_eq!(tags, expected);
    }

    #[test]
    fn language_marker_alone_projects_user_facing_chinese_tag() {
        let semantic = serde_json::json!({
            "resource_language": "zh"
        });
        assert_eq!(canonical_tag_names(&semantic), vec!["Chinese"]);
    }

    fn insert_proposal(conn: &Connection, row: &db::ScrapeProposalRow) {
        conn.execute(
            "INSERT INTO scrape_proposals
             (asset_key, book_key, source_id, path, filename, title, authors_json, provider,
              volume, chapter, state, evidence_json, conflicts_json, semantic_json,
              rule_version, input_revision, materialization_status,
              materialization_error, materialized_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                     ?15, ?16, ?17, ?18, ?19, ?20)",
            params![
                row.asset_key,
                row.book_key,
                row.source_id,
                row.path,
                row.filename,
                row.title,
                row.authors_json,
                row.provider,
                row.volume,
                row.chapter,
                row.state,
                row.evidence_json,
                row.conflicts_json,
                row.semantic_json,
                row.rule_version,
                row.input_revision,
                row.materialization_status,
                row.materialization_error,
                row.materialized_at,
                row.updated_at,
            ],
        )
        .unwrap();
    }

    #[test]
    fn non_ready_or_conflicted_proposals_are_review_only() {
        for (state, conflicts) in [("partial", "[]"), ("ready", "[\"title\"]")] {
            let conn = fixture_conn();
            insert_proposal(&conn, &proposal(state, conflicts, "r1"));
            let result = materialize_ready_proposal_on(&conn, "asset|local|s1|a", "r1").unwrap();
            assert_eq!(result.status, "review-required");
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM book_metas WHERE key = 'local|s1|/books/a'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                0
            );
        }
    }

    #[test]
    fn manual_metadata_and_existing_tags_are_preserved() {
        let conn = fixture_conn();
        conn.execute(
            "INSERT INTO book_metas (key, title, author) VALUES (?1, 'Manual title', 'Manual author')",
            ["local|s1|/books/a"],
        )
        .unwrap();
        let manual_tag = "manual";
        let manual_id = db::tag_id(manual_tag);
        conn.execute(
            "INSERT INTO tags (id, name, created_at) VALUES (?1, ?2, 1)",
            params![manual_id, manual_tag],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO book_tags (book_key, tag_id, updated_at, deleted) VALUES (?1, ?2, 1, 0)",
            params!["local|s1|/books/a", manual_id],
        )
        .unwrap();
        insert_proposal(&conn, &proposal("ready", "[]", "r1"));

        let result = materialize_ready_proposal_on(&conn, "asset|local|s1|a", "r1").unwrap();
        assert_eq!(result.status, "applied");
        assert!(result.skipped_fields.contains(&"title".to_string()));
        assert!(result.skipped_fields.contains(&"author".to_string()));
        assert!(!result.added_tags.contains(&manual_tag.to_string()));
        let meta: (String, String) = conn
            .query_row(
                "SELECT title, author FROM book_metas WHERE key = 'local|s1|/books/a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(meta, ("Manual title".into(), "Manual author".into()));
    }

    #[test]
    fn materialization_is_idempotent_and_stale_revision_is_rejected() {
        let conn = fixture_conn();
        insert_proposal(&conn, &proposal("ready", "[]", "r1"));
        let first = materialize_ready_proposal_on(&conn, "asset|local|s1|a", "r1").unwrap();
        assert_eq!(first.status, "applied");
        assert!(first.sync_dirty);
        let second = materialize_ready_proposal_on(&conn, "asset|local|s1|a", "r1").unwrap();
        assert_eq!(second.status, "skipped");
        assert!(!second.sync_dirty);

        let conn = fixture_conn();
        insert_proposal(&conn, &proposal("ready", "[]", "r1"));
        let stale = materialize_ready_proposal_on(&conn, "asset|local|s1|a", "r2").unwrap();
        assert_eq!(stale.status, "stale");
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM book_metas WHERE key = 'local|s1|/books/a'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );

        let conn = fixture_conn();
        insert_proposal(&conn, &proposal("ready", "[]", "r1"));
        conn.execute(
            "INSERT INTO scrape_queue
             (asset_key, book_key, source_id, path, input_revision, rule_version, trigger, status,
              next_run_at, created_at, updated_at)
             VALUES (?1, ?2, 's1', '/books/a.zip', 'r2', 'catalog-rules-v3', 'test',
                     'succeeded', 1, 1, 2)",
            params!["asset|local|s1|a", "local|s1|/books/a"],
        )
        .unwrap();
        let stale = materialize_ready_proposal_on(&conn, "asset|local|s1|a", "r1").unwrap();
        assert_eq!(stale.status, "stale");
    }

    #[test]
    fn applied_proposal_with_legacy_resource_projection_is_reconciled() {
        let conn = fixture_conn();
        let row = proposal("ready", "[]", "r1");
        insert_proposal(&conn, &row);
        let first = materialize_ready_proposal_on(&conn, &row.asset_key, "r1").unwrap();
        assert_eq!(first.status, "applied");

        // Simulate the historical projection that wrote internal namespaced
        // tags to the materialization audit and was later removed by the
        // startup cleanup migration.
        conn.execute(
            "UPDATE scrape_materializations
             SET added_tags_json = ?1
             WHERE asset_key = ?2 AND proposal_revision = ?3",
            params![
                r#"["resource:censorship:uncensored","resource:language:zh"]"#,
                row.asset_key,
                row.input_revision,
            ],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM book_tags WHERE book_key = ?1",
            params![row.book_key],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM tags WHERE name IN ('Chinese', '无修正')",
            [],
        )
        .unwrap();

        let reconciled = materialize_ready_proposal_on(&conn, &row.asset_key, "r1").unwrap();
        assert_eq!(reconciled.status, "applied");
        let tags: Vec<String> = conn
            .prepare(
                "SELECT tags.name FROM tags JOIN book_tags ON book_tags.tag_id = tags.id
                 WHERE book_tags.book_key = ?1 ORDER BY tags.name",
            )
            .unwrap()
            .query_map(params![row.book_key], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(tags.iter().any(|tag| tag == "Chinese"));
        assert!(tags.iter().any(|tag| tag == "无修正"));
    }

    #[test]
    fn materialization_merges_legacy_path_key_and_preserves_manual_metadata() {
        let conn = fixture_conn();
        conn.execute(
            "INSERT INTO book_metas (key, title, author, series) VALUES
             (?1, '用户命名的标题', '用户作者', '用户系列')",
            [r"local|s1|F:\books\a.zip"],
        )
        .unwrap();
        let manual_tag_id = db::tag_id("用户标签");
        conn.execute(
            "INSERT INTO tags (id, name, created_at) VALUES (?1, '用户标签', 1)",
            params![manual_tag_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO book_tags (book_key, tag_id, updated_at, deleted)
             VALUES (?1, ?2, 1, 0)",
            params![r"local|s1|F:\books\a.zip", manual_tag_id],
        )
        .unwrap();

        let mut row = proposal("ready", "[]", "r1");
        row.book_key = "local|s1|f:/books/a".into();
        insert_proposal(&conn, &row);

        let result = materialize_ready_proposal_on(&conn, &row.asset_key, "r1").unwrap();
        assert_eq!(result.status, "applied");
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM book_metas", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        let meta: (String, String, String) = conn
            .query_row(
                "SELECT title, author, series FROM book_metas WHERE key = 'local|s1|f:/books/a'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            meta,
            ("用户命名的标题".into(), "用户作者".into(), "用户系列".into())
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM book_tags WHERE book_key = 'local|s1|f:/books/a' AND tag_id = ?1",
                params![manual_tag_id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM book_tags WHERE book_key = 'local|s1|F:\\books\\a.zip'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
    }
}
