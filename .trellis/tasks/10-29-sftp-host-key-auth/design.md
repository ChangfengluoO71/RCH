# Design

Use russh's server-key callback to compare a pinned SHA-256 fingerprint and reject mismatches. A separate probe completes only key exchange and returns the fingerprint without attempting login. Flutter prompts for first-use trust, persists the pin after acceptance, then reconnects with password or public-key auth.

Add non-secret hostKeyFingerprint/hostKeyEndpoint source metadata and migrate the SQLite schema. Store private key and passphrase in the shared CredentialVault. Use a native file selector to import OpenSSH/PEM content; never persist the selected path.

Full details and research: docs/superpowers/specs/2026-10-29-remote-source-protocols.md, “SFTP”. Implementation steps: docs/superpowers/plans/2026-10-29-sftp-host-security.md.

