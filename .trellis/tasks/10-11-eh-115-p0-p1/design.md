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
