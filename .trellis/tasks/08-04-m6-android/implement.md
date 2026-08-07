# M6 Android 适配 — 实施计划

> 说明:本任务按 inline 模式执行(不派生子代理),Phase 2 由主会话直接读取 prd/design/implement 实施,跳过 implement.jsonl / check.jsonl 编排。

## 实施顺序

p0-android-buildchain → (p1-local-reader ∥ p3-native-formats) → p2-remote-sources → p4-release

## 前置(用户手动配合项)

1. 安装 Android SDK + NDK(或 Android Studio),配置国内镜像:
   - Gradle 仓库:阿里云 Maven(google + central 镜像)
   - `PUB_HOSTED_URL`(pub 镜像)、crates.io 镜像(rsproxy 等)
2. `flutter doctor` Android 项转绿。
3. 正式 `applicationId = com.rch.reader`(已确认 2026-08-07)。
4. 准备一台 arm64 真机 + 开启开发者模式 / USB 调试;开发期先用模拟器(本机无可用系统镜像源,改用 MuMu / 雷电等自带 adb 的模拟器,连 127.0.0.1:7555 等端口)。

> 网络被墙应对(教训,2026-08-07):直连卡住时**及时**启用 Clash Verge Rev 规则模式(本机 127.0.0.1:7897)并设置 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`,不要长时间硬等(services.gradle.org / dl.google.com 曾各卡 10+ 分钟,0 字节)。
> 镜像兜底:`~/.gradle/init.d/mirrors.init.gradle.kts`(阿里云 google/central/gradle-plugin)覆盖 Flutter included-build 的硬编码 google() 仓库;Gradle 发行版走腾讯镜像(已改 wrapper URL);cargo 走 rsproxy;pub 走 pub.flutter-io.cn。

## 分步检查清单

### Step 1:P0 构建链
- [ ] 安装 SDK/NDK,`flutter doctor` 通过
- [ ] `flutter build apk --debug` 成功;`flutter run -d <设备>` 启动到主界面
- [ ] applicationId / 应用名 / 图标 / INTERNET 权限落位
- [ ] `flutter build apk --release` 成功且可安装

### Step 2:P1 本地阅读(与 P3 并行)
- [ ] 阅读器触屏交互(点按翻页 / 双指缩放 / 长按菜单 / 返回键 / 横竖屏 / insets)
- [x] SAF 导入 → 复制到应用私有目录 → 建索引（2026-08-07 实现：书源页新增“导入本地漫画”，流式复制进应用私有 books/ 并自动建/复用本地书源；单测+analyze 通过，待真机验收）
- [ ] 书架 / 详情 / 缓存 / 进度在安卓上验证
- [ ] AI 超分入口在安卓隐藏
- [ ] Windows 回归:`cargo test` + `flutter analyze`

### Step 3:P3 原生格式(与 P1 并行)
- [ ] libpdfium.so 按 ABI 打包 + pdf.rs 加载路径适配,真机渲染 PDF 一页
- [ ] unrar NDK 编译验证;失败则执行预案并记录结论
- [ ] PDF / CBR 合入阅读器

### Step 4:P2 远程书源
- [ ] WebDAV / SFTP 真机闭环
- [ ] 百度 OAuth(deep link 或复制 code)真机闭环
- [ ] 115 扫码 / 手动授权 + token 刷新真机闭环
- [ ] 夸克 Cookie 登录 + 浏览/打开/缓存真机闭环
- [ ] WebDAV 同步通道:推/拉/恢复(标签/书源/进度)真机闭环
- [ ] 打开策略三态与缓存行为与桌面一致

### Step 5:P4 发布
- [ ] 正式签名 keystore + gradle 配置
- [ ] ABI 拆分构建 + 体积记录
- [ ] README / 发布流程更新
- [ ] Windows + Android 全量回归

## 验证命令

```bash
cd app
flutter analyze
./codegen.ps1                  # Rust API 变更后必跑:重建 FRB 绑定 + release DLL(防 content hash 漂移)
flutter test
cd rust && cargo test          # Rust 单测(Windows 回归)
flutter build apk --debug
flutter build apk --release --split-per-abi
flutter run -d <android-device>
```

## 风险与回滚点

| 风险 | 预案 | 回滚点 |
|---|---|---|
| 原生库(PDF/RAR)NDK 编译失败 | p3 降级:该格式延后,记录结论 | 仅回滚 p3 |
| SAF 适配或 file_selector 行为差异 | 退回"仅应用私有目录 + 系统分享" | 仅回滚导入功能 |
| 百度 OAuth 回调在安卓不可行 | 复制 code 粘贴方案 | 仅回滚授权流程 |
| 网盘同步盘本地目录通道在 Android 不可用(getDirectoryPath) | 首版仅 WebDAV 同步通道,同步盘通道后置 | 不影响 P2 |
| 网络镜像不稳定导致构建慢 | 换镜像源 / 预下载依赖 | 不影响代码 |
| Windows 回归被破坏 | 任何 Rust 改动先过 Windows 测试再合入 | 回滚对应子任务 |

## 质量门

- 每个子任务完成时:验收标准全绿 → `flutter analyze` 无新增 issue → `cargo test` 通过;Rust API 变更后 `codegen.ps1` 已跑(绑定与 content hash 一致)。
- 最后一个子任务(p4-release)完成前,必须跑全量回归(Windows 构建 + 安卓真机冒烟)。
