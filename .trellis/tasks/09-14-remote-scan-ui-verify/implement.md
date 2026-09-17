# 云端扫描界面与跨 provider 验证：执行计划

**Canonical plan:** `docs/superpowers/plans/2026-09-14-remote-cloud-scan.md` Tasks 5–6。

- [ ] 先写设置迁移、首次触发一次、后续增量、重扫模式和重复点击合并测试。
- [ ] 在 `home_page.dart` 增加默认开启且可持久化的 `remoteBackgroundScanEnabled`，保留远程封面总开关语义。
- [ ] 实现 `remote_scan_status.dart` 来源/全局状态面板、暂停/继续/重试、增量/全量重扫和 kill switch。
- [ ] 连接 `source_browser.dart` 根目录与 `remoteSessionFor` 成功事件，复用首个目录列表并在重启时恢复。
- [ ] 建立五 provider fake contract、日志脱敏断言和真实 smoke 证据模板。
- [ ] 执行 Rust/Flutter/Windows 完整门禁，生成 evidence report；缺失真实证据标为 pending。
