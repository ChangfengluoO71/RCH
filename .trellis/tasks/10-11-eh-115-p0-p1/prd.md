# EH Subscription to 115 Auto Save — P0 Gate

## Goal

Enable EH subscription networking that can use direct, system, or an explicitly configured proxy, then verify the 115 torrent route only through a documented and authorized API contract. Preserve existing EH filters and the RCH 115 reading path.

## Requirements

- Treat the checked-out HEAD and live remote state as facts; preserve all pre-existing user work.
- Complete the P0 network and contract audit before implementation beyond safe P0-A support.
- Use EH proxy modes `system`, `direct`, and `manual`; never silently fall back between routes, disable TLS checks, or print proxy credentials.
- Keep the new 115 torrent route separate from EH and from existing RCH 115 reading clients. Its default route is direct.
- Remove unsafe write/clear probes from `cloud115_offline_probe.rs`; only documented, read-only diagnostics may remain.
- Do not bypass EH/115 challenges or WAF. Respect existing 115 rate gates and cooldown behavior.
- Persist P0 evidence and accurately classify any blocked or partial result.
- Do not begin P1 unless every P0_REAL_PASS item has been demonstrated with one authorized real `.torrent`.
- Do not commit, push, merge, rebase, tag, release, or delete/clean user data.

## Acceptance

P0 is `P0_REAL_PASS` only when the EH main and Tracker `.torrent` paths, authenticated 115 folder access, one real Torrent upload, BT submission, successful cloud download, target-folder files, and RCH reading all pass. Otherwise record `P0_PARTIAL`, `P0_BLOCKED`, or `P0_FAIL` with evidence and stop before P1.

The local code must not issue any guessed 115 write endpoint or destructive task-cleanup request. Any implemented P0-A proxy configuration must validate before save and make the selected route explicit.

## Approved follow-up: durable Cloud115 BT task monitoring

This is a read-only monitoring follow-up to the existing manual BT batch and the separately gated Daily workflow. It does not change the P0 result, authorize automatic writes, or start P1 automatic submission. Keep this scope distinct from the historical Open API and Web protocol research above.

### User outcome

- After an RCH-initiated batch, show how many Torrents were parsed, submitted, confirmed as existing duplicates, safely removed locally, and are queued, downloading, completed, failed, or still unknown.
- Continue checking RCH-tracked remote tasks while RCH is running. Persist the latest state and next check time so monitoring resumes after a later launch.
- On exit, tell the user which local work will pause and which remote 115 downloads will continue. Do not claim local monitoring continues while RCH is closed.

### Requirements

1. Reuse the existing 115 Cookie source session, WAF/rate gate, batch ledger, and read-only task-list route. Do not create another login/session or refresh-token owner.
2. Monitor only tasks attributed to an RCH batch/ledger. Never enumerate or adopt unrelated account tasks as RCH work.
3. Keep the append-only JSONL batch ledger as the authority for upload/submit idempotency. SQLite stores monitoring projection, meaningful state history, and due times; it must never authorize a write retry.
4. Fetch one bounded task-list snapshot per account and polling cycle, then match all tracked Infohashes locally. Do not issue one task-list request per Torrent.
5. Treat partial pagination, changing page counts, request errors, auth failures, challenges, unrecognized status codes, and absent rows in an incomplete snapshot as unknown/waiting. Never infer completion or failure from absence or display text.
6. Report a remote duplicate separately from successful upload or completed cloud download. Keep “BT task accepted”, “cloud download completed”, and “RCH library discovered” as distinct states.
7. Delete a local `.torrent` only when the existing cleanup setting is enabled and the evidence required by SPEC §10.5 is positively met. Upload acceptance alone is insufficient. Preserve the seed when account, Infohash, target-folder, or remote-result evidence is incomplete.
8. Use bounded polling/backoff and existing request gates. Do not poll when no RCH-tracked active/unknown tasks are due. Do not continue polling through WAF, challenge, auth expiry, or explicit rate limiting.
9. RCH close pauses local scans, writes, and polling. Remote 115 work is unaffected. The next RCH launch may resume read-only monitoring from SQLite and reconcile uncertain states without repeating writes.
10. Persist no Cookie, Token, request headers, raw response body, or other credential material. Do not store raw remote payloads.
11. The existing “Daily full workflow” remains fail-closed behind `P0_REAL_PASS`; monitoring does not relax or bypass that gate.

### Acceptance

- A mocked paginated snapshot updates all tracked tasks in one account-level read cycle and leaves unrelated remote rows untouched.
- Restart resumes due monitoring from stored state without issuing upload or submit calls.
- Incomplete/unstable pagination and unknown status values remain unknown and never trigger seed deletion or a success count.
- Rate-limit, auth, WAF/challenge, offline, and transient errors receive separate persisted classifications and bounded retry times.
- UI counts distinguish parsed, accepted, duplicate-found, local cleanup, downloading, completed, failed, and unknown. “Completed” is shown only after the terminal state and destination evidence are verified.
- No task-status mapping or target-folder evidence is assumed from SDK examples. Any unverified real contract remains explicitly unknown; no live write is part of this task’s implementation tests.
- Exiting pauses local polling and restarting resumes it; no Windows service, tray daemon, or promise of work while the app is closed is introduced.
- P0 remains `P0_WEB_WRITE_AUTH_BLOCKED` and P1 automatic submission remains `NOT_STARTED` until its independent gate is satisfied.
