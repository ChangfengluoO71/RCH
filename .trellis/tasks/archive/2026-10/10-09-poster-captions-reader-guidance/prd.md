# Mobile poster captions and reader guidance

## Goal

Clarify when to use wide-page splitting versus dual-page stitching, and keep comic captions below a readable, full-sized cover in compact poster grids.

## Requirements

- User-facing documentation and reader settings must describe smart wide-page splitting as a phone portrait feature and dual-page stitching as a large-screen feature.
- Documentation and settings must advise users not to enable both at once.
- In compact poster grids, cover space must be sized independently from captions so titles and metadata remain below the cover without shrinking it. Desktop grid sizing remains unchanged.
- Keep text and caption surfaces readable across the app's color themes.

## Acceptance Criteria

- [x] User guide, v0.6.4 release notes, and changelog describe the intended use of both reader options consistently.
- [x] Reader settings explain the use case and mutual-exclusion guidance below both options.
- [x] Compact poster grids used by recent, statistics, tag, local library, and source browsing render captions below a preserved cover area.
- [x] Desktop poster grid sizing is unchanged.
- [x] Do not change source files or synchronize poster dimensions through `BookMeta`.

## Notes

- No tests are requested for this task.
