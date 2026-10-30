# Remote Source Protocols: SMB, SFTP, and Credential Storage

## 1. Scope / Trigger

Use this contract when changing SMB/SFTP source configuration, connection
sessions, remote browsing or reading, host-key trust, or credential storage.
These paths cross Flutter UI, repository persistence, FRB APIs, Rust protocol
clients, SQLite migrations, and native secure storage.

## 2. Signatures

The Rust FRB boundary owns connection lifecycle and remote operations:

```rust
smb_connect(
    host: String, port: u16, share: String, root_path: String,
    username: String, password: String,
) -> Result<SmbSessionInfo>
smb_disconnect(id: u64)
smb_list(session: u64, path: String) -> Result<Vec<DirEntry>>
open_smb_book(session: u64, path: String, strategy: String) -> Result<BookInfo>

sftp_probe_host_key(host: String, port: u16) -> Result<String>
sftp_connect(
    host: String, port: u16, username: String, auth_method: String,
    password: String, private_key: String, key_passphrase: String,
    expected_fingerprint: String,
) -> Result<SftpSessionInfo>
sftp_disconnect(id: u64)
sftp_list(session: u64, path: String) -> Result<Vec<DirEntry>>
open_sftp_book(session: u64, path: String, strategy: String) -> Result<BookInfo>
```

`CredentialVault` exposes `put(key, value)`, `get(key)`, and `delete(key)`.
`BookSource.credentialRef` is the persisted reference; secrets are loaded only
for an operation. The SFTP database fields `host_key_fingerprint`,
`host_key_endpoint`, and `sftp_auth_method` are non-secret metadata.

## 3. Contracts

- SMB represents one saved source as host (`url`), port (default `445`), share,
  share-relative root, username, and vault-backed password. Empty username and
  password request Guest access. A username without a password is allowed only
  when the server supports it; a password without a username is invalid.
- SMB supports authenticated SMB2/3 read-only sessions. The same remote-source
  path must handle directory listing, indexing, covers, cache downloads, and
  positioned reads. SMB paths never go to local filesystem listing/open APIs.
- Legacy UNC records are parsed at the repository boundary into host, share,
  and share-relative root without changing their stable source/cache identity.
  Extended `\\?\UNC\host\share\...` records must retain their existing
  fingerprint when normalized, including the original host spelling needed to
  reproduce that legacy fingerprint.
  At the share root, pass the empty path to the SMB server; do not substitute
  `.` because some servers treat it as a literal child path.
- The `smb_connect.root_path` argument is already relative to the mounted
  share. Normalize it as such and do not strip a first path segment merely
  because its name equals the share. Canonical logical paths such as
  `share/folder` are translated to provider paths only at the listing/file
  operation boundary.
- Source availability for search and source-tree results must recognize
  vault-backed credentials after secret columns have been redacted. SFTP
  availability uses the selected `sftp_auth_method` and its credential
  reference; a locally configured authenticated source remains openable from
  search results.
- Editing a remote source evicts both its provider-owned session and the
  shared browser/scan session cache. Evicting SMB also calls
  `smb_disconnect` to release the native session and Tokio runtime.
- SFTP probes the endpoint without user authentication and displays the
  SHA-256 fingerprint before accepting a new host. Persist trust only after
  explicit user acceptance. A pin is bound to the exact configured `host:port`
  endpoint and is checked by the authenticated connection before any password,
  private key, or passphrase is used.
- An unknown or changed SFTP key fails closed. Background jobs do not display
  prompts and do not trust keys. Explicit replacement of a changed key is a
  foreground action and updates the endpoint-bound pin only after acceptance.
- `auth_method` is exactly `password` or `privateKey`. The backend validates
  and uses only the selected credential; it must not silently fall back to the
  other method. Private-key bytes and key passphrases are vault secrets.
- New source and sync credentials are written to native secure storage. During
  migration, write and read back the complete secret bundle before clearing
  legacy database values. If verification fails, preserve the old values.
  Native JSON backups include credential references and non-secret settings,
  never secret values. Diagnostics must not echo credentials, private keys,
  passphrases, or raw provider errors that could contain them.
- Keep network operations off the UI thread. Synchronous positioned reads
  bridge to each protocol's private Tokio runtime through blocking workers.

Live protocol tests are opt-in and use these environment variables:

- SMB: `RCH_SMB_TEST_HOST`, `RCH_SMB_TEST_PORT`, `RCH_SMB_TEST_SHARE`,
  `RCH_SMB_TEST_USERNAME`, `RCH_SMB_TEST_PASSWORD`, `RCH_SMB_TEST_FILE`.
