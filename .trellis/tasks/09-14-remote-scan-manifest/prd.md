# 统一云端清单与增量扫描契约

## Goal

为 WebDAV、SFTP、百度网盘、115 和夸克提供统一的远程目录清单、资产分类、指纹、generation 和完整性证明，支持首次全量、后续增量及安全 tombstone。

## Requirements

- 定义 `RemoteAssetKind`、`RemoteEntry`、`RemoteScanState`、`RemoteCoverDependency` 和 provider-neutral adapter contract。
- `library_index` 以 additive migration 保存 asset kind、内容指纹、generation 和 listing 完整标志；新增来源扫描状态、目录 listing 状态和封面依赖表。
- `FolderSnapshotEntry` 升级到 v2 并保留 size/mtime；v1 快照可读取但不能作为删除证据。
- 目录分页只有全部成功后才能提交清单和缺失项 tombstone；部分/失败响应必须保留旧完整清单。
- 图片扩展、隐藏文件过滤、自然排序和图片文件夹/容器/普通目录分类在 Rust 与 Dart 消费端保持一致。
- 不保存或记录凭据、Authorization、下载直链和用户原文件内容。

## Acceptance Criteria

- [ ] 五个 provider fake 都能映射为同一 `RemoteEntry`，缺少 mtime 时可用子清单 hash/unknown 表示。
- [ ] 图片文件夹、容器目录、压缩包和普通目录分类测试通过，首图与页序稳定。
- [ ] schema migration 可重复执行且不改变旧数据；v1 快照不会产生 tombstone。
- [ ] 完整分页写入 `listing_complete=true`，截断/错误/取消保留上一份完整清单。
- [ ] 根指纹相同的增量扫描不递归未变化子树；内容指纹变化会使封面依赖失效。
- [ ] `cargo test remote_scan db:: -- --test-threads=1` 与 `flutter test --no-pub test/remote_scan_manifest_test.dart test/folder_snapshot_store_test.dart` 通过。

## Notes

实现细节和文件边界见父任务 `../09-14-remote-cloud-scan/research/2026-09-14-remote-cloud-scan.md` Task 1；本子任务不启动后台 worker、不改 UI。
