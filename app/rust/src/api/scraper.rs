//! Catalog-only scraping API.
//!
//! The batch runner consumes only `library_index` and `book_sources` rows. It
//! deliberately does not receive a `ByteSource`, source session, or downloader
//! handle, so RemoteOnly assets cannot cause book-source I/O here.

use std::collections::HashMap;

use crate::{catalog_context, db, scraper as parser};
use sha2::{Digest, Sha256};

pub use crate::scrape_projection::MaterializeResult;

pub struct ScrapeRunDto {
    pub id: String,
    pub trigger: String,
    pub status: String,
    pub rule_version: String,
    pub total: i64,
    pub processed: i64,
    pub ready: i64,
    pub ambiguous: i64,
    pub partial: i64,
    pub unmatched: i64,
    pub input_assets: i64,
    pub unique_assets: i64,
    pub proposals_written: i64,
    pub asset_collision_count: i64,
    pub book_group_collision_count: i64,
    pub accounting_status: String,
    pub error: String,
    pub requested_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub updated_at: i64,
}

pub struct ScrapeProposalDto {
    pub asset_key: String,
    pub book_key: String,
    pub source_id: String,
    pub path: String,
    pub filename: String,
    pub title: Option<String>,
    pub authors_json: String,
    pub provider: Option<String>,
    pub volume: Option<String>,
    pub chapter: Option<String>,
    pub state: String,
    pub evidence_json: String,
    pub conflicts_json: String,
    pub semantic_json: String,
    pub rule_version: String,
    pub input_revision: String,
    pub materialization_status: String,
    pub materialization_error: String,
    pub materialized_at: Option<i64>,
    pub materialization_applied_fields_json: String,
    pub materialization_added_tags_json: String,
    pub materialization_skipped_fields_json: String,
    pub updated_at: i64,
}

