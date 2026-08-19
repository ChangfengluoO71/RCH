# Repository Documentation Reorganization

## Goal

Make the repository root focused on GitHub-facing entry points and project configuration, while grouping user, development, planning, history, issue, and release documentation under `docs/`. Preserve document content and working intra-repository links, then publish the focused documentation-only change to `origin/master`.

## Confirmed Facts

- GitHub convention files `README.md`, `LICENSE`, `CODE_OF_CONDUCT.md`, and `CONTRIBUTING.md` are in the repository root and should remain there.
- `AGENTS.md` and `CLAUDE.md` are agent/project instruction files; they remain in the root because external tooling discovers them there.
- Root documentation currently includes user, setup, planning, decision, log, changelog, issue, and release-note files. `docs/` currently contains only `architecture.md`.
- `README.md`, `CONTRIBUTING.md`, and `docs/architecture.md` contain relative Markdown links that will need new paths after relocation.
- Existing untracked directories `build_artifacts/`, `remote_state/`, `.agents/`, `.claude/`, and `.codex/` are local generated data, test state, or AI-tool configuration. They must not be added to the GitHub commit.

## Requirements

1. Keep these root files in place: `README.md`, `LICENSE`, `CODE_OF_CONDUCT.md`, `CONTRIBUTING.md`, `AGENTS.md`, `CLAUDE.md`, `.gitignore`, `.gitattributes`, and `mirrors.json`.
2. Move user documentation to `docs/user-guide.md`.
3. Move development documentation to `docs/development/setup.md`.
4. Move project records to `docs/project/`: `CHANGELOG.md`, `SPEC.md`, `DECISION.md`, `TODO.md`, `LOG.md`, `LOG-INDEX.md`, and the dated issue record.
5. Move all `release_notes_v*.md` files to `docs/releases/`.
6. Update all affected Markdown links and link text so navigating from root documents and documents within `docs/` works on GitHub.
7. Update `.gitignore` to prevent the identified local generated/test/AI-tool directories from being accidentally tracked, while retaining the tracked `build_artifacts/make_app_icon.py` helper.
8. Preserve the existing content changes in `README.md`, `SETUP.md`, and `app/lib/ui/library_page.dart`; do not discard or overwrite them.
9. Commit and push only the document reorganization and ignore-rule changes; do not include unrelated source modifications, local generated data, test output, or private synchronization state.

## Acceptance Criteria

- [ ] The root contains the required GitHub convention files and agent instructions, but no relocated documentation files.
- [ ] The moved files exist in the requested `docs/` categories and retain their content.
- [ ] Every repository-local Markdown link to relocated documentation resolves to an existing target when evaluated relative to its source document.
- [ ] `docs/architecture.md` links to the relocated specification.
- [ ] `.gitignore` excludes `.agents/`, `.claude/`, `.codex/`, `remote_state/`, and generated `build_artifacts/` outputs while keeping `build_artifacts/make_app_icon.py` trackable.
- [ ] The pushed commit contains no application-source changes, artifacts, remote state, or AI-tool configuration.

## Out of Scope

- Changing document prose other than path-dependent wording and links.
- Deleting local generated files or test data.
- Changing application behavior, release assets, version numbers, or the existing uncommitted source change.

## Risks and Deferred Items

- `README.md` is already substantially modified in the worktree. Its link repairs must be staged as a narrow documentation patch, not as the whole pre-existing rewrite.
- The existing setup-path edit and library-page test-directory edit remain uncommitted and are explicitly excluded from this task's commit.
