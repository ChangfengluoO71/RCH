# RCH 施工日志(LOG)

> 记录项目每一步"改了什么 + 为什么",保证可溯源。
> 索引见 `LOG-INDEX.MD`(只记 日期 + 行为 + 行号 + HASH)。
> 规范见 `SPEC.md`,约定见 `CLAUDE.md`。

---

## 2026-07-26|第 1 轮:立项与总体规划

**行为**
- 分析用户 7 项需求:多书源(先本地+WebDAV,可扩展)、流式阅读、端侧 AI 高清、日漫/美漫/条漫、复杂场景(拼页/旋转)、多格式(ZIP/CBZ/PDF/EPUB/MOBI)、未来多端(Android)。
- 与用户确认三大技术决策(架构级,按 CLAUDE.md 先确认后动手):
  1. **技术栈 = Flutter(UI)+ Rust(核心引擎)**,经 flutter_rust_bridge v2 桥接,一套代码跑 Windows + Android。
  2. **AI 引擎 = NCNN + Vulkan 超分**(Real-ESRGAN/Real-CUGAN/waifu2x),列为后续里程碑 M2,不进第一期。
  3. **第一期(M1)范围 = 核心阅读闭环**:骨架 + 本地/WebDAV 书源 + ZIP/CBZ 流式阅读 + 三模式阅读器 + 书架。
- 制定里程碑路线图 M1~M6(见 SPEC.md 第 10 节)。
- 建立核心指导文档 `SPEC.md`(设计原则/技术栈/架构边界/插件规范/验收标准)。
- 初始化 `LOG.md` 与 `LOG-INDEX.MD`。
- 编写 `docs/SETUP.md` 开发环境搭建指南(用户选择自行安装工具链)。

**决策原因**
- **Flutter+Rust**:Flutter 一套 UI 覆盖 Windows 桌面与 Android;Rust 承担流式 IO/解压/解码/AI,性能强、内存安全、可跨端编译,契合"流式 + 端侧 AI + 多端"。
- **流式基石设计**:统一 `ByteSource`(Range 随机读)+ ZIP 只读尾部中心目录按需解单页,使几 GB 压缩包/网盘文件即点即读、边下边读。
- **AI 与 PDF 等后置**:7 项需求一次交付周期过长;M1 先跑通核心阅读闭环形成可用产品,AI(M2)、PDF/EPUB(M3)、拼页/旋转(M4)、MOBI/更多书源(M5)、Android(M6)分期推进,风险可控。
- **先立 SPEC**:按 CLAUDE.md,SPEC 是核心指导;动工前先确立边界与验收标准,后续施工有据可依。

**结果/状态**
- 文档就绪(SPEC/LOG/LOG-INDEX/SETUP);开发工具链(Flutter SDK / Rust / FRB codegen / VS C++ 构建工具)本机尚未安装,用户选择按 `docs/SETUP.md` 自行安装,装好后进入脚手架阶段。

---

## 2026-07-26|第 2 轮:环境就绪、工程骨架与最小阅读链路

**行为**
- 用户装齐工具链(Rust 1.97 / Flutter 3.44 / VS 2022 BuildTools / FRB codegen 2.12);Android SDK 未装(M6 才需,本期跳过)。
- `flutter_rust_bridge_codegen create app` 生成工程骨架(Flutter app + Rust cdylib + cargokit 构建集成),Windows 编译通过。
- 实现 Rust 核心引擎:
  - `source/`:`ByteSource` 统一字节源抽象 + `SourceReader`(Read+Seek 适配)+ 本地源(seek_read 随机读)。
  - `archive/`:`Book` trait + `ZipBook` 流式解析(只读文件尾部中心目录、按需解压单页、自然排序定页序)。
  - `decode/`:image 解码 + 降采样。
  - `util.rs`:自实现自然排序比较(替代 natural-sort crate,规避镜像索引未匹配问题)。
  - `api/book.rs`:FRB 暴露 open_local_book / book_page / close_book / list_local_dir。
- 前端最小验证页:打开 CBZ → 解码 → 显示 + 翻页。
- 验证:Rust 单元测试 3 项通过(自然排序、ZIP 流式+排序+解码、降采样保比例);用用户 `F:\comic\漫畫` 真实 CBZ 实测可打开翻页。

**决策原因**
- `ByteSource` + `SourceReader` 统一本地/远程字节访问,是流式阅读与书源可扩展的基石。
- 自实现自然排序:crates 镜像未匹配 natural-sort 包,自实现零依赖更可控。

**结果/状态**
- 最小链路打通,用户确认“整体不错、清晰度良好”;但翻页流畅度欠佳(每页现场解压解码,无缓存预取)、书架仅列表不美观 → 引出第 3 轮。

---

## 2026-07-26|第 3 轮:翻页流畅(缓存+预取)与海报墙书架

**行为**
- 流畅度:新增 `reader.rs` `Reader`(LRU 内存缓存 12 页 + std::thread 后台预取前后各 2 页 + 预取去重);`book_page` 改走缓存;前端阅读器改 PhotoViewGallery(滑动翻页 + 缩放 + 页面 Future 缓存)。
- 海报墙:书架改 GridView 网格海报墙;新增 `decode::decode_cover`(等比缩放 + 中心裁剪)与 `book_cover` API 生成封面缩略图;卡片含封面 + 标题 + 大小。
- 依赖:新增 Flutter 包 photo_view 0.15。

**决策原因**
- 翻页卡顿根因是每页现场解压解码;LRU 缓存 + 预取前后页,配合 PageView 预构建,翻到相邻页零等待。
- 采用 photo_view 处理“翻页 + 缩放”手势冲突,避免自实现手势协调。

**结果/状态**
- `cargo check` 与 `flutter analyze` 均通过;待 flutter run 实测流畅度与海报墙。封面自定义(选页 + 裁剪)为下一步。

---

## 2026-07-26|第 4 轮:修复阅读器缩放失效 + 建立文档体系

**本轮目标**
修复用户反馈的“缩放功能失效”;按 CLAUDE.md 新版建立文档体系(TODO / DECISION),并顺带优化阅读数据流。

**修改内容**
- 阅读数据流调整(ADR-004):Rust 阅读会话改为**只解压、缓存/预取原始页字节**(LRU 24 页、预取前后各 3 页),不再解码;Flutter 改用标准 `imageProvider`(MemoryImage + ResizeImage)解码显示。
- 前端阅读器:PhotoViewGallery 由 `customChild` 改为标准 `imageProvider` 用法;页面按需 `_ensure` 加载 + 翻页预取相邻。
- 新增文档:`TODO.md`(任务看板)、`DECISION.md`(ADR-001~004);更新 `README.md` 反映当前可运行版本。

**修改原因**
- 缩放失效根因:photo_view `customChild` 无法确定缩放边界;标准 `imageProvider` 是 photo_view 缩放最成熟的用法。
- 顺带优化:Rust 只解压更轻、原始字节更小可预取更多页;Flutter image cache 统一管解码,职责更清晰(KISS)。
- CLAUDE.md 新版要求建立 TODO / DECISION 等文档。

**影响范围**
- `app/rust/src/reader.rs`(重写为原始字节缓存)、`app/rust/src/api/book.rs`(book_page 返回原始字节;Reader::new 去掉 max_dim)、`app/lib/main.dart`(阅读器改 imageProvider)。
- 新增 `TODO.md` / `DECISION.md`;更新 `README.md`。
- 封面功能不受影响(仍走 Rust `book_cover` 解码)。

**是否完成**
已完成:`cargo check`、`flutter analyze` 通过;应用可编译启动。缩放实际手感待用户实测确认。

**遗留问题**
- 缩放实际手感待用户确认;若个别页格式解码异常需单独排查。
- 封面自定义、WebDAV、三模式、进度持久化仍在 Backlog。

---

## 2026-07-26|第 5 轮:滚轮自由缩放 + 阅读器改单页 PhotoView

**本轮目标**
恢复用户习惯的滚轮自由缩放手感(photo_view 双击缩放为固定档位,不够自由)。

**修改内容**
- 阅读器由 PhotoViewGallery(多页画廊)改为**单页 PhotoView + PhotoViewController**,加 Listener 捕获滚轮:向上放大、向下缩小,连续自由调节。
- 翻页方式相应调整为键盘 ←/→ 与底部按钮;翻页时 `_photoCtrl.reset()` 重置缩放。
- 修正脚手架过时的 `integration_test/simple_test.dart`(MyApp → RchApp 冒烟测试),使 flutter analyze 通过。

**修改原因**
- PhotoViewGallery 不便控制当前页缩放;photo_view 官方滚轮缩放推荐单页 PhotoView + PhotoViewController。
- 桌面端以滚轮缩放、键盘翻页为主流交互,更契合 Windows 用户使用习惯。

**影响范围**
- `app/lib/main.dart`(阅读器改单页 PhotoView + 滚轮;去掉 gallery 与滑动翻页)。
- `app/integration_test/simple_test.dart`(改冒烟测试)。
- 书架、缓存预取、封面等不受影响。

