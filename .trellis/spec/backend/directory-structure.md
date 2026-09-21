# Directory Structure（后端 = Rust 核心 `app/rust`）

> 2026-09-21 补实（替换原模板）。

```
app/rust/
├── src/
│   ├── api/            # FRB 边界（Dart 可调）：book.rs / source.rs / remote_scan.rs / remote_cover.rs / cache.rs
│   ├── document/       # 格式层：zip / epub / pdf / mobi / rar / sevenz / tar / folder / image
│   ├── source/         # 书源客户端：quark / cloud115(+web) / baidu / webdav / sftp / local，及 SourceReader
│   ├── remote_scan/    # 远程扫描与封面：engine / cover_store / cover_service / diag / persistence / provider_budget
│   ├── sync/ · ai/ · scraper*.rs · db/mod.rs · cache.rs · reader.rs · perf.rs
│   └── frb_generated.rs  # 生成物，勿手改
├── tests/              # 契约测试 *_contract.rs（跨层不变量、状态机、预算、重试）
└── examples/           # 手动/性能入口（read_profile.rs 等；cargo test 会编译它们）
```

## 放代码的规则

- 新增书源：`source/<provider>.rs` + `source/mod.rs` 注册 + `remote_scan/adapter.rs` 适配 + 契约测试；
- 新增格式：`document/<format>.rs` 实现 `Document`（`page_count` / `page_bytes`，必要时覆写
  `page_bytes_for_display`）；**惰性打开优先**（只读头/目录，按需读页）；
- 新增 FRB 接口放 `api/`，参数与返回用简单类型；**改签名后必须在 `app/` 下跑
  `flutter_rust_bridge_codegen generate`**，生成的绑定一起提交；
- 诊断日志走 `remote_scan::diag::note(...)`（见 logging-guidelines）。

## 测试放哪

- 跨层不变量、状态机、预算、唤醒/回退契约 ⇒ `tests/*_contract.rs`（真机 bug 修复必须补这里）；
- 纯函数/单元行为 ⇒ 同文件 `#[cfg(test)] mod tests`；
- 涉及缓存根的用例必须用隔离临时缓存根（`set_custom_cache_root` + RAII guard），否则会写进用户真实缓存。
