# PRD: Mobile wide-page split reading

## Problem

On portrait phones, a single image containing two scanned comic pages is fitted as one wide page. Both pages become too small to read. RCH needs a conservative split-wide-page flow that does not alter the source file or its physical page numbering.

## Requirements

- Add app-wide default handling modes: Smart (default), Always split wide pages, and Keep whole pages.
- In Smart mode, split only in portrait single-page reading when the effective page ratio is at least 1.3 and a high-confidence center gutter is detected. Keep uncertain pages whole. Landscape defaults to whole-page display.
- Split pages are virtual display locations. Manga reads right then left; comic reads left then right. Webtoon stacks the two halves vertically in the source page's scroll item. Forced dual-page stitching remains unchanged.
- Allow a per-source-page Split / Keep whole override. Persist it on this device only, keyed by canonical book key and source page; do not add it to synchronized metadata.
- Continue recording reading progress and bookmarks against physical source page indices. Show the half number for a split page and reopen at that source page's first half in reading order.
- Mark the book complete only after the last half of the final physical page.
- Fit the displayed half to the available reader area and request enough source resolution for both halves. Keep crop borders out of this delivery.
- Preserve lazy page loading for remote sources; do not inspect or download the full chapter to build a visual-page map.

## Acceptance criteria

- RTL/LTR order, mixed split and whole pages, cover handling, page jumps, and both boundaries map correctly.
- Center-gutter detection accepts qualifying synthetic/fixture images and rejects center-spread content and uncertain images.
- Overrides survive a reader reopen on the same device and are absent from sync payloads.
- Split progress, resume, and completion semantics use physical page identity as specified.
- Concurrent Rust requests at different widths cannot share the wrong memory, disk, in-flight, or prefetch entry; the width-zero disk cache layout remains compatible.
- Existing forced dual-page behavior stays unchanged. Webtoon keeps its vertical source-page list and progress, while wide spreads render as two vertically stacked halves.