**是否完成**
已完成:flutter analyze 通过,应用可启动。滚轮缩放手感待用户确认。

**遗留问题**
- 滚轮缩放当前以画面中心为锚点;以鼠标位置为锚点更自然,可作后续优化。
- 触摸滑动翻页手势已移除(桌面以滚轮/键盘为主);如需可再加。

---

## 2026-07-27|第 6 轮:接入 WebDAV 书源(远程流式阅读)

**本轮目标**
实现 WebDAV 书源:远程 ZIP/CBZ 与本地一样“即点即读”,只发必要的 Range 请求,不下载整包。

**修改内容**
- 新增 `source/webdav.rs`:`WebDavClient`(reqwest blocking,PROPFIND 列目录 / file_size / Range 探测与读取)+ `WebDavFile`(实现 ByteSource,每次 read_at 即一次 HTTP Range);手写 PROPFIND multistatus XML 解析(quick-xml)与路径 percent 编解码。
- API:`webdav_connect`(连接+测试,返回会话与根路径)/ `webdav_list` / `webdav_disconnect` / `open_webdav_book`(先探测 Range 支持再打开);`book::register_book` 抽出复用,本地与远程统一注册阅读会话。
- 前端:新增 `ui/webdav_page.dart`(连接表单 + 远程目录浏览);`ui/reader_page.dart` 支持 webdavSession;拆分 `main.dart` 为 `ui/`(common/library/reader/webdav)多文件,结构更清晰。

**修改原因**
- 用户核心诉求之一是 NAS/WebDAV 远程漫画流式阅读;ByteSource 抽象使远程源与本地复用同一 ZIP 流式路径。
- 按 SPEC,WebDAV 客户端用 reqwest 手写以精确控制 Range;不引入现成客户端库。
- 修复两处真实 bug:quick-xml 把自闭合 `<d:collection/>` 报为 Empty(漏判目录);href 应先取 path 再 percent-decode(否则中文/空格被二次编码)。

**影响范围**
- 新增 `rust/src/source/webdav.rs`、`rust/src/api/source.rs`、`app/lib/ui/{common,library_page,reader_page,webdav_page}.dart`。
- 修改 `Cargo.toml`(加 reqwest/quick-xml/percent-encoding)、`source/mod.rs`、`api/{mod,book}.rs`、`main.dart`(拆分)。
- 新增依赖:reqwest 0.12(blocking+rustls)、quick-xml、percent-encoding。

**是否完成**
已完成:cargo test 7 项全过(含 WebDAV 解析/编码/href)、flutter analyze 通过、应用可启动。真实 NAS 流式效果待用户实测。

**遗留问题**
- WebDAV 流式需真实服务器验证(当前无测试服务器);个别服务器 Range 兼容性待观察(当前不支持则报错提示,整文件回退未做)。
- 远程目录浏览暂为列表(远程封面海报墙需封面缓存,后续)。
- WebDAV 凭据当前仅内存会话,未持久化(并入后续“书源管理”)。

---

## 2026-07-27|第 7 轮:修复 WebDAV 连接失败时的 tokio runtime panic

**本轮目标**
修复用户实测 WebDAV 连接时报错 `Cannot drop a runtime in a context where blocking is not allowed`。

**修改内容**
- `webdav_connect`:client 创建与连接测试全部移入 `spawn_blocking`;`webdav_disconnect` 改为在 blocking 线程销毁 client。

**修改原因**
- reqwest::blocking::Client 内部自带 tokio runtime;`webdav_connect` 是 async fn,连接失败时 client 在异步上下文被 drop,触发 tokio panic,掩盖真实连接错误(如 401 / 网络不通)。
- 修复后失败路径显示真实 HTTP 错误,便于排查。

**影响范围**
- `rust/src/api/source.rs`(connect / disconnect 两函数)。

**是否完成**
已完成:cargo check、generate 通过,应用可启动。待用户重试连接并提供真实错误(若仍失败)。

**遗留问题**
- 真实 WebDAV 连接尚未打通(待用户提供服务器类型与真实错误);可能涉及认证方式(Basic/Digest)或地址格式。

---

## 2026-07-27|第 8 轮:主界面(书源/最近/最多/搜索)+ WebDAV 流式性能重构

**本轮目标**
① 按用户要求重做主界面:左侧导航(书源 / 最近阅读 / 最多阅读 / 搜索),WebDAV 源固定到主界面;② 根治 WebDAV 加载慢。

**修改内容**
- 持久化:新增 `store/models.dart`(BookSource/ReadRecord)与 `store/library_store.dart`(JSON 存应用数据目录,ChangeNotifier 通知 UI)。书源 / 阅读记录 / 进度落盘。
- 主界面:新增 `ui/home_page.dart`(左导航 + 右内容 + 添加书源对话框)、`ui/source_browser.dart`(书源浏览:本地海报墙 / WebDAV 列表,目录下钻)、`ui/comic_cover.dart`(统一本地/WebDAV 封面 + 复用卡片)、`ui/opener.dart`(打开书统一入口:WebDAV 重连 + 进度恢复);`main.dart` 入口改 HomePage;`reader_page` 加 source/initialPage 记录进度。
- WebDAV 性能:ZipBook 重构为“打开读中心目录收集各页偏移/大小,翻页按需一次 Range 下载解压”;`Book::page_bytes` 改 `&self` 去共享锁;`SourceReader` 加 256KB 读放大缓存;`WebDavFile` 去块缓存改无状态;`Reader` 去 Mutex 实现并行预取。
- 新增 `webdav_cover` API(远程封面);新增依赖 flate2。
- 修复 `register_book` 被误改为 pub 致 FRB 桥接 dyn Book 出错(改回 pub(crate));修复侧边栏 ListTile 选中态被背景色遮挡(根改 Material)。

**修改原因**
- 用户要求主界面板块化 + WebDAV 源固定;需持久化书源/记录/进度,选型 JSON(用户确认),布局左侧导航+右侧内容(用户确认)。
- WebDAV 慢根因:zip 单例共享锁致读页串行 + 单窗块缓存 thrash;改为各页独立 Range 下载解压、并行预取,速度接近带宽。

**影响范围**
- 新增 `store/{models,library_store}`、`ui/{home_page,source_browser,comic_cover,opener}`;重写 `rust archive/zip.rs`、`reader.rs`;改 `source/{mod,webdav}`、`api/{book,source}`、`main.dart`、`reader_page.dart`;`Cargo.toml` +flate2。

**是否完成**
已完成:cargo test 6 项全过、flutter analyze 通过、应用可启动。WebDAV 提速与新主界面待用户实测。

**遗留问题**
- WebDAV 打开书仍需读中心目录(读放大后约几个请求),超大目录首次略慢可接受,后续可加中心目录缓存。
- 远程 WebDAV 浏览为列表(远程海报墙待封面缓存);封面自定义未做。

---

## 2026-07-27|第 9 轮:修复书源切换不刷新 + L2 磁盘缓存(WebDAV 提速)

**本轮目标**
修复用户反馈:① 切换书源后主界面卡住不刷新(仍显示旧书源内容);② WebDAV 阅读仍慢。

**修改内容**
- 书源切换 bug:给 `SourceBrowser` 加 `key: ValueKey(source.id)`,切换书源时强制重建并重新加载(此前 StatefulWidget 复用旧 State,始终显示上一个书源内容)。
- L2 磁盘缓存:`Reader` 新增磁盘缓存层——读过的页字节写入应用数据目录(`%APPDATA%/RCH/cache/<hash>/<page>.bin`);`open_local_book`/`open_webdav_book` 传入稳定 cache_ns(`local|path`、`webdav|origin|path`);读页顺序 L1 内存 → L2 磁盘 → 下载并写盘。
- `WebDavClient` 暴露 `origin()` 供 cache_ns 使用。

**修改原因**
- 书源切换不刷新根因:Flutter StatefulWidget 在 `widget.source` 变化时复用旧 State,未重新 `_init`;加 `key` 是标准解法。
- WebDAV 慢:首次下载受服务器带宽/上行限制难避免,但重复阅读可靠 L2 磁盘缓存实现"秒开",大幅提升实际体验;本地重复解压也受益。

**影响范围**
- `app/lib/ui/home_page.dart`(加 key)、`rust/src/reader.rs`(L2)、`rust/src/api/{book,source}.rs`(cache_ns)、`rust/src/source/webdav.rs`(origin)。

**是否完成**
已完成:cargo test 6 项全过、flutter analyze 通过、应用可启动。待用户验证书源切换与 WebDAV 重复阅读速度。

**遗留问题**
- WebDAV 首次阅读速度仍受服务器带宽/上行限制(物理约束);可再优化:并发预取深度、中心目录缓存、HTTP/2。
- 磁盘缓存暂无清理策略(后续加容量上限/LRU 清理/手动清除)。
- 115 等网盘返回 429/401 时,若密码有误也应给出明确提示。

---

