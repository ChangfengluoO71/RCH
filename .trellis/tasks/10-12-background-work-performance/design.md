# 技术设计

## 边界

- `LibraryIndexService` 管理同源索引 single-flight 与索引快照 freshness，不在数据库事务内遍历文件系统。
- `AutomationCoordinator` 保留唯一自动维护调度入口，用源级 dirty generation 和 pending intent 合并触发；`SyncEngine` 提供兼容状态文案与结构化同步结果。
- `LibraryStore`/`TagRepository` 提供一个可等待的批量标签操作。Rust SQLite API 承担集合式写入和短事务；Flutter 负责计划、阶段进度和结果解释。
- 前后台协调只调度常规维护，不接管用户主动操作，也不接管 EH Daily/115 自动化。
- 云端书源 QR/Cookie 操作由现有 UI 异步生命周期显示等待状态，不引入 UI 轮询。

## 核心契约

1. 同一 source ID 同时最多有一个索引扫描。force 请求不能被正在运行的 incremental 请求吞掉；force 结束后才发布完整索引。
2. 自动化周期启动时捕获源级 dirty generation。期间新 generation 进入 pending；周期 `finally` 后按 generation 差异排队下一次工作。协调器自身 reload 不得伪装成新外部目录变更。
3. 同步结果要表达 transport 状态、变更域和目录快照失效范围。远端 revision 不变不能代表本地目录没变；本地源按 freshness 策略复核，未知失效范围保守刷新。
4. 批量标签计划保留每项目标的 path、entryType、name、parentPath；索引补录、内存元数据/关系更新和 SQLite 持久化分别返回结果。批次幂等且事务有界；保存结果绑定本次请求。
5. 自动维护可以延后并合并，按最大等待期限老化；每次只运行经基线限制的服务片段，片段边界检查更高优先级工作。
6. Quark/115 扫码、Cookie 验证、连接和首次目录读取显示真实 loading 状态。重复请求禁用，旧请求不能清理新请求的状态。

## 任务顺序

P0 Profile/Release 基线 → P1-A 索引 single-flight → P1-B dirty 与同步结果 → P1-C 批量标签 → P2 前后台协调 → 云端连接等待反馈。若扫描 CPU 显著，先优化 previous 子树查找，再单独审议 isolate。当前没有 Android 设备，Android 基线需后续真机补采。

## 数据库约束

- 使用唯一全局 SQLite 连接和既有 mutex；不得另开连接。
- 文件/网络 I/O 在事务外，事务内只执行 SQL。
- 批量操作按测量结果拆成有界 SQL 事务，在提交边界释放锁。
- Rust API 新增后通过 Flutter Rust Bridge codegen 生成 Dart bindings，禁止手改生成文件。

## 降级与失败

- 无法确定同步变更范围时刷新相关目录，不按 no-op 跳过。
- 索引扫描/提交失败保留最后一个完整索引。
- 批量标签失败返回阶段级成功/跳过/失败计数；已提交批次通过幂等重试收敛，不伪报整体成功。
- QR/Cookie 取消、过期或连接失败时停止 loading 并显示现有错误/重试入口。

## 非目标

- isolate 扫描器、通用任务框架、全面并行数据库连接、改变远端扫描语义。
- 改变 H1 EH Daily 的每日领取/进程去重/状态持久化，或改变 115 自动化自身调度。
- 以轮询代替现有事件/revision 唤醒。
