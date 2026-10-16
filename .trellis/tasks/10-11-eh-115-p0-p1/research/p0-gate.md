# P0 Gate Evidence — 2026-10-11

## Official 115 contract search

- Searched the official 115 Open Platform for published Torrent upload and BT offline-task submission contracts. The official public portal describes general cloud-file API capabilities, but the public material found does not document a Torrent upload + BT task-submission contract.
- The portal agreement restricts integrations to published/opened interfaces and forbids using unpublished interfaces: <https://open.115.com/>.
- Community SDKs and third-party API references appeared in search results, but they are not an official contract and are not suitable authorization to issue a write request.
- Result: no upload or task-creation request was sent; no candidate write endpoints were tested.

## Local Torrent inventory

- Inspected only names and file sizes in `F:\种子`; did not read Torrent contents, calculate hashes, or upload a file.
- The folder contains many individual Torrent files but does not identify one sample as authorized for this test. No file was selected by the agent.

## P0 implementation boundary

- Implemented EH-only `system` / `direct` / `manual` proxy selection, validation, SOCKS support, and bounded separate EH host/Tracker-root checks.
- Removed all network/write/clear behavior from the old 115 probe example. It is now deliberately inert pending a documented contract.
- Did not implement a new 115 transfer adapter or read-only account diagnostic: the RCH process was not running, and no active already-authorized in-memory session was available to reuse safely. A new login/refresh could rotate the existing session token and would not be a valid check of the user's active account route.
- No P1 checkpoint, queue, daily scheduler, retry worker, notification, or library handoff code was started.

## Validation

- `cargo test --locked --offline --jobs 1 -- --test-threads=1`: exit 0; 616 passed, 0 failed, 4 ignored across the Rust unit and integration suites. The default parallel run initially produced 12 shared-cache/background-worker failures; all passed when serialized.
- `cargo fmt --check`: exit 0.
- `cargo tree --locked --offline -e features -i reqwest`: exit 0; reqwest remains 0.12.4, and the only added feature is `socks` (resolved `tokio-socks` 0.5.3).
- `flutter test --no-pub`: exit 0; 248 passed.
- Targeted `dart analyze` on the changed Dart store, panel, and widget test: exit 0; no issues.
- Full `flutter analyze --no-pub`: exit 1 with 10 existing deprecation infos in `lib/theme/app_theme.dart` and `test/app_theme_mica_test.dart`; no findings remain in changed files.
- Static audit of the retired probe found no HTTP client, POST, task-add, or task-clear operation.
- No mainland-China network matrix was executed. Local mock tests cover manual HTTP proxy routing, explicit direct mode, no fallback after proxy failure, URL validation, SOCKS5H URL acceptance, and case-insensitive challenge detection. An actual SOCKS5 server, system proxy, and the specified mainland failure matrix remain unverified.

## P0 decision

`P0_BLOCKED`: official BT API contract, a specific authorized test Torrent, an active existing 115 session, authenticated target-folder access, Torrent upload, BT task, cloud completion, and RCH reading evidence are absent. Stop before P1.

The observed network probes were performed in the current runtime only; they do not establish mainland-China network coverage. Tracker root reachability does not validate a `.torrent` payload.

## P0-B Follow-up — 2026-10-11

### Contract evidence