## 2026-07-27|第 10 轮:WebDAV 请求精简 + 错误信息改进 + 整包下载优化 + 文档体系补全

**本轮目标**
① 解决用户反馈的"Too many unsuccessful sign-in attempts"与多次请求问题;
② 改进整包下载(减少冗余包装、缓存复用);
③ 更新全量文档并形成工作流程。

**修改内容**
- WebDAV 请求精简:
  - `webdav_connect` 改用单次 PROPFIND(Depth:0 只测根,不列目录)测试连接;
  - `range_supported` 只发一次 `bytes=0-0` 请求,复用其结果;
  - `download_full` 缓存文件命中时直接用,不再重复下载。
- 错误信息改进:
  - `propfind` 和 `check_connection` 失败时解析 HTTP 状态码(401/403/429),给出中文提示(用户名密码错、没有访问权限、请求频繁等),并带上服务器原始返回片段。
- 整包下载优化:
  - `download_full` 改为直接返回 `WebDavFile` 包装(不再额外做 `LocalFile` 装箱);
  - `WebDavFile` 新增 `local_cache` 模式(缓存文件句柄),`read_at` 直接调 `seek_read`,零额外开销。
- 文档补全:
  - 更新 `TODO.md` 反映真实进度与工作流程;
  - 新增 `DECISION.md` 的 ADR-005~008(整包下载回退、JSON 持久化、封面自定义、连接测试与请求精简);
  - 更新 `README.md` 为当前可运行版本;
  - 更新 `LOG-INDEX.md`(起止行+HASH)。

**修改原因**
- 115 WebDAV 的 429/401 限流与之前多次请求相关,精简请求量可减少触发频率;清晰错误提示帮助用户判断是密码错还是网络问题。
- 整包下载过程需要能让 ZipBook 正常读取,`WebDavFile` 包装比 `LocalFile` 更轻,且便于后续做进度回调。
- CLAUDE.md 新版明确每个文档何时更新,本轮补上之前积压的文档更新,形成标准工作流程。

**影响范围**
- rust/src/source/webdav.rs(请求精简 + 错误改进 + 整包下载优化)
- rust/src/api/source.rs(连接测试改 check_connection + open_webdav_book 适配)
- TODO.md / DECISION.md / README.md / LOG.md / LOG-INDEX.md 更新

**是否完成**
已完成:cargo check 通过、应用可启动。文档更新已全部执行。待用户验证 115 书源连接与整包下载。

**遗留问题**
- 不支持 Range 的大文件下载无进度条(后续需 Dart 侧加下载进度通知)。
- 缓存目录长期占用需清理策略(后续加容量上限 / 手动清除)。

---

## 2026-07-27|第 11 轮:缓存管理面板 + 失效数据清理 + 未来拓展规划

**本轮目标**
按用户要求:① 在设置最显眼位置加缓存管理(显示总占用、清页面/下载/全部缓存、显示缓存目录);② 切换封面质量自动清封面内存缓存;③ 清理失效漫画记录与缓存;④ 记录未来智能扫描拓展计划到 SPEC。

**修改内容**
- Rust:新增 `cache.rs`(递归计算目录大小、清空缓存)、`api/cache.rs`(暴露 L2 页面/下载/全部缓存的大小与清理接口给 Dart)。
- 设置面板:新增“缓存管理”板块(磁盘总占用、清下载/清页面/清全部按钮 + 释放字节提示、目录路径);切换封面质量自动 `ComicCover.clear()`;新增“清理失效漫画记录与缓存”按钮。
- 失效清理:`library_store.dart` 新增 `purgeStaleRecords()`:扫描本地源文件是否存在,不存在则清除对应记录与 meta;WebDAV 源暂不做文件存在探测(需重连,成本高)。
- 未来规划:SPEC 新增智能扫描拓展(自动提取漫画名/识别同作者同系列/替换版本精确操作),记录为 M8 里程碑。

**修改原因**
- 用户明确要求缓存清理放在设置最显眼位置;封面质量切换后应重新扫描(否则原分辨率缓存还在)。
- “在 webdav 端删了漫画,这边检测不到了就删信息” → purgeStaleRecords 实现本地源检测(WebDAV 文件存在探测因需重连,后续加)。
- 智能扫描(封面识别/同系列/版本替换)是长期愿景,先记入 SPEC 避免丢失。

**影响范围**
- rust/src/cache.rs(新)、rust/src/api/cache.rs(新)、rust/src/api/mod.rs、rust/src/lib.rs
- app/lib/ui/home_page.dart(设置面板扩增)
- app/lib/ui/comic_cover.dart(加 clear())
- app/lib/store/library_store.dart(加 purgeStaleRecords)
- SPEC.md(加 M8 智能拓展)
- rust/Cargo.toml(无需加任何新依赖)

**是否完成**
已完成:cargo check、flutter analyze 通过,应用可启动。缓存管理面板与清理功能待用户实测。

**遗留问题**
- WebDAV 源的文件存在探测暂未实现(需靠重连+PROPFIND 探测,成本较高,后续可做后台静默扫描)。
- 智能扫描(漫画名提取/同系列识别/版本替换)记录在 SPEC M8,待未来实施。

---

## 2026-07-27|第 12 轮:双页拼接精简 + 文档总结(遗留问题记录)

**本轮目标**
按用户要求:① 删除自动双页模式(仅保留关/开);② 删除翻页步长设置;③ 精简 `pairOf` 配对逻辑;④ 补全文档,重点记录双页拼接开发过程中遇到的问题。

**修改内容**
- 双页拼接模式精简:仅保留 `off`(关)和 `force`(开)。
- `pairOf` 简化为固定配对:不跳过首页 `(0-1)(2-3)(4-5)...` ;跳过首页 `(0)(1-2)(3-4)(5-6)...`。
- 删除 `_isWide`(宽图检测不再需要)、`_step`(步长)。
- 全局设置和阅读设置面板同步删除"自动"选项和"翻页步长"滑块。
- 文档更新:追加 LOG 第12轮并重点记录双页拼接历程中的 5 个问题;更新 LOG-INDEX。

**双页拼接开发历程与遇到的问题**(供未来参考)
1. **customChild 缩放失效**:photo_view 的 customChild 无法确定缩放边界,改用标准 imageProvider。
2. **Mutex 死锁**:`if let Some(img) = cache.lock()...get() { spawn_prefetch() }` 中临时 guard 存活到 block 结束,`spawn_prefetch`在同一线程 lock 同一把锁→死锁。修复:把 guard 限于小代码块,先取出结果再释放锁。
3. **字节数据传错致双页重复**:给 `_buildPair` 传 `leftBytes: _bytes[_page]` 但日漫模式下 left 是 `_page+1`,导致左右页拿到同一份字节。修复:直接用 `pairOf` 返回的索引取字节。
4. **页消失/页号跳跃**:由于 `pairOf` 的首页跳过公式和翻页步长不吻合,配对在翻页后漂移,导致个别页被永久跳过,表现为"第2页消失"或"第3页跳到第5页"。多次修改未能彻底解决。
5. **最终决策**:删除自动模式及翻页步长设置,只保留固定配对的强制双页模式,简化掉所有边界条件。

**修改原因**
- 自动模式依赖图片字节是否已加载,与翻页节奏不匹配,边界情况过多。
- 用户确认"不要了,浪费时间太久了"——删掉有缺陷的功能,保持项目精简稳定。

**影响范围**
- `DualPageMode` 枚举精简(删除 auto);`AppSettings` 删除 `dualPageStep`
- `reader_page.dart` 删除 `_isWide`、`_step`、pairOf 精简、设置面板删除自动模式和步长滑块
- `home_page.dart` 删除翻页步长滑块和自动模式选项
- LOG.md 追加本记录;LOG-INDEX 同步更新

**是否完成**
已完成:flutter analyze 通过,应用可启动。

**遗留问题**
- 双页拼接的固定配对在某些漫画中可能仍有偶发性页号跳跃(但排除自动模式后概率大减)。
- 自动模式(含宽图判定)以及翻页步长功能暂时放弃,日后有更好方案可重新实现(需彻底解决字节异步加载的判定时机问题)。

---

## 2026-07-27|第 10 轮:设置模块 + 漫画详细界面 + 封面自定义 + WebDAV 海报墙

**本轮目标**
按用户要求:① 主界面加设置模块(封面质量/主题);② 每个漫画一个详细界面(自定义封面/标签/简介/感想);③ WebDAV 也支持海报墙;④ 删除测试书源。

