# Implementation Plan — P0 Only Until Gate Passes

## Ordered work

1. Reconfirm baseline, current worktree, and remote HEAD; complete code/docs and proxy behavior audit.
2. Remove all writes, add-task candidate probing, and task-clear calls from the legacy 115 probe. Keep only documented read-only network/auth/folder checks, or make unsupported probes clearly unavailable.
3. Implement the minimum EH network policy/config validation and bounded main-site/Tracker diagnostics. Keep existing filter behavior and default settings compatible.
4. Add read-only 115 direct diagnostics only if they can reuse an already-held session access token and existing gate without changing the reader client's route or rotating tokens. Otherwise record the exact blocker.
5. Run focused Rust and Flutter checks plus fault-injection tests for proxy validation, route isolation, challenge/status classification, credential redaction, and probe write absence.
6. Review official 115 documentation/SDK and local authorization/resources. Execute at most one actual write chain and only after the official API contract, active session, target folder, and authorized test Torrent are verified. Never try candidate write endpoints.
7. Classify P0 with evidence. If P0_REAL_PASS is not fully met, stop. Do not implement P1 in this task.

## Validation commands

- `cargo tree --locked --offline -e features -i reqwest`
- Focused Rust tests for changed EH/network and probe code, followed by repository-required Rust checks if P0 remains eligible.
- Focused Flutter tests/analyzer for any settings UI change.
- Static audit: `rg -n '\.post\(|offline/clear|add_task' app/rust/examples/cloud115_offline_probe.rs`; the safe probe must contain no task-creation or cleanup request.
- `git status --short` and diff review; no commit or destructive cleanup.

## Stop conditions

- Any requirement to send an undocumented or guessed 115 write request.
- No current authorized session, no authorized test Torrent, or no route to safely identify the target folder.
- WAF/challenge, unknown write result, token-rotation race, or impact to the existing reader path.
- Any manifest/history loss or test regression.

## Current execution record

- Baseline and remote state were rechecked at task start; the branch was clean before this task and already contained three commits beyond the requested `origin/master` base.
- EH proxy modes, validation, SOCKS support, host/Tracker-root probe metadata, and a settings panel were implemented. Tracker root checks are explicitly transport-only; this does not validate Torrent bytes. Existing 115 reader clients were not changed.
- The unsafe 115 probe was retired without network activity. Official public 115 materials found no published BT upload/submission contract. The local Torrent folder was inspected for names/sizes only; no specific authorized test sample was identified.
- No live RCH process or existing in-memory 115 session was available. No token refresh, authenticated account probe, or write request was made.
- P0 is `P0_BLOCKED`; P1 remains unstarted. See `research/p0-gate.md` for evidence and constraints.

## Follow-up TODO

- [ ] Fix the 115 source status indicator: the user confirms the existing session is connected, while the UI says `需连接`. Do not use this label as evidence that authentication failed. Derive the displayed state from a successful existing-session request and add regression coverage for connected, disconnected, and expired sessions. This is tracked for follow-up and is not implemented as part of the current P0-B retry.
