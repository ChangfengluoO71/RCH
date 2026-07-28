# TODO.md — 任务看板

> 状态看板:Backlog(待办)/ Doing(进行中)/ Waiting(待验证)/ Done(已完成)。
> 里程碑验收标准见 SPEC.md。

---

## 0. 工作流程(每轮闭环)

1. **读文档对齐** — 读 README(当前状态)+ TODO(看板)+ SPEC(目标)
2. **判断类型** — Bug修复 / 功能开发 / 重构 / 架构调整。架构级先征求用户确认
3. **小步开发** — 最小修改、不动已有功能。改 Rust API 后 generate 桥接
4. **验证** — `cargo test/check` + `flutter analyze` + `flutter run` 实测
5. **立即记档** — 更新 LOG、LOG-INDEX(起止行+HASH)、TODO(状态流转)
6. **交用户验证** — 说清改了什么、怎么测
7. **用户确认后** — 更新 README。若涉及 SPEC 变更再单独确认;TODO 归档

---

## Doing(进行中)

### ADR-016/017 标签系统重做（第27-29轮）
> **状态：第29轮完成，搜索系统统一。已通过 flutter analyze + cargo check，待用户实测确认。**

#### 已完成的修改（3轮共 14 项）
- [x] `BookTag` 缺 `==`/`hashCode` → `Set.contains()` 永久失效
- [x] `TagRepository.load()` 始终从 `metas`（ground truth）重建标签
- [x] `updateMeta()` 同步 author/genre/series 到 TagRepository
- [x] 标签过滤同时检查 `m.tags` 和 `m.metaTags`
- [x] `batchTag()` 元数据字段首次赋值时补 `link()` 调用
- [x] `TagRepository.load()` 错误处理不再静默吞异常
- [x] `LibraryStore.load()` 后立即 `_save()` 纠正脏数据
- [x] 删除"快速批量打标签"（功能重复）
- [x] 跨书源搜索：`globalSearch()` 遍历所有 sources × metas
- [x] 搜索栏内联标签补全（不用 Overlay）+ Chip 标签条
- [x] 搜索栏右侧地球切换按钮：筛选模式 ↔ 跨书源搜索
- [x] 搜索系统统一：筛选模式和全局模式共用 `_onSearch` + 补全 + Chip

#### 遗留
- [ ] `_tagId` 使用 `hashCode.toRadixString(36)` 有碰撞风险
- [ ] `_save()` 返回 `bool` 但调用方丢弃，待后续加 Toast

### M9 缓存基础设施
- [x] 五级缓存目录结构: raw/ cover/ thumb/ ai/ temp + 清理API
- [x] 统一下载器模块: downloader/ (队列/去重/并发/优先级/重试)
- [x] WebDAV 整本下载到 raw/ 缓存 (download_to_raw_cache + open_webdav_book 优先缓存 + 真实百分比进度条)
- [x] 设置页五级缓存分级管理面板: page/raw/cover/download/ai 独立大小查询 + 独立清理 + 封面内存缓存保护
- [x] Rust 侧 SQLite (rusqlite): 缓存索引/书源能力/ETag

### M2 AI 高清引擎
- [ ] Phase 1: 常驻 Worker 进程（命名管道通信 + 临时文件传图）
  - [ ] 制作/配置 librealesrgan-ncnn-vulkan.exe + 模型文件
  - [ ] Rust `ai/` 模块: Worker 进程管理(spawn 一次, stdin/stdout JSON 协议), 超时/崩溃重启
  - [ ] Rust `api/ai.rs`: 暴露 `super_resolve(page_bytes, scale) -> result_bytes`
  - [ ] Dart: 阅读器右键菜单 → 触发超分 → 进度弹窗 → 替换当前页显示
  - [ ] 结果缓存: 超分页写入 L2 磁盘缓存(key 含原图hash+模型名), 避免重复推理
- [ ] Phase 2: 共享内存传图（memmap2），消除磁盘 IO 和 PNG 编解码
- [ ] Phase 3: 抽象 `Upscaler` trait → 多模型可切换(Waifu2x/Anime4K/SwinIR) + ONNX Runtime

