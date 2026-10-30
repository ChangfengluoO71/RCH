# SFTP host-key and private-key authentication

## Purpose

Remove SFTP's accept-any-host-key behavior and support private-key login while retaining password login.

## Requirements

- First connection probes and displays the server's SHA-256 host-key fingerprint before sending user credentials.
- Accepted fingerprints are stored with their endpoint; matching keys connect, mismatched keys fail before authentication.
- Host-key replacement requires an explicit user action; background work never auto-trusts and never opens a modal.
- Existing SFTP records without a pin require foreground confirmation once.
- Password and OpenSSH/PEM private-key login are selectable auth methods.
- Private key contents and key passphrase are stored via CredentialVault; imported file path and secret bytes are not placed in ordinary JSON or logs.
- Wrong password, wrong key, and wrong passphrase fail clearly without leaking secrets.

## Acceptance

- Test first-use accept/cancel, matching pin, changed key, explicit replacement, and endpoint change.
- Live SFTP server accepts correct password and private key, rejects wrong password/key/passphrase.
- Android and Windows vault round-trips and legacy source migration pass.
- Existing password-only sources continue to work after confirming/pinning the host key.

