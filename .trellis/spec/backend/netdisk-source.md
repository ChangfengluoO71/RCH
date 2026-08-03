# 网盘书源规范（百度 / 115 官方 API）

> 覆盖 M6 网盘直连书源：鉴权、API 契约要点、缓存与会话模式、已知坑。实现参考 `app/rust/src/source/baidu.rs`、`app/rust/src/source/cloud115.rs`。

## 1. 选型结论

- 百度网盘：官方开放平台（OAuth2 授权码 + refresh_token），个人实名即可注册应用。
- 115：115 生活开放平台（设备码 + PKCE 扫码），申请制；RCH 复用 AList 同款 API 面（xhofe/115-sdk-go 契约）。
- 不做 OpenList/AList 聚合书源（夸克/PikPak 无官方 API，后续另议）。

## 2. 鉴权

### 百度
- 授权链接：`openapi.baidu.com/oauth/2.0/authorize`，`redirect_uri=oob`（桌面应用），`scope=basic,netdisk`。
- 换 token / 刷新：`openapi.baidu.com/oauth/2.0/token`（authorization_code / refresh_token）。
- refresh_token 长期有效；access_token 过期自动刷新，刷新失败才需重新授权。
- 业务请求带 `User-Agent: pan.baidu.com`（下载 >20MB 文件必须，否则失败/限速）。

### 115
- 设备码：`passportapi.115.com/open/authDeviceCode`（PKCE：sha256(verifier) → base64）。
- 轮询：`qrcodeapi.115.com/get/status/`（status 0→1→2；-1 过期，-2 取消）。
- 换 token：`open/deviceCodeToToken`；刷新：`open/refreshToken`（**每次刷新轮换 refresh_token，必须回写 DB**）。
- 业务头：`Authorization: {access_token}`（Bearer 前缀以实测为准）；接口 401 开头或 code=99 自动刷新重试一次。
- 全接口限速：默认 1~2 r/s（`RateGate`），防风控。

## 3. 文件接口

| 能力 | 百度 | 115 |
|---|---|---|
| 列目录 | `xpan/file?method=list`（path 制，`web=1` 返回缩略图） | `proapi.115.com/open/ufile/files?cid=`（ID 制，limit=200 分页） |
| 直链 | `xpan/multimedia?method=filemetas&dlink=1`（需 fs_id，约 8h 有效） | `open/ufile/downurl`（pick_code 换 url，带 UA） |
| Range | 一般支持，打开时探 bytes=0-0 → 206 | 一般支持，探出总大小（Content-Range） |
| 封面 | 走 RCH 解码管线（CBZ 无云端缩略图） | 同左 |

### 关键设计决策

- **115 路径 = 文件提取码 pc（文件）/ fid（目录）**：downurl 直接可用 pc，绕开"响应 map 键是 fid 还是 pc"的歧义（单文件请求取第一个值）。
- 百度取 dlink 需先列父目录分页找 fs_id（路径 → fs_id → filemetas），超大目录（>1000）边界已知。
- 直链会话级缓存 + 403 失效重取一次；整本下载失败回退 Range 流式（auto 策略）。

## 4. 会话与缓存

- 会话表镜像 WebDAV/SFTP：`BAIDU_SESSIONS` / `CLOUD115_SESSIONS`（`OnceLock<Mutex<HashMap<u64, Arc<Client>>>`）。
- `connect` 时刷新 token + 连通性测试，返回最新 refresh_token 由 Dart 回写 DB（书源 `refresh_token` 列）。
- raw/ 缓存命名空间：`baidu|{app_key}|{path}`、`115|{app_id}|{root_id}|{path}`（DefaultHasher 模式，与 WebDAV 一致）。
- `removeSourceWithCleanup` 按 `type|id|` 前缀清理，天然覆盖新类型；`purgeStale` 只删不存在的源/本地失效文件，不影响网盘书源。

## 5. 已知坑

- 百度 >20MB 下载不带 `pan.baidu.com` UA 会失败；普通用户限速（SVIP 相关，文案提示）。
- 115 同账号同应用仅 2 个有效 refresh_token（第三次获取顶掉第一个）。
- 115 开放平台禁止图床/软件床/外链分发/共享开发者账号；token 泄漏去 115 设备登录管理解除授权。
- token 明文存 SQLite（与现有 password 同等保护级别，风险已知）。
- 115 `cloud115_cover/open` 的 stream 分支需先探 Range 拿总大小；不支持 Range 则整本下载。
- FRB 新 API 后必须跑 `flutter_rust_bridge_codegen generate`（2.12.0），否则 Dart 侧缺函数。

## 6. 前置条件

- 百度开放平台应用（AppKey/SecretKey）与 115 开放平台应用（APP ID）注册后填入 `home_page.dart` 的 `_defaultBaiduKey/_defaultBaiduSecret/_default115AppId`；未填入时用户可在高级选项自填。
