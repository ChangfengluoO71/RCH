# v0.6.2 文档与 Trellis 状态收敛

## Goal

Reconcile the current-state, user-guide, architecture, TODO, and Trellis records with the shipped v0.6.2 release and the verified work still in progress.

## Requirements

- Keep `README.md` as the current product description; document the shipped E-site metadata import separately from offline catalog scraping and the optional EH subscription plugin.
- Update `docs/user-guide.md` with the v0.6.2 import and update/install flows while preserving the offline-only guarantees of catalog scraping.
- Replace the obsolete v0.1.0 snapshot in `docs/architecture.md` with the current Flutter/Rust module boundaries and evidence-backed known gaps.
- Rebuild `docs/project/TODO.md` as a concise current handoff linked to Trellis tasks; leave detailed historical evidence in append-only `LOG.md`.
- Correct stale Trellis notes where later archived evidence proves completion, and state remaining follow-ups separately. Do not mark unverified tasks complete.
- Append this documentation work to `LOG.md` and update `LOG-INDEX.md`.
- Do not modify `SPEC.md`, application code, release artifacts, or the two pre-existing untracked announcement files.

## Acceptance Criteria

- [x] README, user guide, architecture document, TODO, and active Trellis task records agree on v0.6.2 shipped scope and current open work.
- [x] Historical task evidence remains traceable; no unverified acceptance gate is marked complete.
- [x] LOG entry is appended and LOG-INDEX points to the exact new lines.
- [x] Diff contains documentation/Trellis changes only and preserves pre-existing untracked files.

## Notes

- Keep `prd.md` focused on requirements, constraints, and acceptance criteria.
- Lightweight tasks can remain PRD-only.
- For complex tasks, add `design.md` for technical design and `implement.md` for execution planning before `task.py start`.
