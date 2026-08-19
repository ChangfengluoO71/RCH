# RCH v0.3.3

## 新增

- **夸克网盘书源**：Cookie 认证（pan.quark.cn 登录后 F12 复制粘贴），fid 目录浏览，三态打开策略（自动 / 整本 / 流式），封面与本地缓存，cookie 持久化回写

## 修复

- EPUB 图片路径按 HTML 目录解析（修复 ChainLP 漫画 EPUB 打开失败）
- PDF 阅读缺 pdfium.dll（exe 同目录查找 + 友好报错，CI 发布自动捆绑）
- 双页拼接亚像素溢出（round→floor），消除阅读时 `RIGHT OVERFLOWED` 黄黑条遮挡画面

---

安装包：`RCH-v0.3.3-windows-x64.exe`
