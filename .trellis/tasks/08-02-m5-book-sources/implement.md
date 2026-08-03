# M5 书源扩展（SMB / SFTP）— 实施计划

## 执行顺序与验收门

每步完成必须通过该步的验证命令；最后一步为发布构建冒烟。全程不做 git commit，由用户确认后统一走 Phase 3.4。

### Step 0：russh API 探针（先行，独立于主仓）

- 在仓库外的临时 cargo 工程验证：`russh 0.62.5` + `russh-sftp 2.3.0` 在 Windows x64 MSVC 下可编译（`cargo check`）。
- 验证最小链路 API：`client::connect` → 握手/host key 处理 → `authenticate_password` → `channel_session` + `request_subsystem("sftp")` → `SftpSession::new` → `read_dir` / `open` + `seek` + `read` / `metadata`。
- 结论（API 形态、block_on 用法、是否需额外 feature）写入 `research/russh-api-probe.md`。
- 门：编译通过 + 关键 API 用法确认。若 API 与设计不符，回改 design.md 再继续。

### Step 1：Rust 依赖

- `app/rust/Cargo.toml` 增加 `russh = "0.62.5"`、`russh-sftp = "2.3.0"`。
- 验证：`cargo check`（在 app/rust 下）通过。

### Step 2：`src/source/sftp.rs`

- 实现 `SftpClient`（connect / list / file_size / read_at / download_full / disconnect）、`SftpFile: ByteSource`、路径工具 `join_remote_path` / `parse_endpoint`。
- 单测：路径拼接（含根目录 `/`）、endpoint 解析（默认端口 22）、目录在前自然排序。
- 验证：`cargo test --lib` 通过。

### Step 3：`src/api/source.rs` 会话 API + 打开/封面

- `sftp_connect` / `sftp_disconnect` / `sftp_list` / `open_sftp_book` / `sftp_download_progress` / `sftp_has_raw_cache` / `sftp_cover`。
- 打开策略三态：`"auto"`（先整本下载 raw/，失败回退流式）/ `"download"`（强制整本，失败报错）/ `"stream"`（直接 `SftpFile` 流式）；同时给 `open_webdav_book` 增加同名 `strategy` 参数（默认 `"auto"` 保持现状）。raw/ 命名空间用 `endpoint`；`cache_ns = "sftp|{endpoint}|{path}"`。
- 中文错误提示（认证失败 / 连接拒绝 / 超时 / 路径不存在）。
- 验证：`cargo test --lib` + `cargo check` 通过。

### Step 4：FRB 重新生成

- `flutter_rust_bridge_codegen generate`（命令/环境见 SETUP.md）。
- 验证：diff 仅新增 sftp 相关 API，无意外改动；`flutter analyze` 暂不要求（Dart 侧还未接线）。

### Step 5：Dart 模型与会话

- `models.dart`：type 扩展、`port` 字段、`BookOpenStrategy` 枚举 + `AppSettings.bookOpenStrategy`（默认 auto，JSON 兼容）、getter（`isSftp`/`isSmb`/`isLocalFs`/`needsSession`）、`capabilityDisplay`。
- `src/rust/api/source.dart`：sftp wrapper。
- 新增 `store/sftp_session.dart`（镜像 webdav_session.dart）。
- 验证：`flutter analyze` 0 issues。

### Step 6：UI（home_page.dart）

- 添加书源对话框 4 类型 SegmentedButton + 条件字段；SMB 用 `listLocalDir` 连通性测试；SFTP 用 `sftp_connect` + `sftp_list` 测试。
- 编辑书源对话框按类型显示字段；书源图标 / 详情文案按类型区分。
- 设置页新增「远程书源」区块：打开策略三选一 SegmentedButton（auto/download/stream）；「本地漫画」区块的自动转 CBZ 副标题更新为说明适用范围（local + SMB）。
- 验证：`flutter analyze` 0 issues；新增/更新 widget 测试覆盖 4 类型切换与字段显隐。

### Step 7：三路分发接线

- `source_browser.dart`：列表分发（local/smb/webdav/sftp）、自动转 CBZ 按全局开关执行且范围 local+smb、文件夹检测 local+smb、sftp 跳过。
- `book_detail_page.dart` / `ai_upscale_manager.dart`：打开分发加 sftp；所有远程打开调用传 `AppSettings.bookOpenStrategy`（webdav 与 sftp 共用）。
- `comic_cover.dart`：封面分发加 sftp（`sftpCover` + `sftpSessionFor`）。
- 验证：`flutter analyze` 0 issues；`flutter test` 全量通过（现有 10 + 新增）。

### Step 8：全量验证 + 发布冒烟

- `cargo test --lib`（rust 全量）、`flutter analyze`、`flutter test` 全绿。
- `cargo build --release`（cdylib 可构建）→ `flutter build windows --release` 成功（russh 不破坏打包）。
- 手动联调（用户侧）：
  - SMB：添加 `\\server\share` → 浏览 / 打开 / 封面 / 阅读记录；无权限时明确报错。
  - SFTP：添加（含错误端口/密码验证报错）→ 浏览下钻 / 打开 / 进度条 / raw 缓存秒开 / 封面 / 重启后凭据保持 / 删除书源清理记录。

### Step 9：收尾

- `trellis-check` 全量检查（跨层一致、spec 合规）。
- `trellis-update-spec`：记录 russh 选型结论（Windows 无 C 工具链环境禁用 libssh2 系依赖）与 SFTP 会话模式（独立 runtime + block_on）。
- 按 Phase 3.4 提交计划向用户确认后提交。

## 回滚点

- Step 2-3 失败：删除 sftp 模块/API，回退依赖，不影响现有功能（无 schema 变更）。
- Step 5-7 失败：还原 Dart 分支改动，书源 JSON 向后兼容。
- 全程无 DB 迁移，回滚零数据风险。

## 里程碑验收锚点（对应 PRD）

- [ ] SMB 书源浏览/打开/阅读与本地一致；无权限明确提示
- [ ] SFTP 枚举/打开/封面可用；打开优先整本下载（进度），失败回退流式
- [ ] 全局设置「打开策略」三态生效（auto/download/stream 分别在 WebDAV/SFTP 上行为正确）；「自动转 CBZ」开关对 local + SMB 生效、对 webdav/sftp 跳过
- [ ] 重启后凭据可用；删除书源同步清理记录
- [ ] `flutter analyze` 0 issues；`cargo test --lib` 通过
