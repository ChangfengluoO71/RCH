# P0-Web-R3 closeout and next-stage design

Date: 2026-10-11
Scope: protocol research freeze, safe RCH-session readiness check, and a semi-automatic product proposal. No 115 request or write was performed. No production runtime or SPEC change was made.

## Baseline and preserved worktree

- Repository: `D:\Projects\RCH-source`
- Branch: `local-ai/cover-quark-debug`
- HEAD: `96edfd24cf643c2f7928b2039afcf11a5fe6a1be`
- Worktree: existing edits were present before this closeout and remain untouched except for the explicit test-model classification comment and the research notes in this task directory. No reset, stash, cleanup, or Git history operation was run.
- RCH process: no `RCH.exe` process was running when checked.
- Existing Debug binary: `app/build/windows/x64/runner/Debug/RCH.exe`, product version `0.6.5+100605`, timestamp 2026-10-11 15:58:03. The current EH store/panel edits have later timestamps, so this binary is stale relative to the checked-out candidate and was not used as the correct build.

### Read-only gate result

The only session eligible for this check is the existing RCH Web session. There was no running process holding its in-memory `Cloud115WebClient`, so this session could not be reused. No Cookie value or stored credential was read, refreshed, copied, logged, or sent. No second client was created.

Launching the candidate app was not a safely scoped substitute. `main.dart` starts `RemoteScanCoordinator.warmUpSessions` for all stored sources during startup. That coordinator establishes each eligible session and notifies the scan coordinator; whether automatic scanning is enabled in this user's persisted settings was not read. Starting the app here could therefore issue requests beyond the one 115 directory-list operation. The stale binary also cannot stand in for the current candidate. The app was not launched and no authenticated request was sent.

Consequently:

- Normal 115 directory listing through the live RCH session: **not performed**.
- Authentication mode of the in-memory session: **not observable in this process-free state**. No claim is made that the stored source is Open or Web for this check.
- `/漫画/EH订阅` existence, account association, parent chain, and actual directory ID: **not verified**. A configured `root_id` is not treated as the target ID.
- Folder creation or potentially side-effecting `get-id` calls: **not performed**.
- Safe next input: run the correct current candidate build and leave its already configured 115 source/session active, with automatic source warm-up/scanning scoped or paused if the app exposes that control. Once the actual running session is available for ordinary read-only browsing, list one directory component at a time from the verified source root. Stop on a missing or ambiguous child; do not create it.

The existing 115 reader's normal list request includes `record_open_time=1` in `app/rust/src/source/cloud115.rs`. That parameter appears to record an access/open time; this is an inference from the field name and is part of normal browsing behavior. No such request was made in R3.

## Protocol freeze

The loopback tests in `app/rust/tests/cloud115_web_cookie_protocol.rs` and `app/rust/tests/support/cloud115_web_cookie_protocol.rs` are classified `LEGACY_WEB_PROTOCOL_MODEL`. They model a historical clear-form Cookie flow; they do not model the current p115client SSP wire behavior.

Current p115client source at frozen commit `856e4f8f09ce034534bd458f06d3dee4474a1698` has a mixed route selection:

- `clouddownload_task_add_bt` defaults to `type="ssp"`. It posts to `https://clouddownload.115.com/lixianssp/`, adds `ac=add_task_bt` and `app_ver`, stringifies logical payload values, JSON-serializes and RSA-encrypts the payload, then sends the encrypted ASCII in the form field `data`. It sets `ecdh_encrypt=False`. It does not add the legacy `uid`, `sign`, or `time` fields in this wrapper.
- The SSP response path attempts JSON decoding and then RSA-decodes a response `data` field; failed decryption is retained as an undecoded value. No live response confirms this behavior against the service.
- The BT-add logical payload includes `info_hash`, zero-based comma-separated `wanted`, `savepath`, and `wp_path_id`. `savepath` is modeled as a relative path beneath the destination folder. This is distinct from Open API `save_path`, `torrent_sha1`, and `pick_code` naming.
- `clouddownload_torrent` and `clouddownload_task_list` still default to `type="web"`, not SSP. The Web wrapper constructs a `/web/` request path; the source method documentation's URL description is not fully consistent with that construction.
- The sign helper uses `https://115.com/?ct=clouddownload&ac=space`, while the historical model uses `/?ct=offline&ac=space` plus a timestamp argument.
- The pinned upload helper follows a token/init/object-storage/callback flow; its Cookie branch may construct another `P115Client` from a Cookie header. RCH must not copy that behavior because it would create a second client/session lifecycle.

