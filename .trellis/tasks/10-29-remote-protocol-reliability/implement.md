# Implementation Record

The user approved inline implementation on 2026-10-29. The parent task and
SMB/SFTP child tasks remain `in_progress` until the user completes the normal
commit and archive workflow.

## Completed

- Added native credential-vault storage and verify-before-redact migration.
- Added SMB2/3 source parsing, authenticated sessions, remote browsing and
  random-access reads; verified it against a temporary Windows share and on
  MuMuPlayer 15.
- Added explicit SFTP host-key trust and password/private-key authentication;
  verified first-use trust, password/key auth, encrypted key handling, wrong
  credentials, random reads, and host-key rotation against temporary servers.
- Fixed review findings in vault-backed source availability, shared/provider
  session invalidation, native SMB session release, SMB share-relative roots,
  and extended-UNC fingerprint preservation.
- Researched WinSCP host-key trust and Total Commander remote plugins. The
  resulting product-practice notes and links are in the approved design.

## Verification

- `flutter analyze` — passed with no issues.
- `flutter test --reporter compact` — 410 passed, 1 skipped.
- `RUSTFLAGS='-D warnings' cargo check -j 1 --locked --all-targets` — passed.
- `cargo fmt --all -- --check` — passed.
- `cargo test -j 1 --locked -- --test-threads=1` — passed; 571 unit tests
  passed, 4 were ignored, and all integration targets passed.
- `flutter build apk --release` — passed; APK is
  `app/build/app/outputs/flutter-apk/app-release.apk`.
- `flutter build windows --release` — passed; executable is
  `app/build/windows/x64/runner/Release/RCH.exe`.
- Temporary SMB shares/users and SFTP servers reported cleanup complete. The
  generated test directory remains because the environment blocked its
  deletion, even when the exact path and all descendants were verified.

## Remaining workflow

No commit or task archive was created. The code and validation are ready for the
normal Trellis commit/archive steps when requested.
