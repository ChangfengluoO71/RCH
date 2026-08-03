# 115 生活开放平台 API 契约 — 2026-08-03

> 来源：`xhofe/115-sdk-go`（AList 115_open 驱动所用 SDK）源码逐文件核对 + 115 官方语雀文档（yuque.com/115yun/open）。RCH 需要的能力：设备码扫码授权（PKCE）、token 刷新、列目录、取下载直链、下载、封面。

## 1. 应用接入（前置条件）

- 115 生活开放平台（开放个人云存储能力）目前为申请制：填应用类型/名称/描述/相关接口/预计用户量/每日调用次数，审核通过后获得 **APP ID（client_id）**。
- 合规红线：禁止图床、软件床、视频外链分发、多人共享开发者账号、替代 OSS 等场景；token 泄露可去 115 网页端「设备登录管理」解除授权。
- 限制：同一账号同一应用最多 2 个有效 refresh_token（第三次获取顶掉第一个）。

## 2. 鉴权：设备码 + PKCE（手机扫码）

### 2.1 获取设备码 + 二维码内容

```
POST https://passportapi.115.com/open/authDeviceCode
Form: client_id={app_id}
      code_challenge={base64(sha256(code_verifier))}
      code_challenge_method=sha256
```

响应 `data`：`{ uid, time, qrcode, sign }`（`qrcode` 即二维码内容，RCH 在应用内渲染成二维码；`code_verifier` 客户端自己生成并保留）。

### 2.2 轮询扫码状态

```
GET https://qrcodeapi.115.com/get/status/?uid={uid}&time={time}&sign={sign}
```

`data.status`：0 未扫 → 1 已扫待确认 → 2 已确认（成功）；-1 过期，-2 取消。

### 2.3 换 token

```
POST https://passportapi.115.com/open/deviceCodeToToken
Form: uid={uid}
      code_verifier={code_verifier}
```

响应 `data`：`{ access_token, refresh_token, expires_in }`。

### 2.4 刷新 token

```
POST https://passportapi.115.com/open/refreshToken
Form: refresh_token={refresh_token}
```

响应同上。**注意：115 每次刷新会轮换 refresh_token**（旧 refresh_token 失效），必须回写 DB。

### 2.5 业务请求鉴权

`Authorization: {access_token}`（Bearer 前缀以实测为准；SDK 用 resty `SetAuthToken`，等价于 Authorization 头直接放 token）。业务接口返回 `state=false` 且 `code==99` 或 401 开头 → 自动刷新后重试一次（AList 实现如此）。

## 3. 文件接口

Base URL：`https://proapi.115.com`

### 3.1 列目录

```
GET /open/ufile/files
Query: cid={父目录ID，根为 0}
       limit=200
       offset=0
       asc=1
       o=file_name
       show_dir=1
```

响应：`{ state:true, data:[...], count, offset, limit, path:[父目录树] }`；文件对象关键字段：

| 字段 | 说明 |
|---|---|
| `fid` | 文件 ID（RCH 用它作为浏览/打开路径） |
| `pid` | 父目录 ID |
| `fc` | 分类：0=文件夹，1=文件 |
| `fn` | 文件名 |
| `fs` | 文件大小 |
| `pc` | 提取码（downurl 用） |
| `sha1` | 文件哈希 |
| `upt` / `uppt` | 修改 / 上传时间（unix 秒） |
| `thumb` | 图片缩略图地址（仅图片类） |
| `uo` | 原图地址（仅图片类） |
| `ico` | 后缀名 |

分页：`offset` 递增直到取满 `count`。

### 3.2 取下载直链

```
POST /open/ufile/downurl
Form: pick_code={pc}
Header: User-Agent: {RCH UA}    // 必需，AList 会把调用方 UA 原样传给 115
```

响应为 map，**键为文件 ID（fid）**（AList 代码实证：`resp[obj.GetID()]`，GetID 返回 Fid）：`{ "{fid}": { file_name, file_size, pick_code, sha1, url: { url } } }`。

### 3.3 下载

```
GET {url}
Header: User-Agent: {同一 UA}
```

- 一般支持 HTTP Range；打开时探一次（bytes=0-0 → 206）。
- 直链短时效，每次打开现取；失败重取一次再试。

### 3.4 用户信息（连通性验证）

```
GET /open/user/info
```

返回 user_id / user_name / vip_info 等，适合添加书源时的连通性测试。

### 3.5 其它（本任务不需要）

搜索 `/open/ufile/search`、建目录 `/open/folder/add`、移动/复制/删除、回收站、上传（秒传）、离线下载。

## 4. 限速与稳定性

- AList 115_open 默认全接口限速 **1 r/s**（`limit_rate` 默认 1）。RCH 实现一个简单的节流（每会话 1~2 r/s 可配置），避免账号风控。
- 网络层：115 API 在国内可直连；直链域名稳定。

## 5. 实现要点（Rust）

- 设备码流程在 RCH 内完成：生成 `code_verifier`（随机 43~128 字符）→ 二维码渲染交给 Flutter 端（qr_flutter 纯 Dart 依赖）→ 轮询状态（间隔 ~2s）→ 换 token。
- 每会话持有 access_token/refresh_token，刷新自动回写 DB（Dart 在 connect 时保存）。
- `list(cid)` 返回的 `fid` 作为浏览路径（目录也是文件 ID），打开书时直接用 `pc` 换直链，无需额外查找。
