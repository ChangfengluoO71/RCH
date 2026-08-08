# 115 网页扫码 Cookie 书源规范（非官方 Web API，Cookie 认证）
> 覆盖 08-08-115-cookie-qr-login：115 书源新增「扫码获取 Cookie」连接方式，无需等待 APP ID 申请。
> 与 AList 115 驱动 / p115client / SheltonZhu/115driver 同款非官方契约。实现参考 `app/rust/src/source/cloud115.rs`
> （`Cloud115WebClient` + `web_qr_*`）与 `app/rust/src/api/source.rs`（`cloud115_cookie_*`）。

## 1. 选型结论

- 115 官方开放平台需要申请 APP ID（约 7 天），为绕开等待新增**网页扫码获取 Cookie** 方式：
  用户用 115 手机 App 扫码 → 换取 Web 登录 Cookie → 走 115 网页版接口连接书源。
- 官方 APP ID 模式（`Cloud115Client`，open API）保留，二者二选一：
  Dart 侧 `cloud115SessionFor` 按 `BookSource.cookie` 是否非空自动分流，调用方无感知。
- 两种方式**都没有 200MB 下载上限**（200MB 限制只存在于 115 网页版简易下载接口，RCH 未使用）。

## 2. 认证（扫码三接口）

| 步骤 | 接口 | 说明 |
|---|---|---|
| 取二维码 | `GET qrcodeapi.115.com/api/1.0/web/1.0/token/` | 返回 `data.uid/time/sign/qrcode`，无鉴权 |
| 轮询 | `GET qrcodeapi.115.com/get/status/?uid&time&sign` | 0 等待 / 1 已扫 / 2 已登录 / -1 过期 / -2 取消 |
| 换 Cookie | `POST passportapi.115.com/app/1.0/{app}/1.0/login/qrcode/`，form `app=&account=uid` | `data.cookie` 键值对 → 拼 `k=v; k2=v2`，**末尾不带 `;`** |

- 可用设备白名单：`web / android / ios / tv / alipaymini / wechatmini / qandroid`（Windows/Mac/Linux 客户端已下架，直接拒绝）。
- **默认 `wechatmini`**（冷门设备，避免挤掉网页端/App 旧登录；同设备新登录会顶掉旧的）。
- 业务请求必须带 `Cookie` + 浏览器 UA（`WEB_UA`），否则 403/风控。

## 3. 文件接口

| 能力 | 接口 |
|---|---|
| 列表 | `GET webapi.115.com/files`（备用 `http://web.api.115.com/files`、`https://aps.115.com/natsort/files.php`），参数 `aid=1&cid&o=user_ptime&asc=0&offset&show_dir=1&limit=200&snap=0&natsort=0&record_open_time=1&format=json&fc_mix=0` |
| 直链 | `POST proapi.115.com/app/chrome/downurl?t={ms}`，form `data={m115 加密}`（**无 200MB 上限**）；UA 置空字符串 + `Referer: https://proapi.115.com` |
| Range | `bytes=0-0` 探测，206 流式（Content-Range 取总大小），否则整本下载 raw/ 缓存 |
| 下载 | 直链需 Cookie + UA；403 重取直链一次 |

- 列表响应：`{state, code, count, data:[{fid, cid, n, s, t, pc}]}`；**目录项 `fid` 为空**（用 `cid` 进入下一层），
  文件项用 **`pc`**(pickcode) 取直链（**`u` 是缩略图，不是 pickcode**——曾写错导致文件全被过滤）；`s` 可能是数字或字符串；
  `t` 目录为 unix 秒、文件为 `"2006-01-02 15:04"`（按 UTC+8）。
- 直链响应：`{state, data: "<base64>"}`，m115 解密后 `{pickcode: {file_name, file_size, url: {url}}}`。
- 根目录约定：`root_id` 留空 = 网盘根目录（默认）；填 `cid` 只挂载该文件夹（网页端 URL `cid=` 后的数字）。

## 4. 关键设计决策

- **路径 = pickcode（文件）/ cid（目录）**，与官方模式一致；`resolve_name` 用列表缓存或直链响应取真实文件名，
  避免历史记录打开时扩展名探测失败（沿用夸克教训）。
- raw/ 缓存命名空间：`115web:{root_id}`（Cookie 会变，只用 root 保持稳定）。
- m115 加密（`proapi chrome/downurl` 专用）**采用 p115client/p115cipher（当前活跃维护）固定 key 方案**，
  与 115driver 随机 key 的旧 m115 已不一致（曾导致 403 invalid signature / 解密乱码）：
  - 请求：payload JSON XOR 固定 4 字节 key `\x8d\xa5\xa5\x8d` → 字节反转 → XOR 12 字节 client key
    → 前置 16 字节全 0 随机串 → RSA（固定 1024 位公钥，e=65537）按 **128 字节分块** → Base64；
  - 响应：RSA 同公钥 128 字节分块 modpow → 前 16 字节为随机串，经 `m115_xor_derive_key(k, 12)` 派生 key
    → XOR → 字节反转 → XOR 固定 key；
  - 加密与解密必须使用相同分块（曾误写 256 导致响应乱码，有固定向量测试护栏）；
  - payload 必须含 `user_id`（从 Cookie `UID=` 取 `_` 前数字）。
- 取链与下载的请求头必须完全一致（UA 置空 + `Referer: https://proapi.115.com`），否则 403 `invalid signature`。
- 列表多域名 fallback：`webapi.115.com/files` 带参可能被阿里云 WAF 拦成 HTTP 405，自动换备用域名。
- 错误映射：errno 990001 / 40101032 / 40101033 / code 99 → 「登录状态已失效，请重新扫码获取 Cookie」；
  990002 → 请求过于频繁；990004 → 风控；HTTP 405 → 风控拦截提示。
- 列表解析跳过空 pickcode/cid 条目（warn 日志），Dart 侧 `.zip` 判重加 `path.length < 4` 防御。

## 5. 已知坑

- 非官方接口可能随时变动：错误透出 + cookie 续期 + 单点封装便于快速修复。
- Cookie 失效无 refresh 机制（不像官方模式轮换 refresh_token），失效时 UI 提示重新扫码。
- 扫码设备注意：默认 `wechatmini`；选 `web` 会顶掉网页端登录；提示文案已写明。
- 直链敏感：必须带 Cookie；UA/Referer 与取链一致；Range 支持需真机验证。
- Cookie 明文存 SQLite（与现有 password/refresh_token 同级保护）。
- 分享链接/上传/移动/删除/搜索不支持（与夸克一致）。

## 6. 前置条件

- 用户提供 115 手机 App（扫码）即可；无需 APP ID。
- 真机已验证：扫码全流程、240 文件目录浏览、直链解密、打开漫画。
