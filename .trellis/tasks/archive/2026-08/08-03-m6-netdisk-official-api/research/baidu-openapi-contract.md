# 百度网盘开放平台 API 契约 — 2026-08-03

> 来源：百度网盘开放平台官方文档（pan.baidu.com/union）+ AList 百度驱动实践。RCH 需要的能力：OAuth 授权、token 刷新、列目录、取下载直链、下载（含 UA 限制）、封面。

## 1. 应用接入（前置条件）

- 注册：pan.baidu.com/union → 控制台 → 个人实名认证 → 创建应用 → 勾选「网盘基础服务」权限组（文件列表、下载等）→ 审核（一般 1~2 个工作日）。
- 获得：AppKey（client_id）、SecretKey（client_secret）。内置到 RCH，用户无需自己申请。
- 合规红线：不得多人共享开发者账号、不得用个人网盘账号做企业建站/图床/网盘迁移工具、不得未授权下载传播用户数据。

## 2. OAuth2 授权（桌面应用，redirect_uri=oob）

### 2.1 构造授权链接（打开浏览器）

```
GET https://openapi.baidu.com/oauth/2.0/authorize
    ?response_type=code
    &client_id={appkey}
    &redirect_uri=oob
    &scope=basic,netdisk
    &display=popup
```

`redirect_uri=oob`：授权完成后页面直接显示授权码（不跳转），适合桌面/CLI 应用——用户把 code 复制回 RCH。

### 2.2 授权码换 token

```
POST https://openapi.baidu.com/oauth/2.0/token
    grant_type=authorization_code
    &code={code}
    &client_id={appkey}
    &client_secret={secretkey}
    &redirect_uri=oob
```

响应：`{ access_token, expires_in, refresh_token, scope, ... }`。

### 2.3 刷新 token（access_token 过期后）

```
POST https://openapi.baidu.com/oauth/2.0/token
    grant_type=refresh_token
    &refresh_token={refresh_token}
    &client_id={appkey}
    &client_secret={secretkey}
```

响应同上（access_token 一般 30 天有效；refresh_token 长期有效，刷新失败才需要重新授权）。

## 3. 文件接口

### 3.1 列目录

```
GET https://pan.baidu.com/rest/2.0/xpan/file?method=list
    &dir={绝对路径，urlencode}
    &start=0
    &limit=200           // 建议最大不超过 1000
    &order=name
    &desc=0
    &web=1               // 1 = 返回缩略图字段
    &access_token={token}
```

响应关键字段：

| 字段 | 说明 |
|---|---|
| `errno` | 0 成功；负数为错误码（-6 认证失败需刷新；110 等见错误码表） |
| `list[].fs_id` | 文件 ID（filemetas 用） |
| `list[].path` | 完整路径 |
| `list[].isdir` | 1=目录 |
| `list[].server_filename` | 文件名 |
| `list[].size` | 大小（目录恒 0） |
| `list[].server_mtime` | 修改时间（unix 秒） |
| `list[].thumbs` | `icon/url1/url2/url3`（图片、视频类文件才返回；`web=1` 时） |
| `list[].category` | 1 视频 2 音频 3 图片 4 文档 5 应用 6 其他 7 种子 |

分页：`start` + `limit` 循环直到返回条数 < limit。

### 3.2 取下载直链（dlink）

需要文件 `fs_id`（从列目录得到）：

```
GET https://pan.baidu.com/rest/2.0/xpan/multimedia?method=filemetas
    &fsids={JSON 数组字符串，如 [123456]}
    &dlink=1
    &access_token={token}
```

响应：`list[0].dlink` 为下载直链（有效期约 8 小时）；`list[0].size`、`fs_id` 等。
**下载时必须自行拼接当前 `access_token`**：官方要求 dlink 必须带 `&access_token=xxx`，不能依赖 dlink 内嵌 token（内嵌的也可能因刷新轮换而失效 → 31045）。

> 实现注意：`open_book(path)` 只拿到路径，需要先列父目录分页找到该路径对应的 `fs_id`，再调 filemetas 取 dlink。超大目录（>1000 条）需分页查找，记录为已知边界。

### 3.3 下载

```
GET {dlink}&access_token={当前token}
Header: User-Agent: pan.baidu.com     // 官方要求必带，>50MB 文件必须，否则下载失败/限速
```

- 支持 HTTP Range（`bytes=start-end`），流式读页可用；Range 能力可在打开时对目标文件探一次（bytes=0-0 → 206）。
- 下载速度与 SVIP 等级挂钩，普通用户限速（非实现问题，文案提示）。

### 3.4 缩略图 / 封面

- 列目录 `web=1` 返回 `thumbs.url1/url2/url3`（图片/视频）。漫画 CBZ 无缩略图 → 封面走 RCH 现有解码管线（流式读第一页）。

## 4. 错误码要点

| errno | 含义 | 处理 |
|---|---|---|
| 0 | 成功 | - |
| -6 | access_token 无效/过期 | 自动 refresh 后重试一次 |
| 110 | access_token 无效 | 同上 |
| -9 | 文件不存在 | 中文提示路径不存在 |
| -10/-8 | 路径/参数错误 | 中文提示 |
| 31066 | 请求频率超限 | 节流 + 中文提示稍后再试 |
| 31119 / 31329 | 账号被风控（黑名单） | 提示账号状态异常 |

## 5. 实现要点（Rust）

- 用 reqwest blocking（与 WebDAV 一致），`spawn_blocking` 承载。
- dlink 每次打开现取（8h 有效期）；下载失败（403/过期）重取一次再试。
- token 刷新统一入口：API 返回 -6/110 时自动刷新并重试一次；刷新后的 refresh_token 回写 DB。
- 节流：列表/元数据请求加简单 pacing（百度对高频请求会限频）。
- UA 头 `pan.baidu.com` 对列表、下载直链请求统一带上（官方示例也带）。
