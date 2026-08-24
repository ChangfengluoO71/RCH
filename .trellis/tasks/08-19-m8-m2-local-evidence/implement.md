# M8-M2 — Canonical Identity & Migration — Implementation Plan

- [ ] Add failing in-memory migration ledger, rollback, future-version, table and uniqueness tests.
- [ ] Implement ordered migration runner and canonical DDL.
- [ ] Add Rust repository row types, idempotent upserts, tombstones and sync-dirty markers.
- [ ] Verify no `book_metas.work_id`, no `schema_version` reuse, no sync transport calls.
- [ ] Run `cargo test db::`, `cargo test`, `cargo fmt --check`, and clippy.
