# 08-09-sync-rework 会话笔记

## 截图 OCR 方法（本机可用，优先于 view_image）

- 当前环境 view_image 不支持图片输入；截图一律用本机 WinRT OCR：
  PowerShell 加载 `Windows.Media.Ocr.OcrEngine`（zh-Hans-CN 语言包已装），
  `GetFileFromPathAsync` → `BitmapDecoder` → `SoftwareBitmap` → `RecognizeAsync`，
  按行输出文本。markitdown 在本机无 OCR 引擎（返回空），不要依赖。

## 双设备实测问题（2026-08-09 20:20 起）

- 现象：同步历史每分钟一条失败，`state/tags-8.jsonl / records-9/10 / metas-11 为空`；
  坚果云频繁 503；点其他设备云端源"此目录无漫画"。
- 根因：**MuMu 模拟器仍运行旧协议 APK**，在 Windows 新代码初始化（rev 1-6）后，
  旧协议把未变化实体写成空文件（rev 7-16），Windows 新代码读到被"空文件防御"拦截，
  每 60 秒轮询盲重试 → 503 循环。
- 处理：停 MuMu 同步（旧 APK 无法兼容新协议，需重装新 APK）；
  清理远端；Windows 重建后自愈全量重推。

## 修复补丁（第二轮）

- `prepare_sync`：远端无 manifest = 本地全量重推（自愈），不再推空 manifest。
- `sync_remote_revision` FRB：轮询先读 revision，未变化跳过全量。
- SyncEngine：失败退避期间轮询/防抖让位；轮询改轻量 revision 检查。
