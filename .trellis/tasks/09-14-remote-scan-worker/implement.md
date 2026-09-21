# 云端后台调度与封面批处理：执行计划

**Canonical plan:** `../09-14-remote-cloud-scan/research/2026-09-14-remote-cloud-scan.md` Task 2。

- [ ] 在 `reader.rs` 新增 `RequestPriority::Scan` 测试，证明前台槽位保留、队列有界和优先级顺序。
- [ ] 实现 `remote_scan/engine.rs` 的 bounded worker、source rate gate、任务键去重、退避、取消检查点和 full/incremental 状态机。
- [ ] 在 `api/remote_scan.rs` 暴露 `remote_scan_start/status/pause/resume/cancel`，并由 `app/codegen.ps1` 生成 Dart 绑定。
- [ ] 为 WebDAV/SFTP/Baidu/115/Quark 接入 adapter factory，复用既有 session/client 和结构化错误。
- [ ] 实现 `RemoteScanCoordinator.ensureForSession`、增量/全量重扫和重启恢复；封面任务写 dependency。
- [ ] 运行 Rust engine/governor、Flutter coordinator、cover scheduler 测试与 `git diff --check`。
