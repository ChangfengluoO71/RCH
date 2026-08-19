# 环境搭建

RCH 使用 Flutter (Dart) + Rust (cdylib) + flutter_rust_bridge 构建。

## 系统要求

| 组件 | 版本 | 说明 |
|------|------|------|
| Windows | 10/11 (x64) | 主开发平台 |
| Flutter | ≥3.44 | [安装指南](https://docs.flutter.dev/get-started/install/windows) |
| Rust | ≥1.80 | [rustup 安装](https://rustup.rs/) |
| Visual Studio 2022 | BuildTools | C++ 桌面开发工作负载（含 Windows SDK, MSVC） |
| Git | 任意版本 | 版本管理 |

## 安装步骤

### 1. 安装 Flutter

从 [Flutter 官网](https://docs.flutter.dev/get-started/install/windows) 下载 SDK，解压到 `C:\flutter`，添加到系统 PATH：

```powershell
[Environment]::SetEnvironmentVariable('PATH', "$env:PATH;C:\flutter\bin", 'User')
```

验证：

```bash
flutter doctor
```

确保 Windows (desktop) 一项打勾。

### 2. 安装 Rust

```powershell
# 下载 rustup-init.exe 并安装
# https://rustup.rs/

# 或命令行安装
winget install Rustlang.Rustup
```

验证：

```bash
rustc --version  # 应 ≥ 1.80
cargo --version
```

### 3. 安装 Visual Studio 2022 BuildTools

下载 [Visual Studio BuildTools](https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio-2022)，安装时勾选：

- **MSVC v143 - VS 2022 C++ x64/x86 生成工具**
- **Windows 11 SDK**
- **C++ CMake tools for Windows**

> 只用 BuildTools，不需要完整 Visual Studio IDE。

### 4. 克隆仓库

```bash
git clone https://github.com/ChangfengluoO71/RCH.git
cd RCH/app
flutter pub get
```

### 5. 构建 Rust 库

```bash
cd app/rust
cargo build
```

首次构建会下载依赖，可能需要 5-15 分钟。PDF 解析在**运行时**动态加载 `pdfium.dll`，见下方「PDF 支持依赖」。

### 6. 生成桥接代码（仅修改 Rust API 后需要）

```bash
cd app
flutter_rust_bridge_codegen generate
```

> **注意**：改完 Rust API 后请运行 `.\codegen.ps1`（而不是只执行上面的 codegen）。
> FRB 在 `flutter run` 时优先加载 `rust/target/release/rust_lib_app.dll`，只重生成绑定
> 而不重建该 DLL 会导致启动时报 content hash 不匹配。

## 运行

```bash
cd app
flutter run -d windows
```

### PDF 支持依赖

打开 PDF 需要 `pdfium.dll` 与 `RCH.exe` 同目录（应用会依次查找进程工作目录、exe 所在目录、PATH、系统目录）。首次构建后从 [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries/releases) 下载 Windows x64 版本并放到构建输出目录：

```powershell
$ProgressPreference = 'SilentlyContinue'
Invoke-WebRequest -Uri "https://github.com/bblanchon/pdfium-binaries/releases/latest/download/pdfium-win-x64.tgz" -OutFile "$env:TEMP\pdfium-win-x64.tgz"
tar -xzf "$env:TEMP\pdfium-win-x64.tgz" -C "$env:TEMP"
Copy-Item "$env:TEMP\bin\pdfium.dll" "build\windows\x64\runner\Debug\pdfium.dll"
```

正式安装包由 CI（`.github/workflows/release.yml`）自动捆绑该 dll，无需手动处理。

## 测试

```bash
# Rust 单元测试
cd app/rust
cargo test

# Flutter 静态分析
cd app
flutter analyze
```

## 常见问题

### `flutter doctor` 显示 "Unable to find Visual Studio"

BuildTools 安装后需重启终端。如仍找不到，在终端中运行：

```powershell
& 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe' -latest
```

### `cargo build` 报 pdfium 编译错误

pdfium 需要 Python 3 + CMake 在 PATH 中：

```bash
python --version  # 需 ≥ 3.8
cmake --version   # 需 ≥ 3.16
```

### 打开 PDF 报「无法加载 pdfium 动态库」

缺少 `pdfium.dll`。按上方「PDF 支持依赖」下载并放到 `RCH.exe` 同目录后重启应用。

### `flutter run` 报 "MissingPluginException"（Windows）

先执行 `flutter clean && flutter pub get`，再重新运行。该异常通常由桥接代码版本不同步引起。

### Windows SDK 缺失

若出现 `windows.h` 找不到：在 Visual Studio Installer 中确保勾选了 **Windows 11 SDK**（或 10 SDK 最新版）。

## 可选工具

| 工具 | 用途 |
|------|------|
| [RustRover](https://www.jetbrains.com/rust/) | Rust IDE |
| [VS Code](https://code.visualstudio.com/) + Dart/Flutter 插件 | Flutter 开发 |
| [Git for Windows](https://git-scm.com/downloads/win) | 终端的 Git Bash |

## 发布（构建安装包）

打 tag 后由 CI（`.github/workflows/release.yml`）自动构建并上传 GitHub Release：

```powershell
# 1. 升版本号（只需改 app/pubspec.yaml；exe 版本资源 / 安装包文件名由 CI 从 tag 注入）
git add app/pubspec.yaml
git commit -m "release: vX.Y.Z — 摘要"

# 2. 打 tag 并推送（触发 CI 构建 Windows 安装包 + 各 ABI Android APK）
git tag -a vX.Y.Z -m "RCH vX.Y.Z"
git push origin master --tags

# 3. 检查 GitHub Actions Release 工作流结果，必要时在 Release 页补 notes 后发布
```

版本号约定：tag 去掉 `v` 前缀即为版本（如 `v0.4.0` → `0.4.0`），构建号按
`主版本*10000 + 次版本*100 + 修订号` 注入（`0.4.0` → `400`），保证 Android
versionCode 单调递增。

> **Android 依赖仓库镜像开关**：`android/settings.gradle.kts` 与
> `android/build.gradle.kts` 中的阿里云 Maven 镜像（及本机 `D:/Temp/local-maven`）
> 默认关闭，CI 直接使用官方 `google()` / `mavenCentral()`，避免镜像故障导致构建失败。
> 本地构建需要镜像加速时，在 `~/.gradle/gradle.properties` 写入
> `rch.aliyun.mirror=true`（或设置环境变量 `RCH_ALIYUN_MIRROR=true`）。

### 应用内更新

- 入口：设置页「关于与更新」→ 检查更新；启动时也会静默检查一次，发现新版本会提示。
- 数据源：GitHub Releases latest（`https://api.github.com/repos/ChangfengluoO71/RCH/releases/latest`）。
- Windows：下载 `RCH-<版本>-windows-x64.exe` 到临时目录，静默安装（安装器
  `CloseApplications=yes` 自动关闭运行中的应用，装完自动重启）。安装器需要管理员权限，会弹 UAC。
- Android：下载 `app-arm64-v8a-release.apk`（无 arm64 时回退其他 ABI）到应用外部目录，
  经 FileProvider 拉起系统安装器；首次使用需在系统弹窗中允许「安装未知应用」。
- GitHub API 不可达（限流/网络）时，面板会提供「打开 GitHub Releases」兜底。

### Android 正式签名（P4）

发布 APK 必须用正式签名（debug 签名每台机器/每次 CI 都不同，无法覆盖升级）。
本地签名文件不入库（`app/android/.gitignore` 已排除）：

- `app/android/upload-keystore.jks` — 正式 keystore（alias=`upload`，RSA 2048，有效期 10000 天）
- `app/android/key.properties` — 本地签名配置（storeFile / storePassword / keyAlias / keyPassword）

**重要：keystore 与密码务必备份到仓库之外的安全位置。密钥一旦丢失，
已安装用户将无法升级（签名不一致只能卸载重装）。**

CI（GitHub Actions）通过仓库 Secrets 注入签名，缺省会直接报错阻止发布：

| Secret | 值 |
|---|---|
| `RELEASE_KEYSTORE_B64` | keystore 文件的 Base64（见下方命令） |
| `RELEASE_STORE_PASSWORD` | keystore 密码 |
| `RELEASE_KEY_ALIAS` | `upload` |
| `RELEASE_KEY_PASSWORD` | 密钥密码（与 keystore 密码相同） |

生成 Base64（Windows PowerShell）：

```powershell
[Convert]::ToBase64String([IO.File]::ReadAllBytes("app\android\upload-keystore.jks"))
```

> 注意：当前已发布的 v0.4.0 APK 是 debug 签名。首个正式签名版本发布后，
> 老用户无法原地覆盖升级，需要手动卸载重装一次；之后版本即可无缝升级。

### 本地构建 Windows Release（工具环境卡 cl.exe 时）

已知问题：在自动化工具上下文里 `flutter build windows --release` 会在
MSBuild→cl.exe 阶段无限挂起（cl.exe 零 CPU）。改用 WMI 在任务 job 之外启动即可：

```powershell
$cmd = 'cmd.exe /c "cd /d D:\Projects\RCH-source\app && set MSBUILDDISABLENODEREUSE=1 && flutter build windows --release > build_release.log 2>&1"'
Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $cmd }
```
