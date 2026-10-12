# P0-Web: 115 Cookie BT Protocol Feasibility

> **R3 freeze:** the local clear-form test is classified as `LEGACY_WEB_PROTOCOL_MODEL`. It does not represent current p115client SSP wire traffic. Current SSP remains `SSP_WIRE_UNVERIFIED`; the historical 7 Mock passes are not SSP compatibility evidence.

Date: 2026-10-11
Scope: public-source contract comparison and loopback-only Rust mocks. No real Cookie was read, no authenticated request was sent, and no 115 write endpoint was called.

## Finding

RCH can technically host a separate Web Cookie adapter by adding methods to the already-created `Cloud115WebClient` session. Several community implementations describe a complete legacy Cookie flow, but they disagree on endpoints and wire formats. The current p115client `ssp` path encrypts its logical BT payload; the local mock models only the older clear-form flow from `shichao-an/115wangpan`. Neither implementation proves 115 authorization, endpoint stability, or compatibility with the RCH account.

Gate result: `P0_WEB_CONTRACT_READY` for comparative static modeling and the explicitly limited historical clear-form mock; `P0_WEB_WRITE_AUTH_BLOCKED` for any real Cookie-mode write. `P0_WEB_READONLY_PASS` and `P0_REAL_PASS` are not achieved. P1 remains `NOT_STARTED`.

## Evidence classes

