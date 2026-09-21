# Directory Structure（前端 = Flutter `app/lib`）

> 2026-09-21 补实（替换原模板）。

```
app/lib/
├── ui/            # 页面与控件：home_page / book_detail_page / reader_page / comic_cover / source_browser
│                  #   remote_scan_status / cover_editor_page / cloud115_qr_scan / cache_manager
├── store/         # 状态与模型：library_store（含 AppSettings）/ models.dart / remote_scan_* / update_manager
├── repository/    # 数据访问：record_repository / remote_cover_repository（四类 loader 注入）
└── src/rust/      # FRB 生成物（api/*.dart、frb_generated*.dart）—— 勿手改
app/test/          # 单元与 widget 测试 *_test.dart（含契约型用例，如 comic_cover_legacy_fallback_test）
```

## 放代码的规则

- 新页面 ⇒ `ui/<name>_page.dart`；可复用控件 ⇒ `ui/<name>.dart`（同目录，不另建 widgets 层）；
- 可被多处读写的状态 ⇒ `store/`；只读数据访问与 loader 注入 ⇒ `repository/`；
- **网络/DB/原生调用不进 UI**：通过 repository 或 store 暴露的 loader 使用（便于测试注入）；
- 依赖 FRB 新接口时先在 `app/rust` 加 `api/` 函数并跑 codegen（见 backend/directory-structure）。

## 命名

- 文件 `snake_case.dart`；页面类 `<Name>Page`；测试与被测文件同名 + `_test.dart`；
- 一个页面一个文件，避免"多功能大杂烩页"（设置项过多时按类别折叠，见 `home_page.dart`）。
