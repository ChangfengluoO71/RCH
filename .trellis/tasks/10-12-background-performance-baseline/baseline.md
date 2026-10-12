# Performance baseline, 2026-10-12

## Device availability

- Host: Windows 10.0.26200.9457, Windows desktop target.
- `flutter devices`: Windows, Chrome, and Edge are connected; no Android device
  or emulator is available. Android measurements remain pending and are not
  inferred from desktop results.
- `flutter build windows --profile --no-pub` succeeded in 69.1 seconds. The
  updated app is open at `app/build/windows/x64/runner/Profile/RCH.exe`.
- After adding cleanup yield points, the final Profile rebuild succeeded in
  35.6 seconds and was reopened for user testing.
- Profile startup/automatic-maintenance logs captured a cold local scan of
  81 ms for 243 entries, followed by warm scans of 1–5 ms; catalog snapshot
  refreshes across 3 sources took 101–335 ms; sync attempts took 574–1365 ms.
  These are a small number of startup/automatic-maintenance observations, not
  a controlled comparison or p95 result.
- The final rebuilt app launch added a local scan of 2 ms for 243 entries, a
  catalog refresh of 138 ms across 3 sources, and a sync attempt of 580 ms.
- Representative user-triggered batch-tag and cloud-login scenarios still
  need to be exercised in the app before their timings can be recorded.

## Profile markers

Profile-mode logs use `[performance]` and contain phase names, elapsed
milliseconds, and aggregate item counts only. They omit source names, paths,
titles, cookies, and tokens. Current phases include local scan and index
persist, catalog snapshot refresh, sync attempt, batch-tag index/persist, 115
and Quark QR start/exchange, and cloud connection.

The marker around an FFI call measures bridge wait plus the Rust operation. It
does not split SQLite mutex wait from transaction time. Frame build/raster p95
and frame-budget misses also require a representative Flutter DevTools Profile
capture; no such capture has been collected yet.

The unfiltered Windows app output is captured under `%TEMP%` while the app is
open. A filtered performance-only log is available alongside it for review.
The sanitized sample captured so far is `windows-profile-sample.log` in this
task directory; it contains no Dart VM service URL.

## Reader last-page observation

- The latest local reader timing entries show `book_open=1 ms`, `first_page=63
  ms`, and visible page requests at 55 ms and 64 ms. `first_page` and
  `page_load` stop when page bytes are ready; they do not measure Flutter image
  decode or the frame reaching the screen.
- The current desktop capture shows the final spread with its images visible,
  so this capture did not reproduce a still-loading spinner. The diagnostic
  intentionally omits titles, paths, and page numbers, so these events cannot
  be tied to a particular comic. They do not prove the reported case is fixed.
- To isolate the reported delay, reopen the same history entry while capturing
  a Flutter Profile frame trace and compare the visible wait with the existing
  byte-ready timings. If byte-ready remains short while the spinner persists,
  add only a scoped image-frame timing marker after documenting its fields.

## Reproduction notes

1. Run the Windows app in Profile mode with the representative local source
   and cloud accounts configured.
2. Capture Flutter DevTools Performance while opening a local source, running
   sync, applying a batch tag, and starting each cloud QR/connect flow.
3. Record each `[performance]` phase together with build/raster p95, frame
   budget misses, item count, and whether the action was foreground or routine
   maintenance.
4. Repeat the same scenarios on Android Profile mode once an Android device is
   connected. Keep its results separate from Windows.
