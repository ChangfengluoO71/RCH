# 本地漫画转 CBZ（文件夹/ZIP 打包）
## Goal

一键把本地漫画（文件夹或现有 ZIP/CBZ）导出为 CBZ 文件，便于统一归档、迁移到其他设备或阅读器。文件夹场景按图片自然排序打包。

## Background（已确认事实）

- Rust zip crate 2.x 已启用 deflate feature，`ZipWriter` 可写 [app/rust/Cargo.toml]
- Folder 格式已支持 `is_comic_folder` 检测与 ComicInfo.xml / metadata.json 读取（README 当前功能-格式支持）
- 现有 API 风格：Rust 提供命令式 API（open_local_book / decode_cover 等），Dart 经 FRB 调用
- ZIP 与 CBZ 是同一容器格式：zip → cbz 仅需复制/改名，无需重打包
- 书源浏览页已有选择/批量模式（复选框 + 全选/单选）[app/lib/ui/source_browser.dart]

## Requirements

- **R1** Rust API `export_folder_to_cbz(src_dir, out_path)`：按自然排序打包目录内图片；子目录推荐递归包含并保持相对路径（与 Folder 格式读取逻辑一致）；可选写入 ComicInfo.xml。
- **R2** Rust API `export_zip_as_cbz(src_path, out_path)`：直接复制字节并改名，不重压缩。
- **R3** Dart UI：书源浏览页选择模式与漫画详情页均提供「导出 CBZ」入口；输出路径通过目录选择对话框指定；同名文件覆盖需二次确认。
- **R4** 导出进度：大文件夹显示进度提示（可复用下载进度轮询模式或简单转圈 + 完成提示）；失败给出具体错误。
- **R5** 导出产物可被 RCH 与第三方阅读器正常打开（页面顺序正确）。

## Acceptance Criteria

- [ ] 文件夹（含 ComicInfo.xml）导出 CBZ 后，RCH 打开页序正确、元数据保留；第三方阅读器可打开
- [ ] ZIP → CBZ 导出为直接复制，耗时接近文件复制
- [ ] 批量选择多本漫画可逐个导出（或合并询问输出目录）
- [ ] 覆盖已有文件时二次确认
- [ ] `cargo test --lib` 通过（含打包后 zip 读回断言页序）；`flutter analyze` 0 issues

## Out of Scope

- CB7 / CBR / PDF / MOBI → CBZ 转档
- 图片重编码/压缩参数设置
- 导出进度断点续传

## Open Questions

- 无阻塞问题。子目录处理策略（R1）按推荐值：递归包含且保持相对路径。