## Backlog(待办)

### M1 核心阅读闭环(未完成的) — ⚠ 低优先级,暂缓
- [ ] 双页拼接自动模式(需重新实现,解决异步字节加载的判定时机)
- [ ] 滚轮缩放与条漫滚动的优雅解耦(当前滚轮缩放已全部关闭,仅用 `+/-` 键)
- [x] WebDAV 整包下载进度提示（百分比进度条 + 轮询 + Rust AtomicU64 线程安全进度追踪）
- [ ] 自定义按键绑定(键盘 only 版本已实现,鼠标/组合键保留规划)

### 后续里程碑
- [x] ADR-016: Repository 层实施（Tags 已完成，Books/History/Settings 待后续）（第26-29轮已实施 TagRepository）
- [x] ADR-017: 标签独立建模（Tag 实体 + BookTag 关联，解决标签补全 Bug）（第26-29轮已实施）
- [ ] M4 复杂场景(智能拼页 / 旋转 / 裁边)
- [ ] M5 书源扩展(SMB / SFTP / 更多网盘)
- [ ] M6 Android 适配(手机 / 平板)
- [x] M7 标签筛选:按标签过滤书架(标签数据模型 + 跨书源搜索已落地)
- [ ] M8 智能拓展:智能扫描 + 元数据分层(AI辅助漫画管理,已在 SPEC 完整规划)

---

## Done(已完成)
- [x] 立项与总体规划(SPEC / LOG / DECISION / 文档体系建立)
- [x] 开发工具链安装(Rust / Flutter / VS BuildTools / FRB codegen)
- [x] 核心引擎:ByteSource / LocalSource / ZipBook 流式 / 解码 / 自然排序
- [x] 翻页流畅:LRU 内存缓存 + L2 磁盘缓存 + 后台并行预取 + Mutex 死锁修复
- [x] 海报墙书架:网格 + 封面缩略图(等比缩放+中心裁剪)
- [x] 阅读器:三种阅读模式(日漫/美漫/条漫) + `+/-/0` 键缩放 + 右键设置面板 + 页码跳转
- [x] 双页拼接(固定配对模式)
- [x] 自定义按键绑定(5 个动作,键盘 only)
- [x] WebDAV 书源:连接 / 浏览 / Range 流式阅读 / 整包下载回退 / 请求精简 / 错误信息改进
- [x] 书源管理:添加本地/WebDAV、固定显示、持久化凭据、详情页(备注/删除)
- [x] 主界面:左侧导航(书源 / 最近阅读 / 最多阅读 / 标签管理 / 搜索) + 设置
- [x] `#标签` 搜索栏自动补全(Overlay 下拉,输入 `#` 后弹出)
- [x] 漫画详细界面 + 封面自定义(选页+裁剪) + 标签编辑(补全)
- [x] 元数据标签系统:作者/类别/系列/标题/中文标题,红色图标区分,可折叠面板
- [x] 批量标签管理:书源浏览页选择模式、全选/单选、批量打标签(元数据智能识别)
- [x] 标签管理:统计(漫画数+阅读次数)、元数据/普通标签分类、重命名/删除(含元数据栏)
- [x] 应用设置:封面质量(低/中/高)、主题(白天/夜间)、全局阅读默认、缓存管理 & 失效清理
- [x] M2/M3/M5 技术方案决策(ADR-009/010/011: AI可插拔Worker + 格式独立实现 + 图片解码)
- [x] `archive/` → `document/` 重构: `Book` trait → `Document` trait（+ `DocumentMeta`）
- [x] EPUB + Folder + CB7 + CBT + PDF + CBR 6种新格式支持 (document/ 7格式引擎)
- [x] 书源编辑完善(密码遮盖+表单格式化) + 文件夹复选框批量选择(双击进子目录+递归展开)
- [x] 标签系统(补全):输入自动联想已有标签、自动清空、保存后生效
- [x] 文档体系:(LOG / LOG-INDEX / README / TODO / DECISION / SPEC) + 完整工作流程
