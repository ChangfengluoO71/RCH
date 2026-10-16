# Design — P0 Network and 115 Contract Gate

## Verified baseline

- Repository: `ChangfengluoO71/RCH`; checked-out branch `local-ai/cover-quark-debug`, HEAD `96edfd24cf643c2f7928b2039afcf11a5fe6a1be`, clean before this task. The live `origin/master` was `9c3e9ff4e7728c1def7c13f48b9df1d13b7aae12`; current HEAD descends from it and is three commits ahead.
- EH and existing 115 clients use reqwest 0.12.4 with `blocking`, `json`, and `rustls-tls`; no `socks` feature is enabled. No audited application client explicitly calls `.proxy()` or `.no_proxy()`, so they inherit reqwest's default proxy behavior.
- EH creates one client per operation and currently uses it for main-site and tracker requests. It has an in-memory-only progress object; manifest reads collapse parse/read errors to an empty list and writes are not atomic.
- Existing 115 OAuth clients share the established reader source path and have a per-client `RateGate`; 115 web reading also has a 405 WAF cooldown. Token refresh rotates the refresh token, so a second client must not refresh the same credential independently.
- `RemoteScanCoordinator.ensureForSession` starts remote scanning after a source session becomes ready; startup warm-up is separate. The home refresh action reloads local source/catalog state and restores statuses; it does not itself force a remote scan. Existing remote scans already have a durable SQLite job/checkpoint system.
- `cloud115_offline_probe.rs` currently sends several guessed POST add-task requests and POSTs `/offline/clear` despite its comments claiming no destructive request is made. Its `--destructive` flag does not guard those calls.
- Safe live GETs from this runtime reached `e-hentai.org` (200 effective HTML), `ehtracker.org` (301 at root only), and the 115 API host (200 transport with an unauthenticated JSON 401 body). This does not establish mainland-China coverage, Tracker `.torrent` validity, authenticated 115 access, or BT API support.

## Boundaries and flow

```text
EH config -> immutable EH client policy -> main-site and Tracker diagnostics / scan
115 existing session -> read-only direct diagnostic using an already-held access token
115 torrent adapter -> separate implementation, only after official upload and BT API contract is verified
existing 115 reader clients -> unchanged proxy, rate gate, and WAF behavior
```

The EH network policy is part of the optional subscription config and does not alter filter defaults. `system` uses reqwest's supported environment/Windows system proxy discovery. `direct` uses `.no_proxy()`. `manual` validates and installs exactly the user's HTTP(S), SOCKS5, or SOCKS5H proxy; it must not retry through another route. SOCKS is enabled only if the selected implementation needs it. Each operation builds its client from a validated immutable policy, so edits take effect on the next operation without mutating an in-flight client.

EH diagnostics must identify the tested host, selected mode, timing, status/business outcome, challenge suspicion, and a fixed error class. Reqwest's blocking API does not expose every DNS/TCP/TLS subphase for a single request, so the UI/report must mark unavailable subphase details as unobserved rather than claim a successful phase. Tracker root reachability is transport-only; a Tracker `.torrent` business check requires an authorized sample and must not fetch arbitrary content.

Any 115 diagnostic must use a read-only, documented endpoint, an explicit direct client, a bounded request, and the existing session's current access token without independently rotating its refresh token. If the session has no usable access token, report that authentication could not be tested; do not silently refresh through a second client. Resolve `/漫画/EH订阅` by read-only folder listing only. Existing reading clients remain unchanged.

## P0 gate and scope control

P0-A covers only network policies and bounded read-only diagnostics. P0-B requires a verified official 115 upload and `add_task_bt` contract plus an authorized test Torrent and existing valid account session. Public official portal research has not yet established that contract. No write probe is permitted while that remains unresolved. P1 checkpointing, queue, scheduler, recovery, and library handoff are strictly gated and are not started until P0_REAL_PASS.

## Compatibility and failure handling

- Additive config fields default to `system` and preserve existing filter values.
- Invalid manual proxy config fails at save time; errors and logs never contain proxy credentials.
- Do not downgrade TLS, rewrite HTTPS targets, auto-edit system proxy settings, or fall back between modes.
- Diagnostics are bounded, read-only, cancellable where the API permits, and cannot be confused with P0 Torrent acceptance.
- A missing official BT API contract, auth material, or authorized sample leaves P0 blocked; it does not authorize guessed endpoints or P1 work.

## Approved follow-up: durable Cloud115 BT task monitoring

### Boundary and authorities

- `Cloud115BtBatchLedger` remains the write-intent/idempotency authority. Monitoring is a read-only projection and may never create, replay, or authorize an upload/BT-submit operation.
- The SQLite monitoring records belong in the existing operational `database.db`, through the established Rust DB initializer/migration path. They are not canonical manga metadata, `app_settings`, a sync entity, or a replacement for the JSONL ledger.
- Monitor only `(stable account key, Infohash, intended target folder)` tuples that have an RCH batch-ledger entry. Do not adopt tasks merely because they appear in the user's 115 account.
- Keep the 115 Cookie session and all existing request/WAF/rate gates. Polling must call the existing read-only task-list path through the already-owned source session; it must not create or refresh a second session.
- No database transaction or process lease may remain held across an HTTP request. No credential, Cookie, Token, raw request/response, or encrypted SSP body is persisted.

