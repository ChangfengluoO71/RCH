# Quality Guidelines

> Code quality standards for frontend development.

---

## Overview

<!--
Document your project's quality standards here.

Questions to answer:
- What patterns are forbidden?
- What linting rules do you enforce?
- What are your testing requirements?
- What code review standards apply?
-->

（2026-09-21 补实）本文件记录 Flutter 侧的**质量门禁与硬性约定**。执行入口两条命令：
`flutter analyze`（**必须全量**，CI 连 `test/`、`tool/` 一起分析）与 `flutter test`。
以下规则都源于真机问题或 CI 红线。

## Batch Tag Target Contract

### 1. Scope / Trigger

This contract applies whenever the source browser expands a multi-selection
before calling `LibraryStore.batchTag`. It is required because a local image
folder is a comic item in its own right, while its image children are not
standalone comic entries in the browser.

### 2. Signatures

```dart
class BatchTagTarget {
  final String path;
  final String entryType; // 'file' or 'dir'
}

Future<List<BatchTagTarget>> collectBatchTagTargets({
  required Iterable<String> selectedPaths,
  required Iterable<DirEntry> currentEntries,
  required String effectiveRootPath,
  required bool isLocalFs,
  required Future<List<DirEntry>> Function(String path) listDirectory,
  required Future<bool> Function(String path) isComicFolder,
  required bool Function(DirEntry entry) isComicEntry,
});

void LibraryStore.batchTag(
  BookSource source,
  Iterable<BatchTagTarget> targets,
  String tag,
);
```

### 3. Contracts

- A selected normal comic file becomes `BatchTagTarget.file(path)`.
- A selected local directory recognized by `isComicFolder` becomes
  `BatchTagTarget.directory(path)`; the directory path, not its image child,
  is the tag key.
- A selected container directory is recursively expanded. Recognized child
  comic directories retain `entryType == 'dir'`; archive files retain
  `entryType == 'file'`.
- Folder detection shared by list rendering, auto-conversion, and batch-tag
  expansion must reuse the in-flight/completed check for a path and clear that
  cache when a directory is relisted.
- `LibraryStore.batchTag` must pass the target's `entryType` to
  `LibraryIndexService.ensureIndexed`; folder targets must never use the
  default file type.

### 4. Validation & Error Matrix

| Condition | Required behavior |
| --- | --- |
| Empty selection | Preserve the existing user-facing prompt and do not call `batchTag`. |
| Local image folder | Return one directory target for the folder itself. |
| Local container folder | Walk readable descendants and keep each target's type. |
| Mixed files and folders | Keep file targets and directory targets in the same batch. |
| Unreadable descendant | Skip that descendant and continue the rest of the batch; an entirely empty result remains diagnosable in the UI. |
| Remote source | Preserve the existing remote listing behavior; do not call local-only comic-folder detection. |

### 5. Good / Base / Bad Cases

- Good: `[book.cbz, images/]` becomes `[file(book.cbz), dir(images/)]`.
- Base: selecting only `book.cbz` produces the same file target as before.
- Bad: recursively listing `images/` and returning no target because `.png`
  files are filtered out.
- Bad: passing `images/` as a bare string and indexing it with the default
  `entryType: 'file'`.

### 6. Tests Required

- Unit-test file-only selection and assert `entryType == 'file'`.
- Unit-test a local image-folder selection and assert exactly one target with
  the folder path and `entryType == 'dir'`.
- Unit-test mixed selection and assert both targets and their types survive
  expansion.
- Unit-test repeated folder checks and assert the underlying filesystem probe
  runs once until the listing cache is cleared.
- Run Flutter analysis and the existing Rust `is_comic_folder` test when the
  folder-detection or cross-layer target contract changes.

### 7. Wrong vs Correct

Wrong:

```dart
final paths = await collectComicsRecursive(folder);
LibraryStore.instance.batchTag(source, paths, tag);
```

Correct:

```dart
final targets = await collectBatchTagTargets(...);
LibraryStore.instance.batchTag(source, targets, tag);
// Folder targets reach ensureIndexed(..., entryType: 'dir').
```

---

## Forbidden Patterns

<!-- Patterns that should never be used and why -->

（2026-09-21 补实）

- **定时轮询推进数据**（30×350ms 轮询已删除，不得再引入；`comic_cover_state_consumer_test.dart`
  的 "E-NO-POLL" 锁死调用次数不随时间增长）。
- **在 `build` 内做 I/O**；在 locked frame 内推 notifier（历史 `widget tree was locked`）。
- **硬编码颜色/尺寸/魔法数**（颜色取 `Theme.of(context)`）。
- **UI 直接 new 出数据访问对象**（必须可注入）。
- **对不存在的能力写测试**：契约描述但代码未落地的 API ⇒ 编译不过、门禁变红
  （2026-09-21：5 个测试文件按 `UpdateManager.testing(...)`、`NeedsWholeBookDownload` 等编写 ⇒
  CI 64 项 error、发布被卡）。正确做法：锁真实 API + 写明已知缺口。

---

## Required Patterns

<!-- Patterns that must always be used -->

（2026-09-21 补实）

- **先本地缓存、后网络**：封面 unified → legacy 纯本地 → 才发请求；阅读 `raw-cache → stream → fallback-download`。
- **显式失效**：切换数据源/渲染宽度时清 L1（`ComicCover.clear()`、`Reader.set_display_width`）。
- **可注入边界 + 契约测试**；真机 bug 修复必须配"回退即失败"的回归测试。
- **用户文案可断言**：中文、明确（`等待扫描` / `获取失败`）。
- **与 CI 对齐自检**：全量 `flutter analyze` + `flutter test`；Rust 侧
  `RUSTFLAGS="-D warnings" cargo check --all-targets` 与串行 `cargo test`。

---

## Testing Requirements

<!-- What level of testing is expected -->

（2026-09-21 补实）

- 真机 bug 的修复必须补回归测试，且**验证测试本身**：把实现临时回退，确认用例确实失败
  （本仓库做法：`git stash` 或就地改回旧判定后重跑；本会话两处实例：文件头窗口 `before=5 after=6`、
  `cover_native_lib_missing` 误标）。
- 涉及缓存的用例必须用**隔离缓存根**（Rust：`set_custom_cache_root` + RAII guard；Dart：注入 loader），
  否则会写进用户真实缓存目录。
- 时间/顺序敏感的断言不要依赖文件系统或机器速度：先观测（如读回 mtime）再断言不变量
  （CI 上 `set_modified` 未生效曾导致假红）。
- 契约测试命名 `*_test.dart` / Rust `tests/*_contract.rs`，并在失败信息里写清"违反了什么契约"。

---

## Code Review Checklist

<!-- What reviewers should check -->

（2026-09-21 补实）

- 是否引入了轮询（`Timer` / `Future.delayed` 循环）？封面/扫描必须由 revision 唤醒驱动。
- 是否在 `build` 内做 I/O，或在 locked frame 内推 notifier？
- 颜色/尺寸是否取自主题与命名常量（无硬编码）？
- 新增 I/O 能力是否提供了可注入边界，并配了契约测试？
- 测试是否只依赖**真实存在**的 API？若覆盖了尚未落地的设计，是否已在文件与 spec 中写明**已知缺口**？
- 是否跑过**全量** `flutter analyze` 与相关 `flutter test`（而不是只分析改动文件）？