| Data | Source evidence | What remains unverified |
|---|---|---|
| `ac`, `app_ver`, outer `data` | Pinned p115client SSP wrapper | Live request acceptance and app/account entitlement |
| RSA-encrypted JSON; `ecdh_encrypt=False` | Pinned p115client implementation | Exact live bytes, cipher dependency/version behavior, and response compatibility |
| `info_hash`, `wanted`, `savepath`, `wp_path_id` | p115client BT-add logical model | Server-side validation, index rules, target resolution, and real response |
| `uid/sign/time` | Historical clear-form implementation only | Not established as SSP fields; current Web parse/list wire behavior remains unverified |
| `sha1`, `pickcode`, upload callback IDs | SDK-specific upload/parse models | Current Cookie upload-to-parse contract and exact response fields |
| Duplicate detection and terminal task states | SDK descriptions and historical mock logic | Real service matching behavior and response contract |

`info_hash` (SHA-1 of the raw bencoded v1 `info` dictionary) and the complete `.torrent` file SHA-1 identify different byte ranges. Neither a successful historical mock nor SDK source establishes current SSP compatibility. The inspected p115client project metadata specifies `p115cipher >= 0.0.6`, not a single exact cipher revision. Classification: **`SSP_WIRE_UNVERIFIED`**.

The historical seven Mock tests are previous evidence only. This R3 closeout did not extend or rerun the Mock suite. No Open API or Web write authorization is established by third-party code; the Web Cookie write route remains blocked.

### Frozen upstream revisions

Captured using read-only `git ls-remote` on 2026-10-11:

