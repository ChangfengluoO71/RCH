# RCH v0.5.7 发布候选门禁报告

日期：2026-09-13  
基线：`v0.5.7` / `d112433`  
分支：`local-ai/cover-quark-debug`  
范围：Android 持久化与凭据、下载取消/交接、缓存清理、封面安全降级、M8 边界，以及本轮自动化门禁。

## Executive Status

当前状态：**YELLOW（可继续开发，不宜立即发版）**。

本轮已完成代码层的主要收口：Android 使用独立的 `filesDir/RCH/data` 数据根；旧数据库迁移包含 `database.db`、`-wal`、`-shm` 的确定性候选、SQLite 完整性/结构/行数校验、临时复制、同步和原子重命名；Keystore 凭据通过 `rch/credentials` 边界保存，SQLite 只保留引用；WebDAV Range 下载支持协作式取消并清理 `.part`；阅读完成清理只处理 page/raw 内容缓存；更新包支持 `.part`、复用已验证包、平台交接和 GitHub SHA-256 digest 校验；封面 Range 不可用时回退占位符并按书源提示，用户自定义封面只有明确同意后才会下载整本。

自动化测试和 Windows 构建已通过，但以下发布门禁仍无证据：Android SDK/模拟器/`adb` 与真实设备、Android Keystore/系统 Package Installer 实测、Windows UAC 实测、认证后的 OpenList/真实网盘 20/100/300 MiB 性能与内存数据、50+ 页高差异条漫设备 smoke，以及 M8 本地 100 本 truth set。`cargo fmt --check` 和严格 Clippy 仍被历史/混合 dirty diff 阻塞。因此本报告的发布结论为 **DO NOT RELEASE**。

## P0 Android Persistence

### 数据根与启动顺序

- Rust 新增 `setDataRootPath`、`dataRootPath`、`migrateLegacyDatabase`、`migrateCacheContents`、`verifyDatabase`。
- Android 启动顺序固定为：`RustLib.init()` → 设置 `<application support>/RCH/data` → 按固定顺序检查/迁移旧数据库及 WAL/SHM → 执行 JSON→SQLite 迁移 → 加载业务状态 → 凭据迁移/回读校验。
- 候选顺序由 `StorageLayout.legacyDatabaseCandidates` 固定为：有效新路径、缓存根标记路径、默认缓存路径、应用支持目录、旧 `RCH` 支持目录；Rust 不按修改时间盲选。
- 迁移写入 `data.migration.partial`，复制到临时文件并 `sync_all`，执行 SQLite `integrity_check`、应用表存在性、schema/行数快照校验后再原子重命名。损坏目标会保留为 `database.db.corrupt-*`；失败或中断保留旧副本，启动进入保护页面，禁止初始化空库。
- 验证成功并完成 `LibraryStore` 加载后才清理旧副本和迁移标记；有效目标及中断标记在重启时可幂等恢复。

### Android 凭据边界

- `CredentialVault.kt` 使用 Android Keystore AES-256-GCM、随机 IV 和应用私有密文文件；不使用已弃用的 `EncryptedSharedPreferences`。密文及回滚备份排除 Android 自动备份。
- `rch/credentials` MethodChannel 只提供 `put/get/delete`；`BookSource`/`book_sources` 增加 `credentialRef`。Android 的 `password`、`refresh_token`、`client_secret`、`cookie` 在迁移后不再落 SQLite，保存时采用“写入 vault → 回读校验 → SQLite 事务清空敏感列并写引用”的顺序。
- 书源更新、115/夸克续期、百度 refresh、WebDAV/SFTP 登录均经过 vault 边界；同步密码同样使用引用。标准 `.rchpkg` 不含凭据；加密包导出由 Dart 从 vault 读入后交给 Rust，导入先解密到内存，再写入 vault 和引用，不把凭据回写敏感列。
- Android `library.json` 采用脱敏序列化，仅作为一次性迁移输入；桌面端保留历史明文存储行为，并在本报告中明确为安全债务，未宣称跨平台凭据加密完成。

## Storage Classification

| 数据 | Android 目标 | 可重建 | 清理缓存后保留 | 敏感 |
| --- | --- | ---: | ---: | ---: |
| SQLite、书源、设置、标签、历史、封面参数 | `filesDir/RCH/data` | 否 | 是 | 否 |
| provider 凭据、同步密码 | Keystore + 私有密文存储 | 否 | 是 | 是 |
| `library.json` | app support；Android 脱敏 | 部分 | 是 | 否 |
| 导入书籍 | app support/books | 否 | 是 | 否 |
| page/raw/cover/AI/缩略图、快照、错误日志 | cache 根或临时目录 | 是 | 否 | 日志需脱敏 |
| `.part`、临时文件、更新 APK | temp/cache/external-files | 是 | 否 | 否 |