- The 115 developer portal is reachable and describes app authorization and API capability at platform level. It says enabled features depend on the permissions granted to an app, but the endpoint-specific Torrent permission scope was not available in the public material inspected.
- The official Yuque endpoint references linked from both SDKs could not be opened by the current research client. No authenticated or write API response was obtained.
- OpenListTeam's Go SDK maps the Open API routes to `GET /open/upload/get_token`, `POST /open/upload/init`, `POST /open/offline/torrent`, `POST /open/offline/add_task_bt`, and `GET /open/offline/get_task_list`; it also defines `GET /open/offline/get_quota_info`.
- The SDK upload sequence is temporary upload credentials, upload initialization, optional second-signature verification, OSS object upload, then callback data including the Torrent file's `pick_code`. Its request models distinguish whole-file upload SHA-1 (`fileid`) from BT `info_hash`.
- The Open API BT parse request carries `torrent_sha1` and `pick_code`; its response model contains `info_hash` and a `torrent_filelist`. The BT add request carries `info_hash`, `wanted`, `save_path`, `torrent_sha1`, `pick_code`, and `wp_path_id`. SDK documentation describes `wanted` as comma-separated zero-based file indexes. `wp_path_id` is the destination folder ID; `save_path` is a relative subpath beneath it.
- BitTorrent BEP 3 defines a v1 info hash as SHA-1 over the bencoded `info` dictionary. It is therefore distinct from SHA-1 over the complete `.torrent` file bytes.
- Evidence categories remain distinct: official 115 material confirms platform authorization/capability only; OpenList Go and p115client are third-party SDK evidence for endpoint contracts; the only actual response this run observed was an unauthenticated `HTTP 200` envelope from `/open/user/info` with code `40140123` (`access_token` format error).

### References checked

- Official platform portal and developer agreement: <https://open.115.com/>. The portal says only published/opened interfaces and granted app capabilities are available; it does not expose the endpoint-level BT permission scope in the material available to this review.
- OpenListTeam Go SDK route and request/response models: <https://github.com/OpenListTeam/115-sdk-go/blob/main/offline.go>, <https://github.com/OpenListTeam/115-sdk-go/blob/main/const.go>, and <https://github.com/OpenListTeam/115-sdk-go/blob/main/upload.go>.
- OpenList's Go upload implementation, used only to understand the temporary-credential / init / object-upload / callback sequence: <https://github.com/OpenListTeam/OpenList/blob/main/drivers/115_open/upload.go>.
- The p115client source reference supplied for this review: <https://p115client.readthedocs.io/en/latest/_modules/p115client/client.html>; its upload reference page was also accessible at <https://p115client.readthedocs.io/en/latest/reference/tool/upload.html>.
- The SDK comments link the official Yuque endpoint pages, including <https://www.yuque.com/115yun/open/evez3u50cemoict1>, <https://www.yuque.com/115yun/open/svfe4unlhayvluly>, <https://www.yuque.com/115yun/open/kzacvzl0g7aiyyn4>, and <https://www.yuque.com/115yun/open/ul4mrauo5i2uza0q>. Those endpoint pages were inaccessible to the current research client, so their contents and permission scopes remain unverified.
- BitTorrent v1 info-hash definition: <https://www.bittorrent.org/beps/bep_0003.html>.

### Probe and live prerequisites

- `cloud115_offline_probe.rs` stays inert in normal execution: it accepts no credentials and performs no HTTP request. Four test-only in-memory Mock tests cover upload → parse → single BT submit → task-list confirmation; explicit `wanted` index validation; and no resubmission after an unknown upload or submission result. They are orchestration mocks, not a live adapter or wire-level test.
- One unauthenticated read-only GET to `https://proapi.115.com/open/user/info` completed: HTTP 200, DNS 32.8 ms, connect 33.8 ms, TLS 198.9 ms, total 260.4 ms, response code `40140123`. `curl` reported peer `198.18.0.18`; no HTTP proxy environment variables were set. This runtime route does not establish the Windows RCH application's route or authenticated account reachability.
- No RCH process was running, so no active in-memory 115 session could be reused. No refresh-token operation, account/folder/task/quota API request, directory-ID lookup, Torrent upload, or BT submit was performed.
- This follow-up did not access `F:\种子`. No specific authorized sample was provided or selected. The existing directory ID for `/漫画/EH订阅` remains unverified.
- No changes were made to the existing 115 reader implementation. No P1 work was started.

### Parallel Rust failure review

The earlier default-parallel run had 12 failures in files untouched by the EH/P0-B changes. No pre-change parallel baseline result exists, so they cannot be conclusively labeled baseline failures; the source shows shared-state test-isolation hazards rather than evidence of a regression in the changed modules.

