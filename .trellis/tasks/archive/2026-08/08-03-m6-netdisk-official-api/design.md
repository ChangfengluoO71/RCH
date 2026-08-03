# M6 网盘直连书源（百度 / 115 官方 API）— 技术设计

## 1. 结论摘要

- 新增两种书源类型：`'baidu'`（百度网盘开放平台）与 `'115'`（115 生活开放平台），**直连官方 API**，不依赖 OpenList/AList。
- 复用 M5 建立的整套书源框架：三态打开策略（auto/download/stream）、raw/ 缓存、cover/ 封面缓存、`ByteSource` 流式抽象、会话注册表、`removeSourceWithCleanup` 清理。
- 鉴权：百度走 OAuth2 授权码（`redirect_uri=oob`，浏览器授权 → 粘贴 code）；115 走设备码 + PKCE（应用内二维码，APP 扫码）。两者都只需用户一次性授权，refresh_token 持久化，访问令牌自动刷新。
- HTTP 客户端沿用 WebDAV 的 reqwest blocking + `spawn_blocking` 模式，不引入新的大依赖（仅新增纯 Dart 的 `qr_flutter` 用于 115 二维码）。

## 2. 架构与数据流

```
Dart (BookSource type='baidu'|'115')
  |
  +- baidu  -> baidu_connect(refresh_token, client_id, client_secret, root)
  |             +- baidu_list(session, path)      按路径列目录
  |             +- open_baidu_book(session, path, strategy)
  |             |    auto: 整本下载 raw/ 缓存(进度) -> 失败回退 Range 流式
  |             |    download: 强制整本 | stream: 直链 Range 流式
  |             +- baidu_cover / baidu_download_progress / baidu_has_raw_cache
  |             +- Rust: BaiduClient { app_key, secret, tokens, dlink 取链 }
  |
  +- 115   -> cloud115_connect(refresh_token, app_id, root_id)
              +- cloud115_qr_start / cloud115_qr_poll（扫码授权用）
              +- cloud115_list(session, cid)      按文件夹 ID 列目录
              +- open_cloud115_book(session, path=fid, strategy)（同三态）
              +- cloud115_cover / progress / has_raw_cache
              +- Rust: Cloud115Client { app_id, tokens, 1~2 r/s 节流 }
```

缓存命名空间：`baidu|{app_key}|{path}`、`115|{root_id}|{fid}`（raw/ 目录 hash 沿用现有 `DefaultHasher(origin+path)` 模式）；书 key 由 `bookKeyOf(type, sourceId, path)` 生成，阅读记录/标签/搜索无需改动。

## 3. Rust 侧设计

### 3.1 新增 `src/source/baidu.rs`

- `BaiduClient`（`Send + Sync`，`Arc` 持有）：
  - 字段：`client: reqwest::blocking::Client`、`app_key`、`secret`、`refresh_token`、`access_token: Mutex<Option<(String, i64)>>`（token + 过期时间戳）、`root: String`。
  - `auth_url() -> String`：构造 `openapi.baidu.com/oauth/2.0/authorize`（scope=basic,netdisk，redirect_uri=oob）。
  - `exchange_code(code) -> Result<TokenPair>`：授权码换 token（FRB 纯函数，不建会话）。
  - `refresh()`：`grant_type=refresh_token`，更新内存 token，返回新 refresh_token 供回写。
  - `ensure_token()`：过期/缺失则刷新；请求遇 errno -6/110 自动刷新重试一次。
  - `list(dir) -> Result<Vec<Entry>>`：`method=list&web=1&limit=200` 分页；`Entry.path` 用服务端返回的完整路径。
  - `fs_id_of_path(path) -> Result<u64>`：列父目录分页查找（limit=1000，超大目录边界记录在案）。
  - `dlink(path) -> Result<String>`：filemetas 取直链（请求头带 `User-Agent: pan.baidu.com`）。
  - `probe_range(path) -> bool`：对 dlink GET `bytes=0-0` 期望 206。
  - `download_to_raw_cache(path, progress)`：dlink -> GET（UA 头）-> 64KB 分块写盘，进度更新；403/过期重取一次。
  - `read_range(path, offset, buf)`：dlink + Range + UA 头，循环读满。
- `BaiduFile` 实现 `ByteSource`：`len` 来自 filemetas size；`read_at` 委托 `read_range`。为控制复杂度，**v1 采用"会话级 dlink 缓存 + 失效重取"**（打开时取一次 dlink 存 `Mutex<Option<String>>`，403 时重取）。
- `raw_cache_path(root, path)`：复用 WebDAV 的命名/哈希模式。

