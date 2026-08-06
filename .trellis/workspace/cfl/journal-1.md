# Journal - cfl (Part 1)

> AI development session journal
> Started: 2026-07-31

---



## Session 1: 修复缩放拖动 Bug + 实现阅读器页面旋转（含批量规划）

**Date**: 2026-08-02
**Task**: 修复缩放拖动 Bug + 实现阅读器页面旋转（含批量规划）
**Branch**: `master`

### Summary

修复阅读器缩放后移动区域只在第一页生效（photo_view scaleState 同步重置 + 双页 panEnabled）；实现阅读器页面旋转（右键界面旋转、每页独立 90° 旋转、BookMeta.rotations SQLite/JSON 双写持久化、旧库 ALTER TABLE 补列）；批量规划 7 个小功能 PRD。

### Main Changes

- 修复单页翻页后缩放/拖动失效：同步重置 PhotoViewScaleStateController
- 双页模式启用 panEnabled，缩放后可拖动
- 阅读器页面旋转：右键界面旋转 + 单页/双页独立旋转按钮（90° 循环）
- 旋转持久化：BookMeta.rotations + SQLite book_metas.rotations 列 + FRB 桥接重生成

### Git Commits

| Hash | Message |
|------|---------|
| `c2cfc69` | (see git log) |
| `e7a4dac` | (see git log) |
| `8f28e26` | (see git log) |
| `59d2f48` | (see git log) |

### Testing

- [OK] flutter analyze 0 issues；flutter test 5 passed（缩放回归 + 旋转模型 round-trip）
- [OK] cargo test --lib 31 passed（含 rotations 列测试）

### Status

[OK] **Completed**

### Next Steps

- 用户本地 flutter run 实测旋转与缩放手感
- 剩余规划任务待开工（M5 书源 / 标签分层 / 后缀识别 / 转 CBZ / AVIF）


## Session 2: M6 网盘直连书源 + v0.3.2 发布

**Date**: 2026-08-03
**Task**: M6 网盘直连书源 + v0.3.2 发布
**Branch**: `master`

### Summary

M5 收尾提交；M6 实现百度/115 官方 API 书源（OAuth/设备码授权、三态打开、封面缓存、token 回写）；联调通过；归档 M5/M6；发布 v0.3.2（README/CHANGELOG 更新）

### Git Commits

| Hash | Message |
|------|---------|
| `de8de58` | (see git log) |

### Status

[OK] **Completed**


## Session 3: 百度网盘 31045 修复：dlink 拼接 access_token + 403 强制刷新 + 书源删除 SQLite 持久化

**Date**: 2026-08-06
**Task**: 百度网盘 31045 修复：dlink 拼接 access_token + 403 强制刷新 + 书源删除 SQLite 持久化
**Branch**: `master`

### Summary

修复百度网盘源远程下载 31045（access_token 验证未通过）：下载 dlink 统一拼接当前 access_token；下载 403 时强制刷新 token 重取 dlink 重试；API 遇 -6/110/31045 自动刷新；拦截 200+JSON 错误体；书源删除/清理失效记录同步删 SQLite 行。实测 dlink+token+UA → 302 → 200 PDF。已建并归档任务 08-06-baidu-31045-fix。

### Main Changes

- dlink 下载统一拼接当前 access_token（官方要求）
- 下载 403/31045 强制刷新 token 后重试
- removeSourceWithCleanup / purgeStaleRecords 同步删除 SQLite 行，修复删除重启复活

### Git Commits

| Hash | Message |
|------|---------|
| `a637ffc` | (see git log) |
| `0128e1f` | (see git log) |

### Testing

- [OK] cargo check + 8 个百度单测 + flutter analyze 通过；真实账号端到端下载 200

### Status

[OK] **Completed**

### Next Steps

- 跑 flutter run/build windows --release 全量构建，让 Dart 层修复进入正式包