| Test | Source-based assessment |
|---|---|
| `api::cache::remote_book_completion_cleanup_tests::remote_folder_completion_removes_pages_but_preserves_raw_and_cover` | Process-global cache root can change between fixture creation and cleanup, making the cleanup resolve a different directory. |
| `api::source::d2_cache_authority_tests::d2_ca_state_transition_matrix` | Shared cache-root mutation plus a collision on the fixture name `state3` can move/remove the fixture directory. |
| `api::source::d2_cache_authority_tests::d2_ca3_miss_is_side_effect_free` | Another test writing covers under a concurrently selected global cache root can change the counted files. |
| `cache::rg_a_atomic_cache_file_tests::raw_cache_limit_evicts_oldest_packages_and_keeps_the_newest` | Eviction re-resolves the process-global cache root; another test can switch it before the check. |
| `api::remote_scan::session_ready_tests::l4_adjacent_events_consume_the_long_retry_budget_at_most_once` | A global profile reset/requeue test can change the seeded failed row before promotion; strongest candidate for the synchronous count mismatch. |
| `api::remote_scan::wake_tests::source_wake_drains_pending_without_duplicating_the_worker` | Unique worker key rules out these wake fixtures colliding with one another; process-global DB/worker state and retry-sleep wake timing remain candidates. |
| `api::remote_scan::wake_tests::exited_worker_is_restarted_by_a_source_wake` | Unique worker key; timeout is consistent with shared DB/scheduling interference, or a profile reset deleting the fixture row. |
| `api::remote_scan::session_ready_tests::l1_session_creation_promotes_overdue_compensation_and_wakes_a_consumer` | Promotion assertions pass, but the worker does not claim the pending row before timeout; shared worker/DB timing is likely. |
| `api::remote_scan::session_ready_tests::l3_a_renewed_session_rebinds_a_completed_generation_and_recovers_compensation` | Rebind/promotion assertions pass, then worker claim times out; shared state or scheduling is likely. |
| `api::remote_scan::session_ready_tests::l2_repeated_identical_notifications_do_not_duplicate_anything` | Promotion assertions pass, then worker claim times out; shared state or scheduling is likely. |
| `api::remote_scan::wake_tests::pending_without_a_session_survives_until_a_session_is_attached` | Fixture uses a unique source key and attaches session 42; timeout points to worker/DB scheduling or profile reset. |
| `api::remote_scan::wake_tests::repeated_source_wakes_do_not_duplicate_consumption` | All wakes use one unique source/session as intended; timeout points to shared worker/DB scheduling or profile reset. |

The cache tests share `CUSTOM_CACHE_ROOT`; remote-scan tests share the SQLite connection and worker registry. Their own fixture comments require single-threaded execution. The reset sweep is a concrete shared-state candidate. A worker sleeping until a retry deadline also has a plausible lost-wake timing hazard, but the one parallel run plus a serialized pass does not prove that production defect caused these test failures. No unrelated fix or parallelism change was made.

### Current P0 gate

`P0_BLOCKED`: SDKs provide a useful offline contract lead, but official endpoint-level documentation and permissions remain unverified, the app's existing session is unavailable, no authorized Torrent was supplied, and the real folder ID is unknown. The unauthenticated HTTP response establishes transport reachability only. Do not start P1.

### This follow-up validation

- `cargo test --locked --offline --example cloud115_offline_probe`: exit 0; 4 passed, 0 failed, 0 ignored.
- `cargo fmt --all -- --check`: exit 0; `rustfmt --edition 2021 examples/cloud115_offline_probe.rs` had also exited 0.
- Static audit for HTTP methods/client construction, environment credential reads, task deletion, and clear-task calls: no matches in the probe; `rg` exited 1 because no matching lines were found.
- `git diff --check`: exit 0.
- No full Rust or Flutter suite was rerun in this follow-up. The previously recorded results above remain historical evidence, not a new validation of this test-only edit.

## P0-B-R2 evidence - 2026-10-11

### Workspace and stored auth mode

