# OpenList 主流网盘驱动连接方法（百度/夸克/阿里/115/PikPak）— 2026-08-03

> 结论先行：OpenList 与 AList 驱动 100% 兼容，以下连接方法两者通用。这五家共 3 种凭证形态（OAuth2 token / Cookie / 账号密码），以及各自的直链下载约束——这两点直接决定 RCH 原生 Alist 书源（方案 A）的添加对话框和下载链路设计。

| 网盘 | 驱动名 | 凭证类型 | 获取方式 | RCH 直链下载注意 |
|---|---|---|---|---|
| 百度网盘 | Baidu Netdisk（百度网盘） | OAuth2 refresh_token | OpenList 令牌工具 api.oplist.org 或 AList 官方工具授权 | >20MB 文件必须带 `User-Agent: pan.baidu.com` 请求头 |
| 夸克网盘 | Quark（夸克网盘）/ QuarkTV | Cookie（TV 版扫码自动填） | Chrome F12 抓 Cookie；根目录 ID=0 | 原始直链不稳定，本地代理最稳；视频可开转码地址 |
| 阿里云盘 | Aliyundrive Open（阿里云盘 Open） | OAuth2 refresh_token | 令牌工具 APP 扫码（须移动端 token） | 桌面 Web token 直链下载/预览会失败；referer 受限 |
| 115 | 115 Cloud（115 网盘） | Cookie / 二维码令牌 | AList 界面扫码、115ToAlist 插件或 Python 脚本 | 下载必须携带 Cookie（AList 返回签名直链） |
| PikPak | PikPak | 用户名 + 密码（OAuth2） | 直接填账号密码，保存自动刷 refresh_token | 请求出口 IP 需与 OpenList 部署 IP 一致 |

---

## 1. 百度网盘

**添加存储**：驱动选「百度网盘」，填写挂载路径（如 `/baidu`）。

