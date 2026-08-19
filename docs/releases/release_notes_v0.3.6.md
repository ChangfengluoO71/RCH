# RCH v0.3.6 — Android 双端发布

## 新功能

- 📱 **Android 支持**：同一套代码在 Windows 与 Android 双端运行；触屏翻页/缩放、系统返回、横竖屏、SAF 导入本地漫画、窄屏响应式布局
- 📄 **PDF / CBR / RAR 原生格式**（Android）：按 ABI 打包 libpdfium.so 与 libc++_shared.so；PDF 按需渲染，云端下载完秒开、翻页才渲染当前页
- 🔧 安卓构建链打通：Rust 核心（含 unrar C++）NDK 交叉编译、FRB 绑定、Gradle 9 / AGP 9 / Kotlin 2.3 全链路

## 修复

- 详情页在安卓横屏矮视口下封面列 RenderFlex 溢出（黄黑报错条遮挡“开始阅读”按钮）
- 云端 PDF 下载到 100% 后长时间无响应（原为整本全量渲染，改为按需渲染）
- 添加书源对话框错误信息自动滚动可见；窄屏/横屏提交失败不再无反馈
- 书源页新增“导入本地漫画”入口（复制进应用私有目录并建本地书源）

## 发布

- Windows 安装包与 Android APK（arm64-v8a / armeabi-v7a / x86_64）由 GitHub Actions 自动构建并附到本 Release
