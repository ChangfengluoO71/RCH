# RCH Trellis State Reconciliation

Date: 2026-08-26
Source commit: `26c0ca150fcbfa1ea8f259beab52579d299aef51`

## Result

The unarchived Trellis surface is reduced from 26 tasks to 16 actionable tasks. Ten historical tasks are moved to the August 2026 archive. No product source is changed.

## Archived — completed or superseded

1. `00-bootstrap-guidelines` — bootstrap task is stale: backend/frontend Trellis specs are populated with real project guidelines.
2. `07-31-m2-ai-upscaler` — PRD acceptance 10/10 and implementation checklist 12/12 complete; AI upscaler shipped in later releases.
3. `08-01-ai-background-upscale` — background queue, persistence, progress, completion prompt and reader original/upscaled switching were implemented in `e5cdb402` with later progress/robustness fixes (`85d70057`, `33459421`, `7f9c85aa`, `127e6c18`). Remaining unchecked manual boxes are stale relative to shipped behavior.
4. `08-01-m2-backlog` — superseded historical aggregator. Its own PRD directs concrete items into independent tasks and records several items as already completed.
5. `08-02-task-drag-reorder` — PRD acceptance 5/5 complete; implementation commit `72d3a574` added persisted task ordering and drag reorder.
6. `08-04-p0-android-buildchain` — Android build chain and base engineering landed in `42b30b11`.
7. `08-04-p3-native-formats` — PRD acceptance complete; PDF/RAR Android adaptation and release flow landed in `6773e714`.
8. `08-04-p4-release` — superseded by actual signed Android releases and release workflow, including `6773e714` and v0.4.1 `bdf28066`.
9. `08-07-p5-ui-narrow-screen` — implementation checklist 14/15; the only historical open item was a Windows build environment check. Subsequent Windows/Android releases supersede that verification gap.
10. `08-08-sync-and-folder-cover` — folder-cover implementation landed in `b4d5179d`, shipped in v0.4.5 `ff1b867d`, and sync refactor closeout landed in `cac19a3e`.

## Retained — actionable

### Independent backlog

- `08-01-detail-info-settings`
- `08-01-detail-page-stats`
- `08-01-exit-behavior`
- `08-01-first-run-cache-guide`
- `08-02-avif-support`
- `08-02-cover-loading-perf`

These PRDs still contain unfulfilled acceptance criteria. AVIF is also absent from the Rust `image` feature set at the audited commit; cover-loading performance still requires measured before/after optimization rather than merely the existing disk cache/concurrency baseline.

### M6 Android

Keep parent `08-04-m6-android` with two active children:

- `08-04-p1-local-reader` — requires real-device local-reading closure across listed formats and interactions; the existing P1 commit is explicitly only a first batch.
- `08-04-p2-remote-sources` — requires real-device five-source/WebDAV-sync closure and credential-refresh verification.

With P0/P3/P4/P5 archived, Trellis should report M6 as `4/6 done`.

### M8 Smart Scraping

Keep parent `08-08-m8-smart-scraping` and the six planned M1–M6 children:

- `08-19-m8-m1-migration-identity`
- `08-19-m8-m2-local-evidence`
- `08-19-m8-m3-provider-foundation`
- `08-19-m8-m4-candidate-ranking`
- `08-19-m8-m5-review-confirmation`
- `08-19-m8-m6-confirmed-sync-validation`

The catalog-rules-v3 and automation-integration portions are already counted as done, so the parent remains `2/8 done` rather than being archived.

## Parent-child contract

Trellis archive behavior was tested through CWapi before persistence. Archiving a child removes it from the active task set but intentionally does not rewrite the parent's historical `children` array. `task.py list` computes progress using the active set, producing the desired M6 `4/6 done` and M8 `2/8 done`. Parent arrays are therefore preserved exactly.

## CWapi verification context

The prior baseline audit established Rust health (`235 passed`, `0 failed`, `2 ignored`) when MSVC is initialized and `TEMP/TMP` are worktree-local. Flutter batch output remains unreliable through the current CWapi v1.6.1 Windows invocation path, so no new Flutter pass is asserted here. That environment limitation does not affect this Trellis-only reconciliation.

## Product mutations

None.