- Rechecked local repository state: branch `local-ai/cover-quark-debug`, HEAD `96edfd24cf643c2f7928b2039afcf11a5fe6a1be`. Existing modified and untracked files were retained.
- Read-only inspection of source metadata (without selecting credential values) identified source `115_1789360028897`, type `115`, with a stored Web Cookie and no Open refresh token or App ID. `cloud115SessionFor()` selects the Cookie/Web path when the Cookie field is non-empty; it does not convert that Cookie to Open API Bearer auth.
- The source's configured starting folder ID is `3481445593719506553`; this does not establish that the ID is `/漫画/EH订阅`.
- Started the existing local RCH Debug app normally (PID 37892; process remained responsive). The source list showed `需连接`, which is not evidence that its 115 session is valid or invalid. No safe `session_warmup` diagnostic appeared at the default data root during this check, so authenticated Web-session validity remains unconfirmed. No separate login/client or token refresh was used.

### Cookie/Web route contract research

- The p115client reference documents a Cookie/Web cloud-download route `POST https://clouddownload.115.com/lixianssp/?ac=add_task_bt`, with `info_hash`, optional zero-based comma-separated `wanted`, `savepath`, and `wp_path_id`; the same reference documents GET task-list and torrent-inspection methods. This is third-party SDK documentation/source, not an official 115 authorization or stability guarantee.
- The same reference says its `clouddownload_get_id` GET can create the Torrent/task folders when they do not exist. It is therefore not a safe read-only probe and was not called.
- The official Open route remains a separate contract family (`/open/offline/...`) and requires Open auth. The account has no configured Open credentials in the inspected source; no Cookie-to-Bearer assumption was made.
- No Cookie/Web BT adapter was found in RCH's Rust or Dart source. No authenticated read request was sent, and no write request was sent.

### Folder, network, and sample

- No remote folder traversal was made. `/漫画/EH订阅` and its actual folder ID remain unverified; no folder was created.
- `proapi.115.com` resolved locally to `198.18.0.18`, and Windows selected the `Mihomo` interface for that address. WinHTTP was direct, WinINET proxy was disabled, and HTTP(S)/ALL proxy environment variables were absent. A `curl --noproxy '*'` unauthenticated GET reached peer `198.18.0.18` and returned HTTP 200 with business code `40140123` (missing/invalid access token). Together this is evidence consistent with a Mihomo TUN/Fake-IP path, not proof of the upstream public destination. No DNS, proxy, TLS, or security setting was changed.
- No file in `F:\种子` was read, enumerated, hashed, or uploaded during R2. No authorized single Torrent has been designated.

### Gate and validation

- R2 made no code changes and ran no tests. The previous targeted Mock result remains historical: 4 passed. No authenticated 115 response, folder ID, Torrent validation, upload, or BT submission exists.
- The 12 parallel Rust test failures remain `BASELINE_UNRESOLVED`; they were not rerun and no unrelated fixes were made.
- `P0B_READONLY_READY` is not met because there is no confirmed active account session, no authenticated permission result, no verified target-folder ID, and no authorized Torrent sample. Keep `P0_BLOCKED`; `P1_NOT_STARTED`.
- This note records the current read-only research; it is not authorization for any write operation.

