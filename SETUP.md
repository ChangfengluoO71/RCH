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

首次构建会下载依赖、编译 pdfium 等原生库，可能需要 5-15 分钟。

### 6. 生成桥接代码（仅修改 Rust API 后需要）

```bash
cd app
flutter_rust_bridge_codegen generate
```

## 运行

```bash
cd app
flutter run -d windows
```

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