**刷新令牌（必填）**：
- OpenList 官方令牌工具：[https://api.oplist.org](https://api.oplist.org) → 网盘选择「百度网盘」→ 勾选「使用 OpenList 提供的参数」→ 点击获取 Token → 百度授权登录 → 复制返回的 refresh_token。
- 或使用 AList 官方页面提供的获取链接（跳转百度 OAuth 授权）。

**其余字段**：
- 客户端 ID / 密钥：留空使用内置默认（AList 内置 `hq9yQ9w9kR4YHj1kyYafLygVocobh7Sf` / `YH2VpZcFJHYNnV6vLfHQXDBhcE7ZChyE`；OpenList 勾选「使用 OpenList 提供的参数」同理）。
- 根文件夹 ID：默认 `/` 挂整个网盘；想挂子目录按 `/文件夹A/子文件夹` 格式填。
- 下载接口：选 `Official`（官方）；`Crack` 非官方接口已不可用。

**坑**：
- 百度 API 限制：下载 >20MB 的文件，直链请求必须带请求头 `User-Agent: pan.baidu.com`，否则下载失败或限速（SVIP 速度好）。RCH 走 raw_url 直链时请求头必须加上。
- 网页 302 播放/下载同样依赖 UA 修改，或走本地代理中转。

## 2. 夸克网盘

**驱动「夸克网盘」**：
- Cookie：Chrome 按 F12 → 网络（Network）→ 任选一个请求 → 复制请求头里的 `Cookie` 完整值。**必须用 Chrome**，Firefox 抓的 Cookie 会被识别为访客并提示登录。
- 根文件夹 ID：根目录为 `0`；子目录进入文件夹后从顶部地址栏取目录 ID。
- 可选「使用转码地址」：仅对视频生效，返回夸克转码播放地址并支持 302；不可用时自动回退普通下载地址。

**备选「夸克TV / QuarkTV」**：驱动列表切换成表格视图后显示二维码，手机夸克 APP 扫码，Refresh token / Device id / Query token 自动填充（勿手动改）。TV 版仅支持浏览和下载，接口不支持其它操作。

**坑**：夸克原始下载链路依赖请求头、账号状态和服务带宽，**本地代理（OpenList 中转）兼容性最好**；直接 302 原始直链不稳定。对 RCH 而言，stream 打开策略可能频繁失败，auto/download（整本下载）更可靠。

## 3. 阿里云盘

**驱动「阿里云盘 Open」**（官方 API，推荐；旧的「阿里云盘」驱动不稳定、随时可能被屏蔽）。

**刷新令牌（必填）**：用令牌工具扫码获取——手机阿里云盘 APP 扫码授权后返回 refresh_token。**必须使用移动端来源的 token**（阿里云盘 referer 限制，桌面 Web token 直链下载和预览会失败；除非开本地代理中转）。

**其余字段**：
- Oauth 令牌链接：原默认 `https://api.nn.ci` 已被阻断；如连接失败改为 `https://api.alistgo.com/alist/ali_open/token`。
- 客户端 ID / 密钥：留空使用 AList/OpenList 内置。
- 根文件夹 ID：默认 `root` 展示全部；填 file_id（网页进入文件夹后 URL 末尾字符串）只挂该文件夹。
- 云盘类型：默认 / 资源库 / 备份盘（阿里云盘 6.0 后 OpenAPI 仍区分资源库与备份盘）。

**坑**：
- 禁止公开分享或多 IP 访问，否则账号冻结风险（自建自用没问题）。
- 在线播放有容量超限限制（ExceedCapacityForbidden）。
- token 泄露后需在授权管理中解除授权重新获取。

## 4. 115

**驱动「115 网盘」**。Cookie 有三种获取方式：

**a. AList 界面扫码（最省事）**：
1. 点击「获取二维码」→ 115 手机 APP 扫码确认。
2. 点击「使用 115 网盘 APP 扫描」获取 Token。
3. 将 Token 填入「二维码令牌」，选择 Qrcode 源设备（web / android / ios / tv / alipaymini / wechatmini / qandroid），保存后自动换取并填入 Cookie。
4. 不推荐选 web / android / ios（会把日常使用的设备挤下线）；alipaymini / wechatmini 等冷门设备优先。

**b. 手动抓 Cookie**：浏览器 F12 从接口请求中抓取，或用 115ToAlist 插件自动同步；注意 Cookie 结尾不要带分号。Windows/Mac/Linux 客户端已被官方下架，抓到的这三个设备的 Cookie 无效。

**c. Python 脚本扫码**：AList 官方文档内置完整脚本（qrcodeapi.115.com + passportapi.115.com），终端输出 Cookie，`python main.py wechatmini` 可指定设备（脚本存于官方文档页面）。

**根文件夹 ID**：115 网页进入文件夹后，URL 中 `cid` 后面的数字。

**坑**：下载必须携带 Cookie；大目录按页获取；上游提供缩略图时 AList 会透出给前端（对封面友好）。直链为带签名的下载地址。

## 5. PikPak

**驱动「PikPak」**：
- 用户名：邮箱或手机号；密码：账号密码。
- 刷新令牌方式选 `Oauth2`，保存后自动填充 refresh_token、设备信息（无需手动获取）。
- 根文件夹 ID：通过 [https://mypikpak.com](https://mypikpak.com) 获取，默认 `root`。
- 平台字段：正常情况不需要；遇到验证码等问题再参考 AList PR #7024 的更新内容。

**坑**：
- PikPak 国内禁止访问——部署 OpenList 的机器需要能访问国外网络。
- **个人盘"谁发出请求谁能用"**：播放/下载请求的出口 IP 需与 OpenList 部署 IP 一致（用户自建在同一网络下没问题；若 OpenList 在远端服务器，RCH 直链下载会失败，需要本地代理）。
- 用 Google/FB 等第三方快捷注册的账号无法直接密码登录，需先在账号设置里绑定邮箱并设置登录密码。
- 分享挂载有大小限制（超出后只能播放 40%~50%），个人盘无此限制。

---

## 6. 对 RCH 原生 Alist 书源（方案 A）的设计影响

1. **凭证形态（添加对话框）**：五家三种凭证——百度/阿里是 refresh_token，夸克/115 是 Cookie，PikPak 是账号密码。建议书源表单按驱动类型切换输入项，并在界面上内嵌"如何获取"指引（或帮助文案），否则用户无法独立完成配置。
2. **raw_url 直链下载约束**（逐网盘）：
   - 百度：>20MB 请求头必须带 `User-Agent: pan.baidu.com`。
   - 阿里：token 必须为移动端来源；referer 受限。
   - 115：直链带签名，一般无需额外头；个别场景需要 Cookie。
   - 夸克：原始直链最不稳，stream 打开策略易失败 → auto/download 优先。
   - PikPak：出口 IP 绑定，自建同网络场景无碍。
3. **打开策略**：三态（auto/download/stream）继续复用；auto 的"整本下载失败回退 Range 流式"对夸克的实际意义有限，但仍可保留。
4. **分页**：115 等大目录按页获取，`/api/fs/list` 循环翻页直到 `has_more=false`（与现有调研一致）。
5. **封面**：115 有缩略图透出（thumb），其余按现有 Alist 封面的 thumb 字段统一处理。

## 7. 信息来源

- AList 官方文档驱动页：百度 / 夸克 / 阿里云盘 / 阿里云盘 Open / 115 / PikPak（alistgo.com/zh/guide/drivers/）
- OpenList 令牌工具：[https://api.oplist.org](https://api.oplist.org)
- OpenList 官方仓库 DeepWiki：Cloud Storage Drivers / Baidu Netdisk Driver
- 社区教程（OpenList + 百度网盘、夸克、115 挂载实测）
