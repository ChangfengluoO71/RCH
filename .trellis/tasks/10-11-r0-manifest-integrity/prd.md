# R0_MANIFEST_INTEGRITY: EH subscription manifest loss prevention

## Goal

Independent follow-up for confirmed EH manifest integrity risk. The collection path still reads legacy read_manifest(), which collapses I/O/JSON errors to an empty list, then directly overwrites manifest.json at scan completion. A damaged or interrupted manifest can therefore cause prior history to be replaced. Define and implement atomic validated replacement and non-destructive recovery, and add regression coverage. Do not change this as part of H0 GUI acceptance.

## Requirements

- EH collection must preserve valid manifest history when the existing manifest is missing, unreadable, or malformed.
- A failed manifest read must not be treated as an empty history and then overwrite the existing file.
- Manifest replacement must not expose a partially written JSON file after interruption.
- Keep the manifest schema and compatibility with existing records intact.
- Keep canonical manga metadata and unrelated scan behavior outside this task's scope.

## Acceptance Criteria

- [ ] Collection uses an error-aware manifest read path and stops or recovers explicitly on invalid existing data.
- [ ] Manifest writes use a validated atomic replacement strategy appropriate for supported platforms.
- [ ] Regression tests prove malformed or interrupted writes cannot silently erase prior valid history.
- [ ] Existing manifest records remain readable after the migration/change.
- [ ] Focused Rust tests and relevant repository checks pass.

## Notes

- Keep `prd.md` focused on requirements, constraints, and acceptance criteria.
- Lightweight tasks can remain PRD-only.
- For complex tasks, add `design.md` for technical design and `implement.md` for execution planning before `task.py start`.
- Risk confirmed during H0-GUI-R1 static audit: `app/rust/src/eh_subscription.rs::collect` initializes from legacy `read_manifest()` and later calls `std::fs::write(manifest.json, ...)` directly. The legacy reader maps read/parse errors to an empty vector.
- H0-GUI-R1 did not modify the scan or manifest write path.
- This task is registered independently and remains unstarted.