References: [p115client Cookie/Web client reference](https://p115client.readthedocs.io/en/latest/reference/module/client.html), [p115client source reference](https://p115client.readthedocs.io/en/latest/_modules/p115client/client.html), [115 Open Platform](https://open.115.com/).
## P0-B-R2 retry after user confirmation - 2026-10-11

- User confirmed the existing Web Cookie is valid and asked to retry. No QR scan was opened and no credential value was read or shown.
- The running RCH app's existing `漫画` source displayed its configured root ID `3481445593719506553` and a populated direct-child listing. This confirms read-only access through the existing RCH Web session. The direct-child listing did not contain `EH订阅`; no target folder ID was obtained. The source label/root ID does not independently prove the canonical `/漫画` path, so the exact target path remains unverified.
- The app UI already showed an existing full scan in progress after normal startup. No manual refresh, folder traversal beyond the currently displayed root, new scan, cancellation, or folder creation was initiated.
- Rechecked network configuration: HTTP(S)/ALL proxy environment variables absent, WinHTTP direct, WinINET proxy/PAC disabled; `proapi.115.com` still resolves to `198.18.0.18`, with route interface `Mihomo`. No proxy/DNS/TLS settings changed and no extra 115 Open API request was sent.
- No authorized single Torrent was designated or accessed. No upload or task submission was performed. Keep `P0_BLOCKED` and `P1_NOT_STARTED`.

## Correction after wrong-build directory check - 2026-10-11

- The earlier direct-child listing and its conclusion about whether `EH订阅` exists came from the installed executable at `D:\RCH\RCH.exe`, not the requested repository build. Withdraw that listing and every conclusion based on it; it is not evidence about the user's current RCH source or target directory.
- The user confirms the existing 115 session is connected. The repository UI label `需连接` conflicts with that confirmation and is therefore a status-display defect, not evidence of failed authentication. A follow-up TODO was added to `implement.md`; until fixed, do not use this label to classify session validity.
- No correct-build authenticated folder response or target folder ID was independently captured in this retry. Do not treat the old root listing, configured starting ID, or UI label as proof of the `/漫画/EH订阅` folder ID.
- No Torrent file was designated or read, and no remote write was made. P0 remains `P0_BLOCKED`; P1 remains `NOT_STARTED`.

## Final P0-B gate recheck - 2026-10-11

### Baseline and live RCH process

- Rechecked the repository: branch `local-ai/cover-quark-debug`, HEAD `96edfd24cf643c2f7928b2039afcf11a5fe6a1be`; all pre-existing modified and untracked work remains present.
- A live process check found PID 5812 at the repository Debug executable `D:\Projects\RCH-source\app\build\windows\x64\runner\Debug\RCH.exe` (product version `0.6.5+100605`). The separately installed build is `0.6.5+605`; it was not running. This result is from the current process table, not historical UI state.
- No credentials were read into output, copied, or exported.

### Gate 1 - authentication and official capability

- Safe database presence checks identify the user 115 source `115_1789360028897` as Web Cookie mode: Cookie present; Open refresh token, client ID, and client secret absent. The RCH `cloud115SessionFor()` implementation routes a non-empty Cookie to `cloud115CookieSessionFor()`; that Cookie is not an Open API Bearer token. The configured root ID `3481445593719506553` is only stored source configuration, not proof of the requested folder path or ID.
- The repository Debug process is running, but this check did not perform an authenticated 115 request through it. No independent in-memory-session success is claimed. No second client, login, token refresh, or credential export was used.
- An official-source search found the [115 Open Platform](https://open.115.com/) developer service terms, which restrict applications to published interfaces and their granted capabilities. The official page exposed in this check did not provide a BT upload/parse/submit contract or evidence that this application/account is approved for those operations. No Open credentials are configured in the existing source. No authenticated Open request was sent.
- Consequently Gate 1 fails for a compliant real Torrent write route: `P0_AUTH_BLOCKED`. No remote directory traversal was sent, `/漫画/EH订阅` was not confirmed to exist, and no target `wp_path_id` was established. The previously configured root ID and the retracted wrong-build listing are not substitutes.

### Gate 2 - one local Torrent metadata check

- Per the user's instruction to inspect any file in `F:\种子`, PowerShell selected one direct-child `.torrent` and the local parser read only that metainfo file; no other candidate names or contents were reported or inspected. Nothing was uploaded. Full `.torrent` SHA-1: `e4e05b8b7a2b4d4f3b3bb25e2b651a86e81176ba`; v1 infohash SHA-1: `03976b4c0bad6a75f96a0c1b7a715ae5f1c4ad30`.
- Bencode parsed with no trailing bytes, duplicate dictionary keys, or non-canonical dictionaries. It describes one root-level `.zip` payload, 145,657,277 bytes, with 278 piece hashes at 524,288 bytes each; piece-count/size arithmetic is consistent. The candidate file index is `0`. RCH recognizes `.zip` as a comic/archive extension (`app/lib/store/library_index_service.dart`); this does not prove the payload bytes exist or that RCH can open this particular archive.
- Tracker metadata is present, but no tracker/DHT announce or seeder check was made. The payload itself was not downloaded or validated against piece hashes. User authorization to inspect the local metainfo does not establish authorization to upload or remotely download this resource; no such transfer was attempted. Therefore the sample is not cleared for Gate 3 (`P0_SAMPLE_BLOCKED` for remote use).

### Gate 3, tests, and final gate

- Real 115 upload requests: 0. BT task submissions: 0. No task-list or target-folder API request was sent. No file or task was created, modified, or deleted remotely.
- No new Mock suite or broad tests were run. The existing focused P0-B Mock result remains historical: 4 passed. The 12 parallel Rust test failures remain `BASELINE_UNRESOLVED`.
- Final status: `P0_AUTH_BLOCKED` (Gate 1); the selected sample is additionally `P0_SAMPLE_BLOCKED` for remote transfer authorization. P1 remains `NOT_STARTED`. Stop before Gate 3. No commit, push, merge, rebase, tag, release, deletion, or cleanup.

## P0-Web Cookie protocol research - 2026-10-11

- Added [`p0-web-cookie-protocol.md`](p0-web-cookie-protocol.md) with a source-by-source comparison of p115client, OpenList/115driver, historical 115wangpan, and Offline-115. It separates official authorization evidence, third-party implementation evidence, real API responses (none for Web Cookie), and synthetic Mock responses.
- The sources show materially different Web routes: historical clear-form upload/parse/submit versus current p115client `ssp` encrypted logical payload; Open API routes are a separate Bearer-auth contract. `Offline-115` converts Torrent metadata to Magnet and does not perform a real Torrent upload/BT file selection flow.
- A loopback-only, test-only Rust model now covers the historical clear-form flow and fail-closed handling. `cargo test --locked --offline --test cloud115_web_cookie_protocol`: exit 0; 7 passed, 0 failed, 0 ignored. It does not model p115client's encrypted `ssp` wire format and is not connected to production.
- No Cookie value was read, no live 115 request was made, and no file under `F:\种子` was accessed in this turn. No real upload or task submission was performed. The actual `/漫画/EH订阅` folder ID remains unverified.
- `P0_WEB_CONTRACT_READY` applies only to comparative static modeling plus this scoped historical-protocol Mock. `P0_WEB_WRITE_AUTH_BLOCKED` remains the operative gate because official authorization for Web Cookie writes is unproven. `P0_WEB_READONLY_PASS=NO`, `P0_REAL_PASS=NO`, `P1=NOT_STARTED`.
- The 12 parallel Rust failures remain `BASELINE_UNRESOLVED`. No changes were made to the RCH reader chain. Commit/Push/Merge/Rebase/Tag/Release/Delete/Cleanup: NO.

## P0-Web-R3 closeout — 2026-10-11

- The historical clear-form protocol test is now explicitly classified `LEGACY_WEB_PROTOCOL_MODEL`. It is not a model of current p115client SSP wire behavior. The current p115client source at frozen commit `856e4f8f09ce034534bd458f06d3dee4474a1698` uses an RSA-encrypted JSON `data` field for the default SSP BT-add entry; its Torrent parse/task-list calls still default to a separate Web path. No trustworthy current SSP vector or live response was obtained: `SSP_WIRE_UNVERIFIED`.
- Frozen upstream revisions and the full source-level field comparison are recorded in [`p0-web-cookie-protocol.md`](p0-web-cookie-protocol.md) and [`p0-web-r3-closeout.md`](p0-web-r3-closeout.md). Third-party SDK evidence does not establish permission to use Web Cookie writes.
- Correct-build read-only validation was not performed. No RCH process was running; the available Debug binary predates the current EH store/panel edits. Starting the candidate would warm up all stored sources and could fan out requests/scans, so it was not launched. No cookie was read and no authenticated request was sent.
- `/漫画/EH订阅` existence, account/parent relationship, and directory ID remain unverified. No folder-creating endpoint was called.
- Semi-automatic handoff is a proposal only. It uses the existing local manifest and output folder, distinguishes local generation, waiting, user claim (unverified), read-only cloud observation, and RCH identification, and reuses existing scanning without triggering it. SPEC §10 proposal is documented but not applied; no runtime code or new table was added.
- Gate: `P0_BLOCKED`; `P0_WEB_WRITE_AUTH_BLOCKED`; `P0_WEB_READONLY_PASS=NO`; `P0_REAL_PASS=NO`; `P1=NOT_STARTED`. The earlier 12 parallel Rust failures remain `BASELINE_UNRESOLVED`. No R3 test was run because this update only classifies the test model and adds research documentation.
- No real Torrent upload, BT write, remote folder creation, or cloud task operation was performed. Commit/Push/Merge/Rebase/Tag/Release/Delete/Cleanup: NO.

## 2026-10-12 Cookie BT in-app destination follow-up

- The Windows Debug candidate now offers a read-only browser for existing 115 folders. It does not assume the source `root_id` is the destination and does not create folders. The selected folder ID/name/parent is revalidated before preflight, upload recovery, BT submission, and status queries.
- The first H0-validated Torrent is selected by default; a user may choose another. Upload/parse and BT submission remain separate, single-item explicit-confirmation actions. The panel performs no 115 request on initialization; folder listing starts only after the user opens the picker. This is not unattended or batch upload.
- No real 115 request, Cookie read, directory lookup, upload, or submission was performed in this development run. `SSP_WIRE_UNVERIFIED`, `P0_WEB_WRITE_AUTH_BLOCKED`, `P0_REAL_PASS=NO`, and `P1=NOT_STARTED` remain unchanged. The actual target folder ID is still unknown.
- Targeted evidence: Rust BT tests 21/21 passed in three consecutive default-parallel runs; focused Flutter panel tests 5/5 and targeted analyzer passed; `cargo check --lib --locked --offline` passed; standard `flutter build windows --debug --no-pub` passed. The app was not launched. The earlier full Flutter suite result (291 passed, 0 skipped) remains from the same runtime changes before the test-only Mock-server correction.
- The intermittent loopback Mock failures were traced to nonblocking accepted TCP streams on Windows: partial HTTP headers caused `read_exact` to return `WouldBlock`. The test server now switches accepted streams to blocking mode while retaining a bounded read timeout. Its Torrent fixtures also reserve unique temporary directories with exclusive `create_dir`.
- Automatic batch upload remains outside this one-shot P0 candidate. Automatic local `.torrent` deletion is also deferred: there is no durable cleanup marker, so deletion would be indistinguishable from data loss in the H0 manifest and could discard recovery evidence. No cleanup was performed.
- Full-crate `cargo fmt --all -- --check` still reports formatting differences in unrelated EH Daily files; changed Rust files pass scoped `rustfmt --check`. The 12 historical parallel Rust failures remain `BASELINE_UNRESOLVED`.

Commit: NO / Push: NO / Merge: NO / Rebase: NO / Tag: NO / Release: NO / Delete: NO / Cleanup: NO.

## Cookie BT retry and workflow decision — 2026-10-13

### Evidence from the current Windows candidate

- Rechecked the local repository at `bd46f3fa0da388b78d8b0c76d15e425265053b98` on `local-ai/cover-quark-debug`; all pre-existing dirty files remain untouched. The standard Debug RCH process from this checkout is still running (PID 33580); no GUI action or process termination was performed during this audit.
- The user-visible 115 task list previously showed one matching submitted comic task as `下载成功`, and the destination folder showed its downloaded folder. This is evidence that at least one real Cookie-Web BT job reached cloud completion. It does not establish RCH reader success or official authorization for the third-party Web write route.
- In the latest one-item SSP attempt, upload-init response decoding failed. A subsequent read-only recovery found a Torrent in the selected target folder with the same whole-file SHA-1, parsed it, and matched its Infohash and file list. That proves a matching remote Torrent file was available; it does not prove the failed upload attempt created it.
- The single BT submit returned `state=false` and was surfaced as `REJECTED`; no second submit was sent. A subsequent read-only task query stopped at the 300-item safety limit and returned `unknown`, explicitly warning that unread pages may still contain the hash. No positive task proof was obtained and RCH performed no deletion in this flow.
- The experimental one-item upload/submit handlers in `eh_subscription_panel.dart` do not write their intents/results to the persistent batch ledger. The batch path does. This is a real idempotency gap: a later batch could treat a manually tested item as untouched if it is again present in the configured output directory.
- Current rules resolve the configured output directory to `F:\\种子`; a read-only audit found 88 direct `.torrent` files. The exact Infohash shown in the one-item panel was not present in that current directory or its current manifest at audit time, so this report does not fabricate a ledger filename or claim that local copy is retained. The reason for that discrepancy is unknown and must be resolved before retrying that exact sample.
- Current 115 UI showed 1,986 total tasks and a `1477/1500` quota counter. Until the service reports otherwise, treat the apparent headroom as 23 tasks and do not start an 88-item batch.

### Feasible validation and implementation plan

1. Route every user-triggered single-item or batch write through `Cloud115BtBatchProcessor` and its cross-process ledger. Persist `upload_started` / `submit_started` before each write; persist the returned result immediately. Do not keep a parallel production write path in the panel. The same account + Infohash + target-folder key must govern single-item and batch retries.
2. Keep one sequential loop over H0-validated files from the configured directory. A per-item upload/parse/submit uncertainty is recorded, the local Torrent is retained, and the next independent item proceeds. Only account-wide barriers (WAF/challenge, auth loss, explicit rate limit/quota, account or target change, ledger failure) stop the batch. Never translate a per-item unknown into overall success.
3. Preserve 115-side duplicate handling: do not scan all 1,986 tasks before every new submission. Submit at most once for a new ledger key. If the API returns an explicit duplicate result, perform the existing bounded read-only confirmation; delete local `.torrent` only after the matching account, Infohash, and destination are positively confirmed and cleanup is enabled. If the confirmation is still `unknown`, keep the Torrent and continue the batch.
4. Improve SSP rejection diagnostics using an allowlisted business code/reason parser. The current adapter retains only a generic `state=false` rejection, so quota, duplicate, invalid wanted/path, permission, and signature/protocol errors cannot be distinguished. Store/display only the bounded sanitized code/reason; never log raw response bodies, Cookie, or decrypted SSP payload. Add Mock cases for each known response shape before the next real write.
5. Keep upload recovery distinct from BT task recovery. For `UNKNOWN_UPLOAD_RESULT`, check the exact target folder for the same Torrent SHA-1 and parse it; reuse only when the remote Infohash/file list matches. For `UNKNOWN_REMOTE_STATE`, query read-only and never repeat POST. If the bounded task list cannot prove absence, retain a reconciliation-pending state and process other files.
6. Add focused tests proving: one unknown item does not stop later independent items; ledger restart prevents re-POST; known duplicate only cleans after positive task proof; a 300-item query limit remains unknown; quota/WAF/auth stops the batch; safe SSP reason mapping does not expose response secrets. Add UI tests that distinguish item-level pending from a batch-wide stop.
7. Rebuild the standard Windows Debug candidate and run one controlled real item through this shared ledger path. Use a `.torrent` currently present in the configured directory, the already connected Cookie source, and the already selected folder. Verify task appearance in 115, wait for terminal success, verify the target folder, then open the resulting file through the normal RCH source. The present quota counter makes this single-item gate mandatory before any larger batch.
8. Keep Daily writes disabled until that real task completes the full cloud-folder and RCH-reader loop. A prior successful task is useful feasibility evidence, but it does not waive the current P0 gate or establish the latest adapter behavior.

### Gate

- Real BT feasibility: demonstrated by an earlier completed 115 task.
- Latest retry: `P0_PARTIAL`; current SSP submit explicitly rejected, remote task existence remains unknown, and the attempted local file cannot currently be correlated with the configured inventory.
- `P0_REAL_PASS=NO`; official Web-write authorization and the complete RCH-reader loop remain unverified. Daily cloud writes remain P0-gated; `P1_NOT_STARTED`.
- No further 115 write, local Torrent deletion, code change, or test run was performed in this audit. The existing 12 parallel Rust failures remain `BASELINE_UNRESOLVED`.

Commit: NO / Push: NO / Merge: NO / Rebase: NO / Tag: NO / Release: NO / Delete: NO / Cleanup: NO.