**修改内容**
- 数据模型:`models.dart` 加 `BookMeta`(封面页/裁剪/标签/简介/感想)与 `AppSettings`(封面质量/主题);`library_store` 持久化 metas + settings。
- 设置模块:主界面侧边栏加“设置”入口;设置面板含封面质量(低/中/高,默认中)与主题(白天/夜间);`main.dart` 启动加载设置并按 themeMode 切换 light/dark;封面组件按质量档位取封面尺寸。
- 漫画详细界面:新增 `ui/book_detail_page.dart`(大封面 + 进度 + 标签增删 + 简介 + 感想,自动持久化);卡片点击由“直接阅读”改为“进入详情”,详情里“开始阅读”。
- 封面自定义:新增 `ui/cover_editor_page.dart`(翻页选帧 + 拖框裁剪,存 coverPage + 相对裁剪区);Rust 端 `decode_cover` 支持相对裁剪,`book_cover`/`webdav_cover` 加 crop 参数(新 CropRect);封面组件按 BookMeta(页+裁剪+质量)生成并缓存。
- WebDAV 海报墙:书源浏览加“海报墙/简略列表”模式切换,WebDAV 也可海报墙(webdav_cover)。
- 主题修复:侧边栏背景由硬编码黑改为 surfaceContainerLow,白天模式正常。
- 删除测试书源“本地漫画”与 testdata 目录(用户确认不再需要)。

**修改原因**
- 用户明确要求设置模块、详细界面、封面自定义、WebDAV 海报墙、删测试书源。
- 封面质量默认“中”以平衡扫描速度与清晰度;主题切换是常见个性化诉求。
- 封面自定义(选页+裁剪)是用户多次强调的核心体验,落盘到 BookMeta。

**影响范围**
- 新增 `ui/{book_detail_page,cover_editor_page}.dart`;改 `store/(models,library_store)`、`ui/(home_page,comic_cover,source_browser,reader_page,opener)`、`main.dart`;rust `decode.rs`、`api/(book,source).rs`;删除 testdata。

**是否完成**
已完成:cargo test 6 项全过、flutter analyze 通过、应用可启动。待用户验证设置、详细界面、封面自定义、WebDAV 海报墙。

**遗留问题**
- 阅读界面设置(阅读模式/AI 超分)、阅读三模式、自定义按键在 Backlog。
- 标签筛选(按标签过滤书架)待 M7;封面编辑器暂不支持缩放图片后再裁剪。

---

## 2026-07-29|第 13 轮:条漫缩放冲突修复 + 自定义按键绑定(未完成,保留现场)

**本轮目标**
① 解决条漫模式滚轮翻页与缩放手势冲突;② 实现自定义按键绑定(键盘/鼠标/组合键三栏),放在全局设置中。

**修改内容**
- 数据模型:新增 `KeyBind` 类(forwardKey/backKey/forwardWheel/backWheel/zoomMod),持久化到 `library.json`。
- 自定义按键面板:全局设置页新增"键盘/鼠标滚轮/组合键"三栏配置。
- `reader_page.dart` 重写了滚轮分配逻辑和按键处理:
  - 将 `Listener` 从单页 `_buildImage` 提到 `_buildMangaOrComic` 最外层,使双页模式也能收到滚轮事件。
  - 将条漫的 `Listener` 放在 `InteractiveViewer` 外面,避免事件被截断。
  - 滚轮处理逻辑:组合键(修饰键+滚轮)缩放 > 日漫/美漫默认滚轮缩放 > 滚轮翻页(需在设置中开启)。
  - 键盘自定义:前进/后退按键从设置读取,条漫模式下前进=向下翻页,后退=向上翻页。

**已知未解决的 bug**
1. **日漫/美漫模式缩放失灵**:双页模式下 `Listener` 虽然移到了 Stack 最外层,但在某些图片加载或组件重建的时序下滚轮仍不能正确传递到 `_onPointerSignal`。单页模式偶尔也无法接收滚轮事件,表现不稳定。
2. **条漫模式未区分滚轮缩放与滚轮翻页**:在特定的滑动顺序下,`InteractiveViewer` 和 `Listener` 的事件竞争导致滚轮下滑对条漫同时触发了滚动和翻页,而非按照设计只触发其一。
3. **组合键缩放无法稳定工作**:`HardwareKeyboard.instance.isLogicalKeyPressed` 在某些 Windows 环境下无法准确捕获 Ctrl/Shift/Alt 的瞬时状态,导致组合键触发不可靠。

**修改原因**
- 用户希望条漫既能滚轮翻页又能缩放,且缩放可通过自定义按键触发。
- 当前实现因 Flutter 手势竞争机制、Listener 与 InteractiveViewer 的事件传播顺序、以及 Windows 平台键盘状态获取的局限性,未能完全满足需求。

**影响范围**
- `models.dart`(KeyBind 类)、`reader_page.dart`(滚轮/键盘重写)、`home_page.dart`(设置面板中的自定义按键三栏 UI)

**是否完成**
未完成。用户确认"这个功能就这样了,停下不再修"。当前条漫模式下的滚轮翻页/缩放、日漫美漫的滚轮缩放、以及自定义按键绑定功能**均处于不稳定状态,后续重新设计交互方案时再彻底解决**。

**遗留问题**
- 条漫模式交互方案建议:方向键=翻页、Ctrl+滚轮=缩放、裸滚轮=滚动——但需要解决`InteractiveViewer`和`Listener`的竞争
- 日漫/美漫的缩放交互建议:滚轮默认缩放、方向键=翻页——但需要稳定的`Listener`挂载位置
- 按键自定义的 UI 方案可行(三栏设计),但后端的绑定与事件分发需要重新设计架构
- 标签筛选(按标签过滤书架)待 M7;封面编辑器暂不支持缩放图片后再裁剪。

---

## 2026-07-30|第 14 轮:元数据标签系统 + 批量打标签 + 书源详情页 + 标签管理面板

**本轮目标**
按用户要求:① 每个漫画详情加作者/类别/系列/标题/中文标题元数据栏,删简介/感想;② 批量打标签元数据智能识别;③ 书源列表小三点→详情/删除/备注;④ 标签管理区分元数据/普通标签(可折叠+颜色区分),元数据标签也可重命名/删除;⑤ 漫画详情恢复简介/感想。

**修改内容**
- 数据模型:`BookMeta` 加 `author`/`genre`/`series`/`title`/`chineseTitle` 五个元数据字段;`BookSource` 加 `note`(备注),并改为可变的 class(去掉 const 构造函数)。
- 批量标签智能识别:`batchTag` 全库扫描某标签值是否已作为元数据出现过→是则自动填入对应栏(空栏时填入,已填充则落回普通标签);`deleteTag`/`renameTag` 同样处理 author/genre/series 三个元数据栏。
- 标签管理面板:元数据标签用 `ExpansionTile` 折叠(红色图标),普通标签平铺(黄色图标);统计包含元数据和普通标签;元数据标签也支持重命名/删除。修复 `recordsByTag` 从 `metas` 扫描(未打开过的漫画也能显示)。
- 书源页批量操作:进入选择模式后每张卡片/每行出现复选框,支持全选/取消/退出选择,批量打标签对话框含标签自动补全(红色/黄色区分元数据和普通标签)。
- 书源列表 `⋯` 改为书源详情对话框(可编辑备注)和删除书源(连带清理记录和元数据),代替原来的 ✖ 按钮。
- 简介感想恢复:漫画详情页标签下方恢复"简介"和"感想"两个多行文本框。
- 标签管理页点击漫画卡片→进入漫画详情(BookDetailPage),不再直接打开阅读。

**修改原因**
- 用户希望在标签管理系统中区分普通标签和元数据标签,并为未来的智能扫描铺路。
- 批量标签需要智能判断标签类型,避免元数据被误放入普通标签栏。
- 书源需要详情页面编辑备注,方便用户管理多个书源。
- 漫画详情页的简介感想是核心功能,不应删除。

**影响范围**
- `store/models.dart`(+author/genre/series/title/chineseTitle/note 字段)
- `store/library_store.dart`(batchTag/renameTag/deleteTag 含元数据;recordsByTag 从 metas 扫描;removeSourceWithCleanup/updateSourceNote)
- `ui/book_detail_page.dart`(+元数据栏 title/cnTitle;恢复简介感想)
- `ui/home_page.dart`(标签管理元数据折叠+彩色区分+跳详情;书架无 chipt;书源 ⋯→详情/删除)
- `ui/source_browser.dart`(选择模式+批量打标签+自动补全+颜色区分)
- `reader_page.dart`(统一缩放系统:日漫/美漫单页 PhotoView+双页 InteractiveViewer+条漫 InteractiveViewer 禁滚轮+三种模式+/-键缩放;滚轮缩放全部删除;自定义按键键盘 only 5 动作)

**是否完成**
已完成:/flutter analyze 通过,应用可启动。批量标签智能识别、标签管理元数据折叠、书源详情页均已上线,待用户验证。

**遗留问题**
- 批量打标签对话框输入标签时元数据补全 list 需要在 optionsBuilder 中限定,目前全展示。
- 批量打标签自动识别元数据仅遍历全库已有值,首次创建的标签仍为普通标签。

---

## 2026-07-27|第 15 轮:书源编辑完善 + 文件夹复选框批量选择

**本轮目标**
按用户要求:① 书源编辑对话框中密码需要遮盖显示;② 文件夹也支持批量选择(勾选文件夹后递归展开所有子目录漫画)。

