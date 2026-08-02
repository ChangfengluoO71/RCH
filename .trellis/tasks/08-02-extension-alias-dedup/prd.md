# 漫画后缀名变更识别（zip→cbz 视为同一本）
## Goal

漫画文件仅后缀别名变更（如 `xxx.zip` → `xxx.cbz`）时，仍识别为同一本漫画，阅读进度/标签/元数据不丢失、不重复。

## Background（已确认事实）

- 书 key 生成规则：`${sourceType}|${sourceId}|${path}` [app/lib/store/library_store.dart:485]；`RecordRepository.keyOf` 同规则 [app/lib/store/library_store.dart:300]
- key 包含完整路径与扩展名，后缀改名后 key 变化 → 被当作新书
- 格式别名关系：zip↔cbz、cbr↔rar、cbt↔tar、cb7↔7z；mobi/azw/azw3 系列
- Rust 侧按扩展名分发格式引擎（document/），识别列表需与 Dart 侧 key 规范化共用同一规则，避免两侧不一致

## Requirements

- **R1** 定义格式别名规范化函数（扩展名归一化，如 `.cbz → .zip` 或统一去除容器别名），Dart 与 Rust 各实现一份并加一致性单测（或 Rust 提供 API 供 Dart 调用）。
- **R2** 所有 key 生成/查找走规范化路径：后缀改名后命中同一 key。
- **R3** 存量数据迁移：启动或首次访问时将已有记录按规范化 key 归并（阅读记录、标签关联、BookMeta 合并，保留最新阅读进度）。
- **R4** 仅后缀别名变化才归并；文件名主体变化（如 `xxx.zip` → `yyy.zip`）不归并。
- **R5** 归并冲突：两侧都有记录时保留 lastPage 更大 / updated_at 更新的记录。

## Acceptance Criteria

- [ ] 同一目录下 `xxx.zip` 改名为 `xxx.cbz` 后：最近阅读列表不出现重复条目；已读进度、标签、封面、感想均保留
- [ ] 改名后重新打开漫画，续读位置正确
- [ ] 真正的新文件（不同文件名）不受影响，仍为新书
- [ ] 存量已有重复记录的库，升级后自动归并为一条（迁移单测覆盖）
- [ ] Dart/Rust 两侧规范化规则一致性单测通过；`flutter analyze` 0 issues；`cargo test --lib` 通过

## Out of Scope

- 内容 hash 级去重（仅处理后缀别名）
- 跨目录/跨书源识别同一漫画
- 重命名后同步物理文件（不动用户文件）

## Open Questions

- 无阻塞问题。归并触发时机（启动迁移 vs 惰性首次访问）按 R3 推荐启动迁移（量小时无感）。
