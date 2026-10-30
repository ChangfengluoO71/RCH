# Authenticated SMB Source Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Replace SMB UNC pass-through with a real SMB2/3 session that can browse and randomly read comics on Android and Windows.

**Architecture:** A Rust SmbClient session owns one configured share and exposes directory listing, file size, and positioned reads. Flutter treats SMB as a remote session source and routes catalog, covers, and reader access through the shared remote-source flow.

**Tech Stack:** Rust, smb2 crate, Tokio, flutter_rust_bridge, Flutter/Dart.

**Spec:** docs/superpowers/specs/2026-10-29-remote-source-protocols.md, section “SMB”.

## Global Constraints

- No SMB1, C libraries, FFI, or SMB path passed to local filesystem APIs.
- Keep SMB source credentials in CredentialVault on native platforms.
- Legacy UNC source records remain parseable; missing legacy credentials may use Guest or be supplied by the user.
- Network reads must satisfy the existing synchronous ByteSource interface through the per-session runtime boundary.

## Review Focus

- Wrong password and server unreachable are distinguishable; neither logs credentials.
- Empty username/password attempts Guest and does not become an empty-password Windows filesystem access.
- Legacy UNC values normalize without losing spaces, Unicode, or nested directories.
- SMB reads at offset zero, a middle offset, EOF, and beyond EOF return exact expected bytes/length.
- SMB is treated as remote by indexing, cover scan, source browsing, cache cleanup, and book opening.

---

### Task 1: SMB source configuration and legacy-path parsing

**Files:**
- Create: app/lib/store/smb_source_config.dart
- Modify: app/lib/store/models.dart
- Modify: app/lib/repository/book_repository.dart
- Test: app/test/smb_source_config_test.dart
- Test: app/test/book_repository_smb_migration_test.dart

**Interfaces:**
- Produces: SmbSourceConfig.fromSource(BookSource) returns server, port, share, and source-relative root path; parseLegacyUnc(String) returns the same validated shape or a typed parse failure.

- [x] Add failing tests for UNC host/share/path parsing, slash normalization, missing share rejection, and old BookSource serialization round-trip.
- [x] Run the focused test and verify it fails before implementation.
- [x] Implement one shared config parser; set canonical new sources to url=server, port=445 by default, and path=share/relative/path.
- [x] Migrate old SMB UNC records at the repository boundary; preserve the share and nested relative root.
- [x] Run focused Dart tests and verify legacy source JSON still loads.

Run: cd app; flutter test test/smb_source_config_test.dart test/book_repository_smb_migration_test.dart
Expected: PASS.

---

### Task 2: Rust SMB session, listing, metadata, and ByteSource

**Files:**
- Modify: app/rust/Cargo.toml
- Modify: app/rust/Cargo.lock
- Create: app/rust/src/source/smb.rs
- Modify: app/rust/src/source/mod.rs
- Modify: app/rust/src/api/source.rs
- Test: app/rust/src/source/smb.rs
- Test: temporary live SMB integration fixture

**Interfaces:**
- SmbClient::connect(host, port, share, username, password) -> Result<SmbClient>
- SmbClient::list(path) -> Result<Vec<Entry>>
- SmbClient::file_size(path) -> Result<u64>
- SmbClient::read_at(path, offset, buf) -> io::Result<usize>
- SmbFile implements ByteSource; FRB exposes smb_connect, smb_disconnect, smb_list, and open_smb_book.

- [ ] Add parser/unit tests for share-relative paths and tests for short reads, EOF, and out-of-range offsets using the live fixture contract.
- [ ] Confirm the tests fail before the SMB adapter is added.
- [ ] Add smb2 and implement one SMB2/3 session per source, share-scoped paths, stable cache namespace, directory sorting, file-size lookup, and positioned reads.
- [ ] Map authentication/protocol failures to sanitized user-facing errors; keep protocol details out of secrets/logs.
- [ ] Add tests for a correct credential, invalid credential, directory listing, and random reads against a temporary read-only share.
- [ ] Run focused Rust tests and confirm SMB1 is never negotiated by configuration.

Run: cd app/rust; cargo test --locked source::smb -- --test-threads=1
Expected: PASS; integration fixture verifies credentials and random reads against an actual SMB2/3 server.

---

### Task 3: Route SMB through remote browser, catalog, scans, and reader

**Files:**
- Modify: app/lib/store/models.dart
- Create: app/lib/store/smb_session.dart
- Modify: app/lib/ui/home_page.dart
- Modify: app/lib/ui/source_browser.dart
- Modify: app/lib/store/book_open_coordinator.dart
- Modify: app/lib/store/library_index_service.dart
- Modify: app/lib/store/remote_listing.dart
- Modify: app/lib/ui/source_tree.dart
- Modify: app/lib/repository/record_repository.dart
- Modify: app/lib/store/sync_engine.dart
- Modify: app/rust/src/api/remote_scan.rs
- Modify: app/rust/src/api/library.rs
- Modify: app/rust/src/db/mod.rs
- Test: SMB session, source browser, source model, catalog, and open-coordinator tests

**Interfaces:**
- Consumes: SmbSourceConfig and Rust smb_* APIs from Tasks 1–2.
- Produces: needsSession includes SMB; isLocalFs is true only for local; cachedRemoteSession and remote scan adapter support SMB.

- [ ] Add failing dispatch tests proving SMB currently falls through to listLocalDir/openLocalBook.
- [ ] Implement SMB credentials/host/port/share fields in the add-source form and verify the share root before saving.
- [ ] Add Flutter SMB session caching/disconnect handling consistent with SFTP/WebDAV.
- [ ] Switch browser, catalog snapshot, remote scan, book open, and remote cache paths to the SMB API; keep local filesystem checks local-only.
- [ ] Search all SMB type branches and test that batch tagging, read records, source tree, and sync behavior no longer classify SMB paths as host-local.
- [ ] Generate flutter_rust_bridge bindings using app/codegen.ps1 and run focused Flutter tests.

Run: cd app; flutter test test/store/smb_session_test.dart test/ui/source_browser_test.dart
Expected: PASS; dispatch tests prove no SMB path reaches listLocalDir or openLocalBook.

---

### Task 4: Cross-platform build and live smoke

**Files:**
- Modify: no additional files unless verification finds a defect.

- [ ] Run Flutter Analyze and full Flutter tests.
- [ ] Run Rust check with RUSTFLAGS=-D warnings and serial Cargo tests.
- [ ] Build the Android APK and Windows application.
- [ ] On MuMuPlayer 15, connect to the temporary Windows SMB share using valid credentials, list comic folders, open a sample comic, and verify a nonzero-offset page read.
- [ ] Verify invalid credentials fail cleanly and the test share is removed after the run.

Run: cd app/rust; cargo test --locked -- --test-threads=1
Expected: PASS.


