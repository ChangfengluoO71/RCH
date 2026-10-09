# Design: Mobile poster wall and comic entry behavior

## Scope and boundaries

The new behavior applies to compact/mobile layouts. Desktop layout and item interactions remain as they are today. Compact detection will use the existing `isCompact(context)` contract so phones and tablets explicitly configured for mobile navigation share the phone settings.

The setting named `点击漫画文件不进入详细页` is off by default. When enabled in compact layout, tapping a readable comic opens the reader. When disabled, it opens the detail page. Long-press on a comic in compact layout offers the detail-page action independently of the tap setting. Directories, selection mode, index-only records, and ghost-source entries retain their existing non-reader behavior.

## Settings and grid layout

Add `mobilePosterColumns` (integer, default `2`) and `tapComicFileWithoutDetails` (boolean, default `false`) to `AppSettings`. Serialize both through the existing settings JSON path; `fromJson` supplies defaults so old installs need no database schema migration.

Create one reusable poster-grid delegate/helper. In compact layout it uses `SliverGridDelegateWithFixedCrossAxisCount` with the selected 2/3/4 count and existing poster ratio/spacing. In desktop layout it returns the current max-cross-axis-extent behavior unchanged. Use it for the home recent grid, tag-result grid, book-source poster grid, and any other active comic poster wall. The statistics view is currently a list, so column count does not change its layout; its comic-item tap behavior still follows the global mobile setting.

## Comic navigation

Use a shared navigation/action helper for readable comic entries so the setting is evaluated in one place. The helper chooses reader versus detail page for compact-layout taps, keeps desktop callers on their existing callbacks, and provides the phone long-press detail prompt. Catalog entries without a readable local source continue to open `BookDetailPage`.

Apply the shared behavior to recent records, comic rows in reading statistics, tag-result cards, source-browser comic files and comic folders, source-tree results, and global-search results. Do not route folder/container navigation, source selection, random-read actions, or selection-mode taps through the comic-card policy.

Extend shared comic-card actions with an optional long-press callback. Source-browser folder-cover cards and list results need equivalent callbacks because they do not all use `ComicCard`. Long-press on the phone presents a small confirmation/action surface with `进入漫画详情页` and `取消`.

## EH subscription settings

Remove the editable `host` controller and the main-domain/mirror text field and guidance from the EH subscription rules editor. Preserve the connectivity probe, presenting it as a separate action against the fixed `e-hentai.org` host. Normalize legacy rules containing another `host` value to `e-hentai.org` during store load/save so a previously configured mirror is not still used invisibly.

Replace the narrow-screen multi-field horizontal row with responsive layout: fields flow into full-width rows below the available-width threshold; desktop keeps a compact horizontal grouping, with the removed host cell absent. This also prevents the vertical overflow shown in the attached settings screenshot.

## Persistence and compatibility

- App settings remain owned by `LibraryStore` and use its current JSON/SQLite persistence; no synchronized book metadata or Rust setting API is added.
- Old settings deserialize with 2 columns and direct reading disabled.
- Existing EH subscription rules are normalized to the official host; search, filters, intervals, output folder, and manifest are unchanged.
- Desktop poster sizing and all desktop card actions remain on their current code paths.

## Review focus

- Every active poster grid uses the shared helper, while desktop delegates remain equivalent to the current max-extent grid.
- Every comic-item tap in the named surfaces uses the global mobile setting, without changing directory navigation or selection mode.
- The direct-reading setting affects only compact layouts and defaults off for migrated settings.
- Legacy EH host values cannot leak into an EH collection/probe after normalization.
- EH controls remain usable at phone widths and do not produce row overflow.
