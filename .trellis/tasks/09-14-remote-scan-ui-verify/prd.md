# 云端扫描界面与跨 provider 验证

## Goal

提供首次/增量/手动重扫的 Dart 控制和状态 UI，维护设置兼容，并完成五类云端 provider 的 fake contract、真实 smoke 和工程门禁证据。

## Requirements

- 新增 `remoteBackgroundScanEnabled`，默认开启；保留 `remoteCoverFetchEnabled` 作为远程封面网络总开关。
- 来源页/全局状态显示 queued/running/complete/degraded/paused/rangeUnavailable、处理计数和脱敏错误。
- 提供继续、暂停、重试、增量重扫和全量重扫；重复点击合并 job。
- 首次授权/根目录打开触发一次 full，后续打开/授权触发 incremental；应用重启恢复状态。
- fake provider 覆盖分页、Range、限流、权限、认证失效、截断列表、删除和图片文件夹；真实证据缺失标记 pending。

## Acceptance Criteria

- [ ] 设置迁移和显式 false 持久化测试通过；关闭设置不会删除已有封面。
- [ ] UI 状态转移、重扫模式、取消/恢复和 kill switch 测试通过。
- [ ] 五个 provider contract 和日志脱敏检查通过。
- [ ] 完整 Rust/Flutter/Windows 门禁与 `git diff --check` 记录在 evidence report。
- [ ] 无真实账号/设备/样本时报告明确列出 pending，不能伪造 GREEN。

## Notes

实现细节和文件边界见父任务计划 Task 5–6；依赖 worker/manifest/reader/cleanup API 稳定。
