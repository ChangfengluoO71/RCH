# Quality Guidelines

> Code quality standards for backend development.

---

## Overview

Backend code must make concurrency and native-library safety contracts explicit at adapter boundaries. A safe Rust type signature is not sufficient evidence that an underlying native library is re-entrant or that a mutex guard is released before callbacks/prefetch work.

---

## Forbidden Patterns

- Do not call a known non-reentrant native library concurrently merely because its Rust wrapper exposes `Send` / `Sync`.
- Do not keep a `MutexGuard` alive across calls that can re-enter the same cache/state lock. Avoid compact expressions such as `if let Some(v) = mutex.lock().unwrap().get(...) { call_that_may_relock(); }` when the temporary guard lifetime is ambiguous.
- Do not hand unbounded render dimensions directly to an encoder or container format with a documented single-dimension limit.

---

## Required Patterns

- For non-reentrant native libraries such as the current Pdfium integration, serialize every FFI operation that touches shared native state through one adapter-level gate, including document open, page access/render, bitmap copy, and native-object destruction.
- Release cache/state locks in an explicit local scope before scheduling prefetch, invoking callbacks, waiting on other work, or calling code that may acquire the same lock.
- Validate and proportionally cap render dimensions before allocation/encoding when the target format has hard limits; preserve aspect ratio and keep normal-size inputs on the normal quality path.
- Keep CPU-only work outside native-library serialization gates once data has been copied into owned Rust memory, so correctness does not unnecessarily serialize unrelated work.

---

## Testing Requirements

- Concurrency fixes must include a regression that fails before the fix and proves the serialization or lock-lifetime contract after the fix.
- Reader/cache changes must cover both cache-miss and cache-hit paths; a cache-hit path that triggers prefetch must be tested for deadlock/non-return.
- PDF/native-reader acceptance must include a multi-page fixture so neighbor prefetch is exercised; one-page fixtures are insufficient for concurrency coverage.
- Image/PDF rendering tests must include boundary-shaped inputs such as ultra-tall pages that can exceed encoder dimension limits.
- When a native Android failure cannot be reproduced faithfully on the host, retain host regression tests but require same-device/source smoke evidence before closing the incident.

---

## Code Review Checklist

- Does any changed native wrapper assume thread safety that the underlying C/C++ library does not guarantee?
- Can any mutex guard survive into prefetch, callback, wait, or nested state access?
- Are native destructors covered by the same synchronization contract as native constructors and page operations?
- Can computed image dimensions exceed the downstream encoder/container limit?
- Do tests cover multi-page/concurrent behavior rather than only a single happy-path page?
- For Android-native crash fixes, is there device evidence in addition to host tests?
