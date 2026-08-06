# P2 实施清单

## 顺序 ✅ 已完成

1. Rust：`WebDavClient::upload_file` / `download_file` / `make_dir` + `api/source.rs` FRB 函数
2. codegen 重建绑定 + release DLL
3. Dart：`sync_manager.dart`（app_settings 配置、push/pull/restore、状态、定时 Timer）+ `sync_paths.dart` 纯逻辑
4. Dart UI：`sync_panel.dart` + `home_page.dart` 设置页接入 + `main.dart` 启动初始化
5. 测试：冲突副本识别/路径构建 Dart 单测 5 个 + `cargo test`（69 过）+ `flutter analyze` + `flutter test`（16 过）

## 验证

```bash
cd app/rust && cargo test
cd app && flutter analyze && flutter test
```

## 手测清单（需真实环境，未执行）

- 模式 B：选临时目录 → 推送 → 改数据再推送 → 改目录内包 → 拉取 → 恢复；冲突副本文件不参与自动拉取
- 模式 A：需真实 WebDAV（如坚果云）：推送/拉取/恢复全链路；无 Range 服务器同样可用（PUT/GET 不依赖 Range）
- 重启后配置与最近状态保持

## 风险与回滚

- WebDAV MKCOL 在部分服务器 409（父目录不存在）——实现时先建 `RCH` 再建 `RCH/sync`
- 自动拉取在 P3 合并引擎前是"包覆盖本地"语义；推送前如检测到远端包比本地游标新，提示用户确认（避免丢本地编辑）
- 回滚：新增代码全部独立（webdav 方法 + sync 目录），删除即可
