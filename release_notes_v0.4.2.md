# RCH v0.4.2 — 115 网盘扫码获取 Cookie 书源

## 新功能

- ☁️ **115 网盘「扫码获取 Cookie」连接方式（默认入口，无需申请 APP ID）**
  - 应用内显示二维码，用 115 手机 App 扫码即自动获取登录 Cookie，无需等待开放平台审核
  - 浏览 / 打开 / 封面 / 本地缓存全流程打通，支持根文件夹 ID 挂载子目录
  - 默认 `wechatmini` / `tv` 等冷门扫码设备，避免挤掉网页端 / App 旧登录；高级选项可切换
  - Cookie 过期后编辑书源重新扫码即可替换；官方 APP ID 模式保留在高级选项，两种方式均无 200MB 下载上限
- 📝 **全局错误日志**：未捕获异常自动写入缓存目录 `errors.log`（含完整堆栈），便于排查反馈

## 修复

- 扫码获取 Cookie 对话框无法弹出（二维码组件与弹窗布局冲突，改为自绘二维码）
- 115 Cookie 模式目录列表为空 / 打开漫画报错（pickcode 字段解析、直链解密算法、请求头签名）
- 阅读器 / 详情页偶发 `setState() or markNeedsBuild() called during build` 崩溃

## 发布

- Windows 安装包与 Android APK（arm64-v8a / armeabi-v7a / x86_64）由 GitHub Actions 自动构建、正式签名并附到本 Release
