# Cross-Platform Credential Vault Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Store SMB/SFTP secrets through a platform secure vault on every native target and migrate existing source/sync secrets without losing values.

**Architecture:** Keep the current Android Keystore channel, add the plugin-backed adapter for other native targets, and make the repository persist only vault references and non-secret source fields. Migration writes and verifies the vault bundle before redacting the old row.

**Tech Stack:** Flutter/Dart, existing Android Kotlin Keystore channel, flutter_secure_storage.

**Spec:** docs/superpowers/specs/2026-10-29-remote-source-protocols.md, section “Cross-platform credentials”.

## Global Constraints

- Android keeps using the existing rch/credentials Keystore-backed MethodChannel.
- All native platform secret writes require vault read-back verification; a vault failure must not trigger plaintext fallback.
- Native library.json backups exclude all secret values; web remains on its current path.
- Follow .trellis/spec/frontend and the shared code-reuse/cross-layer guides.

## Review Focus

- A legacy source row has a secret but no credentialRef: migrate it once, verify, and redact only after success.
- A vault read/write failure occurs during migration: keep old DB value and in-memory credential.
- A stored secret is intentionally cleared: do not restore the stale vault value on next load.
- A library.json backup is written on Windows: it must omit secrets just as Android does.
- Web builds must not require native storage plugins at runtime.

---

### Task 1: Platform credential adapter and repository migration

**Files:**
- Modify: app/pubspec.yaml
- Modify: app/lib/store/credential_vault.dart
- Modify: app/lib/repository/book_repository.dart
- Modify: app/lib/store/sync_manager.dart
- Modify: app/lib/store/library_store.dart
- Test: app/test/store/credential_vault_test.dart
- Test: app/test/repository/book_repository_test.dart
- Test: app/test/store/sync_manager_test.dart

**Interfaces:**
- Consumes: CredentialVault.put/get/delete and existing credentialRef fields.
- Produces: platformCredentialVault() routes Android to MethodChannelCredentialVault and other native targets to FlutterSecureStorageCredentialVault; BookRepository and SyncManager use it on every native target.

- [x] Write tests for platform adapter routing, verified legacy source migration, DB redaction after verified vault write, failure preservation, and native JSON backup redaction.
- [x] Run focused Flutter tests and confirm the new cases fail against the Android-only vault gate.
- [x] Add flutter_secure_storage with a version compatible with app SDK >=3.12.2; implement the non-Android adapter and make native repository/sync paths use the vault.
- [x] Keep existing secret-bundle encoding generic; the SFTP auth task adds the future private-key/passphrase fields to that bundle and redaction path.
- [x] Run focused Flutter tests, then full Flutter Analyze and Flutter tests.

Run: cd app; flutter test test/credential_vault_test.dart test/book_repository_credential_vault_test.dart test/sync_manager_credential_vault_test.dart
Expected: PASS, including a simulated secure-storage error that leaves legacy DB credentials intact.

---

### Task 2: Parent integration and regression pass

**Files:**
- Modify: all SMB/SFTP source dispatch sites identified by rg search
- Test: existing source/model/browser/session tests plus protocol integration tests

**Interfaces:**
- Consumes: secure vault adapter and the source/session APIs from both child plans.
- Produces: one application path for source creation, stored credentials, session setup, remote listing, and comic reading.

- [ ] Add or update tests proving source credentials round-trip through model → repository → vault → session setup.
- [ ] Search all local/smb and SFTP connect call sites; remove stale local-filesystem assumptions and reject any missing trust pin in background paths.
- [ ] Run Flutter Analyze, Flutter tests, Rust warnings-as-errors check, and serial Rust tests.
- [ ] Build Android and Windows; test actual SMB and SFTP sessions in MuMuPlayer 15 and against the temporary Windows/SFTP fixtures.

Run: cd app/rust; RUSTFLAGS="-D warnings" cargo check --locked --all-targets
Expected: PASS with no warnings.

Run: cd app/rust; cargo test --locked -- --test-threads=1
Expected: PASS.


