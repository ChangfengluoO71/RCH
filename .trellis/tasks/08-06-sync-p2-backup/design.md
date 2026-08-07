# P2 设计 — 备份即同步（双传输模式）

## 1. 配置（app_settings 键）

| 键 | 取值 | 说明 |
|---|---|---|
| `sync_mode` | off / folder / webdav | 传输模式 |
| `sync_dir` | 路径 | 模式 B 同步盘目录 |
| `sync_webdav_url` / `sync_webdav_username` / `sync_webdav_password` | 文本 | 模式 A 的 WebDAV 地址/账号/密码（独立书源配置，仅存本机、不入同步包） |
| `sync_webdav_dir` | 远程目录 | 模式 A 自定义远程目录（默认 `RCH/sync`，推送前逐级 MKCOL） |
| `sync_last_at` | 毫秒 | 最近一次同步时间 |
| `sync_last_status` | 文案 | 最近一次结果/错误 |

## 2. 包路径约定

- 模式 B：`<sync_dir>/latest.rchpkg` + `<sync_dir>/archive/{yyyyMMdd_HHmmss}.rchpkg`
- 模式 A：`<sync_webdav_dir>/latest.rchpkg`（默认 `RCH/sync`，相对服务器根） + 同目录 `archive/`
- 冲突副本（模式 B）：`latest (冲突副本)*.rchpkg` / `latest(1)*.rchpkg` / `latest-*.rchpkg` → 自动拉取时忽略，仅在状态里提示数量
- 归档副本：每次推送的历史快照，作为回滚/恢复依据（"从文件恢复"）；设置面板提供"清理归档"按钮（本地目录直接删除，WebDAV 用 DELETE 接口），保留 `latest.rchpkg` 不清。

## 3. 原子写

- 模式 B：写 `latest.rchpkg.tmp` → rename 成 `latest.rchpkg`，避免网盘客户端上传中途读到半包
- 模式 A：PUT 前用 MKCOL 幂等创建 `RCH/sync`（201=新建、405=已存在均视为成功）

## 4. 流程

- **push**：`rchpkgExport(增量)` → 写入/上传 `latest.rchpkg` → 归档时间戳副本
- **pull**：检测包存在/较新（或用户强制）→ `rchpkgImport` → 更新 last_sync
- **restore**：用户选任意 `.rchpkg` 文件 → `rchpkgImport`（P3 合并引擎前为按行 upsert，凭据保留）

## 5. 新 Rust API

`source/webdav.rs` WebDavClient 增加：

- `upload_file(path, bytes)` — PUT
- `download_file(path) -> Vec<u8>` — GET 全量
- `make_dir(path)` — MKCOL（幂等）

`api/source.rs` 暴露 FRB：`webdav_upload_file` / `webdav_download_file` / `webdav_make_dir`（复用现有会话表 `get_session`）。

## 6. Dart 侧

- `store/sync_manager.dart`：SyncManager（ChangeNotifier 单例）——配置读写（`dbSaveSetting`）、push/pull/restore、最近状态
- `ui/sync_panel.dart`：设置页"备份/同步"面板——模式选择、目录选择（file_selector）、WebDAV 书源下拉、立即推送/拉取/恢复按钮、最近状态与冲突副本提示
- 纯逻辑（冲突副本识别、包路径构建、远程路径构建）拆成可单测的顶层函数

## 7. 定时同步（已移除）

- 2026-08-07 决策：删除定时同步（`Timer` / `autoSync` / `sync_interval_minutes`），逻辑待改善；当前仅手动推/拉/恢复。
- 待改善点（未来重做时的参考）：启动自动拉取的时机、冲突合并前自动覆盖本地编辑的风险、周期防重入与失败重试策略。
