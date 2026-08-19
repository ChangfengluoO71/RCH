# RCH v0.4.4 — WebDAV 同步修复

## 修复

- **WebDAV 同步失败（HTTP 410 Gone）**：修复 WebDAV 地址带基础路径（如 `https://dav.jianguoyun.com/dav/`）时，同步请求丢失 `/dav/` 前缀、MKCOL 被服务器返回 410 的问题；推送 / 拉取 / 归档清理恢复正常。

## 发布

- Windows 安装包与 Android APK（arm64-v8a / armeabi-v7a / x86_64）由 GitHub Actions 自动构建、正式签名并附到本 Release
