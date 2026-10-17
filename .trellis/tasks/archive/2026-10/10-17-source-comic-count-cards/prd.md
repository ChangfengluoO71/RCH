# 书源漫画计数与卡片信息整理

## Goal

修正书源漫画数量口径；整理书源卡片并将完整扫描详情移入书源详情；本地和云端使用不同图标。

## Requirements

- 书源漫画数按当前有效索引中的唯一可阅读资源计算：支持的漫画压缩包/电子书与标记为 `ImageFolder` 的图片漫画文件夹；排除普通目录、散图、已删除索引项。
- 本地/SMB 按现有可阅读候选分类计数；云端文件和图片文件夹仅在对应远程目录列表有完整发布证明时计数。
- 复用现有路径归一化与格式别名去重规则（如 ZIP/CBZ），不改变离线索引总数与 `canBrowseOffline` 的语义。
- 书源卡片保持紧凑，使用不同图标区分本地文件系统来源与云端来源；仅显示漫画数，不显示阅读状态指示或文字，不提供离线书目折叠按钮；点击书源行仍进入正常浏览。
- 将完整远程扫描状态和书源统计信息移入“书源详情”，不在打开详情时发起扫描或网络请求。
- 不修改数据库结构、书源认证、阅读链、扫描行为或用户数据。
- 保留工作区中与本任务无关的既有修改。

## Acceptance Criteria

- [x] Rust 书源树 DTO 同时返回漫画数量和原离线索引数量。
- [x] 计数过滤普通目录、散图和已删除条目，云端仅计完整目录证明，ZIP/CBZ 等别名按现有规则去重。
- [x] 书源卡片仅显示名称与漫画数，本地和云端图标有明确区分；不显示阅读状态指示或文字，移除离线书目折叠按钮。
- [x] 书源详情显示漫画数、索引数及完整远程扫描状态。
- [x] FRB 绑定由官方生成器更新；不手工修改生成文件。
- [x] 不新增数据库表/列，不触发网络或扫描。

## Notes

- Keep `prd.md` focused on requirements, constraints, and acceptance criteria.
- Lightweight tasks can remain PRD-only.
- For complex tasks, add `design.md` for technical design and `implement.md` for execution planning before `task.py start`.
