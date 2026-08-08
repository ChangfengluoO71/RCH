# RCH v0.4.3 — 115 Cookie 自动续期 + 更新下载镜像

## 新功能

- 🔄 **115 网盘 Cookie 自动续期**：扫码 Cookie 失效时自动弹出扫码框，扫码成功后自动替换 Cookie 并重连，无需手动进设置；编辑书源也支持一键「重新扫码获取 Cookie」
- ⚡ **更新下载镜像（国内加速）**：设置 → 关于与更新 → 下载通道
  - 内置 ghproxy.net / ghfast.top 等常用镜像，也可填写自定义镜像前缀
  - 应用自动从 CDN 拉取最新镜像列表（打开面板自动更新 + 手动刷新按钮）
  - 下载失败自动切换下一个通道，全部失败会列出已尝试的通道

## 发布

- Windows 安装包与 Android APK（arm64-v8a / armeabi-v7a / x86_64）由 GitHub Actions 自动构建、正式签名并附到本 Release
