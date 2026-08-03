# M6 网盘直连书源（百度 / 115 官方 API）— 实施计划

## 前置条件（用户侧，需在实现前完成）

- [ ] 注册百度网盘开放平台应用（个人实名 + 网盘基础服务权限），拿到 AppKey / SecretKey
- [ ] 申请 115 生活开放平台应用（申请制），拿到 APP ID
- [ ] 准备两个真实网盘账号（含漫画 CBZ）用于联调

> 若 115 审核未完成：仍可按「用户自填 APP ID」先行实现，联调时用临时 APP ID 验证。

## 实施步骤（顺序执行）

### 步骤 0：API 冒烟探针（独立最小验证）

- 用真实凭证验证（**等开放平台应用审核通过后执行**）：
  - 百度：refresh_token -> list(root) -> fs_id_of_path -> filemetas(dlink) -> Range GET(206)
  - 115：authDeviceCode -> 轮询 -> deviceCodeToToken -> list(0) -> downurl -> Range GET(206)
- 产出：把实际响应 JSON 样例、Authorization 前缀确认写回 `research/` 两个契约文档。
- 关卡：两家都能列目录 + 取到直链 + Range 206 才联调；否则先修契约理解（实现已按 AList SDK 实证编写，单测覆盖 JSON 解析）。

### 步骤 1：Rust 客户端模块

- `app/rust/src/source/baidu.rs`：BaiduClient + BaiduFile + 单测（auth_url 构造、list/filemetas JSON 解析、token 刷新逻辑、UA 头）
- `app/rust/src/source/cloud115.rs`：Cloud115Client + Cloud115File + 单测（PKCE challenge、list/downurl JSON 解析、节流、qr 状态机）
- `source/mod.rs` 注册两个模块
- 验证：`cargo test`（工作目录 `app/rust`）

状态：✅ 已完成（49 项单测通过）

### 步骤 2：DB 迁移 + 模型层

- `db/mod.rs`：幂等 ALTER 加 `refresh_token / client_id / client_secret / root_id` 列；`BookSourceRow` 增补；`load_all_sources/upsert_source` 更新
- `api/db.rs` + `frb_generated.*`：`flutter_rust_bridge_codegen generate` 重新生成
- 验证：`cargo test` + `flutter analyze`

状态：✅ 已完成

### 步骤 3：Rust API 层（`api/source.rs`）

- 百度：`baidu_auth_url / baidu_exchange_code / baidu_connect / baidu_disconnect / baidu_list / open_baidu_book / baidu_download_progress / baidu_has_raw_cache / baidu_cover`
- 115：`cloud115_qr_start / cloud115_qr_poll / cloud115_connect / cloud115_disconnect / cloud115_list / open_cloud115_book / cloud115_download_progress / cloud115_has_raw_cache / cloud115_cover`
- 三态打开逻辑镜像 `open_webdav_book`；token 回写：connect 返回最新 refresh_token
- FRB regen + Dart wrapper（默认 strategy=auto）
- 验证：`cargo test` + `flutter analyze`

状态：✅ 已完成（codegen 已跑，Rust API 全链路编译通过）

### 步骤 4：Dart 模型与会话

- `models.dart`：BookSource 新字段 + getter + capabilityDisplay
- `book_repository.dart`：字段映射（load/save/update）
- `store/baidu_session.dart`、`store/cloud115_session.dart`
- 验证：`flutter analyze` + `flutter test`

状态：✅ 已完成

### 步骤 5：UI

- `home_page.dart` 添加/编辑书源对话框：类型选择改 DropdownMenu（6 类），百度/115 表单 + 授权按钮 + 高级折叠
- 115 扫码对话框（`qr_flutter`），`pubspec.yaml` 加依赖
- 分发点：`source_browser.dart` / `book_detail_page.dart` / `ai_upscale_manager.dart` / `comic_cover.dart`
- `add_source_dialog_test.dart` 更新（6 类型切换）
- 验证：`flutter analyze` + `flutter test`

状态：✅ 已完成（analyze 0 issues，11 项测试全过）

### 步骤 6：全量验证

- `cd app/rust; cargo test` 全绿；`cargo build --release` 通过
- `cd app; flutter analyze` 0 issues；`flutter test` 全绿
- 清理逻辑回归：`removeSourceWithCleanup` 与 `purgeStale` 对 baidu/115 前缀正确

状态：✅ 已完成（removeSourceWithCleanup 按 type|id 前缀天然覆盖；purgeStale 只删不存在的源/本地失效文件，不影响 baidu/115）

### 步骤 7：用户联调（需真实账号）

- 百度：授权添加 -> 浏览 -> 三策略打开 CBZ -> 封面 -> 重启保持 -> 删除书源清理
- 115：扫码授权 -> 浏览 -> 三策略打开 CBZ -> 封面 -> 重启保持 -> 删除书源清理
- 手动 `cd app; flutter build windows --release`
- 关注点：dlink 失效重取、token 过期刷新、115 限速体感、百度 UA 头生效（>20MB 文件）

状态：⏳ 待用户联调（需要开放平台凭证 + 真实账号）

### 步骤 8：收尾

- `trellis-check` 全量检查（spec 合规 / lint / 测试 / 跨层一致性）
- `trellis-update-spec`：把两家 API 契约要点 + 鉴权流程 + 坑写进 `.trellis/spec/backend/`
- 按 Phase 3.4 批量提交（先工作提交，再归档/日志提交）

状态：⏳ 联调通过后执行

## 验证命令速查

```powershell
cd app/rust; cargo test; cargo build --release
cd app; flutter analyze; flutter test
```

## 回滚点

- 步骤 0 失败：不进入实现，回到 research 修正契约
- 步骤 1~3 任一失败：只动 Rust 层，删除模块/API 即可，Dart 未动
- 步骤 4~5 失败：删除 Dart 分支 + 依赖，DB 列保留无害
- 任何步骤发现 prd 缺陷：回到 Phase 1 修订 prd/design 再继续

## 评审关卡

- 步骤 0 探针结果（真实 API 响应）-> 关卡 A
- 步骤 3 完成（Rust API 全链路可编译 + 单测）-> 关卡 B
- 步骤 5 完成（UI + 测试全绿）-> 关卡 C
- 步骤 7 用户联调通过 -> 关卡 D（之后才提交）