### 3.2 新增 `src/source/cloud115.rs`

- `Cloud115Client`（`Arc`）：
  - 字段：`client`、`app_id`、`refresh_token`、`access_token: Mutex<Option<String>>`、`root_id`、`limiter`（简单 `Mutex<(Instant, Duration)>` 节流，默认 1 r/s，可调 2）。
  - `qr_start() -> QrPayload { uid, time, sign, qrcode }`：设备码 + PKCE；`code_verifier` 由 Rust 端 `OnceLock<Mutex<HashMap<uid, verifier>>>` 持有（轮询时匹配 uid），避免泄漏到 Dart。
  - `qr_poll(uid, time, sign) -> QrPollResult`：状态 0/1/2/-1/-2；status=2 时 `deviceCodeToToken` 换 token 并返回 refresh_token。
  - `refresh()`：`open/refreshToken`，轮换 token 并返回新 refresh_token 供回写。
  - `ensure_token()`：业务请求遇 `code==99` 或 401 开头自动刷新重试一次（对齐 SDK 行为）。
  - `list(cid) -> Result<Vec<Entry>>`：`open/ufile/files`，limit=200 分页；**文件用提取码 pc 作为浏览/打开路径（downurl 直接可用），目录用 fid（下一级列表需要 cid=fid）**，`is_dir = fc=="0"`。
  - `downurl(pc) -> String`：`open/ufile/downurl`，带 UA；单文件请求响应只有一项，**直接取 map 第一个值**（键是 fid 还是 pc 不影响）。
  - `probe_range` / `download_to_raw_cache` / `read_range`：同 Baidu（UA 头保持一致）。
- `Cloud115File` 实现 `ByteSource`（dlink 会话级缓存同上）。
- 纯函数单测：列表 JSON 解析、downurl map 解析、PKCE `code_challenge` 计算、节流逻辑。
- 注意：Rust 模块名 `cloud115`（`115` 不是合法标识符）；Dart 类型名仍为 `'115'`。

### 3.3 `src/api/source.rs` 新增（FRB 需 regenerate）

- 会话表：`BAIDU_SESSIONS`、`CLOUD115_SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<...>>>>`（复用 `NEXT` 自增）。
- 百度 API：
  - `baidu_auth_url(app_key: String) -> String`（纯函数）
  - `baidu_exchange_code(app_key, secret, code) -> TokenPair { access_token, refresh_token }`
  - `baidu_connect(refresh_token, app_key, secret, root) -> BaiduSessionInfo { id, root, capability_label="baidu" }`（连接时刷新/校验 token + `list(root)` 连通性测试；返回最新 refresh_token 供 Dart 回写）
  - `baidu_disconnect(id)`、`baidu_list(session, path) -> Vec<DirEntry>`
  - `open_baidu_book(session, path, strategy) -> BookInfo`（三态逻辑镜像 `open_webdav_book`）
  - `baidu_download_progress(session) -> f64`、`baidu_has_raw_cache(session, path) -> bool`
  - `baidu_cover(session, path, page, width, height, crop) -> PageImage`
- 115 API：
  - `cloud115_qr_start(app_id) -> QrPayload { uid, time, sign, qrcode }`
  - `cloud115_qr_poll(uid, time, sign) -> QrPollResult { status, access_token?, refresh_token? }`
  - `cloud115_connect(refresh_token, app_id, root_id) -> Cloud115SessionInfo { id, root, capability_label="115" }`
  - `cloud115_disconnect / list / open_cloud115_book / download_progress / has_raw_cache / cover`（同上）
- 错误提示（中文，镜像 WebDAV 风格）：token 失效需重新授权 / 账号被风控 / 路径不存在 / 请求频率超限 / 网络超时。

### 3.4 依赖

- 无新增 Rust 依赖（reqwest/json/anyhow/tracing 已有）。
- 新增 Dart 依赖：`qr_flutter`（纯 Dart 二维码渲染）。

## 4. Dart 侧设计

### 4.1 DB 迁移 + 模型

- `book_sources` 新增 4 列（幂等 ALTER，模式同 M5 `port` 列）：`refresh_token TEXT`、`client_id TEXT`、`client_secret TEXT`、`root_id TEXT`。
- Rust `BookSourceRow` + `load_all_sources/upsert_source` 增补字段；FRB `BookSourceDto` 同步（regen）。
- `models.dart` `BookSource` 新增：`String? refreshToken / clientId / clientSecret / rootId`；`toJson/fromJson` 可选字段；getter：`isBaidu`、`is115`、`needsSession` 加入两者；`capabilityDisplay`：百度 🟠「百度网盘」、115 🟡「115 网盘」。
- `book_repository.dart`：`loadFromSqlite/saveToSqlite/updateSource` 增补字段映射。

