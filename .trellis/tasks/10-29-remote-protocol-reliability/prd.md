# Remote protocol reliability: SMB and SFTP

## Purpose

Fix the previously verified gaps in SMB and SFTP source support so users can securely connect, browse, and read comics on Android and Windows.

## Requirements

1. SMB sources use an authenticated SMB2/3 client session rather than UNC filesystem calls.
2. The SMB source form exposes server, port, share/initial directory, username, and password; credentials may be blank for Guest.
3. Existing UNC-based SMB records remain readable and are converted at the repository boundary without losing the share or subdirectory.
4. SMB browsing, indexing, cover scanning, and comic reading use the remote-source path and random-access reads.
5. SFTP verifies and pins server host keys before any password or private-key authentication.
6. SFTP supports password and imported OpenSSH/PEM private-key authentication; private key material stays in secure storage.
7. Existing SFTP records with no pin require explicit foreground trust once. Background work must never auto-trust or show a trust modal.
8. Native platform secrets use the secure vault, including migration of existing persisted source/sync credentials with verify-before-redact behavior.
9. Secret values stay out of native JSON backup, database secret columns, and diagnostic logs.
10. Local search and source-tree availability recognize vault-backed credentials and the selected SFTP auth mode after SQLite secret columns are redacted.
11. Editing a remote source evicts the shared browser/scan cache and its provider session; SMB eviction releases the native Rust session.
12. SMB configured roots are interpreted as share-relative paths exactly once, and normalizing an extended UNC record preserves its existing source fingerprint.

## Constraints

- Keep the existing Android Keystore channel.
- Support SMB2/3 only; do not add SMB1, share discovery, or SMB writes.
- Preserve the existing SFTP raw-cache identity and password authentication behavior after host verification.
- Do not change OpenList or its server configuration.

## Acceptance criteria

- On MuMuPlayer 15 and Windows, a real temporary SMB2/3 share accepts valid credentials, rejects invalid credentials, lists directories, and reads an archive at head/middle/tail offsets.
- SMB test paths never reach local filesystem listing or open APIs.
- A new SFTP host displays its SHA-256 fingerprint before login; cancel sends no credentials.
- A changed SFTP host key blocks login until the user explicitly replaces the pin.
- Password and private-key login both work against a temporary SFTP server; wrong password/key/passphrase fail.
- Android Keystore and Windows secure-storage round trips pass; vault failure preserves old persisted values.
- Search results backed by vault-only credentials remain openable after restart; extended UNC source IDs remain stable after normalization.
- SMB roots whose first subdirectory has the same name as the share resolve correctly, and source edits do not reuse stale sessions.
- Flutter Analyze/tests, serial Rust tests, Android build, and Windows build pass.

