# PRD: Mobile poster wall and comic entry behavior

## Problem

On phones, the current two-column comic poster wall makes covers larger than the user prefers. Comic cards also open different destinations depending on which part of the app they came from, and the EH subscription form exposes a custom main-domain field while compressing several controls into one row on narrow screens.

## Requirements

- Add a phone-layout poster-column setting with 2, 3, or 4 columns. Default to 2 to preserve the current phone layout.
- Apply the selected phone column count consistently to comic poster walls in recent reading, tags, and book-source browsing. The desktop grid layout stays unchanged.
- Add a global setting named `点击漫画文件不进入详细页`. It defaults off. When enabled, tapping a readable comic item in the phone layout opens the reader directly; when disabled, tapping opens its detail page.
- Apply that tap behavior consistently to comic entries in recent reading, statistics, tags, book sources, and global search. Entries that cannot be opened for reading, such as index-only or ghost entries, continue to open details.
- On phones, long-pressing a comic offers an explicit action to enter its detail page. Folder navigation and multi-select behavior keep their existing actions.
- Leave desktop card layout and interaction behavior unchanged.
- Remove the editable EH main-domain/mirror field and its mirror guidance. Use `e-hentai.org` as the fixed host and normalize any saved custom host back to it. Keep connectivity checking available without requiring a host text field.
- Reflow EH subscription controls on narrow screens so labels, inputs, and buttons have enough width and do not overlap or overflow.

## Acceptance criteria

- Selecting 2, 3, or 4 columns changes every phone-layout comic poster wall and persists after restart; desktop grids retain their current layout.
- With direct reading disabled, phone comic taps open details. With it enabled, readable comic taps open the reader across the listed surfaces.
- Long-pressing a phone comic offers the detail-page action regardless of its list surface. Desktop behavior is unchanged.
- Unreadable/index-only entries still reach their detail page, while source folders and selection mode continue to work.
- EH settings no longer display a user-editable host or mirror field, old custom host values are reset to `e-hentai.org`, and connectivity can still be checked.
- EH subscription settings fit narrow phone widths without control overflow.
