# 后台任务与前台响应优化

## Goal

Reduce foreground lag during local source scans, sync, batch tagging, and cloud-source connection waits while preserving eventual background progress and data correctness.

## Requirements

- Measure representative scan, sync, batch-tag, and cloud-connect paths in Profile/Release mode before tuning concurrency.
- Coalesce same-source index refreshes without dropping forced rebuilds.
- Preserve source-specific catalog dirty intent raised while an automation cycle is active.
- Distinguish local-directory freshness from remote sync outcomes; unknown sync effects must not skip required refreshes.
- Make batch tagging awaitable, observable by phase, and efficient across index, metadata, tag-link, and persistence layers; cap transaction lock time.
- Let routine background maintenance defer to current foreground work, then resume with bounded service so it cannot starve indefinitely.
- Show immediate, truthful loading feedback for Quark/115 QR and Cookie validation/connect flows; prevent duplicate submission.
- Keep H1 EH Daily and 115 automation scheduling, process dedupe, and persisted state outside this change.
- Do not introduce an isolate scanner or a general-purpose task framework in this implementation.

## Acceptance Criteria

- [ ] Android and Windows Profile/Release baselines distinguish frame delay, database lock wait, file I/O, and stage duration; missing Android hardware is recorded as an explicit limitation.
- [ ] Same-source refreshes are single-flight and a forced rebuild arriving behind an incremental refresh still runs.
- [ ] Dirty generations raised during a running cycle are processed after it; local filesystem freshness is not inferred from remote revision alone.
- [ ] Sync outcomes represent success/no-op/failure and catalog invalidation scopes; unknown invalidation conservatively refreshes.
- [ ] Batch tags preserve file/directory target kind and current tag semantics, report per-phase outcomes, persist metadata and links, and use bounded transactions.
- [ ] Foreground reads and explicit user tasks remain responsive; routine background jobs coalesce, resume, and receive bounded service after maximum wait.
- [ ] Cloud QR/Cookie flows show loading immediately, block duplicate submission, and clear loading on success, cancellation, and failure.
- [ ] H1 EH Daily and 115 automation contracts remain unchanged.

## Notes

- Accepted design: `docs/superpowers/specs/2026-10-12-background-work-performance-design.md`.
- Execution is authorized by the user; isolate work remains excluded.
