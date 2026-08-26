# P4 安卓发布与回归

## Goal

安卓首版可正式发布:已签名 APK、ABI 拆分、版本号策略、发布文档,并完成 Windows 全量回归。

## Requirements

- release 正式签名:生成 keystore 并配置 gradle(替换 debug 签名)。
- ABI 拆分构建:arm64-v8a 必发,armeabi-v7a / x86_64 视需要;记录各 ABI 包体积。
- 版本号:沿用 0.3.x 版本 + versionCode 策略。
- 发布文档:README / GitHub Releases 流程与 Windows 发布并列或扩展。
- 全量回归:Windows 端 + 安卓端功能检查。

## Acceptance Criteria

- [ ] 生成已签名 release APK(arm64-v8a),可安装运行。
- [ ] Windows 构建与现有功能不回归。
- [ ] 发布流程 / README 已更新。

## Dependencies

- 前置:p1-local-reader + p2-remote-sources + p3-native-formats(全部合入后)。
