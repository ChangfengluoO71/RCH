# 云端后台调度与封面批处理

## Goal

实现共享 Reader governor 的最低优先级云端扫描 worker，完成首次全量、后续增量、检查点、取消恢复和压缩包/图片文件夹封面批处理。

## Requirements

- 新增 `RequestPriority::Scan`，优先级低于当前页、预取和可见封面，并保留前台槽位。
- Rust worker 使用有界列表/封面队列、来源限速、全局并发上限、任务键去重和 cancellation token。
- 暴露 start/status/pause/resume/cancel FRB API；同来源重复触发合并为一个 job。
- 首次授权/根目录打开只启动一次 full；后续打开/授权启动 incremental；重启从目录检查点恢复。
- 429/5xx/断线只做有界退避；认证、权限、Range 不可用、malformed 和取消不重试风暴。
- 封面优先使用安全局部读取；Range 不可用保持占位符，不自动整本下载，并写入封面依赖状态。

## Acceptance Criteria

- [ ] foreground、prefetch、cover、scan 的 governor 顺序和队列上限测试通过。
- [ ] 重复 start 只返回同一个 job；暂停/取消不提交结果，重启可从最后完整目录继续。
- [ ] full/incremental generation、429 Retry-After、authExpired 和 rangeUnavailable 状态可复核。
- [ ] 每个 archive file/image folder 最多产生一个同键封面任务；没有 `Future.wait` 收集整棵树。
- [ ] FRB 生成绑定和 Dart coordinator 测试通过，日志无凭据/直链。

## Notes

实现细节和文件边界见父任务计划 Task 2；本子任务依赖 `remote-scan-manifest` 的模型和 SQL 契约。
