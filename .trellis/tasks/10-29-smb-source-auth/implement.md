# Implementation Record

The SMB source implementation and live validation are complete. The task
remains `in_progress` until the parent task is committed and archived through
the normal Trellis workflow.

- SMB2/3 connection, guest and credential auth, list, read, and disconnect APIs
  are integrated into browsing, scanning, covers, and book opening.
- Legacy UNC and extended UNC paths retain source identity; configured roots
  are handled as share-relative coordinates.
- Source edits evict the shared cache and close the native SMB session.
- Real SMB auth/list/read was verified against a temporary Windows share and
  from MuMuPlayer 15. The share/user cleanup markers confirm cleanup.
- Rust SMB path tests and the full Flutter/Rust suites passed; Android and
  Windows release builds passed.