Android 自定义缓存目录迁移只复制 `cache/` 与可重建错误日志，不再复制数据库、`library.json`、凭据或其他持久数据；Windows 保留“数据库与缓存同一用户根目录”的历史整根迁移契约。阅读完成清理使用 page/raw 内容缓存入口，保留封面、元数据、书源、凭据、标签、历史、完成状态和自定义封面参数。系统 `Clear Data`、`pm clear` 和卸载导致应用数据/Keystore 消失属于平台预期，不计作持久化通过。

## Android Evidence

### 已有证据

- Rust 单元测试覆盖 Android data root 独立性、有效数据库及 WAL/SHM 迁移、损坏库拒绝、中断标记恢复、缓存迁移排除持久文件、凭据引用原子提交和敏感列清空；Android 严格启动在 vault 缺失/损坏时会保留数据并进入保护模式。
- Flutter 测试覆盖 Android 路径/候选顺序、脱敏 JSON、Keystore vault 严格解析、同步密码、缓存清理、更新交接和封面提示；启动契约通过。
- Android Manifest、backup rules/data-extraction rules、FileProvider 和系统安装 Intent 已实现，但本机无法构建或运行 Android。

### 缺失证据

- `flutter build apk --debug`：**SKIP/FAIL（环境阻塞）**，输出为 `[!] No Android SDK found. Try setting the ANDROID_HOME environment variable.`。
- 没有 Android SDK、AVD、`adb`、真实 OEM 设备，未能验证首次安装、旧库/WAL 迁移、损坏/中断恢复、重启幂等、Clear Cache 后保留数据、Keystore 回读、备份恢复和系统 Package Installer/未知来源权限。
- 50+ 页高差异条漫只完成代码与 Flutter smoke，Android emulator smoke 标记 `REAL_DEVICE_PENDING`；OEM 清理器标记 `OEM_CLEANER_REAL_DEVICE_PENDING`。

## Download Evidence

### 已实现并自动验证

- WebDAV `DownloadProgress` 增加取消标志和 `.part` 指针；`webdavCancelDownload(session)` 在读取边界协作式停止 worker，删除 `.part`，不重试、不把部分文件 rename 为正式文件，并释放连接/句柄/任务。迟到取消不会删除已存在的正式缓存。
- Range pilot 严格检查 `206`、`Content-Range`、长度和版本标记；`200`、`416`、认证/限流、缺失或不匹配响应降级为串行或安全失败。分段合并使用临时文件、flush/sync 和原子提交。
- 同一资源 single-flight、失败后可重试、请求指标脱敏和受控 HTTP benchmark 回归均通过；受控服务只证明协议行为，不代表真实网盘吞吐或内存结果。
- 更新交接使用 `.part` 和已验证包复用；GitHub asset `digest: sha256:<hex>` 会在流式下载后校验，hash 不匹配删除包并进入可重试错误态。Windows 使用参数化 installer handoff，Android 走系统 Package Installer Intent，并保留未知来源/用户取消后的 APK。
- 阅读完成清理保留封面/元数据/凭据等持久数据；封面 Range 不可用时使用占位符和一次性书源提示，自定义封面仅在用户确认后进入整本下载入口。

### 尚缺真实下载证据

- OpenList 当前仅确认服务在线，未认证；凭据只能通过临时环境变量 `RCH_TEST_OPENLIST_PASSWORD` 或交互输入，禁止写入代码、日志、fixture、报告、截图或命令历史。
- 仍需将真实本地漫画复制到隔离 fixture workspace（约 20/100/300 MiB，绝不修改原文件），串行、Range 分段、取消、断线、错误响应、截断响应分别记录 HTTP status/`Content-Range`、segment 数、wall time/throughput、peak RSS、`.part` 状态、retry/recovery 和最终 SHA-256。300 MiB 固定缓冲门禁为 RSS 增量不超过 128 MiB，且 100→300 MiB 增长小于 2 倍；当前标记 `REAL_PROVIDER_PERF_PENDING`。

## M8 Evidence

M8 继续保持 **Catalog-only / local-only**：从本地 SQLite catalog snapshot 读取文件名和上级目录，生成可解释 proposal；`RemoteOnly` 不访问 ByteSource、远程书源、Downloader、Provider 或同步传输。scraper、semantic projection、canonical identity、proposal、同步 dirtiness 和安全自动 materialization 的现有回归均在 Rust 全量测试中通过。

线上智能刮削、E-site 元数据抓取和 115 自动化不进入本轮主流程，保留为未来插件边界（M8-M3，P3）。当前没有可审计的本地 100 本 truth set，因此 exact accuracy、FP、FN、unresolved 和代表性失败案例仍为 **M8 corpus pending**，不能用既有小样本/生产 dry-run 结果替代。待处理的 M8 规划项包括 M8-M1 Catalog-only 识别、M8-M2 canonical identity/migration、M8-M4 本地候选排序、M8-M5 review/confirmation 和 M8-M6 corpus validation。

