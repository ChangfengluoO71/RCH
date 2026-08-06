# P3 实施清单

## 顺序 ✅ 已完成

1. db：`sync_tombstones` 表 + 墓碑写入（delete_* / unlink / set_book_tags / 前缀批量删除）+ `load_tombstones_for_sync` + `find_source_id_by_fingerprint`
2. db：六个 `merge_*_sync_on(conn, row, force)`（LWW + 墓碑应用）
3. rchpkg：`merge_package(conn, reader, force)`（fingerprint 匹配 + key 重写 + 幽灵书源 + 墓碑应用 + 设备注册）；导出 tombstones 并入 sync_tombstones
4. api：`rchpkg_import(path, force)`、`BookSourceDto` 增列、`db_list_devices`；codegen + release DLL
5. Dart：BookSource 模型、BookRepository 读写、SyncManager（crossDeviceSearch + 设备名）、globalSearch 过滤/标注、详情页幽灵态、SyncPanel 开关
6. 测试：Rust merge LWW / 墓碑 / 幽灵 / key 重写 / force（73 过）+ `flutter analyze` + `flutter test`（16 过）

## 验证

```bash
cd app/rust && cargo test
cd app && flutter analyze && flutter test
```

## 风险与回滚

- key 重写依赖 `|sourceId|` 出现在 key 中；路径含同名 token 的极端场景回退为按原 key 合并（不丢数据）
- 本地硬删除传播依赖墓碑写入点完整覆盖（delete_* 系列）；漏写只会导致该删除不同步，不影响数据安全
- 回滚：merge 只新增函数；`rchpkg_import` 改签名会波及 P2 调用点（同步改回）
