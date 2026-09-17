# 全云端远程扫描与封面索引：执行计划

规范化实现计划位于 [`docs/superpowers/plans/2026-09-14-remote-cloud-scan.md`](../../../docs/superpowers/plans/2026-09-14-remote-cloud-scan.md)。本文件是 Trellis 父任务的执行索引，执行者必须同时阅读该计划和设计 spec。

## 执行顺序

- [ ] **阶段 0：工作区与安全边界**
  - 记录 `git status --short`、HEAD 和已有 dirty 文件列表。
  - 只允许修改计划 File Map 中的文件；不使用 `git reset`、`git clean` 或覆盖用户已有文件。
  - 准备隔离 fake provider fixture；真实凭据只能来自当前进程/授权会话，不能写入仓库。

- [ ] **阶段 1：`remote-scan-manifest`**
  - 完成 `RemoteAssetKind`、`RemoteEntry`、`RemoteProviderAdapter`、schema migration、快照 v2 和目录完整性证明。
  - 通过 model、migration、fake contract 和 Dart snapshot 测试后才进入阶段 2。

- [ ] **阶段 2：`remote-scan-worker`**
  - 完成 `RequestPriority::Scan`、Rust bounded worker、full/incremental state machine、FRB API、Dart coordinator、封面批处理。
  - 验证前台 > 预取 > 扫描、任务去重、取消不提交、重启续跑和 Range 降级。

- [ ] **阶段 3：`remote-folder-reader`**
  - 完成 `RemoteFolderBook`、各 provider 的图片文件夹打开路径、SourceBrowser/Reader 接入和按页缓存。
  - 验证不产生整本文件夹 raw，单图片上限和完成清理契约保持不变。

- [ ] **阶段 4：`remote-cover-cleanup`**
  - 完成封面依赖表、proof-carrying tombstone、逻辑键/别名/依赖缓存清理。
  - 验证部分列表、认证/网络失败、取消和暂时 404 均保留旧封面。

- [ ] **阶段 5：`remote-scan-ui-verify`**
  - 完成后台扫描设置、来源页/全局状态 UI、重扫控制和 kill switch。
  - 验证首次触发一次、后续增量、手动全量、设置迁移和已有封面保留。

- [ ] **阶段 6：跨 provider 验证与工程门禁**
  - 运行五类 provider fake contract、Rust/Flutter 回归、FRB codegen、APK/Windows 构建和 `git diff --check`。
  - 真实 provider/设备/样本缺失时标记 pending，不能把 fake 或占位数据写成 PASS。

## Review gates

每个阶段结束必须：

1. 运行该阶段列出的 targeted tests；
2. 运行 `git diff --check` 并检查没有秘密、大文件或临时产物；
3. 记录失败证据和回滚点；
4. 获得父任务审查后再进入下一阶段。

全量实现完成后执行计划中的完整工程门禁。当前任务不提交 tag、release 或版本变更；任何提交动作都需要独立明确授权。

## Stop conditions

- schema migration 不能幂等、旧数据无法读取或发现空库初始化风险：停止阶段 1。
- 扫描能够占用前台 Reader 槽位、队列无界或取消会写成功状态：停止阶段 2。
- 图片文件夹 Reader 需要整本下载或完成清理删除封面：停止阶段 3。
- 未经完整目录证明就产生 tombstone 或清理缓存：停止阶段 4。
- 真实 provider 证据缺失、工程门禁失败或出现凭据泄露：保持 `YELLOW / DO NOT RELEASE`，不得发布。
