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

(To be filled by the team)

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

(To be filled by the team)

---

## Required Patterns

<!-- Patterns that must always be used -->

(To be filled by the team)

---

## Testing Requirements

<!-- What level of testing is expected -->

(To be filled by the team)

---

## Code Review Checklist

<!-- What reviewers should check -->

(To be filled by the team)
