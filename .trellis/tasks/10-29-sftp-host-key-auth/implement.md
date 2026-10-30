# Implementation Record

The SFTP host-key and authentication implementation and live validation are
complete. The task remains `in_progress` until the parent task is committed
and archived through the normal Trellis workflow.

- New and changed host keys require explicit foreground trust. Background
  connection paths fail closed without prompting.
- Password and private-key authentication are explicit modes; missing selected
  credentials fail without fallback.
- Private keys and passphrases use the platform credential vault; host-key
  pins and auth mode are persisted as non-secret metadata.
- Temporary server tests covered first-use trust, password and key auth,
  encrypted-key passphrase behavior, random reads, wrong credentials, and key
  rotation with an authentication-log assertion.
- Full Flutter/Rust suites and Android/Windows release builds passed.
