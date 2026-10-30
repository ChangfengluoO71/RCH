# Design

Use a pure Rust SMB2/3 client in app/rust/src/source/smb.rs, adapted to the project's ByteSource and remote session patterns. Add Rust APIs for connect/disconnect/list/open, then route Flutter source UI and session state through them. Existing SMB UNC records normalize to host/share/path; new profiles store host in url, port in port, and share-relative path in path.

SMB is removed from isLocalFs and added to needsSession. Extend remote scan adapters so catalog and cover behavior remain consistent with WebDAV/SFTP. Use SMB2/3 only and keep the profile read-only.

Full details and research: docs/superpowers/specs/2026-10-29-remote-source-protocols.md, “SMB”. Implementation steps: docs/superpowers/plans/2026-10-29-smb-source.md.

