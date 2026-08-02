# 本地漫画转 CBZ（文件夹/ZIP 打包）
## Goal

刷新本地书源时，后台自动把漫画文件夹 / zip 打包为 CBZ（全局设置可关闭），
转换产物与原内容视为同一本漫画，阅读进度/标签无缝延续，无需手动逐本导出。

## Background（已确认事实）

- Rust zip crate 2.x 已启用 deflate feature，`ZipWriter` 可写 [app/rust/Cargo.toml]
- Folder 格式已支持 `is_comic_folder` 检测与 ComicInfo.xml / metadata.json 读取（README 当前功能-格式支持）
- 现有 API 风格：Rust 提供命令式 API（open_local_book / decode_cover 等），Dart 经 FRB 调用
- ZIP 与 CBZ 是同一容器格式：zip → cbz 仅需复制/改名，无需重打包
- 书源浏览页已有选择/批量模式（复选框 + 全选/单选）[app/lib/ui/source_browser.dart]

## Requirements

- **R1** 全局设置新增「自动转 CBZ」开关（默认开启；关闭后刷新不再转换）。
- **R2** 刷新本地书源时触发后台转换：漫画文件夹（无同名 .cbz）→ `name.cbz`；`.zip` 文件（无同名 .cbz）→ `stem.cbz`；产物写回原目录。
- **R3** 转换与"后缀名变更识别"打通：`normalizeComicPath` 剥离 zip 家族扩展名，文件夹 / zip / 生成的 .cbz 使用同一书 key，阅读进度、标签、封面自动延续。
- **R4** 书源列表隐藏已被同名 .cbz 取代的源条目（文件夹 / zip），避免书架重复；源文件保留在磁盘不删除。
- **R5** 转换后台执行不阻塞 UI（逐项顺序）：底部非模态进度条显示 `i/N` 与当前文件名，可点击取消；完成/取消后 SnackBar 汇总；单项目失败不中断其余项。
- **R6** Rust API：`export_folder_to_cbz`（顶层图片自然排序 + ComicInfo.xml / metadata.json）、`export_zip_as_cbz`（直接复制字节）。

## Acceptance Criteria

- [ ] 本地书源点「刷新」后，漫画文件夹自动生成同名 .cbz，书架上原文件夹/zip 不再重复显示
- [ ] 转换后打开 .cbz 阅读，原文件夹的进度/标签/封面延续（key 一致）
- [ ] 全局设置关闭「自动转 CBZ」后，刷新不再转换
- [ ] 生成的 .cbz 可被 RCH 与第三方阅读器打开，页序正确、ComicInfo.xml 保留
- [ ] `cargo test --lib` 通过（含 export 打包/复制测试）；`flutter analyze` 0 issues

## Out of Scope

- rar/cb7/tar 等非 zip 容器 → CBZ 重打包（仅 zip 家族可字节复制）
- 转换后自动删除原文件夹 / 原文件（当前保留在磁盘，仅列表隐藏）
- 手动导出入口（详情页/批量按钮已按用户要求移除）
- 图片重编码 / 压缩参数设置

## Open Questions

- 无阻塞问题。

## Decisions（2026-08-02 二次修订）
- 按用户反馈改为「刷新时后台自动转换」：移除详情页/批量手动导出 UI，Rust 打包 API 保留并由自动任务调用。
- 转换产物写回原目录（`name.cbz` / `stem.cbz`），仅本地来源；WebDAV 不转换。
- 文件夹打包仅顶层图片（FolderBook 只读顶层，保持一致）；ComicInfo.xml / metadata.json 原样进包；zip→cbz 直接复制字节。
- 列表隐藏已转换源条目（同名 .cbz 存在），源文件不删除。

## Verification（2026-08-02）

- [x] `cargo test --lib` 34 passed（含 export 3 项：自然排序打包 / 空目录拒绝 / zip 复制）
- [x] `flutter analyze` 0 issues
- [ ] `flutter run` 实测：刷新自动转换、开关生效、转换后进度延续
