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

---

## 质量门禁（与 CI 完全对齐，2026-09-21 补）

提交前三条命令都要跑；缺一条就可能"本地绿、CI 红"：

```bash
cd app && flutter analyze                                              # 全量：CI 连 test/ 与 tool/ 一起分析
cd app/rust && RUSTFLAGS="-D warnings" cargo check --locked --all-targets
cd app/rust && cargo test --locked -- --test-threads=1                  # 必须串行
```

- **CI 的 Rust Test 注入 `RUSTFLAGS: -D warnings`**（由 `actions-rust-lang/setup-rust-toolchain` 设置）
  => **任何 warning 都是错误**；诊断用的死字段/方法要加 `#[allow(dead_code)]` 并注明用途。
- **`flutter analyze` 必须全量**：定点分析（`flutter analyze <file>`）查不出 `test/`、`tool/` 里的问题。
- **测试必须串行**：`cache::set_custom_cache_root` 是进程级全局，并行线程会互相覆盖缓存根。
- vendored 第三方工具（`rust_builder/cargokit/**`）已在 `analysis_options.yaml` 中排除；
  排除范围**只准是 vendored 代码**，不得用来掩盖本仓库自身的问题。
- **不要对未实现的能力写测试**：契约里描述但代码未落地的设计，测试会直接编译不过、把门禁拖红。
  实例（2026-09-21）：5 个测试文件按 `UpdateManager.testing(...)`、`NeedsWholeBookDownload` 等
  **从未实现**的 API 编写 => CI 64 项 error、发布被卡。正确做法是：锁"真实存在的 API"，
  并在测试与 spec 中写明**已知缺口**。
- **发布门禁**：`ci.yml` 四项（Flutter Analyze / Rust Test / Windows Build / Android Build）全绿后才打 tag；
  打 tag 触发 `release.yml` 产出 Windows 安装包与分 ABI 的 APK。发版流程见 `docs/development/setup.md`。
