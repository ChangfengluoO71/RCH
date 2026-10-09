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

## Scenario: Width-Scoped Comic Page Reads

### 1. Scope / Trigger

Use this contract when the same source page can be rendered at multiple display widths, including split-wide-page views and PDF rasterization. A session-global width can make a foreground page, neighbor prefetch, or in-flight request reuse bytes rendered for a different viewport.

### 2. Signatures

```rust
Reader::get_page(index: u32) -> Result<Arc<Vec<u8>>>;
Reader::get_page_with_width(index: u32, target_width: Option<u32>)
    -> Result<Arc<Vec<u8>>>;
book_page(handle: u64, index: u32, target_width: Option<u32>) -> Result<Vec<u8>>;
book_page_dimensions(handle: u64, index: u32)
    -> Result<Option<BookPageDimensions>>;
```

`reader_page_split_overrides(book_key, page_index, split)` stores device-local corrections. `split = None` deletes the correction.

### 3. Contracts

- Normalize `None` and non-positive widths to the default request profile. The default disk path remains `page/<namespace>/<index>.bin`; a width `w` uses `page/<namespace>/w<w>/<index>.bin`.
- Key memory cache, in-flight coalescing, scheduled prefetch, and disk lookup by the same `(page_index, normalized_width)` pair. Prefetch inherits the originating request width but stays low priority.
- Preserve `get_page(index)` as the default-width convenience API. Width-aware callers use `get_page_with_width`; do not reintroduce mutable session-wide display state.
- PDF dimension queries and rasterization use the same Pdfium synchronization gate. Formats without cheap dimensions return `None`; the caller may use a bounded preview to learn image geometry.
- Split overrides live only in the local SQLite table. They are not `BookMeta`, app settings, or sync entities.

### 4. Validation & Error Matrix

| Condition | Required behavior |
| --- | --- |
| Width omitted or zero | Read/render with the default profile and legacy disk location. |
| Two requests ask for different widths | Keep independent cache and in-flight entries; neither waits on or receives the other's render. |
| Neighbor prefetch starts from a width-specific foreground read | Use that same width under prefetch priority. |
| Document cannot report dimensions | Return `None` without rasterizing; let the reader request a bounded preview if needed. |
| Split correction is cleared | Delete only the matching `(book_key, page_index)` row. |

### 5. Good / Base / Bad Cases

- Good: a page read at 1080px and 3200px has two cache entries; returning to 1080px hits the prior 1080px entry.
- Base: a default-width request still reads an existing legacy `<index>.bin` cache file.
- Bad: mutate `Reader.display_width`, clear one global L1, then allow asynchronous requests to observe the width at different times.

### 6. Tests Required

- Assert different widths produce distinct bytes and `w<width>` files, then prove each is independently served after L1 eviction.
- Block one width while requesting another for the same page; assert the second request completes without joining the first.
- Assert prefetch carries the originating width and a legacy cache file remains readable.
- Assert split overrides are page- and book-scoped, delete cleanly, and do not appear in sync metadata.
- Keep Pdfium dimension/render tests behind the existing native gate and use a multi-page fixture when available.

### 7. Wrong vs Correct

Wrong:

```rust
reader.set_display_width(3200);
reader.get_page(index);
```

Correct:

```rust
reader.get_page_with_width(index, Some(3200));
```

The request-scoped form keeps cache identity and the actual render dimensions aligned even when neighboring reads overlap.

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
