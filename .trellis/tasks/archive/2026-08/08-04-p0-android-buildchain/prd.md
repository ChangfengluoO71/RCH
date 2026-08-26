# P0 安卓构建链与基础工程

## Goal

打通安卓构建链:让现有 Flutter + Rust 工程能在安卓设备上构建、安装、启动,并完成基础应用标识与网络权限,为所有后续子任务提供可运行的地基。

## Confirmed Facts

- 本机无 Android SDK / NDK(`flutter doctor` 报错),网络访问 maven.google.com / github 有 TLS 错误,需要国内镜像。
- Flutter 3.44.8 默认:compileSdk 36 / targetSdk 36 / minSdk 24 / NDK 28.2.13676358。
- `app/android` 为默认模板:`com.example.app`、debug 签名、无 INTERNET 权限、默认图标。
- `rust_builder` 用 cargokit,已带 Android Gradle 构建支持。

## Requirements

- 安装并配置 Android SDK + NDK(含国内镜像),`flutter doctor` 的 Android 项转绿。
- Rust 核心经 cargokit 编译为 Android ABI(至少 arm64-v8a),应用可启动到主界面。
- applicationId 正式化、应用名 "RCH"、图标替换默认 Flutter 图标。
- AndroidManifest 增加 `INTERNET` 权限。
- debug 与 release APK 均可构建。

## Acceptance Criteria

- [ ] `flutter doctor` 无 Android 错误项。
- [ ] `flutter run -d <android 设备>` 启动到主界面,无崩溃。
- [ ] `flutter build apk --debug` 与 `--release` 成功,release APK 可安装到真机(arm64-v8a)。
- [ ] 包名 / 应用名 / 图标正确,manifest 含 INTERNET 权限。

## Dependencies

无。本任务是 M6 的第一个子任务,其余子任务均依赖它。
