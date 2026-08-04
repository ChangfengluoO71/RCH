# 夸克网盘书源规范（非官方 Web API，Cookie 认证）

> 覆盖 08-04-quark-book-source：夸克无官方开放平台 API，采用与 AList Quark 驱动同款的非官方 Web 接口（Cookie 认证）。实现参考 `app/rust/src/source/quark.rs` 与 `app/rust/src/api/source.rs`。

## 1. 选型结论

- 夸克网盘**无官方开放平台 API**（M6 已确认排除，见 `.trellis/tasks/archive/2026-08/08-03-m6-netdisk-official-api/`），改走被 AList / quark-auto-save / QuarkPanTool 等长期使用的**非官方 Web API**，契约以 AList `drivers/quark_uc` 为基准。
- 认证采用**粘贴 Cookie**（pan.quark.cn 登录后复制），不做扫码登录逆向（v2 候选）。

## 2. 认证

- 请求头三件套：`Cookie` + `Referer: https://pan.quark.cn` + quark-cloud-drive Electron UA；query 固定 `pr=ucpro&fr=pc`。
- 响应 Set-Cookie 中的 `__puus` 回写续期（会话内更新，Dart 侧对比后回写 DB `cookie` 列）。
- Cookie 失效/风控错误码以真机冒烟为准（目前 401/4000 映射为「登录状态失效，请重新粘贴 Cookie」，其余透出 message）。

## 3. 文件接口

| 能力 | 接口 |
|---|---|
| 连通性 | `GET https://drive.quark.cn/1/clouddrive/config` |
| 列目录 | `GET /file/sort?pdir_fid={fid}&_page=N&_size=100&_fetch_total=1&fetch_all_file=1&fetch_risk_file_name=1&_sort=file_type:asc,file_name:asc`（根目录 `0`，分页拉全；`data.list[]` 含 `fid/file_name/size/file/updated_at`） |
| 直链 | `POST /file/download` body `{"fids":[fid]}` → `data[0].download_url`（`file_name`/`size` 可选） |
| Range | `bytes=0-0` 探测：206 则流式（`Content-Range` 拿总大小），否则整本下载 raw/ 缓存回退 |
| 下载 | 直链仍需三件套头；403 重取直链一次 |

- 错误：响应 `{status, code, message, data, metadata}`，`code != 0` 即失败。

## 4. 关键设计决策

- **fid 是浏览/API/缓存键，不做格式探测**：`open_document` 的 path 参数传 **download 响应/列表缓存里的真实文件名**（`resolve_name`）。吸取 115 用提取码当 path 导致扩展名探测失败的教训。
- 列表时缓存 `fid → file_name`（会话内 `names` 表），保证历史记录直接打开也能解析文件名。
- raw/ 缓存命名空间：`quark:{root_id}:{fid}`（DefaultHasher 模式）；目录内任意非空文件即命中。
- 会话表 `QUARK_SESSIONS` 镜像 115；`quark_connect` 用 `/config` + 根目录首屏 list 做连通性测试。
- 限速：`RateGate` 约 2 r/s，防风控。

## 5. 已知坑

- 非官方接口，官方随时可能变动：错误透出 + cookie 续期 + 单点封装便于快速修复。
- 直链敏感：必须带 Cookie/Referer/UA；Range 支持需真机验证。
- Cookie 明文存 SQLite（与现有 password/refresh_token 同级保护）。
- 分享链接/上传/移动/删除/搜索不支持；扫码登录为 v2 候选。
- 若 `open_document` 前拿不到文件名会报「无法获取夸克文件名，请从书源浏览打开」——从历史记录打开且 download 响应无 `file_name` 时可能出现。

## 6. 前置条件

- 用户提供 pan.quark.cn 登录后的 Cookie（浏览器 F12 从任意请求复制）。
