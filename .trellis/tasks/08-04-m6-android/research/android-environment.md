# 安卓环境与构建链调研

## 结论

- 本机 Flutter 3.44.8 已就绪;缺 Android SDK / NDK(flutter doctor 报 X)。
- Flutter 3.44.8 默认值(来自 `D:\flutter\flutter\packages\flutter_tools\gradle\src\main\kotlin\FlutterExtension.kt`):
  - compileSdk 36,targetSdk 36,minSdk 24,NDK 28.2.13676358
- `app/android/local.properties` 已配置 `flutter.sdk=D:\flutter\flutter`;`settings.gradle.kts` 用 AGP 9.0.1 + Kotlin 2.3.20,模板为默认值。
- 网络:访问 maven.google.com / github.com 有 TLS 握手失败;需国内镜像(阿里云 Maven、pub 镜像、crates 镜像)。

## 需要用户在安装时注意

- 不装 Android Studio 也可:用命令行 sdkmanager 安装 `platform-tools`、`platforms;android-36`、`build-tools`、`ndk;28.2.13676358`。
- 国内镜像:腾讯 / 阿里 SDK 镜像,或 Android Studio 内设置 HTTP Proxy 为镜像地址。

## 现有工程与安卓相关的骨架

- `app/rust_builder/` 为 flutter_rust_bridge 标准模板,含 `cargokit/`(跨平台构建工具)与 `android/`(Gradle 插件),Android 侧 Rust 构建路径已预置。
- `app/android/app/src/main/AndroidManifest.xml` 目前无 INTERNET 权限(远程书源必须补)。
- `app/android/app/build.gradle.kts`:`applicationId=com.example.app`,release 用 debug 签名。
