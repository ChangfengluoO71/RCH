# SMB 与 SFTP 书源可靠性设计

**状态：** 用户已批准，正在实施。

## 问题与目标

SMB 目前把 UNC 路径当成本地文件系统路径处理：连接表单没有用户名或密码，Android 上会把 UNC 交给 std::fs，因此无法建立 SMB 会话。SFTP 当前无条件接受服务器主机密钥，只支持密码认证。目标是让 SMB 成为真正的远程只读书源，并让 SFTP 在认证前校验主机身份，支持密码或私钥登录。

## 调研：成熟产品的做法

- Android 文件管理器 X-plore 把 SMB 作为可保存的 LAN 连接档案，支持手动添加服务器；用户名或密码可以留空并在连接时输入。它的 SSH 文档将账号密码和私钥认证作为两种独立方式，并说明私钥可保存在应用安全存储中、加密私钥可要求输入口令。[X-plore 产品说明](https://www.lonelycatgames.com/apps/xplore) · [X-plore SSH 文档](https://www.lonelycatgames.com/docs/xplore/ssh)
- OpenSSH 的交互式 ask 模式会在首次遇到新主机密钥时征求用户确认，并拒绝密钥变更；accept-new 会自动记录首次密钥，但仍拒绝已知主机的密钥变更。我们的 UI 应采用显式指纹确认，并对变更 fail closed。[OpenBSD ssh_config](https://man.openbsd.org/ssh_config.5)
- FileZilla 按站点配置主机、端口和私钥文件，并在私钥有口令时提示解锁。这支持把“服务器认证密钥”和“SFTP 登录认证方式”作为两个清晰、互不混淆的设置。[FileZilla 私钥连接文档](https://filezillapro.com/docs/v3/basic-usage-instructions/connect-to-sftp-with-a-private-key-in-filezilla/)
- WinSCP 在首次遇到主机密钥时展示指纹并等待用户接受；接受后才记住该密钥，密钥变更会在发送密码或其他敏感信息前阻断连接。它也提供复制指纹的操作。我们的首次信任和轮换对话框采用同一顺序，并提供复制，避免用户手抄长指纹。[WinSCP 首次主机密钥提示](https://winscp.net/eng/docs/message_host_key) · [WinSCP 主机密钥校验](https://winscp.net/eng/docs/ssh_verifying_the_host_key)
- Total Commander Android 将 SFTP 与 LAN/SMB 作为不同连接插件，并在名称解析故障排查中建议尝试数字 IP。我们的协议专属表单已分开 SMB 与 SFTP；DNS 失败提示可补充“尝试服务器 IP”作为实用恢复指引。[Total Commander Android 插件说明](https://www.totalcommander.ch/android/plugins.htm)

采用的交互：SMB 保存服务器连接档案，显式显示服务器、端口、共享路径、用户名和密码；凭据留空表示尝试 Guest。SFTP 显示 SHA-256 主机指纹供用户确认和固定，登录方式可选密码或私钥；密钥文件由系统文件选择器导入。首次确认后不再打扰，主机密钥变化则拒绝连接并要求用户明确替换信任。暂不增加 SMB 局域网发现或每次连接都询问凭据，以避免把扫描和后台恢复变成模态交互。

## 技术选择

### SMB

在 Rust 书源层接入纯 Rust SMB2/3 客户端，优先评估 smb2 crate。其客户端没有 C/FFI 依赖；FileReader 的 read_at(offset, len) 是并发安全的定位读取，直接匹配项目 ByteSource 的随机读取需求。[smb2 crate](https://docs.rs/smb2/latest/smb2/index.html) · [FileReader API](https://docs.rs/smb2/latest/smb2/client/stream/struct.FileReader.html)

Pavão 包装成熟的 libsmbclient，但依赖本机 Samba C 库，而且当前文档说明它会序列化进程中的 SMB 操作；这会提高 Android 多 ABI 打包成本并限制并发。[Pavão 文档](https://docs.rs/pavao/latest/pavao/struct.SmbClient.html) 另一纯 Rust smb crate 也支持随机读取，但 smb2 的 FileReader API 与现有 ByteSource 更直接。[smb crate](https://docs.rs/smb/latest/smb/resource/file/struct.File.html)

SMB 只协商 SMB2/3，不启用 SMB1。所有网络 I/O 通过 Rust 会话执行，不将 SMB 路径交给 Dart File、Rust std::fs 或 Windows UNC 文件 API。

配置契约：
- url 存服务器主机名/IP；port 默认 445。
- path 以共享名开头，后续为共享内的相对目录，例如 comic/manga。用户名、密码复用 BookSource 字段；用户名支持 DOMAIN\user 格式。
- 空用户名和密码尝试 Guest。初版只读，不实现共享写入、删除或上传。
- 旧 BookSource.path 为 \\server\share\folder 且 url 为空时，在加载/连接边界解析为服务器、共享名和相对目录，不要求用户手工重建源。无法解析的旧值给出可操作错误。
- 新建 SMB 源表单拆分服务器地址、端口、共享/初始目录、用户名和密码，并在保存前验证连通性和目录读取权限。
- 旧 SMB 源或新建的访客源遇到 Guest 认证失败时，前台显示一次账号/密码提示；用户可选择保存到书源（默认，走 vault）或仅用于当前会话。取消不保存并返回明确的认证错误。后台扫描不弹窗、不反复尝试。
- SMB 改为 needsSession；isLocalFs 仅代表本地文件系统。浏览、目录快照、索引、封面扫描、漫画打开、封面读取和随机读都使用同一个 SMB 会话路径。remote-scan adapter 纳入 SMB。
- Rust 会话持有客户端和所需的 Tokio runtime；SmbFile 实现 ByteSource，提供 len 与定位读取；关闭源会话时释放文件句柄。

### SFTP

- 增加不发送任何登录凭据的主机密钥探测 API，返回 SHA-256 指纹。
- 新主机：界面展示主机、端口及完整指纹；用户确认后才保存 hostKeyFingerprint 和其 endpoint 绑定，再使用固定指纹连接。
- 已有旧 SFTP 源没有 pin：前台连接时执行同样的确认流程。后台扫描不弹窗；缺少 pin 时停止该次连接并保留可恢复状态。
- 已固定主机出现指纹变化：认证前拒绝连接；展示旧、新指纹，并提供显式替换动作。取消或失败绝不自动覆盖 pin。
- 新增密码/私钥认证选择。私钥用 Flutter 的 file_selector 读取为内存内容，再放入 CredentialVault；支持 russh 可解码的 OpenSSH/PEM 格式，不承诺 PuTTY PPK。加密私钥口令与私钥一同经保险库存储。选择文件路径不写入数据库、日志或备份。
- hostKeyFingerprint 与 hostKeyEndpoint 是非敏感源设置，加入 BookSource、SQLite 迁移、JSON 元数据和 FRB DTO；它们与现有 source fingerprint（目录身份）分开。
- SFTP 会话只在主机指纹匹配后执行密码或公钥认证；现有 raw-cache 命名规则保持不变。

### 跨平台凭据

现有 CredentialVault 只在 Android 启用。保留 Android 当前 Keystore MethodChannel；Windows/macOS/Linux/iOS 用 flutter_secure_storage 对接平台安全存储。BookRepository 和 SyncManager 对所有原生平台使用 vault；web 行为保持当前实现。旧 SQLite/JSON 中已有的敏感字段在读取后迁入 vault，成功回读校验后才清空 SQLite；任何 vault 错误都不得退回为明文写入。原生平台的 library.json 备份不包含密码、令牌、cookie、私钥或私钥口令。Windows CI/build 作为必要验证；Linux 官方插件运行时依赖 libsecret，macOS 需要按插件文档配置 Keychain 权限。

## 文件与边界

- Rust：新增 source/smb.rs；扩展 source/mod.rs、api/source.rs 和 api/remote_scan.rs；BookSource DTO 与 SQLite 迁移增加 SFTP pin 字段。
- Flutter：新增 store/smb_session.dart 和 SFTP 信任提示/私钥导入流程；修改 store/models.dart、store/credential_vault.dart、repository/book_repository.dart、store/sync_manager.dart、store/library_index_service.dart、store/book_open_coordinator.dart、ui/home_page.dart、ui/source_browser.dart，以及所有将 smb 判为 local 的调用点。
- FRB 生成文件由项目 codegen 命令生成，不手改生成内容。
- 数据边界验证集中在 SourceConfig/helper；Rust API 负责网络协议参数校验和安全连接；UI 负责显式确认，不在后台静默接受。

## 兼容与错误行为

| 情况 | 行为 |
| --- | --- |
| SMB 旧 UNC 源且可解析 | 加载时转换为 host/share/path，并尝试现有凭据；旧记录没有凭据时允许 Guest，认证失败提示填写账号密码。 |
| SMB 主机不可达 | 显示网络/端口错误，不误报成本地路径不存在。 |
| SMB Guest/认证失败 | 前台提示输入账号密码，可选保存到 vault 或仅当前会话；后台不弹窗并报告 auth-required；不得输出凭据。 |
| SMB 文件短读/EOF | ByteSource 依约返回实际字节数；上层 read_exact_at 给出 UnexpectedEof。 |
| SFTP 首次遇到未知主机 | 显示指纹；未确认则取消，未发送密码/私钥。 |
| SFTP 固定指纹变化 | 拒绝登录；只允许用户显式核验后替换 pin。 |
| SFTP key 文件格式/口令错误 | 返回可理解的导入或解锁错误，不记录私钥内容。 |
| 安全存储暂不可用 | 保留内存值和旧数据库值，不把凭据覆盖为空或回退明文落盘。 |

## 验收标准

1. SMB：Android MuMuPlayer 15 与 Windows 均可用 SMB2/3 连接临时只读共享；正确凭据可浏览并打开样例漫画，验证文件头、尾部及中间位置的随机读取；错误凭据明确失败；Guest 行为可验证。
2. SMB：旧 UNC 配置兼容；源码/测试证明 SMB 不进入本地 filesystem 分支；远端目录快照和阅读器流程可运行。
3. SFTP：首次连接确认正确显示指纹，取消不发起认证；保存后正确 pin 可连接；替换服务端 host key 后旧 pin 必须拒绝认证，用户明确替换后才可连接。
4. SFTP：密码和 OpenSSH/PEM 私钥认证均能通过；错误密码、错误私钥和错误 passphrase 均失败。
5. 凭据：Android Keystore 与 Windows 安全存储 round-trip 通过；旧明文迁移只在 vault 回读确认后清理；原生 JSON 备份和诊断日志不含 secrets。
6. 完成项目 Flutter Analyze、Rust warnings-as-errors check、串行 Rust tests、Flutter tests、Android build、Windows build，并用 MuMuPlayer 15 与临时 SMB/SFTP 测试端做真实协议 smoke test。

## 参考资料

- [X-plore SMB 与 SSH 功能](https://www.lonelycatgames.com/apps/xplore)
- [X-plore SSH、密码/私钥和受保护密钥存储](https://www.lonelycatgames.com/docs/xplore/ssh)
- [OpenBSD StrictHostKeyChecking 与 known_hosts](https://man.openbsd.org/ssh_config.5)
- [FileZilla SFTP 私钥和口令流程](https://filezillapro.com/docs/v3/basic-usage-instructions/connect-to-sftp-with-a-private-key-in-filezilla/)
- [smb2 pure-Rust SMB2/3 client](https://docs.rs/smb2/latest/smb2/index.html)
- [smb2 FileReader 定位读取 API](https://docs.rs/smb2/latest/smb2/client/stream/struct.FileReader.html)
- [Pavão/libsmbclient compatibility tradeoffs](https://docs.rs/pavao/latest/pavao/struct.SmbClient.html)
- [flutter_secure_storage 平台安全存储与平台要求](https://pub.dev/packages/flutter_secure_storage)
- [Flutter file_selector 原生文件选择 API](https://pub.dev/packages/file_selector)