### 4.2 会话缓存

- 新增 `store/baidu_session.dart`、`store/cloud115_session.dart`（镜像 `webdav_session.dart` / `sftp_session.dart`）：按 sourceId 缓存会话；连接失败抛中文错误；提供「重新授权」状态透传。

### 4.3 添加/编辑书源对话框（`home_page.dart`）

- 类型选择：6 种后 SegmentedButton 放不下 -> **改用 `DropdownMenu` 选类型**（本地 / WebDAV / SMB / SFTP / 百度网盘 / 115），下方表单按类型切换（现有 4 类字段保持不变，widget 测试同步更新）。
- 百度表单：根目录（默认 `/`）、「授权登录」按钮（`url_launcher` 打开 `baidu_auth_url`，弹窗输入授权码 -> `baidu_exchange_code` -> 自动填 refresh_token）、折叠高级项（AppKey/SecretKey，留空用内置）、连通性测试（`baidu_connect` + list）。
- 115 表单：根文件夹 ID（默认 `0`）、「扫码授权」按钮（弹 QR 对话框：`cloud115_qr_start` -> `qr_flutter` 渲染 -> 2s 轮询 `cloud115_qr_poll` -> 成功自动填 refresh_token）、折叠高级项（APP ID，留空用内置）、连通性测试。
- 高级模式：直接粘贴 refresh_token（跳过授权按钮）。

### 4.4 分发点三路改造

| 位置 | 现状 | 改造 |
|---|---|---|
| `source_browser.dart` | webdav/sftp/local 分支 | 增加 `baiduList` / `cloud115List`（needsSession 分支统一） |
| `book_detail_page.dart` | webdav/sftp 分支 | 增加 `openBaiduBook` / `openCloud115Book` |
| `ai_upscale_manager.dart` | 同上 | 增加两分支 |
| `comic_cover.dart` | `bookCover` / `webdavCover` / `sftpCover` | 增加 `baiduCover` / `cloud115Cover`（session 来自各自 store） |
| 自动转 CBZ | local + smb | 不适用（无远端写能力），跳过 |
| `removeSourceWithCleanup` | 按 type 前缀清理 | baidu/115 自动纳入；联调时验证 purgeStale 不误删 |

- 打开 API 签名：`open_baidu_book` / `open_cloud115_book` 均带 `strategy` 参数（同 M5），Dart wrapper 默认 `auto`，调用点统一读 `AppSettings.bookOpenStrategy`。

## 5. 兼容性与回滚

- DB 仅加列（幂等），旧数据可读；JSON 序列化新字段全部可选。
- 回滚：删除两个 source 模块 + API + Dart 分支 + 依赖即可，不动数据层（保留列无害）。
- 已知边界：百度超大目录（>1000 条）取 fs_id 需分页；百度普通用户下载限速（SVIP 相关）；115 分享挂载不在范围；token 明文存 SQLite（与现有 password 同等保护）。

## 6. 风险与对策

| 风险 | 对策 |
|---|---|
| 115 开放平台审核不通过 / 未申请 | 前置阻塞项，需项目方先申请；降级方案：表单允许用户自填 APP ID |
| 百度应用审核被拒 | 申请「网盘基础服务」个人应用，场景为个人漫画阅读，一般可过；被拒则先支持用户自填凭证 |
| 115 限速（1 r/s）导致翻页慢 | 实现每会话节流，浏览时可放宽到 2 r/s（AList 默认 1 已稳定运行） |
| 百度 filemetas 需要 fs_id | 父目录分页查找；后续可评估 search 接口（R2） |
| dlink 失效（8h / 短时效） | 每次打开现取，失败重取一次再报错 |
| 115 downurl 响应键格式 | 以 AList SDK 代码为准（键=fid），实现时用真实账号冒烟验证 |
| token 刷新竞态 | access_token 用 Mutex 保护；刷新只发生在 ensure_token 单一入口 |
| QR 渲染依赖 | qr_flutter 纯 Dart 无原生依赖，体积小 |
| M5 遗留：purgeStale 误删 | 新增类型注册时验证清理前缀，联调清单含「删除书源不影响其他书源」 |
