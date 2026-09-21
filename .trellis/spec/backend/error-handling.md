# Error Handling（Rust 核心的错误约定）

> 2026-09-21 补实（替换原模板）。

## 分层

| 层 | 形态 | 说明 |
|---|---|---|
| 内部实现 | `anyhow::Result<T>` | 允许上下文链（`.context("…")`） |
| 扫描/封面边界 | `RemoteScanError` | 固定枚举：`TransientNetwork` / `RateLimited` / `Provider(String)` / `MalformedResponse(String)` / `Unsupported` |
| FRB 边界（给 Dart） | `Result<T, String>` | 不带内部细节，文案面向用户与诊断 |

## 失败码：固定枚举 + 收敛判定

- 落库/落盘的码是固定枚举（`COVER_REASONS`）：`cover_read_budget_exceeded`、`cover_document_open_failed`、
  `cover_page_render_failed`、`cover_native_lib_missing`、`cover_decode_failed`、`cover_bytes_limit`、
  `cover_pdf_bytes_limit`、`cover_pixels_too_large`、`cover_size_missing`、`cover_partial_decode` 等。
- **判定收敛到自家文案常量**：`cover_native_lib_missing` 只认
  `document::pdf::PDFIUM_LOAD_FAILURE_MARKER`。教训（2026-09-21）：`contains("pdfium")` 会把
  pdfium-render 的库内错误误标成"部署缺库"，把排查带偏。
- provider 文案先经 `provider_failure_code(...)` 映射成受控子码再落库，**原文不入库、不上面**。

## 重试资格（必须显式决定）

- 终态（`long_retry_is_retryable == false`）：`cover_document_open_failed`、`cover_page_render_failed`、
  `cover_decode_failed`、`cover_size_missing` 等确定性失败；
- 可长期补偿（`COVER_RETRYABLE_REASONS`）：`cover_native_lib_missing`、`cover_bytes_limit`、
  `cover_pdf_bytes_limit`、`cover_pixels_too_large`、`cover_partial_decode`、`cover_read_budget_exceeded`；
- 新增码若不在任何集合 ⇒ 默认**不可重试**：要么显式加入，要么在契约里写明为终态；
- 用户侧逃生口：源浏览器「刷新」重排该源当前档的终态失败任务（`remote_cover_retry_failed`）。

## 红线

- provider 原文/凭据不写进数据库、日志或 UI（用 `diag::safe_asset_label` 脱敏）；
- 可恢复错误不用 `unwrap()`/`expect()`；测试或"逻辑上不可能"处使用要写明理由；
- 不把"预算/我们的决定"混成"文件坏了"：预算中止有独立码，便于区分。
