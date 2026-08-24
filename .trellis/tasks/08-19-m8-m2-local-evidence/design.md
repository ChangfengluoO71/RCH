# M8-M2 — Canonical Identity & Migration — Technical Design

## Migration

Add `db_migrations(version, name, applied_at)` and ordered transactional migration v1 for `works`, `work_external_ids`, `work_links`, `work_field_provenance`, and `canonical_sync_dirty`. Leave `schema_version` untouched as the `library.json` import marker.

## Canonical Relations

- `works.id` is a generated stable text ID.
- `work_external_ids` is unique by `(provider, external_id)` and references `works`.
- `work_links` is unique by `(work_id, book_key, relation_kind)` and references `works`.
- canonical rows use millisecond `updated_at` and tombstone `deleted`.
- `canonical_sync_dirty` is a local queue marker only; repository operations never call sync transport.

## Compatibility

Existing `book_metas`, `library_index`, sync import version and reader behavior remain unchanged. M8-M2 does not add canonical rows to sync snapshots; that is M8-M5/M8-M6 work after confirmation behavior is defined.