| Project | Commit | Pinned reference |
|---|---|---|
| `ChenyangGao/p115client` | `856e4f8f09ce034534bd458f06d3dee4474a1698` | [client.py](https://github.com/ChenyangGao/p115client/blob/856e4f8f09ce034534bd458f06d3dee4474a1698/p115client/client.py) · [upload.py](https://github.com/ChenyangGao/p115client/blob/856e4f8f09ce034534bd458f06d3dee4474a1698/p115client/tool/upload.py) · [offline.py](https://github.com/ChenyangGao/p115client/blob/856e4f8f09ce034534bd458f06d3dee4474a1698/modules/%40p115/p115/%40pending/offline.py) |
| `OpenListTeam/OpenList` | `482464c2affc5eeba5d4a25bb272aeb352ec5918` | [115 driver](https://github.com/OpenListTeam/OpenList/blob/482464c2affc5eeba5d4a25bb272aeb352ec5918/drivers/115/driver.go) · [115 Open upload](https://github.com/OpenListTeam/OpenList/blob/482464c2affc5eeba5d4a25bb272aeb352ec5918/drivers/115_open/upload.go) |
| `OpenListTeam/115-sdk-go` | `788107ec0545cf126ee9b22329678679cc6b3b2a` | [offline.go](https://github.com/OpenListTeam/115-sdk-go/blob/788107ec0545cf126ee9b22329678679cc6b3b2a/offline.go) · [const.go](https://github.com/OpenListTeam/115-sdk-go/blob/788107ec0545cf126ee9b22329678679cc6b3b2a/const.go) |
| `SheltonZhu/115driver` | `542720cb0034954750454e89f32b4852add6e2a9` | [offline.go](https://github.com/SheltonZhu/115driver/blob/542720cb0034954750454e89f32b4852add6e2a9/pkg/driver/offline.go) |
| `shichao-an/115wangpan` | `e7cc935313f675e886bceca831fcffcdedf1e880` | [api.py](https://github.com/shichao-an/115wangpan/blob/e7cc935313f675e886bceca831fcffcdedf1e880/u115/api.py) |
| `owtotwo/Offline-115` | `780fd3d0792fb172af5e71d8de8514f90bb9bfca` | [offline_115.py](https://github.com/owtotwo/Offline-115/blob/780fd3d0792fb172af5e71d8de8514f90bb9bfca/offline_115.py) |

## Semi-automatic handoff proposal

This is a design proposal only. It does not authorize P1 implementation, change SPEC, add a local table, or invoke any 115 write API.

### User flow

1. Read the existing EH `manifest.json` and show saved `.torrent` items only; do not crawl the output directory or other local paths.
2. Let the user select exactly one item. Before handoff, resolve its relative path under the configured output directory, reject traversal/out-of-root paths, verify existence and byte count, parse bencode, and compare the computed v1 infohash with the manifest. The current manifest has `file`, `bytes`, and `infohash`, but no whole-file checksum. If persistent byte-integrity evidence is required, propose an additive `torrent_sha256` field; do not add it in this research stage.
3. Show local de-duplication by manifest infohash and whether the matching local file passed validation. This is only local EH-manifest de-duplication; it says nothing about 115 tasks.
4. Reuse the existing Windows folder-opening action (`explorer`) to open the configured Torrent directory. The current UI already has an `_openOutDir` helper in `eh_subscription_panel.dart`.
5. Give instructions to use an official 115 client/web flow that is actually available to the user's account. Do not automate browser actions or fall back to undocumented Web endpoints. If no supported official BT operation is available, stop after local preparation.
6. Allow an explicit user action “I submitted this manually.” Persist it only as a user claim in the existing local manifest, with a timestamp and an `unverified` marker. It must not be displayed as task accepted, downloading, completed, or remotely confirmed.
7. Show cloud discovery only when the existing 115 source/listing has already observed the expected output under the mounted target. Match against the selected Torrent's parsed file paths and sizes where possible; do not start a new unbounded scan or infer a match from title alone. A discovered file proves presence in the listing, not that a BT task completed or that this exact submission created it.
8. Show “RCH identified” only after the existing RCH source/library index identifies the corresponding comic. Reuse existing source browsing and scan state; do not force a scan. If no 115 book source is mounted, prompt the user to mount the target directory without changing source configuration.

Represent the five requested milestones as separate evidence facts, not a single monotonic state: `TORRENT_GENERATED`, `WAITING_FOR_MANUAL_SUBMISSION`, `USER_CLAIMS_SUBMITTED` (unverified), `CLOUD_READONLY_DISCOVERED`, and `RCH_IDENTIFIED`. Cloud discovery or library identification can occur independently of the user's claim. A user claim alone must never advance either remote-evidence state.

### Minimal local data and UI changes if later authorized

- No SQLite work-state table is needed for the proposal. Generated and waiting states derive from the existing manifest and local-file validation.
- Add optional manifest metadata only if the product needs it: `torrent_sha256` and `handoff.user_claimed_submitted_at` plus an explicit schema/versioned unverified-claim marker. Preserve unknown/old manifest fields across read/write and keep all writes in the already selected output directory.
- Derive `CLOUD_READONLY_DISCOVERED` and `RCH_IDENTIFIED` from existing 115 source and library observations. Do not persist a second competing cloud task state. If durable remote-evidence history later requires a new table, return with a separate design and wait for authorization first.
- Add a per-item status/evidence panel, one-item selection, integrity/dedup display, “open folder,” official manual-operation instructions, and the explicit unverified claim action. Do not add batch cloud controls or deletion.
- Likely implementation touchpoints, subject to a separate authorization: `app/rust/src/eh_subscription.rs` for additive manifest integrity fields and safe local verification; `app/lib/store/eh_subscription_store.dart` for manifest/status mapping; `app/lib/ui/eh_subscription_panel.dart` for the handoff UI. This R3 task changed none of those production files.

### SPEC §10 proposal (not applied)

- **§10.1 Positioning:** add the optional local handoff after local Torrent creation. State that RCH prepares a local file for the user; external manual submission is performed by the user in an officially supported 115 interface.
- **§10.2 B2 Write boundary:** clarify that an optional manual-submission claim may be stored additively in the same user-selected `manifest.json`. It is local user-provided metadata, never a remote success fact. No canonical metadata, library, credential, or reading data writes are added.
- **§10.2 B3 and B4:** preserve the independent reading-chain boundary and default-off/explicit interaction. Handoff must not start source scans or remote writes.
- **§10.4 Acceptance:** assert the distinction between generated, waiting, user-claimed, cloud-read-only-discovered, and RCH-identified; assert that user claims do not imply remote success and that remote/library evidence comes only from existing RCH reads.

This proposed change remains a design for review. SPEC §10 was not edited.

## Future authorized automation boundary

Only if 115 formally authorizes the route and the product owner separately authorizes automation, the smallest adapter should live on the existing RCH session/client registry and accept a source/session identifier—not a raw Cookie. It should reuse the existing reqwest client, request budget, WAF cooldown, and refresh coordination; it must not construct a second client or credential store.

Conceptual operations:

1. `list_children(session, parent_id)` and exact one-component-at-a-time `resolve_folder(session, root_id, path)`; no recursive search and no create-on-read endpoint.
2. `list_tasks(session, account_id, folder_id, info_hash)` for read-only duplicate reconciliation.
3. `upload_torrent(session, local_path, expected_torrent_sha1)` returning a structured remote object reference or `UNKNOWN_REMOTE_STATE`.
4. `parse_torrent(session, pick_code, torrent_sha1)` returning validated `info_hash` plus the ordered file list.
5. `submit_bt_once(session, folder_id, info_hash, wanted, relative_save_path)` with explicit outcomes `NOT_SENT`, `ACCEPTED`, `REJECTED`, or `UNKNOWN_REMOTE_STATE`.
6. `reconcile_submission(session, account_id, folder_id, info_hash)` before any retry after an unknown result.
7. Read-only status/file listing, followed by existing RCH source discovery; never mark library discovery before RCH itself identifies the item.

The idempotency key must include stable account identity, `info_hash`, and target folder ID. Validate `wanted` as zero-based indexes within the exact parsed file list. WAF/405/challenge pauses the account's background work and does not trigger a proxy, host, or account change. These are interface requirements, not proof that any Web route is authorized or stable.

## Gates and unresolved quality items

- Historical Web model: `LEGACY_WEB_PROTOCOL_MODEL`; its prior 7 Mock passes remain limited to that model.
- Current SSP: `SSP_WIRE_UNVERIFIED`.
- RCH authenticated read-only listing: not performed; no active RCH process and current Debug binary stale relative to working tree.
- Target `/漫画/EH订阅` and directory ID: not verified.
- Web write permission: `P0_WEB_WRITE_AUTH_BLOCKED`.
- Real Torrent upload/BT submission/download/RCH reading: none performed; `P0_REAL_PASS=NO`.
- P0: remains `P0_BLOCKED`; this R3 protocol/design closeout is not a real BT acceptance.
- P1: `NOT_STARTED`.
- Earlier 12 parallel Rust failures: `BASELINE_UNRESOLVED`; this turn did not rerun or resolve them.
- No R3 tests were run: changes were limited to test classification documentation and Trellis research notes.

## Git boundary

Commit: NO · Push: NO · Merge: NO · Rebase: NO · Tag: NO · Release: NO · Delete: NO · Cleanup: NO.
