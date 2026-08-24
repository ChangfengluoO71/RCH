# M8-M5 — Review, Confirmation & Sync-Dirtiness

## Goal

让用户看懂、修改并确认本地规则结果或 Provider 候选，通过唯一原子事务把选择物化为作品身份和文件关系。

## Requirements

- Flutter `WorkRepository + ScrapeController`，FRB job API，单项/批量 Review UI。
- 展示来源、逐项分数、警告、部分失败与采纳字段；支持确认、拒绝、刷新。
- Provider 无结果时仍能审阅 M8-M1 的本地 title/author proposal。
- `confirm_proposal` 是唯一 canonical 写入入口，具备 revision check、原子性、幂等性和 typed conflict。
- UI 关闭、重启或请求重试不得造成重复 work/link 或半确认状态。
- 确认只完成 SQLite transaction 与 sync-dirty 标记；同步传输由既有调度或用户操作在事务外执行。
- 展示由 M8-A0 自动流程产生的 pending/last-result/degraded 状态；“立即刮削”只触发本地队列，“运行完整周期”遵循先同步后刮削。

## Acceptance Criteria

- [ ] Windows 完成选择 → 分析 → 候选审阅 → 确认/拒绝 → 重启保持的闭环。
- [ ] 确认前 canonical 零写入；确认后 work、external IDs、links、provenance 一次提交。
- [ ] 重放确认请求不重复创建；过期 proposal 被拒绝并提示刷新。
- [ ] 任一步故障完整回滚，批量操作逐项返回结果且不掩盖失败。
- [ ] sync transport spy 证明 `confirm_proposal` 期间没有调用 `sync_now`、sync actor 或任何远程同步端点；同步/远程书源离线时仍可确认。
- [ ] 自动生成的 proposal 可在重启后恢复审阅；确认后只发出 canonical-changed/sync-dirty 事件，下一次既有同步周期才处理传输。

## Dependencies / Out of Scope

- 依赖 M8-M1 至 M8-M4；同步传输仍不属于本任务的确认调用路径。
- 不实现静默自动确认；不以“确认全部”跳过每项 proposal 的可审阅记录。