## Automated Verification

| 精确命令 | 结果 | 证据/说明 |
| --- | --- | --- |
| `cargo fmt --all -- --check` | **FAIL** | 输出包含本轮触及文件与历史文件的混合格式差异；按 A（本轮文件）、B（历史债务）、C（混合）分类后未做全量重排，避免污染既有 dirty diff。 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | **FAIL** | lib 报 64 个、lib test 报 69 个严格 lint；主要为 `ptr_arg`、`too_many_arguments`、`type_complexity`、`doc_lazy_continuation`、`needless_borrow`、`io_other_error` 等既有/跨模块债务，未用 allow 掩盖。 |
| `cargo test --workspace --locked -j 1 -- --test-threads=1` | **PASS** | 275 passed，0 failed，2 ignored；`tests/reader_l1_hit_deadlock.rs` 1 passed；doc-tests 0。 |
| `flutter analyze --no-pub` | **PASS** | `No issues found!` |
| `flutter test --no-pub` | **PASS** | `+111: All tests passed!` |
| `dart run tool/startup_contract_check.dart` | **PASS** | `startup contract satisfied` |
| `flutter build apk --debug` | **SKIP/FAIL** | 本机无 Android SDK，无法进入 Gradle/设备验证。 |
| `flutter build windows` | **PASS** | `√ Built build\\windows\\x64\\runner\\Release\\RCH.exe`。 |
| `git diff --check` | **PASS** | 退出码 0；仅有工作区 LF→CRLF 提示。 |
| `app/codegen.ps1` | **PASS** | FRB 绑定重新生成，`rust/target/release/rust_lib_app.dll` 重建。 |

## Remaining External Validation

按优先级，发布前仍需完成：

1. **P0 Android**：补齐 SDK/AVD/`adb`，执行数据根/旧库/WAL/SHM/损坏/中断/重启幂等、Clear Cache、Keystore put/get/delete/迁移失败保护、脱敏 JSON 和系统 Package Installer/未知来源/取消重试。
2. **P1 真实交接**：Windows UAC 允许/取消与重试；Android APK 系统确认界面和最终安装结果。`adb install` 只能安装测试基线，不能替代更新交接验收。
3. **P1 真实 provider 性能**：认证 OpenList 及可用的 WebDAV/115/夸克/百度环境，完成隔离 20/100/300 MiB fixture、Range/串行/取消/断线/截断矩阵和固定内存上限报告。
4. **P1 阅读 smoke**：Android emulator 50+ 页高差异条漫；真实 OEM 清理器仍标记 `OEM_CLEANER_REAL_DEVICE_PENDING`。
5. **P2 M8**：建立本地 100 本 truth set，输出 exact accuracy、FP、FN、unresolved 和失败案例，完成 M8-M1/M2/M4/M5/M6 验收；在线智能刮削保持未来插件，不作为本轮 release 依赖。

当前 Trellis 中仍可见的未完成规划包括：反馈整改父任务、海报墙封面、更新下载/安装交接、条漫稳定性、M8 主任务（`in_progress`）；M8-M1/M2/M4/M5/M6、AVIF、详情信息/统计、退出行为、首次启动缓存引导（`planning`）；M8-M3 在线 Provider 插件（P3，延期）。这些文档状态不应在没有上述证据时被手动勾选为完成。

## Git Boundary

- 基线仍为 `v0.5.7` / `d112433`，分支仍为 `local-ai/cover-quark-debug`；未创建 commit、版本号、tag、release 或发布包。
- 报告收口时工作区有 62 个 tracked 文件修改和 26 个 untracked 条目（包含本报告及本轮生成物）；本轮没有 reset、checkout、stash、删除或覆盖用户已有内容。FRB 生成文件和本报告也是未提交工作区内容。
- `.pi/`、`graphify-out/`、飞书授权二维码、既有报告和 Trellis 任务文档均按现状保留；未将任何 provider 密码或 token 写入代码、日志、fixture、报告、截图或命令历史。
- `cargo fmt` 未执行全量改写；后续提交应按功能边界拆分并保留可回滚点。

## Release Recommendation

**DO NOT RELEASE**。

只有在 P0 Android 持久化/凭据保护、真实更新交接、真实 provider 下载性能与取消、条漫设备 smoke、M8 100 本 truth set 全部有可复核证据，并重新评估 fmt/Clippy 门禁后，才可将状态升级为 `RELEASE CANDIDATE READY`。当前代码可继续开发和评审，但不应作为 v0.5.7 发布候选对外发版。
