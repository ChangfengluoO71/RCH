# RCH v0.4.1 — 应用内更新系统 + 安卓正式签名

## 新功能

- 🔄 **应用内更新系统（Windows + Android）**：设置页「关于与更新」一键检查 / 下载 / 安装；启动时自动检测新版本
  - Windows：下载安装包后静默安装，安装器自动关闭并重启应用
  - Android：下载 APK 经系统安装器升级；首次使用需允许「安装未知应用」
- 🔑 **Android 正式签名**：release APK 改用正式 keystore 签名，后续版本可无缝覆盖升级（已安装 v0.4.0 的用户需手动卸载重装一次）
- 🏷️ 版本号由发布 tag 自动注入（Windows exe / Android versionName / versionCode）

## 修复

- 日漫 / 美漫模式无法滑动翻页
- 条漫模式图片模糊（高 DPI 屏幕）
- 条漫模式无法双指缩放
- 美漫模式底栏翻页箭头方向反了

## 发布

- Windows 安装包与 Android APK（arm64-v8a / armeabi-v7a / x86_64）由 GitHub Actions 自动构建、正式签名并附到本 Release
