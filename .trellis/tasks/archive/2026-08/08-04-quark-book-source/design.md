# 夸克网盘书源 — 技术设计

## 1. 结论摘要

- 新增书源类型 `'quark'`：**粘贴 Cookie 认证**，浏览 / 打开 / 封面 / 缓存 / 凭据持久化全部复用现有书源框架，改动面与 115（M6）相当。
- Rust 侧：新增 `src/source/quark.rs`（`QuarkClient` + `QuarkFile` 实现 `ByteSource`）+ `api/source.rs` 会话表与 `quark_*` API；DB 仅加 `cookie` 列。
- Dart 侧：`models.dart` / `book_repository.dart` / `store/quark_session.dart` / `home_page.dart` 表单 / 4 个分发点（`source_browser`、`book_detail_page`、`ai_upscale_manager`、`comic_cover`）/ widget 测试。
- **与 115 的关键差异**：115 把提取码当 path 传给 `open_document`（扩展名探测会失败，115 联调未完成、属遗留隐患）；夸克改用 **download 响应/列表中的真实文件名**做格式探测，fid 仅作 API 与缓存键。

## 2. 夸克 API 契约（非官方，实现前需真机冒烟确认）

- base：`https://drive.quark.cn/1/clouddrive`；请求头：`Cookie` + `Referer: https://pan.quark.cn` + quark-cloud-drive Electron UA；query 固定 `pr=ucpro&fr=pc`。
- `GET /config` — 连通性 / 凭据校验。
- `GET /file/sort?pdir_fid={fid}&_page=N&_size=100&_fetch_total=1&fetch_all_file=1&fetch_risk_file_name=1&_sort=file_type:asc,file_name:asc` — 分页列目录；响应 `data.list[]`（`fid` / `file_name` / `size` / `file` 布尔 / `updated_at` 毫秒）+ `metadata._total`；根目录 `pdir_fid=0`。
- `POST /file/download` body `{"fids":[fid]}` — 取直链，响应 `data[0].download_url`（`file_name` / `size` 为可选字段，用于格式探测与缓存命名）。
- 下载：直链需带 Cookie + Referer + UA；`Range: bytes=0-0` 返回 206 则流式可用（`Content-Range` 可拿总大小），否则整本下载 raw/ 缓存。
- 错误：响应 `{status, code, message, data, metadata}`，`status>=400 || code!=0` 即失败；登录失效 / 风控的具体 code 在步骤 0 冒烟时记录并映射中文提示；响应 Set-Cookie 中的 `__puus` 回写续期（尽力而为）。

## 3. Rust 侧设计

### 3.1 新增 `src/source/quark.rs`

- `QuarkClient`（`Arc` 持有，`Send + Sync`）：
  - 字段：`client: reqwest::blocking::Client`、`cookie: Mutex<String>`、`root: String`（根 fid）、`gate: RateGate`（约 2 r/s，防风控）。
  - `origin() -> String`：`format!("quark:{root}")`（cookie 会轮换，缓存命名空间只用 root，稳定）。
  - `request(...)` 统一封装：带 Cookie/Referer/UA + `pr=ucpro&fr=pc`；`code != 0` 报错（中文映射）；从响应 Set-Cookie 回写 `__puus` 到 `cookie`。
  - `list(fid) -> Result<Vec<Entry>>`：`/file/sort` 分页拉全，`Entry { name: file_name, path: fid, is_dir: !file, size, mtime: updated_at/1000 }`；目录在前 + `natural_cmp` 排序（沿用 115 约定）。
  - `downlink(fid) -> Result<DownloadInfo { url, size: Option<u64>, name: Option<String> }>`：`/file/download`，取 `data[0]`。
  - `probe(url) -> (bool, u64)`：`Range: bytes=0-0`；206 → `(true, Content-Range 总大小)`；200 → `(false, content_length)`。
  - `read_range(dlink, fid, offset, buf)`：Range + 三件套头；403 时重取 dlink 重试一次。
  - `download_to_raw_cache(fid, name, progress)`：整本下载写 `raw/{hash}/{name}`（hash = `quark:{root}:{fid}`，沿用 baidu 的 `DefaultHasher` 模式）；已缓存复用并更新进度。
  - `raw_cache_path(origin, fid) -> Option<PathBuf>`：与 baidu 同名 helper。
- `QuarkFile` 实现 `ByteSource`：`{ client, fid, len, dlink: Mutex<Option<String>> }`；`read_at` → `read_range`，失败清 dlink 重取一次（镜像 `BaiduFile`）。
- 单测：sort 列表 JSON 解析、download 响应解析、`probe` 尺寸解析、错误码映射、raw 缓存命名 / fid 处理、分页参数构造。

### 3.2 `src/api/source.rs` 新增（FRB 需 regenerate）