pub struct ScrapeQueueDto {
    pub id: i64,
    pub asset_key: String,
    pub book_key: String,
    pub source_id: String,
    pub path: String,
    pub input_revision: String,
    pub rule_version: String,
    pub trigger: String,
    pub status: String,
    pub attempt: i64,
    pub next_run_at: i64,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub fn db_parse_catalog(
    book_key: String,
    filename: String,
    ancestor_dirs: Vec<String>,
    ancestor_depth: i64,
    rule_version: String,
) -> ScrapeProposalDto {
    let snapshot = parser::CatalogSnapshot {
        book_key,
        filename,
        ancestor_dirs,
        parent_siblings: Vec::new(),
    };
    proposal_dto(
        &snapshot,
        parser::parse_catalog(&snapshot, depth(ancestor_depth), &rule_version),
    )
}

/// Run one persisted catalog-only scrape pass. The caller may invoke this
/// after sync, from a timer, or from the review screen; all paths are local.
pub fn db_run_catalog_scrape(
    trigger: String,
    ancestor_depth: i64,
    rule_version: String,
) -> Result<ScrapeRunDto, String> {
    let requested_at = db::now_ms();
    let job_id = format!(
        "scrape-{requested_at}-{}",
        trigger.replace(|c: char| !c.is_ascii_alphanumeric(), "-")
    );
    let started_at = db::now_ms();
    let mut job = db::ScrapeJobRow {
        id: job_id,
        trigger,
        status: "running".into(),
        rule_version: rule_version.clone(),
        total: 0,
        processed: 0,
        ready: 0,
        ambiguous: 0,
        partial: 0,
        unmatched: 0,
        input_assets: 0,
        unique_assets: 0,
        proposals_written: 0,
        asset_collision_count: 0,
        book_group_collision_count: 0,
        accounting_status: "pending".into(),
        error: String::new(),
        requested_at,
        started_at: Some(started_at),
        finished_at: None,
        updated_at: started_at,
    };
    db::upsert_scrape_job(&job).map_err(|e| e.to_string())?;

    let result = run_catalog_pass(&mut job, depth(ancestor_depth), &rule_version);
    match result {
        Ok(()) => {
            job.status = "succeeded".into();
            job.finished_at = Some(db::now_ms());
            job.updated_at = db::now_ms();
            db::upsert_scrape_job(&job).map_err(|e| e.to_string())?;
            Ok(run_dto(job))
        }
        Err(error) => {
            job.status = "failed".into();
            job.error = error.clone();
            job.finished_at = Some(db::now_ms());
            job.updated_at = db::now_ms();
            let _ = db::upsert_scrape_job(&job);
            Err(error)
        }
    }
}

pub fn db_load_scrape_proposals(limit: i64, state: String) -> Vec<ScrapeProposalDto> {
    db::load_scrape_proposals(limit, (!state.trim().is_empty()).then_some(state.as_str()))
        .into_iter()
        .map(proposal_row_dto)
        .collect()
}

pub fn db_load_scrape_jobs(limit: i64) -> Vec<ScrapeRunDto> {
    db::load_scrape_jobs(limit)
        .into_iter()
        .map(run_dto)
        .collect()
}

pub fn db_enqueue_catalog_scrape(
    asset_key: String,
    book_key: String,
    source_id: String,
    path: String,
    input_revision: String,
    rule_version: String,
    trigger: String,
) -> Result<(), String> {
    let now = db::now_ms();
    db::enqueue_scrape_queue(&db::ScrapeQueueRow {
        id: 0,
        asset_key,
        book_key,
        source_id,
        path,
        input_revision,
        rule_version,
        trigger,
        status: "queued".into(),
        attempt: 0,
        next_run_at: now,
        last_error: None,
        created_at: now,
        updated_at: now,
    })
    .map_err(|error| error.to_string())
}

pub fn db_claim_catalog_scrape(now: i64) -> Option<ScrapeQueueDto> {
    db::claim_due_scrape_queue(now)
        .ok()
        .flatten()
        .map(queue_dto)
}

pub fn db_complete_catalog_scrape(
    id: i64,
    status: String,
    last_error: Option<String>,
    next_run_at: i64,
) -> Result<(), String> {
    db::finish_scrape_queue(id, &status, last_error.as_deref(), next_run_at)
        .map_err(|error| error.to_string())
}

pub fn db_materialize_ready_proposal(
    asset_key: String,
    expected_revision: String,
) -> Result<MaterializeResult, String> {
    crate::scrape_projection::materialize_ready_proposal(&asset_key, &expected_revision)
        .map_err(|error| error.to_string())
}

fn run_catalog_pass(
    job: &mut db::ScrapeJobRow,
    ancestor_depth: usize,
    rule_version: &str,
) -> Result<(), String> {
    let entries = db::load_all_library_index();
    let by_id: HashMap<String, db::LibraryIndexRow> = entries
        .iter()
        .cloned()
        .map(|entry| (entry.id.clone(), entry))
        .collect();
    let source_types = db::load_source_type_map();
    let files = entries
        .iter()
        .filter(|entry| !entry.deleted && entry.entry_type == "file")
        .collect::<Vec<_>>();
    let input_asset_count = files.len() as i64;
    job.total = input_asset_count;

    // First build the complete in-memory batch. No queue/proposal write is
    // allowed until physical identity and accounting invariants are known.
    let mut proposals = Vec::with_capacity(files.len());
    let mut queue_items = Vec::with_capacity(files.len());
    let mut asset_keys = std::collections::HashSet::new();
    let mut book_groups: HashMap<String, usize> = HashMap::new();
    for entry in files {
        let source_type = source_types
            .get(&entry.source_id)
            .cloned()
            .unwrap_or_else(|| "unknown".into());
        let context = catalog_context::normalize_entry(entry, &source_type, &by_id, ancestor_depth);
        let book_key = context.book_key.clone();
        let asset_key = context.asset_key.clone();
        asset_keys.insert(asset_key.clone());
        *book_groups.entry(book_key.clone()).or_default() += 1;
        let input_revision =
            input_revision(entry, &context.ancestor_dirs, &context.parent_siblings);
        let snapshot = catalog_context::to_snapshot(&context);
        let proposal = parser::parse_catalog(&snapshot, ancestor_depth, rule_version);
        match proposal.state {
            parser::ParseState::Ready => job.ready += 1,
            parser::ParseState::Ambiguous => job.ambiguous += 1,
            parser::ParseState::Partial => job.partial += 1,
            parser::ParseState::Unmatched => job.unmatched += 1,
        }
        job.processed += 1;
        proposals.push(proposal_row(
            &snapshot,
            &proposal,
            &asset_key,
            &entry.source_id,
            &entry.path,
            &input_revision,
        ));
        queue_items.push((
            asset_key,
            book_key,
            entry.source_id.clone(),
            entry.path.clone(),
            input_revision,
        ));
    }

    job.input_assets = input_asset_count;
    job.unique_assets = asset_keys.len() as i64;
    job.asset_collision_count = job.input_assets - job.unique_assets;
    job.book_group_collision_count =
        book_groups.values().filter(|count| **count > 1).count() as i64;
    if job.asset_collision_count > 0 {
        job.accounting_status = "asset-collision".into();
        return Err(format!(
            "catalog accounting collision: {} physical assets mapped to {} asset keys",
            job.input_assets, job.unique_assets
        ));
    }

    for (asset_key, book_key, source_id, path, input_revision) in &queue_items {
        let now = db::now_ms();
        db::enqueue_scrape_queue(&db::ScrapeQueueRow {
            id: 0,
            asset_key: asset_key.clone(),
            book_key: book_key.clone(),
            source_id: source_id.clone(),
            path: path.clone(),
            input_revision: input_revision.clone(),
            rule_version: rule_version.to_string(),
            trigger: job.trigger.clone(),
            status: "queued".into(),
            attempt: 0,
            next_run_at: now,
            last_error: None,
            created_at: now,
            updated_at: now,
        })
        .map_err(|e| e.to_string())?;
        // Claim the exact revision before parsing. This keeps queue state
        // restart-safe and avoids claiming an unrelated book from a global
        // due queue when multiple catalog passes overlap.
        let _claimed =
            db::claim_scrape_queue_for(asset_key, input_revision, rule_version, db::now_ms())
                .map_err(|e| e.to_string())?;
    }
    db::upsert_scrape_proposals(&proposals).map_err(|e| e.to_string())?;
    let asset_key_list = queue_items
        .iter()
        .map(|item| item.0.clone())
        .collect::<Vec<_>>();
    job.proposals_written =
        db::count_scrape_proposals_for_assets(&asset_key_list).map_err(|e| e.to_string())?;
    if job.proposals_written != job.input_assets {
        job.accounting_status = "proposal-count-mismatch".into();
        return Err(format!(
            "catalog accounting mismatch: input_assets={}, proposals_written={}",
            job.input_assets, job.proposals_written
        ));
    }
    for (asset_key, _, _, _, input_revision) in queue_items {
        db::mark_scrape_queue_succeeded(&asset_key, &input_revision, rule_version)
            .map_err(|e| e.to_string())?;
    }
    job.accounting_status = "pass".into();
    job.updated_at = db::now_ms();
    db::upsert_scrape_job(job).map_err(|e| e.to_string())?;
    Ok(())
}

fn input_revision(
    entry: &db::LibraryIndexRow,
    ancestors: &[String],
    siblings: &[String],
) -> String {
    let mut siblings = siblings.to_vec();
    siblings.sort();
    let payload = format!(
        "{}|{}|{}|{}|{}|{}|{}",
        entry.id,
        entry.path,
        entry.name,
        entry.hash.as_deref().unwrap_or_default(),
        entry.updated_at,
        ancestors.join("\u{1f}"),
        siblings.join("\u{1f}"),
    );
    let mut hasher = Sha256::new();
    hasher.update(payload.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn depth(value: i64) -> usize {
    value.clamp(1, 8) as usize
}

fn state_name(state: parser::ParseState) -> &'static str {
    match state {
        parser::ParseState::Ready => "ready",
        parser::ParseState::Partial => "partial",
        parser::ParseState::Ambiguous => "ambiguous",
        parser::ParseState::Unmatched => "unmatched",
    }
}

fn proposal_row(
    snapshot: &parser::CatalogSnapshot,
    proposal: &parser::NameRoleProposal,
    asset_key: &str,
    source_id: &str,
    path: &str,
    input_revision: &str,
) -> db::ScrapeProposalRow {
    db::ScrapeProposalRow {
        asset_key: asset_key.into(),
        book_key: snapshot.book_key.clone(),
        source_id: source_id.into(),
        path: path.into(),
        filename: snapshot.filename.clone(),
        title: proposal.title.clone(),
        authors_json: serde_json::to_string(&proposal.authors).unwrap_or_else(|_| "[]".into()),
        provider: proposal.provider.clone(),
        volume: proposal.volume.clone(),
        chapter: proposal.chapter.clone(),
        state: state_name(proposal.state).into(),
        evidence_json: serde_json::to_string(&proposal.evidence).unwrap_or_else(|_| "[]".into()),
        conflicts_json: serde_json::to_string(&proposal.conflicts).unwrap_or_else(|_| "[]".into()),
        semantic_json: serde_json::to_string(proposal).unwrap_or_else(|_| "{}".into()),
        rule_version: proposal.rule_version.clone(),
        input_revision: input_revision.into(),
        materialization_status: "pending".into(),
        materialization_error: String::new(),
        materialized_at: None,
        updated_at: db::now_ms(),
    }
}

fn proposal_dto(
    snapshot: &parser::CatalogSnapshot,
    proposal: parser::NameRoleProposal,
) -> ScrapeProposalDto {
    let input_revision = input_revision_from_snapshot(snapshot);
    let row = proposal_row(
        &snapshot,
        &proposal,
        &format!("catalog|{}", snapshot.book_key),
        "",
        &snapshot.book_key,
        &input_revision,
    );
    proposal_row_dto(row)
}

fn input_revision_from_snapshot(snapshot: &parser::CatalogSnapshot) -> String {
    let mut hasher = Sha256::new();
    hasher.update(snapshot.book_key.as_bytes());
    hasher.update(b"|");
    hasher.update(snapshot.filename.as_bytes());
    hasher.update(b"|");
    hasher.update(snapshot.ancestor_dirs.join("\u{1f}").as_bytes());
    hasher.update(b"|");
    let mut siblings = snapshot.parent_siblings.clone();
    siblings.sort();
    hasher.update(siblings.join("\u{1f}").as_bytes());
    format!("{:x}", hasher.finalize())
}

fn proposal_row_dto(row: db::ScrapeProposalRow) -> ScrapeProposalDto {
    let materialization = db::load_scrape_materialization(&row.asset_key, &row.input_revision);
    ScrapeProposalDto {
        asset_key: row.asset_key,
        book_key: row.book_key,
        source_id: row.source_id,
        path: row.path,
        filename: row.filename,
        title: row.title,
        authors_json: row.authors_json,
        provider: row.provider,
        volume: row.volume,
        chapter: row.chapter,
        state: row.state,
        evidence_json: row.evidence_json,
        conflicts_json: row.conflicts_json,
        semantic_json: row.semantic_json,
        rule_version: row.rule_version,
        input_revision: row.input_revision,
        materialization_status: row.materialization_status,
        materialization_error: row.materialization_error,
        materialized_at: row.materialized_at,
        materialization_applied_fields_json: materialization
            .as_ref()
            .map(|value| value.applied_fields_json.clone())
            .unwrap_or_else(|| "[]".into()),
        materialization_added_tags_json: materialization
            .as_ref()
            .map(|value| value.added_tags_json.clone())
            .unwrap_or_else(|| "[]".into()),
        materialization_skipped_fields_json: materialization
            .as_ref()
            .map(|value| value.skipped_fields_json.clone())
            .unwrap_or_else(|| "[]".into()),
        updated_at: row.updated_at,
    }
}

fn queue_dto(row: db::ScrapeQueueRow) -> ScrapeQueueDto {
    ScrapeQueueDto {
        id: row.id,
        asset_key: row.asset_key,
        book_key: row.book_key,
        source_id: row.source_id,
        path: row.path,
        input_revision: row.input_revision,
        rule_version: row.rule_version,
        trigger: row.trigger,
        status: row.status,
        attempt: row.attempt,
        next_run_at: row.next_run_at,
        last_error: row.last_error,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn run_dto(row: db::ScrapeJobRow) -> ScrapeRunDto {
    ScrapeRunDto {
        id: row.id,
        trigger: row.trigger,
        status: row.status,
        rule_version: row.rule_version,
        total: row.total,
        processed: row.processed,
        ready: row.ready,
        ambiguous: row.ambiguous,
        partial: row.partial,
        unmatched: row.unmatched,
        input_assets: row.input_assets,
        unique_assets: row.unique_assets,
        proposals_written: row.proposals_written,
        asset_collision_count: row.asset_collision_count,
        book_group_collision_count: row.book_group_collision_count,
        accounting_status: row.accounting_status,
        error: row.error,
        requested_at: row.requested_at,
        started_at: row.started_at,
        finished_at: row.finished_at,
        updated_at: row.updated_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_projection_round_trips_through_scrape_row() {
        let snapshot = parser::CatalogSnapshot::new(
            "fixture/dlsite",
            "[RJ01234567][Circle (Artist)] Title 10续 [中国翻訳].zip",
            vec![],
        );
        let proposal = parser::parse_catalog(&snapshot, 4, "catalog-rules-v3");
        let row = proposal_row(
            &snapshot,
            &proposal,
            "asset|source|fixture",
            "source",
            "/library/file.zip",
            &input_revision_from_snapshot(&snapshot),
        );
        let semantic: serde_json::Value = serde_json::from_str(&row.semantic_json).unwrap();

        assert_eq!(semantic["title"], "Title");
        assert_eq!(semantic["chapter"], "10");
        assert_eq!(semantic["chapter_relation"], "continuation");
        assert_eq!(semantic["external_id_candidates"][0]["raw"], "RJ01234567");
        assert_eq!(semantic["creators"][0]["role"], "circle");
        assert_eq!(semantic["creators"][1]["role"], "artist");
        assert_eq!(semantic["authors"], serde_json::json!(["Artist"]));
        assert_eq!(semantic["resource_language"], "zh");
        assert_eq!(semantic["translation_state"], "translated");
        assert!(semantic["translation_method"].is_null());
        assert_eq!(row.rule_version, "catalog-rules-v3");
    }
}
