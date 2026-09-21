# 封面依赖与失效缓存清理：执行计划

**Canonical plan:** `../09-14-remote-cloud-scan/research/2026-09-14-remote-cloud-scan.md` Task 4。

- [ ] 先写完整/部分/失败列表与缓存依赖测试，固定 tombstone proof 条件。
- [ ] 在 `remote_scan/persistence.rs` 让 tombstone API 接受 complete listing、有效 session 和 generation 校验。
- [ ] 在 `api/cache.rs` 按逻辑 BookKey、cover aliases、依赖图片和 page/raw 扩展 stale cleanup；阅读完成路径继续只清内容。
- [ ] 在 `library_store.dart`/`remote_cache_cleanup.dart` 仅消费 engine 已验证 tombstone，保留 `alignFailed` 失败语义。
- [ ] 验证首图删除/改名会重新排队，整文件夹删除按依赖路径清理，无关路径不受影响。
- [ ] 运行 Rust cache/db 与 Flutter cleanup 回归测试并检查临时缓存根。
