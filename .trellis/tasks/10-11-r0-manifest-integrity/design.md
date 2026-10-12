# R0 Manifest Integrity Design

## Scope

Keep the manifest schema and legacy `read_manifest()` behavior unchanged for its other callers. Make the collector use a strict reader before creating a client or issuing any request. A missing file and a valid empty array both start with empty history; any read or JSON/schema error stops collection with a typed-in-text error and leaves existing data untouched.

At commit time, serialize writers across processes without creating sidecar files: use a named Windows mutex keyed by the canonical output directory, and an advisory lock on the output-directory handle on Unix platforms. Under the lock, reload the latest manifest, merge new records by case-insensitive infohash, and atomically replace `manifest.json`. This keeps SPEC §10’s sole write outputs unchanged.

Write JSON to a uniquely created same-directory temporary file, flush and `sync_all`, parse it back and verify the intended records, then replace the target without first deleting it. On Unix, sync the parent directory after rename where supported. On Windows, use the standard library replacement operation only after a real Windows regression proves replacement of an existing target; document that OS/filesystem and power-loss durability are not proven by process-level tests. Remove only the temporary file created by the current operation on ordinary failure; ignore abandoned temp files during reads.

Keep H0 checked-reader categories unchanged. Do not change `read_manifest()` semantics, schema, filtering, network behavior, canonical metadata, or SPEC §10 write boundaries.

## Error distinctions

Collection errors distinguish a damaged manifest, an unreadable manifest, an atomic write failure, and a concurrent writer conflict. A successful run with no new entries is a normal zero-addition result and does not rewrite the manifest.

## Validation

Unit tests exercise strict load, atomic replacement failure points, history merge/deduplication, stale temp isolation, Unicode paths, and serialized competing commits. Injected failures test process-level error recovery only; they are not power-loss durability tests.