**修改内容**
- `home_page.dart`:编辑书源对话框密码字段加 `obscureText: true`;`_showEditSource` 从超长单行改多行格式。
- `source_browser.dart`:
  - 新增 `_collectComicsRecursive`:递归遍历目录收集所有 .cbz/.zip 文件路径。
  - 新增 `_batchTagFromSelection`:批量打标签前自动判断选中项是文件夹还是漫画,文件夹递归展开子漫画,统一打标签。
  - 「全选」按钮改为包含文件夹(不再只选漫画文件)。
  - 海报墙选择模式下:文件夹卡片不再直接进入子目录,改为单击勾选、双击进入(避免手势竞争);取消勾选时退出选择态,恢复正常单击进入。
  - 列表视图选择模式下:文件夹行显示复选框+箭头按钮(进入子目录)。

**修改原因**
- 用户反馈「没有编辑书源」——实际上编辑书源已在第14轮实现,仅密码字段未遮盖;本次补上 `obscureText`。
- 多层级目录结构下,手动逐层进入打标签效率低;文件夹批量选择后递归展开是自然需求。

**影响范围**
- `ui/home_page.dart`(`_showEditSource` 密码遮盖+格式化)
- `ui/source_browser.dart`(文件夹选择+递归展开+手势修复)

**是否完成**
已完成:flutter analyze 通过(仅3条 info 级风格提示),应用可启动,待用户验证。

**遗留问题**
- 选择模式下双击进子目录，Flutter 默认双击间隔约300ms；可用箭头按钮立即进入。
- 原始超长单行代码导致 Dart 解析器间歇性解析失败(文件行数增到127行后触发)；本轮已将 `home_page.dart` 全部方法重写为多行格式，根治此问题。

---

## 2026-07-27|第 16 轮:架构升级 — AI 超分 + 格式扩展技术方案确认

**本轮目标**
用户确认进入下一阶段：支持更多格式（PDF/EPUB/MOBI/CBR 等）和 AI 超分。本轮完成**技术调研与架构决策**，不写代码。

**修改内容**
- **ADR-009**（AI 超分）：采用**三层可插拔 Worker 架构**。Phase 1 常驻 Worker 进程 + 命名管道通信 + 临时文件传图；Phase 2 共享内存传图（memmap2）消除磁盘 IO；Phase 3 抽象 `Upscaler` trait 支持多模型 + ONNX Runtime 后端。**不选每张图 spawn 一次 CLI**（Vulkan 初始化 ~500ms 太慢），**不选 DLL/FFI**（模型 bug 会崩主程序）。
- **ADR-010**（格式扩展）：统一 `Document` trait（page_count + metadata + page_bytes），每种格式独立实现。**不选 MuPDF**（漫画阅读器不需要重型排版引擎），**不选 pdfium 一统**（各格式用最合适的纯 Rust 库）。
  - PDF: `pdfium-render` | EPUB: `zip` + `quick-xml`（漫画 EPUB 本质是 ZIP+图片，无需排版）| CB7: `sevenz-rust` | CBT: `tar` | Folder: 目录枚举 | MOBI: 后台转 EPUB | CBR: `unrar`
- **ADR-011**（图片解码）：继续 `image` crate，AVIF 按需扩展。
- **SPEC 修订**（v1 → v1.1）：
  - AI 引擎：NCNN CLI 子进程 → 可插拔 Worker 架构
  - 格式引擎：`ArchiveParser` → `Document` trait，新增引擎列和 CB7/CBT/Folder 格式
  - 架构图：`archive/` → `document/`，新增 `ai/` 模块
  - 关键依赖更新：`mupdf-sys` 替换为 `pdfium-render`/`quick-xml`/`sevenz-rust`/`tar`/`interprocess`/`memmap2`
- **TODO** 重写 Doing 区：M1 低优先级标注暂缓；M2/M3/M5 拆分为 Phase 子任务。
- **DECISION.md** 新增 ADR-009/010/011。

**修改原因**
- 用户明确下一阶段：「格式扩展（PDF/EPUB/MOBI 等）+ AI 超分」
- 用户提供了详细的架构建议（三层 Worker + 常驻进程 + 不绑 MuPDF + MOBI 后台转 EPUB），经分析全部采纳
- SPEC 需要同步修订以反映新的技术路线

**影响范围**
- 文档层：SPEC.md / DECISION.md / TODO.md / LOG.md / LOG-INDEX.md
- 尚无代码变更；后续第 17 轮开始实施 `archive/` → `document/` 重构

**是否完成**
已完成：所有架构文档已更新，进入实施待命状态。

**遗留问题**
- pdfium-render 在 Windows 上需 pdfium.dll（可从 Chrome 提取或 vcpkg 安装）
- librealesrgan-ncnn-vulkan 的模型文件（.param/.bin）需打包或提供下载路径
- unrar 许可需在发布时明确标注

---

## 2026-07-27|第 17 轮:archive/ → document/ 重构（格式引擎统一）

**本轮目标**
将 `archive/` 模块重命名为 `document/`，`Book` trait 升级为 `Document` trait（新增 `metadata()` 返回 `DocumentMeta`），所有引用全量更新。这是后续集成 PDF/EPUB/MOBI 等新格式的根基。

**修改内容**
- 新建 `document/mod.rs`：`Document` trait 替代 `Book` trait，新增 `DocumentMeta` 结构体（title/author/genre/series），`open_book` → `open_document`
- 新建 `document/zip.rs`：`ZipBook` 实现 `Document` trait（含 `metadata()`），测试用例同步迁移
- 删除 `archive/mod.rs` + `archive/zip.rs`（旧模块）
- `lib.rs`：`pub mod archive` → `pub mod document`
- `reader.rs`：`Book` → `Document`，`book.title()` → `book.metadata().title`
- `api/book.rs`：`archive::open_book` → `document::open_document`，`archive::Book` → `document::Document`
- `api/source.rs`：同上的引用更新
- `Cargo.toml`：删除误添加的 `mupdf = "0.8"` 依赖（该依赖需要 libclang，当前环境不可用且未被代码使用）

**修改原因**
- 用户确认采用「统一 Document trait + 按格式独立实现」架构（ADR-010），不绑 MuPDF
- `Document` 比 `Book` 更能表达"多格式文档"的语义；`DocumentMeta` 为后续 ComicInfo.xml / OPF 元数据提取预留
- mupdf crate 需要 libclang 编译 C 库，当前 Windows 环境不满足，且按 ADR-010 已决定不选 MuPDF

**影响范围**
- Rust：`document/`（新）、`lib.rs`、`reader.rs`、`api/book.rs`、`api/source.rs`、`Cargo.toml`
- Dart：无变更（FRB 桥接的 `open_local_book`/`open_webdav_book` 接口签名不变）
- 旧 `archive/` 已删除

**是否完成**
已完成：cargo test 6 项全过、flutter analyze 零 error（仅 6 条 info 级风格提示）、应用可启动。

**遗留问题**
- `DocumentMeta` 当前仅 ZIP/CBZ 的 title 填充，其余字段为空；后续 EPUB(OPF)、PDF(元数据) 格式可补充
- flutter run 调试已启动，Windows 端运行正常

---

## 2026-07-27|第 18 轮:EPUB 格式支持（ZIP + OPF spine 自研）

**本轮目标**
用户导入了 EPUB 文件但无法识别。按 ADR-010 方案实现 EPUB(漫画)支持，不依赖排版引擎。

**修改内容**
- **新增 `document/epub.rs`**（352行）：
  - `EpubBook<S: ByteSource>` struct：ZIP 解包 → 解析 `META-INF/container.xml` 找 OPF 路径 → 解析 OPF manifest + spine → 按阅读顺序提取图片 → 实现 `Document` trait
  - `page_bytes` 复用与 ZipBook 相同的机制（按 data_start/compressed_size 精确定位 + Deflate 解压），保证流式阅读兼容
  - 退化逻辑：spine 未产出图片时自动回退为扫描 ZIP 中所有图片（自然排序）
  - 辅助函数：`find_opf_path`（container.xml 解析）、`parse_opf`（manifest/spine 状态机）、`extract_img_src`（xhtml img 标签提取）、`resolve_path`（相对路径解析）
  - 单元测试：3 项（resolve_path / extract_attr / extract_img_src）
- **修改 `document/mod.rs`**：`pub mod epub` + `open_document` 注册 `.epub` 扩展名
- **修改 `source_browser.dart`**：文件过滤列表加 `.epub`（与 `.cbz/.zip` 同级），使 EPUB 在海报墙/列表中可见

**修改原因**
- 用户已有 EPUB 漫画文件需要阅读
- 漫画 EPUB 本质是 ZIP+图片，不需要排版引擎——ZipBook 的流式解析机制可直接复用

**影响范围**
- Rust：`document/epub.rs`（新）、`document/mod.rs`（改1行）
- Dart：`source_browser.dart`（改1行）
- **零新增 Cargo 依赖**（OPF 解析用手写状态机，不引入 quick-xml）

**是否完成**
已完成：cargo test 9项全过（3项 EPUB 新测试 + 6项原有）、flutter analyze 零 error、flutter run 编译运行成功。