| Evidence class | What it establishes | What it does not establish |
|---|---|---|
| Official 115 platform terms | Integrations must use interfaces and capabilities published/granted by 115. See [115 Open Platform](https://open.115.com/). | No publicly reviewed document established that this RCH app/account may use Web Cookie BT upload, parse, or submission endpoints. |
| Third-party SDK / driver source | Concrete observed request shapes and implementation differences below. | Official authorization, service guarantees, current server acceptance, or RCH account permission. |
| Actual 115 API response | None for Cookie mode in this investigation. | No live endpoint or business behavior is confirmed. Earlier unauthenticated Open API responses recorded in `p0-gate.md` do not validate Web Cookie behavior. |
| Local Mock response | The test model sends/handles the modeled shapes and fails closed on uncertainty. | No compatibility claim for a live 115 service. |

The official Yuque endpoint pages linked by SDK comments were not independently accessible during research. A third-party source or an HTTP response from a Mock is not a substitute for those permissions.

## Web Cookie and Open API contract comparison

| Stage | Legacy Web Cookie examples | Open API examples | Important distinction |
|---|---|---|---|
| Auth/session | Cookie + browser-like User-Agent; `sign`/`time` is fetched from a Web cloud-download/legacy endpoint. `p115client` uses its `clouddownload_sign`; historical `115wangpan` uses `/?ct=offline&ac=space` and receives sign/time. | Open SDK calls use Open authorization and the app's granted scopes. | A Web Cookie is not an Open Bearer token. RCH must not synthesize or exchange one. |
| Torrent upload | Historical `115wangpan` uploads the actual `.torrent` as multipart fields `Filename`, `target=U_1_<folder cid>`, `Filedata`, and `Upload`; response `file_id` is followed by file metadata lookup for `pickcode` and whole-file `sha`. | Open upload uses temporary upload credentials, `/open/upload/init`, optional token/signature step, object-storage transfer and callback. `OpenList` computes full-file SHA-1 and prefix SHA-1 for initialization. | These are distinct upload/auth flows. Do not transplant Open fields or credential handling into the Web client. |
| Parse Torrent | Historical route `/?ct=lixian&ac=torrent` receives `pickcode`, whole-file `sha`, `uid`, `sign`, `time`; response includes `info_hash` and `torrent_filelist_web`. `p115client.clouddownload_torrent` is a newer, different route/parameter model using `sha1`. | `POST /open/offline/torrent` uses `torrent_sha1` and `pick_code`; SDK response model includes `info_hash` and file list. | Parameter spelling and endpoint families differ. The mock verifies only the historical Web model. |
| BT submission | Historical `115wangpan`: `/?ct=lixian&ac=add_task_bt` clear form with `info_hash`, zero-based comma-separated `wanted`, `savepath`, `uid`, `sign`, `time`; this version does not send `wp_path_id`. Current p115client logical payload includes `info_hash`, `wanted`, `savepath`, `wp_path_id`, and its default `ssp` wrapper serializes/encrypts it into `data`. | `POST /open/offline/add_task_bt` uses `info_hash`, `wanted`, `save_path`, `torrent_sha1`, `pick_code`, `wp_path_id`. | `savepath`/`save_path` and inclusion of `wp_path_id` are not interchangeable across versions. Never try multiple variants against a live account. |
| Task list | p115client exposes a Web `GET ...?ac=task_lists` shape with paging/status parameters. Historical `115wangpan` uses a signed `POST` task-list route. | `GET /open/offline/get_task_list`. | Method and response shape vary. No actual Web response was obtained. |

Sources: [p115client client.py](https://github.com/ChenyangGao/p115client/blob/main/p115client/client.py), [pending offline.py](https://github.com/ChenyangGao/p115client/blob/main/modules/%40p115/p115/%40pending/offline.py), [p115client tool/upload.py](https://github.com/ChenyangGao/p115client/blob/main/p115client/tool/upload.py), [OpenListTeam 115 SDK offline.go](https://github.com/OpenListTeam/115-sdk-go/blob/main/offline.go), [OpenListTeam 115 SDK upload.go](https://github.com/OpenListTeam/115-sdk-go/blob/main/upload.go), [OpenListTeam 115 SDK const.go](https://github.com/OpenListTeam/115-sdk-go/blob/main/const.go), [OpenList 115 driver](https://github.com/OpenListTeam/OpenList/blob/main/drivers/115/driver.go), [OpenList 115 Open driver](https://github.com/OpenListTeam/OpenList/blob/main/drivers/115_open/driver.go), [OpenList Open upload](https://github.com/OpenListTeam/OpenList/blob/main/drivers/115_open/upload.go), [OpenList Open offline client](https://github.com/OpenListTeam/OpenList/blob/main/internal/offline_download/115_open/client.go), [115driver offline module](https://github.com/SheltonZhu/115driver/blob/main/pkg/driver/offline.go), [115wangpan historical API](https://github.com/shichao-an/115wangpan/blob/master/u115/api.py), [Offline-115](https://github.com/owtotwo/Offline-115/blob/master/offline_115.py).

### Torrent hashes and existing-file paths

- `info_hash` is SHA-1 over the raw bencoded `info` dictionary for a v1 torrent. Torrent-file SHA-1 is over the complete `.torrent` bytes. They identify different objects and are not substitutes.
- The historical upload path receives `file_id`, then looks up `pickcode` and full torrent `sha`, then parses to obtain `info_hash` and file indexes.
- p115client's generic upload path can return `reuse` from initialization and skip object transfer. That is an upload-layer fast path; it does not establish that a matching offline task exists or that the returned object can be safely used for BT parsing.
- If the exact remote Torrent object already exists and a trusted `pickcode` plus matching whole-file SHA-1 are already known, a parse-only path may be possible. No safe, authorized Web lookup path for discovering an arbitrary existing Torrent by hash was verified here.
- Before any submit, an existing remote task must be matched by `info_hash` and account/task context. A duplicate task is separate from upload-object reuse.

### Destination IDs and folder lookup

- Open `wp_path_id` is the destination folder ID. Open `save_path` is a relative subpath below it according to the SDK model.
- Cookie implementations disagree: the historical clear-form flow only uses `savepath`; current p115client's logical payload also carries `wp_path_id`. Do not infer that a folder name can be supplied in either field.
- A future authorized read-only resolution should reuse the RCH source's configured root and existing session; list one directory at a time, match exactly one child by name and directory type, then continue to the next component. Stop on missing or ambiguous names. Do not recurse or create folders.
- No `/漫画/EH订阅` response or actual target folder ID was obtained in this investigation.

### GET endpoints and possible side effects

- p115client documents a legacy `clouddownload_get_id` GET that may create Torrent/task folders when absent. It is not a safe read-only probe; do not call it to “check” a directory.
- RCH's existing Web file listing sends `record_open_time=1`; this appears to update access/open-time metadata (inference from the parameter), even though it is part of normal reading.
- No source inspected here proves that sign lookup, Torrent parse, or task-list reads are mutation-free under every server version. Historical `115wangpan`'s task-list call is POST, so its comment about a default download folder must not be misreported as a GET side effect.
- Treat undocumented endpoints as unverified. Do not use GET as a write-probe or call an endpoint merely because it is named “get”.

## Minimum RCH session reuse design

RCH's existing Cookie login path stores a shared `Arc<Cloud115WebClient>` in its Cookie-session registry and `cloud115_cookie_list` reuses that client. A future adapter, if separately authorized, should be exposed as a method on that existing session/client and receive a session/source identifier—not a Cookie value. This preserves the existing reqwest client, Cookie lifecycle, User-Agent, request gate, and WAF cooldown. Do not instantiate a second client, duplicate Cookie storage, or add an independent QR/login/token-refresh flow. The test adapter in this change is not wired into production or the reader chain.

## Hidden-write and account risk

Cookie-mode cloud-download routes are community-documented legacy Web interfaces; the sources themselves do not grant permission. Uploading a Torrent and adding a BT task mutate the user's cloud account. Using the account Cookie with an undocumented route could expose a broad session credential and trigger account restrictions/WAF controls. A 405/challenge must stop the flow; do not rotate accounts, hosts, proxies, or fingerprints. The official platform agreement points to published interfaces and granted scopes, but no evidence found here confirms Web Cookie writes are within an approved RCH use.

## Offline-only Rust model

Added test-only files:

- `app/rust/tests/cloud115_web_cookie_protocol.rs`
- `app/rust/tests/support/cloud115_web_cookie_protocol.rs`

The helper rejects non-loopback origins, disables proxy discovery and redirects, uses only a fixed fake Cookie, and never enters the app runtime. Its clear-form protocol is a historical model, not the current p115client encrypted `ssp` wire format. Tests cover Cookie/User-Agent and sign/time fields, multipart upload and response IDs, SHA-1 vs infohash, parse results, wanted indexes, `savepath` vs absent historical `wp_path_id`, duplicate task precheck, upload timeout/unknown state with no second upload, lost BT response reconciliation without a second submit, WAF 405 stop, and non-loopback rejection.

Command: `cargo test --locked --offline --test cloud115_web_cookie_protocol` (from `app/rust`). Result: 7 passed, 0 failed, 0 ignored. The Rust library emitted the existing Windows linker informational warning. These are Mock-only results, not live API tests.

Upload timeout remains `UNKNOWN_REMOTE_STATE`: because no authorized, verified Web lookup contract was established, the adapter deliberately does not retry or pretend it can reconcile the object. A human/officially supported read path would need to establish whether the Torrent exists before any further action.

## Semi-automatic option while Web writes are unauthorized

Keep all network writes out of RCH. RCH may prepare a local review package containing the Torrent's parsed file list, whole-file SHA-1, infohash, wanted indexes, and intended destination; the user inspects it and proceeds only through a 115 interface and operation that 115 officially makes available to that account. If no supported manual BT operation is available, stop at local preparation. Do not automate browser clicks, call the legacy POST, or treat a user Cookie as authorization for an unpublished API.

## Stop point

- Existing Cookie session was not read, refreshed, or exercised by this research.
- No account, folder, task, quota, or authenticated read-only response was obtained; the existing RCH `需连接` label is a known status-display issue and is not evidence of auth failure or success.
- No `.torrent` file was read in this turn. No real upload/BT write occurred.
- The earlier 12 parallel Rust test failures remain `BASELINE_UNRESOLVED`; this focused suite does not resolve them. No unrelated test or reader-chain fixes were made.
- P0 web contract comparison and offline mock work is complete; authorization is still blocked. Keep `P0_WEB_WRITE_AUTH_BLOCKED`, `P0_WEB_READONLY_PASS=NO`, `P0_REAL_PASS=NO`, `P1=NOT_STARTED`.

## R3 protocol freeze — 2026-10-11

### Classification and current SSP comparison

`app/rust/tests/cloud115_web_cookie_protocol.rs` and its support module are explicitly classified as `LEGACY_WEB_PROTOCOL_MODEL`. They model the historical `shichao-an/115wangpan` clear-form flow, including its legacy `sign`/`time` fields. They do **not** simulate the current p115client SSP request and must not be used as a production implementation or compatibility claim.

The pinned p115client source shows a mixed protocol rather than one unified SSP flow:

| Operation | Current p115client source behavior | Difference from the legacy model | Evidence status |
|---|---|---|---|
| BT add | `clouddownload_task_add_bt` defaults to `type="ssp"`; wrapper posts to `https://clouddownload.115.com/lixianssp/`, adds `ac="add_task_bt"` and `app_ver`, stringifies logical values, serializes JSON, RSA-encrypts it, then sends a form whose `data` field contains the encrypted ASCII payload. The wrapper explicitly sets `ecdh_encrypt=False`. | Not a clear form of `info_hash`, `wanted`, `savepath`, `wp_path_id`, `uid`, `sign`, and `time`. SSP source does not add legacy `uid/sign/time`. | Third-party implementation evidence only; no live request/response vector. |
| SSP response | Attempts JSON decoding, then attempts RSA decryption/JSON decoding of a response `data` field; if that fails, the source retains the undecoded value. | The legacy mock handles ordinary clear JSON envelopes and does not exercise SSP decryption. | Third-party implementation evidence only. |
| Torrent parse | `clouddownload_torrent` defaults to `type="web"` and uses the Web request wrapper with logical `sha1` and `ac="torrent"`. | Not the legacy `ct=lixian&ac=torrent` clear form with `pickcode`, `sha`, `uid`, `sign`, and `time`. | Third-party implementation evidence only; no live response. |
| Task list | `clouddownload_task_list` defaults to `type="web"`, with paging/status parameters and `ac="task_lists"`. Its Web wrapper constructs a `/web/` path. | Not the legacy signed POST task-list shape in `115wangpan`. The current source's method documentation and constructed `/web/` URL are not fully consistent. | Third-party implementation evidence only; no live response. |
| Sign lookup | `clouddownload_sign` uses `https://115.com/?ct=clouddownload&ac=space`. | The old model uses `/?ct=offline&ac=space&_=...`. | Third-party implementation evidence only; no live response. |
| Torrent upload | Current p115client's generic upload helper uses the selected client's upload initialization/token flow and storage callback. For its Cookie-client branch it can construct a `P115Client` from a Cookie header. | Not the old direct multipart `Filename/target/Filedata/Upload` model. RCH must not copy the helper's second-client behavior. | Third-party implementation evidence only; no live response. |

At the logical BT layer, current p115client documents `info_hash`, zero-based comma-separated `wanted`, `savepath` (a relative path under the destination), and `wp_path_id`. This is distinct from Open API naming (`save_path`, plus `torrent_sha1` and `pick_code` in its BT-add model). The complete Torrent file SHA-1 is also distinct from the SHA-1 infohash of the bencoded v1 `info` dictionary. Source-level field names do not establish what a current server accepts.

### Field verification matrix

| Field / behavior | Source-level status | Current live SSP status |
|---|---|---|
| `ac=add_task_bt`, `app_ver`, outer `data` | Confirmed in pinned p115client SSP wrapper. | Unverified. |
| RSA-encrypted JSON payload; `ecdh_encrypt=False` | Confirmed in pinned p115client implementation. | Unverified, including key/version compatibility. |
| Logical `info_hash`, `wanted`, `savepath`, `wp_path_id` | Confirmed in p115client method model/docs. | Unverified. |
| Legacy `uid`, `sign`, `time` | Present in the historical clear-form model. | Not evidence of current SSP fields; unverified for current Web parse/list endpoints. |
| Upload `pickcode` / `pick_code`, whole-file `sha` / `torrent_sha1` | Names differ by historical Web/Open API flow and SDK model. | No current SSP upload-to-parse wire sequence verified. |
| Response encryption/decryption and business result shape | Decryption attempt is present in the pinned p115client response path. | Unverified. |
| Duplicate-task behavior, task terminal states, server-side wanted semantics | Third-party source/model descriptions only. | Unverified. |

Therefore the correct classification is `SSP_WIRE_UNVERIFIED`. No trustworthy current SSP request/response test vector, real account response, or official endpoint-level permission evidence was obtained. The p115client package declares `p115cipher >= 0.0.6` rather than pinning one exact cipher implementation in the inspected project metadata, which is another reason not to assert byte-for-byte reproducibility from this evidence alone.

### Frozen source revisions

The following repository HEADs were captured by read-only `git ls-remote` on 2026-10-11. These immutable commits, rather than floating `main`/`master` links, are the evidence baseline for the comparisons above.

| Repository | Frozen commit | Relevant source |
|---|---|---|
| `ChenyangGao/p115client` | `856e4f8f09ce034534bd458f06d3dee4474a1698` | [client.py](https://github.com/ChenyangGao/p115client/blob/856e4f8f09ce034534bd458f06d3dee4474a1698/p115client/client.py), [tool/upload.py](https://github.com/ChenyangGao/p115client/blob/856e4f8f09ce034534bd458f06d3dee4474a1698/p115client/tool/upload.py), [pending offline.py](https://github.com/ChenyangGao/p115client/blob/856e4f8f09ce034534bd458f06d3dee4474a1698/modules/%40p115/p115/%40pending/offline.py) |
| `OpenListTeam/OpenList` | `482464c2affc5eeba5d4a25bb272aeb352ec5918` | [115 driver](https://github.com/OpenListTeam/OpenList/blob/482464c2affc5eeba5d4a25bb272aeb352ec5918/drivers/115/driver.go), [115_open driver](https://github.com/OpenListTeam/OpenList/blob/482464c2affc5eeba5d4a25bb272aeb352ec5918/drivers/115_open/driver.go), [Open upload](https://github.com/OpenListTeam/OpenList/blob/482464c2affc5eeba5d4a25bb272aeb352ec5918/drivers/115_open/upload.go) |
| `OpenListTeam/115-sdk-go` | `788107ec0545cf126ee9b22329678679cc6b3b2a` | [offline.go](https://github.com/OpenListTeam/115-sdk-go/blob/788107ec0545cf126ee9b22329678679cc6b3b2a/offline.go), [const.go](https://github.com/OpenListTeam/115-sdk-go/blob/788107ec0545cf126ee9b22329678679cc6b3b2a/const.go) |
| `SheltonZhu/115driver` | `542720cb0034954750454e89f32b4852add6e2a9` | [offline.go](https://github.com/SheltonZhu/115driver/blob/542720cb0034954750454e89f32b4852add6e2a9/pkg/driver/offline.go) |
| `shichao-an/115wangpan` | `e7cc935313f675e886bceca831fcffcdedf1e880` | [api.py](https://github.com/shichao-an/115wangpan/blob/e7cc935313f675e886bceca831fcffcdedf1e880/u115/api.py) |
| `owtotwo/Offline-115` | `780fd3d0792fb172af5e71d8de8514f90bb9bfca` | [offline_115.py](https://github.com/owtotwo/Offline-115/blob/780fd3d0792fb172af5e71d8de8514f90bb9bfca/offline_115.py) |

No upstream source was copied into RCH. Third-party code establishes neither official authorization nor server behavior. The official platform material reviewed did not grant permission for RCH to use Web Cookie BT writes; those writes remain blocked.

### Mock disposition

The existing seven loopback tests remain historical-contract tests only. No generalized Mock surface was added in R3, and no test was rerun as part of this documentation-only freeze. Do not cite those seven passes as SSP, Web production, or account compatibility evidence.
