# P1 标准包格式（版本化交换格式）

## Goal

定义并实现版本化标准包（`.rchpkg`）：manifest + 按实体分块 + 墓碑 + schema 版本，本地备份、云备份、设备间同步共用；导出/导入往返无损，敏感凭据零进入。

## Requirements

- R1 单文件 zip 包：manifest.json（schema_version / device_id / created_at / 分块清单）+ tags / book_tags / metas / records / sources / settings 分块
- R2 增量导出：基于 sync_state 游标，仅含自上次游标以来的变更 + 墓碑
- R3 敏感字段剔除：sources 分块不含 password / refresh_token / client_secret / cookie；导入后书源"待填凭据"
- R4 schema 版本校验：不兼容版本拒绝并提示
- R5 目录约定：`RCH/sync/latest.rchpkg` + 时间戳归档（父任务 design.md）

## Acceptance Criteria

- [ ] 导出→导入往返无损（标签关联、漫画详情、阅读记录一致）
- [ ] 包内不含敏感凭据；导入后书源为"待填凭据"状态
- [ ] schema 版本不兼容可拒绝/提示
- [ ] 增量导出只含游标后变更；墓碑块可表达删除