**验证方式**
- 将一个漫画 EPUB 文件放入本地书源目录 → 打开书源 → 应能在海报墙中看到 EPUB → 点击进入详情 → 开始阅读，各页自动按 OPF spine 顺序排列
- 若 EPUB 无 OPF spine/无 container.xml → 自动退化为按 ZIP 内图片自然排序

**遗留问题**
- EPUB 元数据（title/author/series/genre）尚未从 OPF metadata 提取，当前 title 用文件名
- xhtml 中包含多个 `<img>` 时仅提取第一个
- ZipEntryMeta.path 字段当前未使用（仅作预留），编译有死代码警告
- PDF 需要 pdfium.dll 放在 app.exe 同目录（可从 Chrome 提取或单独下载）
- CBR/RAR 需要 unrar.dll（需系统已安装 WinRAR 或 UnRAR 运行库）

---

## 2026-07-27|第 19 轮:Folder + CB7 + CBT + PDF + CBR 5种格式一次性实现

**本轮目标**
按 ADR-010 方案，将格式引擎从 2 种 (ZIP/CBZ + EPUB) 扩展到 7 种，覆盖漫画所有常见分发格式。

**修改内容**
- **Folder** (`folder.rs` 125行)：枚举目录图片文件 + 自然排序，`open_local_book` 和 `book_cover` 新增目录判断分支
- **CB7** (`sevenz.rs` 96行)：`sevenz-rust` crate 集成，ByteSource → tempfile → 解压到内存 → Document
- **CBT** (`tar.rs` 85行)：`tar` crate 解包，ByteSource → Cursor → tar → 内存
- **PDF** (`pdf.rs` 107行)：`pdfium-render` crate 集成，渲染每页为 1600px WebP 位图（需 pdfium.dll）
- **CBR** (`rar.rs` 118行)：`unrar` crate 集成，ByteSource → tempfile → 解压到内存（需 unrar.dll）
- **统一入口**：`document/mod.rs` 注册全部扩展名 (.cb7/.7z/.cbt/.tar/.pdf/.cbr/.rar)
- **Flutter 前端**：`source_browser.dart` 过滤列表加入所有新扩展名
- **MOBI 留作后续**：需要 Calibre CLI (`ebook-convert`)，当前环境无 Calibre

**修改原因**
- 用户明确要求支持 ZIP/PDF/EPUB/MOBI/CBR 等格式
- 每个格式用最适合的纯 Rust 库（ADR-010），不绑定重型引擎
- 零流式依赖的格式（CB7/CBT/PDF/CBR）采用"全量读入内存"策略，与 ZIP/EPUB 的流式策略互补

**影响范围**
- Rust：`document/` 模块新增 5 个格式文件，`api/book.rs` 目录判断分支，`Cargo.toml` 新增 3 个依赖（`sevenz-rust`/`tar`/`unrar` + 已有 `pdfium-render`）
- Dart：`source_browser.dart` 文件过滤列表扩充
- 文档：`README.md` 格式表更新，`TODO.md` 标记完成

**是否完成**
已完成：cargo test 11/11 passed · flutter analyze 0 errors · √ Built Windows app.exe

**遗留问题**
- PDF 需 pdfium.dll 运行时依赖（需打包或提供下载说明）
- CBR 需 unrar.dll 运行时依赖
- MOBI 需 Calibre CLI（`ebook-convert`），当前环境未安装
- Folder 格式的缓存命名空间用 directory_path 做 key，与 ZIP 文件路径一致

---

## 2026-07-30|第 16 轮:M2/M3/M5 技术方案决策与 SPEC 修订

**本轮目标**
用户确认:① M1 剩余项(双页自动/滚轮解耦/WebDAV进度/按键扩展)暂缓,低优先级;② 启动 M2(AI超分)、M3(PDF/EPUB)、M5(MOBI)。

**决策内容**
- **M2 AI 超分**:采用 CLI 子进程方案(librealesrgan-ncnn-vulkan 预编译 exe),Rust 侧 `std::process` 调用+进度解析,图片经临时文件传递。后续可优化为 FFI 直调。
- **M3 PDF/EPUB**:采用 MuPDF(mupdf-sys crate)统一引擎,一个引擎覆盖 PDF+EPUB+XPS,Apache 2.0 许可。适配 Book trait。
- **M5 MOBI**:采用 mobi crate(v0.8.0,纯 Rust),独立实现 Book trait。CBR 搁置(unrar 许可)。
- **新增 ADR-009**(AI超分CLI方案)、**ADR-010**(MuPDF统一引擎);更新 **ADR-003**(AI引擎方案由FFI改为CLI)。

**修改原因**
- 三个里程碑一同决策,避免反复调研切换上下文。
- CLI 方案开发最快可先跑通流程;MuPDF 一统 PDF/EPUB 减少维护面;mobi 纯 Rust 零 C 依赖。
- M1 低优先级项不影响日常使用,标注暂缓。

**影响范围**
- `SPEC.md`(技术栈表更新、里程碑验收标准细化)
- `DECISION.md`(+ADR-009/ADR-010, ADR-003更新)
- `TODO.md`(M2/M3/M5 细化拆解为可执行任务,M1四项标暂缓)
- `Cargo.toml`(后续加 mupdf-sys、mobi)

**是否完成**
已完成:文档更新完毕,待用户确认后开始 M2/M3 实施。

**遗留问题**
- MuPDF 编译需要 Windows 侧静态库(通过 vcpkg 或预编译 dll),待实施时解决。
- librealesrgan-ncnn-vulkan 模型文件(.param/.bin)需确定分发策略(随应用打包 vs 首次下载)。

---

## 2026-07-27|第 21 轮:SPEC v2.0 架构重塑 + 四层架构决策

**本轮目标**
用户提出系统化架构方案。据此更新全量文档，确立新的设计规范。

**修改内容**
- **SPEC v2.0**：四层架构；10条核心原则；多级缓存；统一下载器；WebDAV保守策略；SQLite；M9里程碑
- **DECISION**：新增 ADR-012(四层架构)/013(缓存+SQLite)/014(WebDAV保守策略)/015(MOBI直接解析)
- **README**：格式表合并到功能描述；新增待建设区；更新架构说明
- **TODO**：MOBI标记完成；M9缓存基础设施标为优先

**修改原因**
- 用户确立核心原则：「阅读器永远只操作本地资源，网络只是同步层，AI只是处理层」
- 已实现代码与文档不一致需要同步

**影响范围**
- 文档层：SPEC/DECISION/README/TODO全量更新
- 代码层：无变更

**是否完成**
已完成。文档全量对齐新架构。

**遗留问题**
- M9缓存基础设施（下载器+SQLite+整本缓存）待实施
- library.json→SQLite迁移方案待细化
- WebDAV整本下载策略待实施

---

## 2026-07-27|第 22 轮:修复 WebDAV 封面不加载 + 添加下载进度指示

**行为**
- 修复 WebDAV 书源浏览页（SourceBrowser）不监听 LibraryStore 变化,导致封面永远显示"未缓存"的问题:在 build() 方法外包裹 `ListenableBuilder(listenable: LibraryStore.instance)`,阅读记录更新后海报墙自动重建,`_shouldLazyLoad` 重新求值。
- 修复 ComicCover._cache 失败后永久缓存的问题:原实现用 `putIfAbsent` 缓存 Future,失败后的 Future 也被永久缓存;改为失败时自动从缓存移除,Widget 重建时重试。新增 `evict()`/`evictAll()` 静态方法。
- Rust 端 WebDAV 下载进度追踪:新增 `DownloadProgress` 结构体(AtomicU64 线程安全);`download_to_raw_cache` 改为逐块(64KB)写入并更新进度;`open_webdav_book` 在下载期间将进度注册到全局追踪表。
- Flutter 端下载进度指示:WebDAV 打开一本书时,`ReaderPage` 新增 `_downloading` 状态和下载中 UI(转圈 + "正在下载漫画..."提示),下载完成后自动切换到阅读视图。
- Rust 端新增 `webdav_download_progress(session)` API 供 Dart 侧轮询。

**修改原因**
- 封面不加载是用户直接报告的 bug:点开漫画阅读后回到海报墙,封面仍然显示"未缓存"。根因一是 SourceBuilder 不监听 LibraryStore 导致 Widget 不重建;根因二是 Cache 失败后永久保留失败 Future 导致重试无效。
- 下载进度是用户反馈的体验问题:WebDAV 大文件整本下载期间无任何反馈,用户不知道应用是在工作还是卡死了。

**影响范围**
- Flutter: source_browser.dart(外层包裹 ListenableBuilder), comic_cover.dart(重写 _load/新增 evict), reader_page.dart(新增下载中 UI)
- Rust: webdav.rs(新增 DownloadProgress/改造 download_to_raw_cache), api/source.rs(新增进度追踪表/API)
- 未修改 FRB 生成文件

**是否完成**
已完成。Flutter analyze 通过,无 error。

