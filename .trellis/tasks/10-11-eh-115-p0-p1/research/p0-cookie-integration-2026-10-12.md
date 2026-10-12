# 115 Cookie BT in-app integration checkpoint

Date: 2026-10-12
Branch: `local-ai/cover-quark-debug`
HEAD at checkpoint: `c4bfa65ea59998f3923ddabc7a6608b6f6ff638e`

## Scope and safety boundary

This checkpoint adds a manually invoked, read-only preflight to the existing EH handoff panel. It reuses the existing RCH Web Cookie session registry and request client. It does not add an independent login/client, send any 115 write request, scan `F:\种子`, start at app launch, connect to H1 Daily, change SPEC §10, or touch the reader path.

The fixed third-party source reference remains p115client commit `856e4f8f09ce034534bd458f06d3dee4474a1698`. Its Cookie upload flow and SSP BT wrapper are implementation evidence only; they do not grant 115 authorization. See the pinned [client source](https://github.com/ChenyangGao/p115client/blob/856e4f8f09ce034534bd458f06d3dee4474a1698/p115client/client.py) and [upload helper](https://github.com/ChenyangGao/p115client/blob/856e4f8f09ce034534bd458f06d3dee4474a1698/p115client/tool/upload.py). Current SSP wire behavior and Web write authorization remain unverified.

## Implemented

- Rust validates one H0-manifest-relative `.torrent`, canonicalizes it beneath the configured EH output directory, rejects unsafe path components/symlinks, parses bounded v1 bencode, computes the raw `info`-dictionary infohash and whole-file SHA-1 separately, and produces the ordered wanted-file index list.
- A read-only preflight uses the existing Cookie session client, request gate and WAF cooldown. It resolves `/漫画/EH订阅` one exact child at a time from account root with bounded pagination and `record_open_time=0`, then reads the pinned SDK's Web task-list model with a bounded page budget. Missing/malformed/paging-inconsistent results do not become `not_found`; they fail or remain `unknown`.
- FFI exposes only this preflight. The Flutter panel is inert during initialization and invokes it only after a user click. It shows local Torrent identity, Cookie UID, target folder ID, duplicate-check result and selectable zero-based wanted indexes. The upload/BT button remains disabled with `P0_WEB_WRITE_AUTH_BLOCKED`.
- The SSP logical payload/envelope builder exists only under Rust tests. It is not wired to production network calls. No upload, remote Torrent parse, BT submit, task status, cloud-file verification or RCH reading is claimed.

## Evidence

| Gate | Result |
|---|---|
| Cookie session reused by code path | Implemented; live session not exercised |
| Actual account/session confirmation | Not tested |
| `/漫画/EH订阅` directory ID | Not tested against a real account |
| Local Torrent parser and two SHA-1 values | 6 Rust unit tests passed with synthetic fixtures |
| SSP logical payload model | Static/mock test only; `SSP_WIRE_UNVERIFIED` |
| Torrent upload / remote parse / BT submit | Not implemented in the live path; no request sent |
| Remote task query / target file / RCH reader | No real request or result |
| 115 real writes | 0 |
| P0 | `P0_WEB_WRITE_AUTH_BLOCKED`; `P0_REAL_PASS=NO` |
| P1/H1 automatic upload | `NOT_STARTED` |

## Verification

- `cargo test --lib --locked --offline source::cloud115::offline::tests`: 6 passed.
- `cargo check --lib --locked --offline`: passed.
- `flutter test --no-pub test/eh_cookie_bt_preflight_panel_test.dart`: passed. The injected preflight runner was called once only after the explicit button action; no network was used.
- `flutter analyze --no-pub lib/store/cloud115_session.dart lib/ui/eh_subscription_panel.dart test/eh_cookie_bt_preflight_panel_test.dart`: no issues.
- `git diff --check`: passed; Git emitted only existing LF/CRLF normalization warnings.
- `flutter build windows --debug --no-pub`: the first attempt reached CMake `INSTALL` while a pre-existing `RCH.exe` process (PID 31284, default Debug path) was running, so it failed. The process was not terminated. After it exited, the same standard Debug build passed in 14.2 seconds. The bundle contains a fresh `kernel_blob.bin` (2026-10-12 14:02:43) and `rust_lib_app.dll` (2026-10-12 13:58:17) at `app/build/windows/x64/runner/Debug`; the candidate was not launched.

The historical `LEGACY_WEB_PROTOCOL_MODEL` mocks are not evidence for current SSP compatibility. Earlier 12 parallel Rust failures remain `BASELINE_UNRESOLVED` and were not rerun as part of this checkpoint.

## Stop point

Only local parsing, the read-only app entry and static/mock SSP payload shape are ready. Real write UI remains disabled because neither official Web Cookie write authorization nor a trustworthy current live protocol contract has been established. No SPEC change is proposed while no cloud write is enabled.

Commit: NO · Push: NO · Merge: NO · Rebase: NO · Tag: NO · Release: NO · Delete: NO · Cleanup: NO.

## 2026-10-12 follow-up — in-app one-shot Cookie BT path

This follow-up supersedes the earlier “Real write UI remains disabled” stop-point above. The application now contains separate, explicit-confirmation actions for uploading/parsing one checked Torrent and for submitting one BT task. This is an experimental user-operated entry, not evidence that the 115 Web write route is authorized or compatible with the live service.

### Session and request isolation

- Flutter BT wrappers accept only an already cached Web Cookie session for the selected 115 source. If none is cached, the action fails closed and tells the user to connect through the ordinary source flow. It does not reconnect, refresh, scan a QR code, replace the source Cookie, or export credentials.
- Rust FFI looks up and clones the existing session `Arc<Cloud115WebClient>`; the BT methods only read its current Cookie. The offline mock deliberately returns a changed `Set-Cookie`, then verifies that `client.cookie()` still equals the original Cookie. The request path does not copy a response Cookie back into the source or login state.
- BT requests use a strict no-redirect client owned by that same Web client and reuse its request gate and WAF cooldown. The OSS byte upload sends only the short-lived OSS authorization/token headers and no 115 Cookie. A WAF response can activate the existing session cooldown; that may temporarily defer ordinary background Web requests but does not change the Cookie or replace the session.
- Upload and BT task creation each require a separate confirmation dialog. Once either write is attempted, the panel will not offer a second write for that item in the current UI state. An uncertain upload exposes read-only reconciliation; an uncertain task submission exposes read-only task lookup.

### SSP encryption evidence

The BT `add_task_bt` request body is a single form field named `data`; the logical fields (`info_hash`, `wanted`, `savepath`, `wp_path_id`, `ac`, and `app_ver`) are serialized and encrypted, and are not sent as clear form fields. The SSP wrapper is aligned to the pinned third-party implementation at p115client commit `856e4f8f09ce034534bd458f06d3dee4474a1698`, including its fixed `0x02` RSA type-2 fill bytes and default all-zero 16-byte seed. The existing randomized `m115_encode()` used by reader `downurl` requests is unchanged. See the pinned [SSP request code](https://github.com/ChenyangGao/p115client/blob/856e4f8f09ce034534bd458f06d3dee4474a1698/p115client/client.py), [cipher wrapper](https://github.com/ChenyangGao/p115client/blob/856e4f8f09ce034534bd458f06d3dee4474a1698/modules/p115cipher/p115cipher/__init__.py), and [RSA block encoding](https://github.com/ChenyangGao/p115client/blob/856e4f8f09ce034534bd458f06d3dee4474a1698/modules/p115cipher/p115cipher/util.py). This is source-level compatibility evidence only; the live SSP request/response remains `SSP_WIRE_UNVERIFIED`, and third-party code does not establish authorization.

### Verification in this follow-up

- `flutter test --no-pub test/eh_cookie_bt_preflight_panel_test.dart`: 4 passed. Coverage includes no request before click, separate upload/submit confirmations, read-only recovery after an unknown upload result, disabled actions after the source disappears, and no cached-session reconnect or Cookie mutation.
- `flutter analyze --no-pub lib/store/cloud115_session.dart lib/ui/eh_subscription_panel.dart test/eh_cookie_bt_preflight_panel_test.dart`: passed with no issues.
- `cargo test --lib --locked --offline source::cloud115::offline::tests -- --test-threads=1`: 17 passed. The tests use synthetic Torrent data and a loopback HTTP server; no real 115 endpoint is contacted. One earlier run had a local mock TCP connection reset; the failing test passed individually on immediate rerun and the complete 17-test suite then passed. The transient is recorded rather than treated as a live-service result.
- `cargo check --lib --locked --offline`: passed without warnings after test-only helper scoping was corrected.
- `flutter build windows --debug --no-pub`: passed (50.4 s). Fresh bundle inputs include `kernel_blob.bin` (102,119,416 bytes, 2026-10-12 07:41:02 UTC) and `rust_lib_app.dll` (42,923,008 bytes, 2026-10-12 07:40:52 UTC). `RCH.exe` is the existing unchanged runner binary (2026-10-11 17:05:15 UTC); the bundle was built but not launched, so no startup/source warmup was triggered.
- `git diff --check`: passed; only existing line-ending normalization warnings were printed.

### Gate status and operator boundary

No real 115 request was made by this development run. Cookie authentication, target folder ID, upload, remote parse, BT submission, remote task state, downloaded target files, and RCH reading are all still unverified against the live account. Keep `P0_WEB_WRITE_AUTH_BLOCKED`, `P0_REAL_PASS=NO`, and `P1_NOT_STARTED`; do not infer success from the Mock suite or build.

To exercise the candidate, open the newly built Debug app, connect the chosen Cookie source using its normal RCH source flow, open EH settings, select one H0-validated Torrent, then click “检查账号、目录与重复任务”. Review account/folder/hash/duplicate details. “上传 Torrent 并解析” has its own confirmation; after a verified parsed result, choose the wanted file indices and use the separate “提交 BT 任务（一次）” confirmation. If either result is uncertain, use only the read-only reconciliation/status action and do not repeat a write. The dialogs warn that the route and live compatibility are unverified. The 12 historical parallel Rust failures remain `BASELINE_UNRESOLVED`.

Commit: NO · Push: NO · Merge: NO · Rebase: NO · Tag: NO · Release: NO · Delete: NO · Cleanup: NO.
