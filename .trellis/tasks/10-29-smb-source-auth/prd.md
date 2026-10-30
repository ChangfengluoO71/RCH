# SMB source sessions and credentials

## Purpose

Replace SMB's local UNC pass-through with a real authenticated SMB2/3 source on Android and Windows.

## Requirements

- SMB add-source UI collects server, port (default 445), share/initial directory, username, and password.
- Blank SMB credentials attempt Guest; DOMAIN\user usernames are accepted.
- Legacy UNC paths normalize to the same host/share/path profile.
- An SMB session lists remote entries and supplies file size and positioned reads to the reader.
- SMB is remote for browser, offline snapshots, catalog indexing, cover scans, cache identity, sync decisions, and book opening.
- SMB operations are read-only in this task; no writes/deletes/uploads or network discovery.
- SMB1 is never enabled.

## Acceptance

- A real SMB2/3 server accepts valid credentials and rejects invalid credentials.
- Directory listing works and comic archives read at offset 0, a middle offset, EOF, and beyond EOF.
- MuMuPlayer 15 and Windows can browse and open a sample comic from the same test share.
- Old UNC records preserve share and nested path; no SMB source uses std::fs/local File APIs.