- `QUARK_SESSIONS` / `QUARK_DOWNLOADS`：`OnceLock<Mutex<HashMap<u64, Arc<QuarkClient>>>>` + getter（镜像 baidu/115）。
- API：
  - `quark_connect(cookie, root_id) -> QuarkSessionInfo { id, root, capability_label: "quark" }`：`spawn_blocking` 内 `GET /config` + `list(root)` 连通性测试；root 为空默认 `"0"`。
  - `quark_disconnect(id)` / `quark_list(session, fid) -> Vec<DirEntry>`。
  - `open_quark_book(session, fid, strategy) -> BookInfo`：三态逻辑镜像 `open_cloud115_book`；**`open_document` 的 path 参数传真实文件名**（downlink 的 `name`，缺失时回退 `raw 缓存文件名` / `"file.cbz"`），fid 仅作 API / 缓存键。
  - `quark_download_progress(session) -> f64` / `quark_has_raw_cache(session, fid) -> bool`。
  - `quark_cover(session, fid, page, width, height, crop) -> PageImage`：cover/ 磁盘缓存 → raw/ 本地缓存 → 流式解码第一页（镜像 `cloud115_cover`）。
  - `cache_ns = "quark|{origin}|{fid}"`；错误提示中文（登录失效 / 风控 / 路径不存在 / 频率限制 / 超时）。

### 3.3 DB 迁移（`src/db/mod.rs`）

- `book_sources` 加 `cookie TEXT` 列：`PRAGMA table_info` 幂等 `ALTER`（模式同 `port` / `refresh_token` 列）。
- `BookSourceRow` 补 `cookie: Option<String>`；`load_all_sources` / `upsert_source` 的 SQL 与映射同步。
- 依赖：无新增（`reqwest blocking/json/rustls-tls` 已有）。

## 4. Dart 侧设计

### 4.1 `store/models.dart`

- `BookSource` 加 `String? cookie`；类型注释与 getter：`isQuark`、`needsSession` 加 `'quark'`、`capabilityDisplay` → `(🟡, '夸克网盘')`；`toJson/fromJson` 可选字段。
- 字段映射：Cookie → `cookie`；根目录 fid → `rootId`（默认 `'0'`，镜像 115）；浏览/打开路径 = fid 存 `path`。

### 4.2 `repository/book_repository.dart`

- `updateSource` 加 `cookie` 参数；`loadFromSqlite` / `saveToSqlite` 映射 `dto.cookie`。

### 4.3 `store/quark_session.dart`（镜像 `cloud115_session.dart`）

- `quarkSessionFor(source)`：按 sourceId 缓存；`quarkConnect(cookie: source.cookie ?? '', rootId: source.rootId ?? '0')`；会话建立失败抛中文错误。
- `clearQuarkSession(sourceId)`：书源被编辑（Cookie 变更）/删除后失效，下次自动重连。

### 4.4 `ui/home_page.dart`

- 添加对话框：`DropdownMenu` 加 `'quark'`（夸克网盘）；表单字段：Cookie（必填，密码样式多行）+ 根文件夹 ID（默认 `0`）；`_submitQuark` → `quarkConnect` 连通性测试 → `addSource(id: 'quark_…', cookie, rootId, path: s.root)`。
- 编辑对话框 quark 分支：显示 Cookie + 根文件夹 ID；保存 `updateSource(cookie:…, rootId:…)` 并 `clearQuarkSession`。
- `_sourceTypeLabel` 加 `'quark' => '夸克网盘'`；列表图标分支同步。

### 4.5 分发点（4 处）

| 位置 | 改动 |
|---|---|
| `source_browser.dart` | `needsSession` 分支 + `list` switch 加 `'quark' => quarkList(session, path)` |
| `book_detail_page.dart` | `'quark' => openQuarkBook(session, path=fid, strategy)` |
| `ai_upscale_manager.dart` | `'quark'` 分支（同 115） |
| `comic_cover.dart` | `quarkHasRawCache` / `quarkCover` 分支 |

- 书 key / 记录 / 元数据沿用 `bookKeyOf('quark', sourceId, fid)`，`removeSourceWithCleanup` 按 `'quark|{id}|'` 前缀天然覆盖，无需改清理逻辑（回归时验证 `purgeStale` 不误删）。

### 4.6 测试

- `test/add_source_dialog_test.dart`：由 6 类型改为 7 类型，quark 表单字段随类型切换。
- Rust 单测覆盖 JSON 解析 / fid 处理 / 缓存命名 / 错误映射。

## 5. 兼容性与回滚

- DB 仅加列（幂等 ALTER），旧数据可读；未配置 cookie 的书源不受影响；重启不重复加列。
- 回滚：删除 `quark.rs` / API / Dart 分支即可，`cookie` 列保留无害（模式同 M6）。
- 已知边界：Cookie 明文存 SQLite（与现有 password / refresh_token 同级保护）；Cookie 失效需手动重贴；分享链接 / 上传 / 移动 / 删除 / 搜索不支持；风控限速提示。

## 6. 风险与对策

| 风险 | 对策 |
|---|---|
| 直链 Range 支持未定 | 步骤 0 真机冒烟（Range 206）；不支持则整本下载 raw/ 回退（已内建） |
| 登录失效 / 风控错误码未知 | 步骤 0 记录实际 code/message，映射中文提示；`__puus` 续期 |
| 非官方接口随官方变动 | 错误透出 + cookie 续期 + 集中封装便于快速修复；不依赖第三方库 |
| 115 式 fid 路径打开隐患 | 夸克 `open_document` 传真实文件名（download 响应），fid 只做 API/缓存键 |