**遗留问题**
- M9 缓存基础设施（下载器+SQLite+整本缓存）待实施

---

## 2026-07-27|第 23 轮:缓存管理面板分级 + 下载百分比进度 + 封面缓存策略优化

**行为**
- 重做设置页缓存管理面板:从「一锅端」改为5类独立管理的列表——页面缓存/整本下载(raw/)/封面缩略图(cover/)/旧下载目录(download/)/AI超分(ai/),每项展示实时占用空间、用途说明和独立「清理」按钮（清理前二次确认对话框）。
- 新增 Rust「CacheSize」结构体和 `cache_sizes()` API，一次性返回5类+总计的大小，避免多次 FFI 调用。
- 重新生成 FRB 绑定，Flutter 端新增 `CacheSizes()` 和 `webdavDownloadProgress()`。
- ReaderPage 从 boolean 下载指示升级为真实百分比进度条：通过轮询 `webdavDownloadProgress()` 驱动 `LinearProgressIndicator` + 百分比数字，每 300ms 更新一次。
- 封面缓存清理策略分离：只有点击「封面缩略图（cover/）」的清理按钮或「清空全部缓存」时才清除 Dart 侧封面内存缓存（`ComicCover.clear()`）；页面缓存/原始文件/旧下载/AI 缓存清理均不影响已加载的封面，封面内存缓存保持有效。
- 「清理失效漫画记录」按钮从清除全库封面改为仅清理读取记录，不再触发 `ComicCover.clear()`。
- 新增 `ComicCover.evictAll()` 方法，支持按 source+path 清理指定封面的缓存。

**修改原因**
- 用户反馈清理缓存「一锅端」不合理：删除页面缓存后已阅读漫画的封面也被清除，海报墙长时间转圈。分5类独立管理后，用户可以有选择地清理，不影响封面展示。
- 封面已缓存但仍转圈：原 `ComicCover._cache` 用 `putIfAbsent` 缓存失败 Future，失败后无法重试。已改为失败自动移除。
- 下载进度需要百分比而非简单的转圈指示。

**影响范围**
- Rust: api/cache.rs(新增 CacheSize/重构分类大小 API), api/source.rs(webdav_download_progress)
- Flutter: cache_manager.dart(新文件), home_page.dart(替换旧缓存面板), reader_page.dart(真实百分比进度), comic_cover.dart(evictAll), common.dart(fmtNum)
- FRB 重新生成: cache.dart/source.dart 等自动生成文件

**是否完成**
已完成。编译成功，Flutter analyze 无 error。

**遗留问题**
- M9 缓存基础设施（下载器+SQLite+整本缓存）待实施

---

## 2026-07-28|第 24 轮:修复标签补全点击无反应 + 开发流程总结

**行为**
- 修复 `#` 标签搜索框补全点击无反应 bug。

**根因分析**
- 原代码 `_searchCtrl.text.substring(0, lastIdx + 1)` 保留了草稿 `#彩` 再拼 `$t ` 得 `#彩彩漫 `——草稿未替换而是叠加。
- 正确做法：取 `lastIndexOf('#')` 之前的内容作为前缀，拼接 `#$t ` 完整替换草稿。

**修改内容**
- `home_page.dart` `_showOverlay()` onTap: 改为 `text.substring(0, lastIdx) + '#$t '`。

**教训总结**
- 遇到 bug 先和用户对齐现象、分析可能原因，双向反馈后再动手修。
- 不反复"编译→启动→看结果"循环，先静态分析定位根因，一次修对。

**影响范围**
- Flutter: home_page.dart (标签补全拼接逻辑)

**是否完成**
已完成。

**遗留问题**
- 标签补全系统性问题待彻底修复（详见下轮）

---

## 2026-07-28|第 26 轮:实施 ADR-016/017 — Repository 层 + 标签独立建模 + 全量架构评价

**行为**
- `models.dart` 新增 `Tag` 实体（id/name/createdAt）和 `BookTag` 关联（bookKey/tagId）
- 新建 `app/lib/repository/tag_repository.dart`：Single Source of Truth for Tags
- `TagRepository`：独立标签集合（`_tags` + `_bookTags`），提供 `all()`/`allNames()`/`search()`/`ensure()`/`link()`/`setBookTags()`/`rename()`/`delete()`/`tagStats()`/`bookKeysForTag()`
- `library.json` 格式升级：新增 `tags` 和 `book_tags` 独立字段，向后兼容回填
- `LibraryStore` 标签操作全委托 TagRepository；`updateMeta()` 自动同步；`_save()` 返回 bool
- `home_page.dart` 补全列表走 `TagRepository.allNames()`
- DECISION.md 追加 ADR-016/017/018（全量架构评价 + P0/P1/P2/P3 风险排序）
- TODO.md 更新 Doing + Backlog

**修改原因**
标签补全 bug 多轮无效，根因是标签寄生于 BookMeta 且无统一数据入口。从架构层一次性解决。

**影响范围**
- 新增: repository/tag_repository.dart, repository/repository.dart
- 修改: models.dart, library_store.dart, home_page.dart
- 文档: DECISION.md, LOG.md, LOG-INDEX.md, TODO.md

**是否完成**
已完成。Flutter analyze 通过（仅 info/warning，无 error），Rust cargo check 通过。

**遗留问题**
- 标签补全需运行中验证（新标签→#补全→点击筛选）
- P0: BookRepository/HistoryRepository/SettingsRepository
- P0: JSON→SQLite 迁移
- P1: Downloader→TransferManager / CachePolicy ADR / Reader-Downloader解耦

**行为**
- 对标签补全失效做完整的根因分析，更新文档记录已知问题。

**已确认的现象**
1. 补全列表点击标签后搜索框不自动填补
2. 后续新增的标签在 `#` 输入后不出现补全列表
3. 手动打完 `#标签名 ` 后筛选功能正常

**系统性问题分析**

A. **标签数据模型缺陷** — `allTags()` 只从 `BookMeta.tags`（per-book）收集标签，没有独立全局标签表。如果标签从未关联漫画，`allTags()` 遍历 `metas` 时不会包含它。

B. **`_save()` 静默失败** — `library_store.dart` 的 `_save()` 用空 `catch (_) {}` 吞掉异常，写入失败无提示。

C. **补全点击与 onChanged 时序冲突** — `onTap` 赋值触发 `onChanged` → `_onSearch` 二次调用。已加 `_ignoreChange` 标记但仍需验证。

D. **Overlay 点击可能被拦截** — `ListTile` 在 Material ink splash 警告下点击事件可能被吞。

**遗留问题**
- 标签补全仍待验证。后续：
1. 加 print 日志确认运行时数据流
2. 检查 `library.json` 文件内容
未完成，待 ADR-016/017 实施后修复。

**遗留问题**
- 标签补全系统性问题待彻底修复（详见 DECISION.md ADR-016/017）

---

## 2026-07-28|第 27 轮:标签系统重做 — 全面兼容新架构 + 搜索系统重新设计

**本轮目标**
接手第 26 轮遗留的 ADR-016/017 实施，全面审查标签系统兼容性，修复所有 bug，重新设计搜索交互。

**修改内容**
- 修复 `BookTag` 缺 `==`/`hashCode` 导致 `Set.contains()` 永远为 `false`（每次 `link()` 添加重复条目）
- 修复 `TagRepository.load()` 数据源问题：`metas` 始终是 ground truth，`tags`/`book_tags` 只是持久化缓存。`load()` 每次都从 `metas` 重建标签数据，移除了有缺陷的 `hasNewFormat` 分支
- 修复 `LibraryStore.updateMeta()` 不同步 `author/genre/series` 到 `TagRepository`，导致元数据标签不出现于补全列表
- 修复 `_recordGrid` 和 `SourceBrowser._filtered` 的标签过滤只查 `m.tags` 不查 `m.metaTags`
- 修复 `TagRepository.load()` 回填遗漏元数据字段
- 修复 `batchTag()` 元数据字段首次赋值时不调用 `TagRepository.link()` 的问题
- 修复选择模式批量打标签 (`_batchTagFromSelection`) 重复实现，改为复用 `LibraryStore.batchTag()`
- `TagRepository.load()` 的错误处理从静默 `catch (_) {}` 改为 `print()` 输出
- `LibraryStore.load()` 结束后立即 `_save()`，确保纠正后的标签数据写回 `library.json`
- 重新设计搜索系统：
  - 搜索栏不再用正则暴力提取 `#tag`，改为交互式补全
  - 输入 `#` + 文字 → Overlay 弹出匹配标签列表 → 点击标签自动补全（格式 `#tag名 `）
  - 搜索框下方显示已选标签 Chip 条，点 × 移除
  - 文字搜索跨全部书源（不再限制在当前分类）
  - 标签 + 文字联合过滤
- 删除"快速批量打标签" PopupMenu（功能重复），改为直接图标按钮进入选择模式