### Persistence model

Use additive, idempotent SQLite migrations for three operational tables:

1. `cloud115_bt_monitor_run`: one RCH batch correlation record with batch ID, account key, source trigger, target folder ID, started/ended time and aggregate counters. Counters are derived from ledger + monitor state, not used as task authority.
2. `cloud115_bt_task_monitor`: one current projection per account/Infohash/target-folder key, with the ledger correlation, normalized remote state, safe remote task identifier when available, remote folder evidence, percent when valid, `last_checked_at`, `next_check_at`, attempt count, last error class, and a short cross-process lease owner/expiry.
3. `cloud115_bt_task_event`: append-only meaningful state/classification changes (including first observation, transition, error-class change, and recovery). Identical repeated poll results update `last_checked_at` and `next_check_at` but do not append duplicate events.

Use a unique constraint on the same identity tuple as the ledger. Keep identifiers stable across launches without using Cookie/Token. Add indexes for account + due time, account + Infohash, and batch ID. Migrations must be safe on existing databases and must not change the library JSON import/schema marker merely because operational tables were added.

Ledger-to-monitor reconciliation is idempotent: on startup and after a batch, read the ledger and upsert eligible monitor rows. A crash between ledger append and SQLite upsert is repaired this way. A monitor row never flows in the opposite direction to the write ledger.

### Polling contract

`Cloud115BtMonitor` owns due-time scheduling only while the app process is alive. A monitor cycle:

1. Claim one short SQLite lease per account in a transaction; re-read eligible due rows while holding that transaction; commit and release the DB transaction before networking.
2. Use one bounded, paginated task-list snapshot for that account and polling cycle, not one request per torrent. Match all tracked Infohashes in memory and require account and intended-folder evidence before a row can satisfy same-target cleanup/completion rules.
3. Persist only normalized results and the next due time. Release the lease in `finally`. Lease expiry must exceed the maximum bounded HTTP deadline plus margin so a slow request cannot overlap another process's poll; a crashed owner is recoverable after expiry.
4. If page count changes, the configured page/request budget is exhausted, the response is malformed, or the API result does not prove a task's folder/status, preserve `unknown` and schedule a bounded recheck. An absent hash is definitive only when the service contract and complete stable snapshot make absence meaningful; otherwise it stays unknown.
5. Use bounded backoff (initial proposal: 1, 5, 15, then at most every 30 minutes), honoring explicit `Retry-After` within a safe cap. Auth expiry waits for the existing source session; WAF/challenge and access denial pause that account. No high-frequency timer runs when no row is due.

### State and evidence

Keep batch outcomes (`parsed`, `upload accepted/unknown`, `BT submit accepted/unknown`, duplicate, local seed cleanup result) separate from remote lifecycle (`queued`, `downloading`, `completed`, `failed`, `unknown`) and RCH discovery (`not checked`, `discovered`, `waiting`). Unknown API codes map to `unknown`, never to a guessed terminal state. Persist status code only after its meaning is supported by a trustworthy contract/test vector; do not treat a localized status label as the state machine.

Current-code evidence limits the first implementation: the task-list reader is bounded to 10 pages × 30 entries, checks for page/count instability, matches by Infohash, and currently extracts status/display text and progress. It does not yet provide verified remote task ID, target-folder identity, or a validated terminal-state map. Until each item is independently established, absent rows, terminal state, same-target completion, and cleanup eligibility remain unknown. Do not expand the budget or infer folder identity from the UI-selected folder.

Local `.torrent` cleanup remains governed by SPEC §10.5 and the existing opt-in. A remote same-hash match in an unverified/different folder does not prove the intended destination is complete; retain the local seed unless the required same-account and target-folder evidence is positive. Upload acceptance by itself never permits deletion.

### UI and lifecycle

Show one compact batch/monitor summary with counts sourced from ledger + monitor projections: parsed, submitted/accepted, remote duplicate found, local `.torrent` removed, queued/downloading, completed, failed, and unknown. Keep “115 accepted”, “115 download completed”, and “RCH library discovered” visibly distinct. A completed count requires a validated terminal status; “library discovered” additionally requires the existing targeted remote scan to find the file.

Polling is suspended when RCH exits. On startup, reconcile JSONL to SQLite and resume due read-only checks; do not resume EH scans or replay writes from the monitor. This is not a tray/service/background promise.

### SPEC §10.5 additive wording proposal (not applied)

> A separate operational SQLite projection may record only RCH-attributed Cloud115 task monitoring state and due times. It contains no credentials or raw responses, is not canonical library metadata or a sync entity, and never authorizes or replays upload/BT-submit writes. Closing RCH pauses local monitoring; a later launch may resume bounded read-only checks. P0 and Daily write gates remain unchanged.

This wording is a proposal only. Do not edit `docs/project/SPEC.md` or create the SQLite tables until this additive storage boundary is explicitly approved for implementation.

### Exit behavior integration

The exit prompt is designed in the existing `.trellis/tasks/08-01-exit-behavior` task. Its in-memory registry represents currently executing process work; durable Cloud115 monitor rows represent resumable remote work awaiting another poll. The exit summary combines both without copying domain state. Closing stops local polling and foreground workers after their existing cancellation/flush path; the remote 115 task continues independently.
