# 执行清单

本任务由子任务逐个交付；父任务只负责统一顺序与最终集成验收，不直接承接业务实现。

## 顺序与门禁

1. [ ] `10-12-background-performance-baseline`：Profile/Release 基线。先采 Windows；Android 设备当前未连接，留待真机。
2. [ ] `10-12-index-refresh-sync-dirty-integrity`：source single-flight、pending dirty generation、类型化 sync outcome 和 conservative freshness。
3. [ ] `10-12-batch-tag-bulk-persistence`：有界索引批次、批量关系/元数据写入、绑定本次操作的保存结果及阶段摘要。
4. [ ] `10-12-background-foreground-coordination`：routine maintenance defer/coalesce/aging/安全边界让路。
5. [ ] `10-12-cloud-source-connect-loading`：QR/Cookie/连接 loading、重复提交防护和错误清理。
6. [ ] 父任务集成审阅：核对跨任务接口、已有 dirty 工作区差异及 EH/115 契约。

## 验收数据

- Profile/Release：FrameTiming budget misses、build/raster p95、前台打开延迟、数据库锁等待 p95、事务耗时、扫描数、FFI 调用数。
- 功能：运行中 dirty 补跑；远端/本地变更组合；强制刷新不丢；批量标签阶段部分失败可辨认/幂等恢复；后台 aging 后有界进展；云端 loading 生命周期完整。
- Android 手持基线需用户设备；不可用 Windows 数据替代。

## 执行边界

- 不改动未归属本任务的既有 dirty files；对同文件的已有实现先读 diff 后增量修改。
- 不提交、不推送。
- 按当前会话规则，本轮不新增或运行测试；因此所有实现结果须明确标为未测试，不能宣称质量门禁通过。
- 阶段 3 isolate 不在本任务范围。