**修改原因**
- 第 26 轮代码仅通过编译分析，从未实际运行验证。本轮系统性地追踪了所有标签相关数据流，发现并修复了 10+ 个兼容性 bug
- 搜索交互从"魔法正则解析"改为"显式补全 + Chip 可视化"，消除补全点击无反应的根本原因
- 标签补全碎片化根因是 `hasNewFormat` 守卫逻辑：一旦 `library.json` 有脏 `tags` 字段就跳过从 `metas` 重建

**影响范围**
- `store/models.dart`: `BookTag` 添加 `==`/`hashCode`
- `repository/tag_repository.dart`: load 重建逻辑重写，错误处理改进，新增 `_addTagAndLink` 辅助
- `store/library_store.dart`: `updateMeta()` 同步元数据，`batchTag()` 补 `link()`，`load()` 后立即 `_save()`
- `ui/home_page.dart`: 搜索系统重新设计（_onSearch/_showOverlay + Chip 标签条），跨书源搜索
- `ui/source_browser.dart`: 标签过滤兼容 `metaTags`，批量打标签复用 `batchTag()`，删除快速批量打标签

**是否完成**
已完成。Flutter analyze 0 error，Rust cargo check 0 error。待用户运行验证。

**遗留问题**
- `_tagId` 使用 `hashCode.toRadixString(36)` 有碰撞风险（TODO.md 已记录）
- `_save()` 返回 `bool` 但所有调用方丢弃返回值（TODO.md 已记录）

---

## 2026-07-28|第 28 轮:跨书源搜索重新设计 — 搜索栏内联补全 + globalSearch API

**本轮目标**
用户反馈跨书源搜索"只搜已打开的漫画"，且标签补全不工作。彻底重做搜索交互：在侧栏搜索框旁加切换按钮，点开后在同一个搜索栏内实现标签补全+文字搜索。

**修改内容**
- 新增 `LibraryStore.globalSearch()` 方法：遍历**所有书源下的所有 metas**（不限于已打开过的漫画），按文字+标签联合过滤
- 搜索栏右侧新增地球图标按钮：筛选模式（filter_list）/ 跨书源搜索模式（search），点击切换
- 跨书源模式下：输入 `#` + 文字 → 搜索框下方内联显示匹配标签列表（最多8个）→ 点击补全。不同于 Overlay，内联列表稳定在 Widget 树中，不会因 focus 变化消失
- 已选标签显示为 Chip 条在搜索框下方，点 × 移除
- 标签+ 文字联合过滤，结果在右侧实时更新
- 从侧边栏删除了独立的"跨书源搜索"导航项，合并到搜索栏切换按钮中

**修改原因**
- 旧跨书源搜索从 `records`（ReadRecord）中搜索，只有打开过的漫画才有记录，导致大量漫画搜不到
- Overlay 补全因 `_onFocus` 等复杂时序问题一直不工作，改为内联列表方案从根本上规避
- 用户要求搜索入口放在搜索栏旁边，不另开一页

**影响范围**
- `store/library_store.dart`: 新增 `globalSearch()` 方法（遍历 sources × metas）
- `ui/home_page.dart`: 搜索系统重做（_globalMode 切换 + 内联补全 + Chip + globalSearch）
- `ui/source_browser.dart`: 无变更

**是否完成**
已完成。Flutter analyze 0 error。待用户运行验证。

**遗留问题**
- `globalSearch()` 遍历所有 metas 是 O(sources × metas)，数据量大后可能需加索引
- 标签补全候选取前8个，搜索框宽度有限，多标签时可能需要滚动

---

## 2026-07-28|第 29 轮:搜索系统统一 — 筛选模式与全局模式共用一套搜索逻辑

**本轮目标**
用户反馈"筛选当前视图"搜索栏没有标签补全功能，要求两种模式共用一套搜索。

**修改内容**
- `home_page.dart` 完整重写，核心改动：
  - 搜索状态 `_textSearch` / `_tags` / `_tagDraft` 不再区分模式，始终解析
  - 侧栏搜索框统一支持：输入 `#` → 内联补全列表 → 点击填入 → Chip 展示 → 点 × 移除
  - 地球图标切换仅改变右侧数据来源：筛选模式下过滤当前视图书架，全局模式下调用 `globalSearch()` 跨所有书源
  - 点击导航项时自动切换回筛选模式
  - 全局搜索结果顶部新增筛选条件条（显示文字 + 标签 Chip + 清除按钮）
- 代码从 708 行压缩至约 300 行，消除重复逻辑

**修改原因**
- 第 28 轮 `_onSearch` 中 `if (_globalMode)` 分支导致筛选模式完全不处理 `#` 标签
- 用户明确要求"两者共用同一套代码模块"

**影响范围**
- `ui/home_page.dart` (重写)
- 其他文件无变更

**是否完成**
已完成。Flutter analyze 0 error，仅 3 条 info（216行单行 if-else 风格提示）。待用户运行验证。

---

## 2026-07-28|第 30 轮:Git 仓库初始化 + 数据层稳定化 + Folder 元数据全面支持

**本轮目标**
1. 为项目建立 Git 版本管理
2. ADR-016/017：确立 Repository 唯一数据源 + 标签独立建模
3. 修复 `_save()` 静默失败和 TagRepository 孤儿标签丢失
4. Folder 格式全面支持 ComicInfo.xml + metadata.json 双元数据源
5. 浏览页智能识别漫画文件夹（图片+JSON），显示为海报封面卡片
6. library.json 增加版本号字段

**修改内容**

### 数据层稳定化
- `DECISION.md`：新增 ADR-016（Repository + Single Source of Truth）、ADR-017（标签独立建模）
- `app/lib/repository/tag_repository.dart`：修复 `load()` 中独立标签（无漫画关联）重载后丢失的 bug，改为优先从序列化 `tags` 数组加载，再从 `metas` 补充
- `app/lib/store/library_store.dart`：`_save()` 取消 `catch(_)` 静默吞异常，改为 `debugPrint + rethrow`
- `app/lib/store/models.dart`：新增 `LibraryData` 包装类，包含 `version` 字段

### Folder 元数据支持
- `app/rust/Cargo.toml`：新增 `serde_json = "1"` 依赖；`quick-xml` 开启 `serialize` feature
- `app/rust/src/document/comicinfo.rs`（新建）：ComicInfo.xml 解析模块，支持 Title/Series/Writer/Genre/Summary 等字段，映射到 `DocumentMeta`
- `app/rust/src/document/folder.rs`：
  - 新增 `metadata.json` 解析（`serde_json`），支持 `title/author/genre/series/description/tags` 字段
  - 元数据优先级：ComicInfo.xml > metadata.json > 目录名
  - 新增 `cover_path()`：按优先级查找 `cover.jpg/png/webp/jpeg`
  - 新增 `is_comic_folder()`：检测目录是否包含至少一张图片
- `app/rust/src/api/book.rs`：暴露 `is_comic_folder()` 和 `folder_cover_path()` 给 Flutter

### Flutter 浏览页改造
- `app/lib/ui/source_browser.dart`：
  - 本地模式下异步检测子目录是否为漫画文件夹（`isComicFolder`）
  - 漫画文件夹显示为 `_ComicFolderCoverCard`（带封面缩略图），普通文件夹保持 `_FolderCard`（文件夹图标）
  - 漫画文件夹点击进详情页而非下钻
  - 封面优先 `folderCoverPath()`（cover.jpg），无显式封面取首页缩略图
  - 批量标签递归收集支持漫画文件夹（不再只认 .cbz/.zip）
- FRB 桥接代码重新生成（`flutter_rust_bridge_codegen generate`）

### README 更新
- 格式支持表：Folder 更新为 "目录枚举 + ComicInfo.xml + metadata.json"
- 新增"漫画文件夹识别"和"漫画文件夹元数据"说明
- 已实现能力：新增 Folder 双元数据源 + 封面优先 + 智能检测
- 目录结构：source_browser.dart 注释更新

**修改原因**
- 标签补全问题根因是数据来源分散、孤儿标签无持久化，需要 Repository + 独立标签表
- `_save()` 静默失败导致数据丢失无感知
- 用户明确要求"文件夹下图片+json"作为一等漫画格式被识别和展示

**影响范围**
- Rust: `document/folder.rs`, `document/comicinfo.rs`(新), `document/mod.rs`, `api/book.rs`, `Cargo.toml`
- Dart: `store/models.dart`, `store/library_store.dart`, `repository/tag_repository.dart`, `ui/source_browser.dart`
- FRB 生成文件: `lib/src/rust/api/book.dart`, `lib/src/rust/frb_generated.*`
- 文档: `DECISION.md`, `README.md`, `.gitignore`
- 19 个 Rust 单元测试全部通过（18 passed, 1 ignored）

**是否完成**
已完成。cargo test 18/18 passed，flutter analyze 仅 2 info。待用户运行验证。

**遗留问题**
- WebDAV 模式下暂不检测漫画文件夹（避免大量网络 IO）
- EPUB 元数据仍未从 OPF 提取
- metadata.json 的 tags 字段已解析但尚未自动同步到 TagRepository（需在 Flutter 侧打开时同步）
