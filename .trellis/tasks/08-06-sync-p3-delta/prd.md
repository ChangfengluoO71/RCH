# P3 增量同步（LWW + 墓碑合并）

## Goal

设备间增量交换与合并：按实体 LWW + 墓碑；书源 fingerprint 匹配 + source_alias；stable_id 跨设备命中；幽灵书源与跨设备搜索生效。

## Requirements

- R1 增量推送/拉取：自游标 delta 交换，多设备时钟与离线编辑处理
- R2 合并引擎：按实体 LWW（updated_at 最新者胜）+ 墓碑（deleted 传播）；标签重命名=删旧建新
- R3 书源合并：fingerprint 匹配；新 fingerprint 创建幽灵条目（remote_only + origin_device_id）或 source_alias；凭据不覆盖目标端
- R4 漫画详情/标签：stable_id 重映射 book_tags / book_metas / read_records，路径无关命中
- R5 幽灵书源 UI：不可打开、封面占位、元数据可编辑回传；全局设置"跨书源搜索是否跨设备"开关，`globalSearch()` 命中标注设备来源与不可阅读状态

## Acceptance Criteria

- [ ] 两端各自离线编辑后合并不丢标签/漫画详情/书源、不复活已删项
- [ ] 其他设备本地书源以"仅元数据"条目显示、可编辑不可打开
- [ ] 跨设备搜索开关生效且结果标注设备来源
- [ ] 合并不覆盖目标端已存在的书源凭据
