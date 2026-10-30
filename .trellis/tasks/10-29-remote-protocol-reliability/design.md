# Design

The approved route is one Rust-backed SMB2/3 session per saved SMB source, with the existing remote-source pipeline owning browsing, indexing, covers, and random comic reads. New SMB profiles store host in url, 445-default port in port, and share-relative location in path; old UNC records are normalized on load.

SFTP will probe host keys without user authentication, require explicit first-use trust, pin SHA-256 fingerprints to the endpoint, and reject changed keys before auth. Password and OpenSSH/PEM private-key auth remain separate choices. Private key bytes and passphrase are vault secrets; host-key pin metadata is non-secret source data.

The shared credential vault keeps Android's existing Keystore channel and adds secure platform storage elsewhere. Repository migration verifies the stored bundle before clearing legacy database values; native JSON backup omits secrets.

Full technical design and product research: docs/superpowers/specs/2026-10-29-remote-source-protocols.md. Detailed implementation plans: docs/superpowers/plans/2026-10-29-credential-vault.md, docs/superpowers/plans/2026-10-29-smb-source.md, and docs/superpowers/plans/2026-10-29-sftp-host-security.md.

The user approved the design and inline implementation on 2026-10-29. Work is
in progress; the final verification pass now also covers vault-backed search
availability, shared/provider session invalidation, SMB share-relative roots,
and stable fingerprints for extended UNC migration. Do not archive until the
release checks and platform builds pass.

