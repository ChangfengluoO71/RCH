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
