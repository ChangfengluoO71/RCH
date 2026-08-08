# 115 扫码 Cookie 登录书源（无需 APP ID）

## Goal
为 115 书源新增「扫码获取 Cookie」连接方式：用户用 115 手机 App 扫码即可获得 Web 登录 Cookie，走 115 网页版接口（webapi/proapi）连接书源，绕开等待 APP ID 申请（约 7 天）。官方 APP ID 模式保留，二选一。

## Requirements
- Rust（cloud115.rs）：Web 扫码三接口（取二维码 / 轮询 / 换 Cookie，默认设备 web），新增 Cloud115WebClient（列表多域名 fallback / chrome/downurl 加密直链 / Range 探测与读取 / 整本下载 raw 缓存 / 文件名缓存）
- 下载统一走 proapi.115.com/app/chrome/downurl（m115 RSA+XOR 加密，无 200MB 上限）
- 错误映射：Cookie 失效（errno 990001/40101032/code 99）、风控 405、二维码过期/取消、直链 403、登录被顶下线等，提示文案明确
- FRB 导出 + codegen.ps1；Dart 添加书源对话框增加「扫码获取 Cookie」入口、Cookie 字段、扫码设备选择（默认 web）
- 会话层 cloud115_session.dart 按 BookSource.cookie 是否非空自动分流 cookie / open 模式，调用点不改
- 保存 cookie 到 BookSource.cookie，与现有夸克 Cookie 同级保护

## Acceptance Criteria
- [ ] cargo build / flutter analyze / flutter test 通过
- [ ] 官方 APP ID 模式回归不受影响
- [ ] 115 书源可通过扫码获取 Cookie 添加并浏览/打开（真机验证项，网络受限环境无法全流程验证）
- [ ] spec 文档更新（backend/netdisk-source.md 补充 115 Cookie 模式契约）
