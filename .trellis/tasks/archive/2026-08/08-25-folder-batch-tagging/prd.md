# 修复文件夹批量打标签失效

## Goal

恢复文件夹批量打标签能力，同时保持现有文件批量打标签行为不变。

## Background / Observed Facts

- 用户反馈：文件可以批量打标签。
- 用户反馈：文件夹不能批量打标签。
- 当前任务范围是定位该差异的根因，并在确认根因后修复和验证。
- `SourceBrowser._batchTagFromSelection` 会把选中的目录统一交给 `_collectComicsRecursive`，再把结果传给 `LibraryStore.batchTag`。
- `_collectComicsRecursive` 当前只收集识别为漫画格式的文件，以及继续遍历的子目录；它不会把“目录内直接包含图片”的漫画文件夹自身加入结果。
- Rust 的 `is_comic_folder` 明确把目录内直接包含图片的目录识别为漫画文件夹，现有测试 `document::folder::tests::is_comic_folder_works` 已通过。
- 因此选中图片型漫画文件夹时，展开结果为空，UI 在调用 `batchTag` 前直接提示“所选路径下没有漫画”；文件选择则直接进入 `batchTag`，所以文件批量操作正常。
- 历史提交 `da22720` 已在递归遇到子目录时增加漫画文件夹识别，但初始选中的目录仍从目录内容开始遍历，未复用同一识别逻辑；这是当前文件夹选择分支与漫画文件夹模型之间的边界遗漏。

## Requirements

- 文件批量打标签继续可用。
- 文件夹批量打标签应能对选中的文件夹完成与单个文件夹一致的标签写入和界面刷新。
- 文件与文件夹混合选择时，不得因文件夹项导致文件标签能力回退或静默失败。
- 失败场景应保留可诊断的错误信息，不能只表现为无响应。

## Acceptance Criteria

- [x] 选中多个文件并批量添加/移除标签，结果与现有行为一致。
- [x] 选中多个文件夹并批量添加/移除标签，所有选中项均正确更新。
- [x] 选中文件和文件夹混合批量操作，两类项均正确更新。
- [x] 相关自动化测试或可重复验证覆盖文件、文件夹和混合选择三种场景。
- [x] 运行与改动范围匹配的测试、类型检查或构建验证通过。

## Out of Scope

- 不改变标签模型、标签展示样式或单个项目打标签的产品交互。
- 不扩展与本 Bug 无关的批量操作能力。

## Open Questions

- 无阻塞性产品决策；具体根因和修复边界通过代码、测试和运行证据确认。

## Technical Risk to Validate During Fix

- 图片型漫画文件夹应以文件夹路径作为漫画条目参与标签关联；若同时写入离线索引，需要保持目录条目的类型为 `dir`，不能沿用当前批量路径默认的 `file`。

## Implemented

- 抽出 `collectBatchTagTargets`，在递归前先识别当前选中的本地漫画文件夹，并以 `BatchTagTarget.directory` 返回。
- `LibraryStore.batchTag` 接收带 `entryType` 的目标，目录目标写入离线索引时使用 `dir`。
- 复用列表预检测、自动转换和批量展开之间的同一路径漫画文件夹检查，避免弹窗前重复扫描。
- 新增文件、图片文件夹、文件与文件夹混合选择的回归测试。

## Verification Evidence

- `flutter test test/source_browser_batch_tagging_test.dart`：3/3 通过。
- `flutter analyze`（改动相关 5 个 Dart 文件）：通过。
- `flutter test`：76 项通过。
- `cargo test --lib document::folder::tests::is_comic_folder_works`：1 项通过。
