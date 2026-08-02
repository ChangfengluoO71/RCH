# 标签管理：元数据标签分层折叠
## Goal

标签管理页的「元数据标签」目前是平铺列表，作者/类别/系列混在一起，量大时难找。按一级类别（作者 / 类别 / 系列 / 状态）折叠分组，点击类别再展开具体标签。

## Background（已确认事实）

- 标签管理页将标签分为 metaTags / normalTags 两组渲染 [app/lib/ui/home_page.dart:361-362]
- 元数据标签以单个 ExpansionTile 平铺展示（标题「元数据标签 (N)」，子项为每个标签的统计行）[app/lib/ui/home_page.dart:379]
- 标签行展示名称 + 关联漫画数 + 总阅读次数，支持重命名/删除 [app/lib/ui/home_page.dart:364-367]
- 元数据标签集合来自 author/genre/series/已读 四字段（metaTagNames / metaFields）[app/lib/store/library_store.dart:352, 480]
- 重命名/删除元数据标签会同步更新 BookMeta 的 author/genre/series 三栏 [app/lib/store/library_store.dart:432-434, 443-444]
- Tag 模型无类别字段（id + name + createdAt），类别需由 BookMeta 字段推断 [app/lib/store/models.dart:331]

## Requirements

- **R1** 元数据标签组内按一级类别分组：作者（author）/ 类别（genre）/ 系列（series）/ 状态（已读），每组一个可折叠 ExpansionTile，默认折叠，组标题显示类别名与该组标签数。
- **R2** 分组为显示层逻辑：不改 Tag 数据模型与持久化格式；类别归属由各 BookMeta 的 author/genre/series 字段推断。
- **R3** 标签出现在多个类别时的归属规则：推荐「所属书籍最多的类别」，平局取 author > genre > series。
- **R4** 搜索过滤时：命中组自动展开并只显示命中标签；空组隐藏。
- **R5** 标签统计行与重命名/删除操作保持不变；重命名后自动迁移到正确分组。
- **R6** 已读标签归入「状态」组，保持红色元数据标签样式。

## Acceptance Criteria

- [ ] 元数据标签按 4 个类别分组显示，每组可独立折叠/展开，折叠状态在会话内保持
- [ ] 搜索关键字时命中的组自动展开，未命中组保持折叠
- [ ] 重命名 author 标签后，其所在分组与 BookMeta.author 同步更新
- [ ] 普通标签（黄色）展示行为不变
- [ ] `flutter analyze` 0 issues

## Out of Scope

- 自定义多级标签树（用户自定义父/子标签）
- 标签拖拽排序
- 标签类别持久化（本次为显示层分组；后续需要时另行建任务）

## Open Questions

- 无阻塞问题。R3 归属规则按推荐值实现；若与用户预期不符可在验收时调整。
