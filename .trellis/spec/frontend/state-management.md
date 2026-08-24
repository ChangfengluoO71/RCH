# State Management

> How state is managed in this project.

---

## Overview

<!--
Document your project's state management conventions here.

Questions to answer:
- What state management solution do you use?
- How is local vs global state decided?
- How do you handle server state?
- What are the patterns for derived state?
-->

(To be filled by the team)

## Source and Catalog Projection Convergence

Source configuration is owned by `LibraryStore`; the source tree is projected
by `LibraryCatalogStore`. A source mutation must not trigger a tree reload while
the debounced persistence is still pending.

```dart
await LibraryStore.instance.updateSource(id, rootId: editedRoot);
// updateSource waits for saveToDisk() and then reloads LibraryCatalogStore.
```

The add/edit UI awaits `LibraryStore.addSource`/`updateSource` before closing
the dialog. Credential refresh paths may use the same futures. This keeps the
current screen, SQLite source tree, and the next browser session on the same
revision without requiring an application restart.

For 115 and Quark, widgets must display and open `BookSource.effectiveRootPath`
rather than assuming `path` is authoritative. Empty root IDs are normalized to
`'0'`.

### Common Mistake: Reloading Before Debounced Save

```dart
store.updateSource(id, rootId: root);
LibraryCatalogStore.instance.loadTree(); // may read the old SQLite row
```

Use the awaited source CRUD future instead. The catalog store should not
observe every `LibraryStore` notification; the explicit completion boundary
avoids stale reads and notification loops.

---

## State Categories

<!-- Local state, global state, server state, URL state -->

(To be filled by the team)

---

## When to Use Global State

<!-- Criteria for promoting state to global -->

(To be filled by the team)

---

## Server State

<!-- How server data is cached and synchronized -->

(To be filled by the team)

---

## Common Mistakes

<!-- State management mistakes your team has made -->

(To be filled by the team)
