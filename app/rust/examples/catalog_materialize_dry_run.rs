//! Validate the production safe-auto projection on an isolated SQLite copy.
//!
//! Usage:
//!   cargo run --example catalog_materialize_dry_run -- <database> <output-json> [target-label]
//!
//! The example intentionally opens only the supplied database copy. It runs
//! the same catalog scrape and `Ready + conflicts=[]` projection used by the
//! coordinator, but it never constructs a source adapter, downloader,
//! `ByteSource`, provider client or sync transport.

use std::{collections::BTreeMap, env, fs::File, path::Path};

use anyhow::{Context, Result};
use rust_lib_app::{api::scraper, db};
use serde_json::{json, Value};

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let database = args.next().context("missing database path")?;
    let output = args.next().context("missing output path")?;
    let target_label = args
        .next()
        .unwrap_or_else(|| "isolated-database-copy".into());
    db::open_at(&database)?;

    let run = scraper::db_run_catalog_scrape(
        format!("offline-materialize-{target_label}"),
        8,
        "catalog-rules-v3".into(),
    )
    .map_err(anyhow::Error::msg)?;
    let run_started_at = run.started_at.unwrap_or_default();
    let proposals = scraper::db_load_scrape_proposals(100_000, String::new())
        .into_iter()
        .filter(|proposal| proposal.updated_at >= run_started_at)
        .collect::<Vec<_>>();

    let mut counts = BTreeMap::<String, i64>::new();
    let mut results = Vec::with_capacity(proposals.len());
    let mut sync_dirty_count = 0_i64;
    let mut eligible_count = 0_i64;
    for proposal in proposals {
        let conflicts = serde_json::from_str::<Value>(&proposal.conflicts_json)
            .unwrap_or_else(|_| json!([]));
        let eligible = proposal.state == "ready"
            && conflicts.as_array().is_some_and(|items| items.is_empty())
            && !proposal.input_revision.trim().is_empty();
        if eligible {
            eligible_count += 1;
        }

        let result = if eligible {
            scraper::db_materialize_ready_proposal(
                proposal.asset_key.clone(),
                proposal.input_revision.clone(),
            )
            .map_err(anyhow::Error::msg)?
        } else {
            // The coordinator never calls the projection for ineligible rows;
            // retain an explicit result so accounting is easy to audit.
            rust_lib_app::scrape_projection::MaterializeResult {
                asset_key: proposal.asset_key.clone(),
                book_key: proposal.book_key.clone(),
                status: "review-required".into(),
                changed_fields: Vec::new(),
                added_tags: Vec::new(),
                skipped_fields: vec!["safe-auto-eligibility".into()],
                sync_dirty: false,
            }
        };
        *counts.entry(result.status.clone()).or_default() += 1;
        if result.sync_dirty {
            sync_dirty_count += 1;
        }
        results.push(json!({
            "asset_key": result.asset_key,
            "book_key": result.book_key,
            "status": result.status,
            "changed_fields": result.changed_fields,
            "added_tags": result.added_tags,
            "skipped_fields": result.skipped_fields,
            "sync_dirty": result.sync_dirty,
        }));
    }

    let document = json!({
        "schema": "rch.catalog-materialize-dry-run/v1",
        "target": target_label,
        "offline": true,
        "remote_book_source_io": false,
        "provider_requests": false,
        "content_reads": false,
        "sync_transport_invoked": false,
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
        "materialization": {
            "proposal_count": results.len(),
            "eligible_count": eligible_count,
            "by_status": counts,
            "sync_dirty_count": sync_dirty_count,
            "canonical_meta_count_after": db::load_all_metas().len(),
            "tag_count_after": db::load_all_tags().len(),
            "book_tag_link_count_after": db::load_all_book_tags().len(),
            "results": results,
        },
    });

    if let Some(parent) = Path::new(&output).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    serde_json::to_writer_pretty(File::create(output)?, &document)?;
    Ok(())
}
