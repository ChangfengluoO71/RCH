# SFTP Host Verification and Key Authentication Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Stop accepting arbitrary SFTP host keys and add protected private-key authentication without breaking existing password-authenticated sources.

**Architecture:** Probe the SSH host key before login, require a foreground user decision for unknown keys, and pass an expected fingerprint to the actual SFTP connection. Store the pin as non-secret source metadata and store private key material in CredentialVault.

**Tech Stack:** Rust, russh 0.62.5, russh-sftp 2.3, flutter_rust_bridge, Flutter/Dart, file_selector.

**Spec:** docs/superpowers/specs/2026-10-29-remote-source-protocols.md, section “SFTP”.

## Global Constraints

- SFTP login credentials are never sent before the host key matches an accepted pin.
- Unknown or changed host keys fail closed; background workers never prompt or trust automatically.
- Private key/passphrase values are never logged, placed in SQLite secret columns, or included in native JSON backups.
- Preserve current SFTP endpoint and raw-cache identity rules.

## Review Focus

- New source host key confirmation is cancelled: no authenticated session and no persisted trust.
- Existing source has no fingerprint: foreground can confirm once; background reports a recoverable untrusted-host state.
- Host key changes after first connection: no password or key authentication is attempted until explicit replacement.
- OpenSSH/PEM key has no passphrase, correct passphrase, or incorrect passphrase.
- Endpoint changes cannot silently reuse a pin for the prior endpoint.

---

### Task 1: Fingerprint probe, pin storage, and trust UI

**Files:**
- Modify: app/lib/store/models.dart
- Modify: app/rust/src/source/sftp.rs
- Modify: app/rust/src/api/source.rs
- Modify: app/rust/src/db/mod.rs
- Modify: app/rust/src/api/db.rs
- Modify: app/lib/repository/book_repository.dart
- Modify: app/lib/store/sftp_session.dart
- Modify: app/lib/ui/home_page.dart
- Modify: app/lib/ui/source_browser.dart
- Modify: app/lib/store/book_open_coordinator.dart
- Test: SFTP host-key verifier and source persistence/widget tests

**Interfaces:**
- SftpClient::probe_host_key(host, port) -> Result<String>
- SftpClient::connect(host, port, username, auth, expected_fingerprint) rejects missing/mismatched trust.
- BookSource stores hostKeyFingerprint and hostKeyEndpoint as non-secret settings.

- [ ] Add failing unit tests for unknown key, matching pin, changed key, and endpoint mismatch.
- [ ] Add database migration and DTO round-trip tests for host_key_fingerprint and host_key_endpoint.
- [ ] Implement a no-login probe and strict server-key comparison; remove the current unconditional Ok(true).
- [ ] Add first-use confirmation and explicit changed-key replacement UX; persist only after user acceptance.
- [ ] Ensure foreground SFTP actions can request trust while background scans fail without showing dialogs.
- [ ] Run Rust and Flutter focused tests; regenerate FRB bindings with app/codegen.ps1.

Run: cd app/rust; cargo test --locked source::sftp -- --test-threads=1
Expected: PASS; changed-key tests assert no auth call is made.

---

### Task 2: Password/private-key auth and secure import

**Files:**
- Modify: app/pubspec.yaml
- Modify: app/lib/store/models.dart
- Modify: app/lib/repository/book_repository.dart
- Modify: app/lib/ui/home_page.dart
- Modify: app/lib/store/sftp_session.dart
- Modify: app/rust/src/source/sftp.rs
- Modify: app/rust/src/api/source.rs
- Test: SFTP auth widget, vault, and live server tests

**Interfaces:**
- SftpAuth is Password(String) or PrivateKey { private_key_pem: String, passphrase: Option<String> }.
- SftpClient decodes supported OpenSSH/PEM private keys with russh and calls public-key authentication.
- BookSource stores auth mode as non-secret setting; private key and passphrase are CredentialVault bundle entries.

- [ ] Add failing tests for password auth, a generated Ed25519 private key, encrypted-key correct/incorrect passphrase, and vault round-trip/redaction.
- [ ] Add file_selector using a version compatible with app SDK >=3.12.2; import file content via XFile.readAsBytes and retain no external path.
- [ ] Add an explicit password/private-key selector; require username and one valid auth method before connection.
- [ ] Implement russh public-key auth with supported key decoding; retain password auth.
- [ ] Store secret fields through the native vault adapter; remove private key and passphrase from local JSON backup.
- [ ] Run focused widget and protocol tests against a temporary SFTP server configured for both auth methods.

Run: cd app; flutter test test/ui/sftp_source_dialog_test.dart test/repository/book_repository_test.dart
Expected: PASS; key bytes and passphrase appear only in the in-memory source/vault test fixture, never serialized JSON.

---

### Task 3: Rotation, legacy migration, and release verification

**Files:**
- Modify: app/lib/store/sftp_session.dart
- Modify: app/lib/ui/source_browser.dart
- Modify: app/lib/store/remote_scan_coordinator.dart
- Test: host-key rotation and legacy source tests

- [ ] Verify old password-only sources with no host pin prompt once on foreground access and migrate only after acceptance.
- [ ] Rotate the temporary SFTP server host key; confirm old pin blocks login and explicit replacement permits the next attempt.
- [ ] Confirm a background scan of an unpinned source does not prompt and does not accept the key.
- [ ] Run Flutter Analyze, Flutter tests, Rust warnings-as-errors check, serial Rust tests, Android build, and Windows build.
- [ ] Verify test credentials and temporary server/key files are removed after the live test.

Run: cd app/rust; cargo test --locked -- --test-threads=1
Expected: PASS.


