//! Offline catalog proposal export for a copied SQLite database.
//!
//! Usage:
//!   cargo run --example catalog_dry_run -- <database-copy> <output-json>
//!
//! The example opens only SQLite. It never constructs a source adapter,
//! downloader, ByteSource, or sync transport.

use std::{env, fs::File, path::Path};

use anyhow::{Context, Result};
use rust_lib_app::{api::scraper, db};
use serde_json::{json, Value};

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let database = args.next().context("missing database path")?;
    let output = args.next().context("missing output path")?;
    db::open_at(&database)?;

    let run = scraper::db_run_catalog_scrape(
        "offline-catalog-dry-run".into(),
        8,
        "catalog-rules-v3".into(),
    )
    .map_err(anyhow::Error::msg)?;
    let run_started_at = run.started_at.unwrap_or_default();
    let source_types = db::load_source_type_map();
    let proposals = scraper::db_load_scrape_proposals(100_000, String::new())
        .into_iter()
        .filter(|proposal| proposal.updated_at >= run_started_at)
        .map(|proposal| {
            let source_type = source_types
                .get(&proposal.source_id)
                .cloned()
                .unwrap_or_else(|| "unknown".into());
            let semantic = serde_json::from_str::<Value>(&proposal.semantic_json)
                .unwrap_or_else(|_| json!({}));
            json!({
                "asset_key": proposal.asset_key,
                "book_key": proposal.book_key,
                "source_id": proposal.source_id,
                "source_type": source_type,
                "path": proposal.path,
                "filename": proposal.filename,
                "title": proposal.title,
                "authors": serde_json::from_str::<Value>(&proposal.authors_json).unwrap_or_else(|_| json!([])),
                "provider": proposal.provider,
                "volume": proposal.volume,
                "chapter": proposal.chapter,
                "state": proposal.state,
                "semantic": semantic,
                "evidence": serde_json::from_str::<Value>(&proposal.evidence_json).unwrap_or_else(|_| json!([])),
                "conflicts": serde_json::from_str::<Value>(&proposal.conflicts_json).unwrap_or_else(|_| json!([])),
                "rule_version": proposal.rule_version,
                "input_revision": proposal.input_revision,
                "materialization_status": proposal.materialization_status,
                "updated_at": proposal.updated_at,
            })
        })
        .collect::<Vec<_>>();
    let mut by_source = std::collections::BTreeMap::<String, i64>::new();
    for proposal in &proposals {
        if let Some(source_type) = proposal.get("source_type").and_then(Value::as_str) {
            *by_source.entry(source_type.to_owned()).or_default() += 1;
        }
    }

    let document = json!({
        "schema": "rch.catalog-dry-run/v2",
        "offline": true,
        "remote_book_source_io": false,
        "provider_requests": false,
        "content_reads": false,
        "run": {
            "id": run.id,
            "trigger": run.trigger,
            "status": run.status,
            "rule_version": run.rule_version,
            "total": run.total,
            "processed": run.processed,
            "ready": run.ready,
            "ambiguous": run.ambiguous,
            "partial": run.partial,
            "unmatched": run.unmatched,
            "input_assets": run.input_assets,
            "unique_assets": run.unique_assets,
            "proposals_written": run.proposals_written,
            "asset_collision_count": run.asset_collision_count,
            "book_group_collision_count": run.book_group_collision_count,
            "accounting_status": run.accounting_status,
            "error": run.error,
        },
        "proposal_count": proposals.len(),
        "by_source": by_source,
        "proposals": proposals,
    });

    if let Some(parent) = Path::new(&output).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    serde_json::to_writer_pretty(File::create(output)?, &document)?;
    Ok(())
}