- SFTP: `RCH_SFTP_TEST_HOST`, `RCH_SFTP_TEST_PORT`,
  `RCH_SFTP_TEST_ROTATED_PORT`, `RCH_SFTP_TEST_USERNAME`,
  `RCH_SFTP_TEST_PASSWORD`, `RCH_SFTP_TEST_FINGERPRINT`,
  `RCH_SFTP_TEST_FILE`, `RCH_SFTP_TEST_AUTH_LOG_FILE`,
  `RCH_SFTP_TEST_PRIVATE_KEY_FILE`,
  `RCH_SFTP_TEST_ENCRYPTED_PRIVATE_KEY_FILE`,
  `RCH_SFTP_TEST_KEY_PASSPHRASE`.

## 4. Validation & Error Matrix

| Condition | Required behavior |
| --- | --- |
| SMB host, port, share, or root is invalid | Reject before listing; return a sanitized operation error. |
| SMB password is supplied without username | Reject locally; do not attempt authentication. |
| SMB credentials are wrong | Fail connection; never include the password in the error. |
| SMB source root is empty | List the share root with an empty server path. |
| SMB configured root begins with a directory named like its share | Keep that directory; the root argument is share-relative and is not stripped twice. |
| An extended UNC source is normalized during load/save | Preserve its existing source fingerprint and indexed identity. |
| Vault-backed source credentials have been redacted from SQLite | Source tree and search still classify the local source as network-readable when its credential reference and selected auth mode are present. |
| Remote connection settings are edited | Evict the shared browser cache and provider session; SMB releases its native connection. |
| SFTP host has no saved pin | Probe and request foreground confirmation; background connection fails recoverably. |
| SFTP pin belongs to a different endpoint | Do not reuse it; require confirmation for the new endpoint. |
| SFTP server key changed | Fail before password/public-key auth; require explicit foreground replacement. |
| Selected password or private key is absent/invalid | Fail using only that method; never fall back to the other method. |
| Credential vault write/read-back fails during migration | Keep legacy database secrets intact and report migration failure. |
| Native JSON backup is exported | Omit password, private key, passphrase, and other secret values. |

## 5. Good / Base / Bad Cases

- Good: a MuMu Android SMB source authenticates, lists a real test share,
  resolves a cover, and opens an archive through random reads; its password is
  absent from SQLite and JSON export.
- Good: a vault-backed SFTP/private-key or cloud-cookie source appears as
  openable in local search after restart, and opening it uses the selected
  authentication mode.
- Good: an extended UNC SMB source keeps its fingerprint after conversion to
  canonical host/share coordinates, and a root folder named like the share is
  still listed from the correct directory.
- Base: an existing password-only SFTP source is prompted once on foreground
  use, then the accepted fingerprint is saved with that endpoint.
- Bad: a saved SFTP pin changes after a server rotation and the client sends a
  password or private key before refusing the connection.
- Bad: the SMB share root is translated to `.` or passed to local filesystem
  APIs, making a working remote source appear empty or opening a local path.
- Bad: SMB root normalization removes a real first directory, a source edit
  reuses a stale remote session, or an SMB reconnect leaves the Rust session
  and runtime retained.
- Bad: public-key authentication failure silently retries with a saved
  password, or a failed vault migration clears the only copy of a credential.

## 6. Tests Required

- Unit tests cover legacy UNC parsing, source/cache identity, SMB path
  normalization (including a root directory named after the share), empty
  share-root behavior, SFTP endpoint-bound pins, and explicit selected-auth
  validation.
- Database/API tests cover vault-reference credential availability in source
  tree and local search results, plus fingerprint preservation when extended
  UNC paths are normalized.
- Session-cache tests cover invalidating the shared cache when a source is
  edited, and SMB cache eviction must close the native session.
- Repository/database tests prove credential round trips, legacy migration
  write/read-back before redaction, endpoint/pin persistence, and no secret
  values in database or JSON serialization.
- Live SMB tests authenticate, list, read head/middle/tail/EOF offsets, and
  reject bad credentials without exposing them. Live SFTP tests cover no-auth
  host probing, password and key auth, encrypted-key passphrase cases, random
  reads, and key rotation with an auth-log assertion proving no login occurred.
- Flutter widget/session tests cover first-use trust, cancel-without-persist,
  changed-key replacement, background no-prompt behavior, and explicit auth
  method selection.
- Release verification includes full Flutter analyze/tests, serial Rust tests,
  warnings-as-errors Rust check, and Android and Windows builds. A native
  Android SMB fix also requires same-device smoke evidence.

## 7. Wrong vs Correct

Wrong:

```rust
// Trust-on-first-use happens in the network callback before the user decides.
Ok(true)
```

Correct:

```rust
// Probe without credentials, then require the saved foreground-approved pin.
SftpClient::probe_host_key(host, port)?;
SftpClient::connect(host, port, user, selected_auth, password, key,
                    passphrase, expected_fingerprint)?;
```

Wrong:

```dart
// Persist secrets beside source metadata or let SMB fall through local I/O.
source.password = password;
```

Correct:

```dart
await credentialVault.put(credentialKey, password);
source.credentialRef = credentialKey;
```
