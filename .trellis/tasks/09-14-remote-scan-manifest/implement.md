# 统一云端清单与增量扫描契约：执行计划

**Canonical plan:** `docs/superpowers/plans/2026-09-14-remote-cloud-scan.md` Task 1。

- [ ] 在 `app/rust/src/remote_scan/model.rs` 先写 `RemoteAssetKind`、指纹、自然排序和图片扩展测试。
- [ ] 在 `app/rust/src/remote_scan/adapter.rs` 写 fake adapter contract，覆盖分页 cursor、metadata 缺失和结构化错误。
- [ ] 在 `app/rust/src/db/mod.rs` 写 additive migration 测试，验证 `library_index` 新字段、`remote_scan_state`、`remote_listing_state`、`remote_cover_dependency` 可重复迁移。
- [ ] 实现 persistence 的完整 listing 事务、checkpoint 读取和 proof-carrying tombstone；失败事务保留旧清单。
- [ ] 将 `FolderSnapshotEntry` 升级到 v2，保留 size/mtime/asset kind/fingerprint，并在 `remote_listing.dart` 保留 `DirEntry` 元数据。
- [ ] 运行 `cargo test remote_scan db:: -- --test-threads=1`、`flutter test --no-pub test/remote_scan_manifest_test.dart test/folder_snapshot_store_test.dart` 和 `git diff --check`。
