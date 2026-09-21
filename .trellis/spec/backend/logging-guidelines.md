# Logging Guidelines（本项目的日志与诊断规范）

> 本文件描述**项目现在的做法**（2026-09-21 补实，替换原模板占位）。
> 每条规则都能指向真实代码或真实日志文件；不确定的写进"现状缺口"。

## 1. 三套机制，别混用

| 机制 | 落点 | 用途 | 现状 |
|---|---|---|---|
| `remote_scan::diag::note(...)` | 应用数据目录下的 `*_diag.log`（追加写文本） | **现场取证**：用户报 bug 时唯一可读的证据 | 主力，桌面与手机都可用 |
| `tracing::info!` / `warn!` | 无订阅者 => **不产出任何输出** | — | **历史遗留**：不要靠它排障（项目没有 `tracing_subscriber`） |
| `crate::perf` 事件 | `perf` 事件（JSON，含 `t_us`） | 结构化性能度量与 A/B | 用于读取/封面性能对比 |

## 2. 诊断日志清单（新增日志必须沿用同一形态）

| 文件 | 写入者 | 事件与关键字段（真机实测样例） |
|---|---|---|
| `scan_diag.log` | 远程扫描 / 封面 worker | `scan_terminal status=… mode=… gen=… checked=…`、`cover_fail … code=… asset=…`、`cover_budget detail=…`、`cover_whole_file_read bytes=…`、`startup_recovered_cover_leases=…` |
| `pdf_diag.log` | PDF 文档层 | `pdf_open mode=lazy ms=… lazy_reads=… lazy_bytes=… size=…`、`pdf_page index=… width=… ask_reads=… ask_bytes=… out_bytes=…` |
| `reader_diag.log` | 阅读打开路径 | `reader_open mode=raw-cache|stream|fallback-download stream_ms=… download_ms=…` |
| `mobi_diag.log` | MOBI 文档层 | `mobi_open mode=lazy|cover-lazy|lazy-cached|full-fallback pages=… cache=hit|miss ms=…`、`mobi_page index=… bytes=… ms=…`、`mobi_lazy_declined reason=…`、`mobi_lazy_skipped_ranges count=…`、`mobi_lazy_clamped_offsets count=…` |

规则：
- 一行一条事件；**首字段是事件名**，其余 `key=value` 空格分隔，便于 `grep` 与人工扫读；
- 时间戳格式**跟随所在文件**的既有写法（`scan_diag` 用 RFC3339 UTC，如 `2026-09-21T13:29:12Z`；`mobi_diag` 用毫秒 epoch），不要在同一文件里混两种；
- 单位写进字段名（`ms`、`bytes`、`reads`、`pages`）；
- 失败必须自证"**哪一条**先超"：见 `AdapterByteSource::read_at` 的 `cover_budget detail=<used>/<budget> bytes, <calls>/384 reads, <ms>`；
- 拒绝/回退路径必须留**原因枚举**：如 `mobi_lazy_declined reason=no_mobi_magic|first_image_index|probe_failed|…`；
- 诊断只增不减：新增字段向后兼容，不改已有字段含义（历史日志仍要能读）。

## 3. 绝不写入日志的内容（红线）

- **provider 凭据**：Cookie、token、签名、`Authorization` 一律不落盘；
- **完整 URL 与逻辑路径**：用 `diag::safe_asset_label(&path)` 取代原路径；
- 用户隐私（书名、账号）默认不打；确需定位时用去掉目录与前缀的短标识（如 `file_stem`）；
- 错误对象**不要**整段 `{:?}` 落盘：上游把 provider 文案映射成**固定枚举码**后再落库/落盘。

## 4. 失败码是固定枚举，不要自由发挥

封面失败码定义在 `api/remote_scan.rs`：`cover_read_budget_exceeded`、`cover_document_open_failed`、
`cover_page_render_failed`、`cover_native_lib_missing`、`cover_size_missing`、`cover_bytes_limit`、
`cover_pdf_bytes_limit`、`cover_pixels_too_large`、`cover_decode_failed` 等。

- 判定必须收敛到**自家文案常量**而非宽泛子串：`cover_native_lib_missing` 只认
  `document::pdf::PDFIUM_LOAD_FAILURE_MARKER`。历史教训（2026-09-21）：用 `contains("pdfium")` 判定，
  而 pdfium-render 的任何库内错误文案都含 "pdfium" => 渲染/解析错误被误标成"部署缺库"，把排查带偏；
- 新增码时必须同时决定**重试资格**（`COVER_RETRYABLE_REASONS` 还是终态），并在契约里写明。

## 5. 现状缺口（诚实记录）

- `tracing` 宏没有订阅者 => 现有 `tracing::info!/warn!` 调用是**死输出**；要么接 `tracing_subscriber`，要么改用 `diag::note`；
- 诊断日志没有轮转或体积上限（依赖文件系统），长时间真机运行会持续增长；
- `diag` 写入是 best-effort（失败静默），因此**日志缺失本身不能作为"没发生"的证据**。
