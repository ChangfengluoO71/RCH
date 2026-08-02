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

- [x] 元数据标签按 4 个类别分组显示，每组可独立折叠/展开，折叠状态在会话内保持（按组名存于 `_metaExpandedGroups`）
- [x] 搜索关键字时命中的组自动展开，未命中组保持折叠/隐藏（`ExpansionTile` key 含搜索词强制重建）
- [x] 重命名/删除标签逻辑与展示行未改，分组由 `BookMeta` 字段实时推断，重命名后自动归组
- [x] 普通标签（黄色）展示行为不变
- [x] `flutter analyze` 0 issues

## Out of Scope

- 自定义多级标签树（用户自定义父/子标签）
- 标签拖拽排序
- 标签类别持久化（本次为显示层分组；后续需要时另行建任务）

## Open Questions

- 无阻塞问题。R3 归属规则按推荐值实现；若与用户预期不符可在验收时调整。

## Verification（2026-08-02）

- `flutter analyze`：No issues found（0 issues）。
- 代码走查：`_buildTagManager()` 按 作者/类别/系列/状态 分组渲染；`_metaTagCategory` 按"所属书籍最多字段"归类，平局 author > genre > series；已读 → 状态组且保留红色元数据标签样式；普通标签行不变。
- 分组展开状态按组名分别保存（`_metaExpandedGroups`），独立折叠/展开；搜索时命中组展开、空组隐藏。
- 交互验收（展开/折叠手感、重命名后归组）待桌面运行确认。

## Decisions

- 纯显示层分组：不改 `Tag`/`BookMeta` 模型与持久化格式（R2）；类别由 `BookMeta.author/genre/series` 推断。
- 分组键固定顺序：作者 → 类别 → 系列 → 状态，状态组始终最后。
- "AI超分" 是独立元数据标签（超分完成时打标，不写入 author/genre/series），单独成组，显示在 系列 与 状态 之间，避免按平局规则落入作者组。
- 组展开状态为会话内内存态，不持久化；每组独立。
