# Mobile Poster Wall and Comic Entry Settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Let phone users choose poster-wall density and whether comic taps open the reader, while preserving desktop behavior and removing the editable EH mirror host.

**Architecture:** Persist the phone settings in `AppSettings`, route all comic entries through one compact-layout action policy, and reuse a single grid-delegate helper across poster walls. Normalize EH subscription rules to the official host and reflow its editor based on available width.

**Tech Stack:** Flutter/Dart, existing `LibraryStore` settings persistence, existing EH subscription Store/Rust API.

**Spec:** `.trellis/tasks/10-09-mobile-poster-wall/prd.md` and `.trellis/tasks/10-09-mobile-poster-wall/design.md`

## Global Constraints

- Compact-layout poster columns are `2`, `3`, or `4`; default is `2`.
- `tapComicFileWithoutDetails` defaults to `false`; only compact-layout taps consult it.
- Desktop grids and comic item actions keep their existing callbacks and layout.
- Index-only and ghost-source catalog entries remain detail-page destinations.
- Directories, source navigation, and selection-mode actions do not use comic-card routing.
- EH subscription requests use the fixed `e-hentai.org` host after legacy-rule normalization.
- Do not add or run automated tests unless the user asks to test or verify the implementation.

## Review Focus

- Four-column compact grids fit the available width without card overflow.
- A compact comic whose source cannot open in the reader still opens its details.
- Source-browser selection mode and folder navigation keep their existing actions.
- Desktop taps remain exactly as before, including screens whose existing behavior differs.
- A saved legacy EH mirror host is normalized before probe or collection calls.

---

### Task 1: Persist phone poster and tap settings and share grid sizing

**Files:**
- Modify: `app/lib/store/models.dart`
- Create: `app/lib/ui/poster_grid.dart`
- Modify: `app/lib/ui/home_page.dart`
- Modify: `app/lib/ui/source_browser.dart`
- Modify: `app/lib/ui/library_page.dart`

**Interfaces:**
- Produces `AppSettings.mobilePosterColumns` (`int`, default `2`).
- Produces `AppSettings.tapComicFileWithoutDetails` (`bool`, default `false`).
- Produces `SliverGridDelegate comicPosterGridDelegate(BuildContext context, {required double maxCrossAxisExtent, required double childAspectRatio, required double crossAxisSpacing, required double mainAxisSpacing})`.

- [x] Add both fields to `AppSettings`, `toJson`, and `fromJson`; clamp a persisted column value to `2..4` and use defaults for missing keys.
- [x] Implement `comicPosterGridDelegate`: compact layout uses `SliverGridDelegateWithFixedCrossAxisCount` and the saved phone count; desktop returns the existing max-cross-axis-extent delegate with the supplied dimensions.
- [x] Add the 2/3/4-column setting under appearance/layout and a switch titled `点击漫画文件不进入详细页` under reading settings. Call `LibraryStore.updateSettings` on change; the switch defaults off and its subtitle explains that phone long-press offers the detail-page option.
- [x] Replace each active comic poster grid's inline delegate with the shared helper. Keep existing aspect ratios, spacing, and desktop max extent unchanged.
- [x] Apply the helper to the legacy `LibraryPage` grid as well, so no remaining poster grid has a separate compact column policy.
- [x] Inspect all app-library `GridView` and `SliverGridDelegate` occurrences to confirm every comic poster wall uses the helper and non-poster grids remain untouched.

### Task 2: Centralize compact comic tap and long-press behavior

**Files:**
- Create: `app/lib/ui/book_navigation.dart`
- Modify: `app/lib/ui/comic_cover.dart`
- Modify: `app/lib/ui/home_page.dart`
- Modify: `app/lib/ui/source_browser.dart`
- Modify: `app/lib/ui/source_tree.dart`
- Modify: `app/lib/ui/global_search.dart`

**Interfaces:**
- Produces `VoidCallback comicTapHandler(BuildContext context, {required bool canRead, required VoidCallback onRead, required VoidCallback onDetails, required VoidCallback onDesktopTap})`.
- Produces `Future<void> showComicDetailPrompt(BuildContext context, {required VoidCallback onDetails})`.
- `ComicCard` gains an optional `VoidCallback? onLongPress`.

- [x] Add the shared tap handler. In compact layout, choose reader only when the setting is enabled and `canRead` is true; otherwise choose details. In non-compact layout, call the caller's existing `onDesktopTap` unchanged.
- [x] Add the compact long-press confirmation asking `是否进入漫画详情页？` with `进入详情` and `取消` actions.
- [x] Add the optional long-press callback to `ComicCard` and wire it to its `InkWell`.
- [x] Update recent and tag comic cards, the comic dimension in statistics, source-browser files/comic folders/list entries, source-tree results, and global-search results to use the shared handler and phone long-press prompt.
- [x] Keep source-browser folder/container navigation and selection-mode tap/long-press actions on their current paths. Keep index-only/ghost entries detail-only.
- [x] Search the named surfaces for direct `BookDetailPage`/`openBook` callbacks and review each remaining call as either intentionally non-card navigation or an unchanged desktop/fallback path.

### Task 3: Remove editable EH host and make the rules editor responsive

**Files:**
- Modify: `app/lib/store/eh_subscription_store.dart`
- Modify: `app/lib/ui/eh_subscription_panel.dart`

**Interfaces:**
- Keep the EH Rust API contract unchanged; `EhSubscriptionStore` normalizes its loaded rule map to `host: e-hentai.org` before any probe, save, or collect operation.

- [x] Normalize legacy rules in `EhSubscriptionStore.init` before exposing loaded state; persist the normalized rules so a custom mirror is not retained invisibly.
- [x] Remove `_hostCtrl`, its initialization/disposal, host input, and mirror-specific failure guidance from the EH rules editor.
- [x] Keep connectivity checking as a separate action that probes the fixed official host and tracker.
- [x] Replace the phone-width four-field row with a responsive layout that stacks or wraps the remaining fields; preserve the desktop horizontal layout as far as the reduced field set allows.
- [x] Inspect the complete EH editor for any remaining editable mirror/domain copy or host writes from the UI.

### Task 4: Cross-surface review and UI verification

**Files:**
- Review: `app/lib/store/models.dart`
- Review: `app/lib/ui/poster_grid.dart`
- Review: `app/lib/ui/book_navigation.dart`
- Review: `app/lib/ui/home_page.dart`
- Review: `app/lib/ui/source_browser.dart`
- Review: `app/lib/ui/source_tree.dart`
- Review: `app/lib/ui/global_search.dart`
- Review: `app/lib/store/eh_subscription_store.dart`
- Review: `app/lib/ui/eh_subscription_panel.dart`

- [x] Format changed Dart files and run `flutter analyze`; resolve issues caused by this change.
- [ ] On a phone-sized layout, inspect 2, 3, and 4-column grids in recent, tags, and source browsing; confirm settings persist after reopening the app.
- [ ] With the tap setting off/on, inspect comic taps in recent, statistics, tags, source browsing, source tree, and global search; confirm long-press offers details and index-only items remain details-only.
- [ ] Inspect source-browser folders and multi-select mode to confirm navigation and selection remain usable.
- [ ] Inspect desktop grid and tap behavior before/after the change to confirm no new desktop behavior was introduced.
- [ ] Load EH rules containing a non-official legacy host, then confirm the saved rules, connectivity probe, and collection use `e-hentai.org`; inspect the editor at narrow and wide widths.

## Execution Method

Native inline implementation in this session. The active collaboration constraints prohibit spawning agents unless the user explicitly requests delegation.
