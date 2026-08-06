# 数据层同步 — 执行计划（阶段地图）

## 阶段顺序与入口

| 阶段 | 子任务 | 依赖 | 入口验收 |
|---|---|---|---|
| P0 schema | 08-06-sync-p0-schema | fix-tag-persistence（已完成） | 新列/新表就绪、老库可迁移、回归通过 |
| P1 标准包 | 08-06-sync-p1-package | P0 | 导出→导入往返无损、无凭据、schema 版本拒绝 |
| P2 备份即同步 | 08-06-sync-p2-backup | P1 | WebDAV + 同步盘目录两通道可用、失败可重试 |
| P3 增量合并 | 08-06-sync-p3-delta | P2 | 离线编辑两端合并不丢不复活、幽灵条目生效 |

## 通用验证命令

```bash
cd app/rust && cargo check && cargo test      # Rust 侧
cd app && flutter analyze && flutter test     # Dart 侧回归
./app/codegen.ps1                             # 仅当新增 FRB API 时
```

## 风险与回滚

- schema 变更只加列/加表、不删不改旧列，老版本 app 打开新库不受影响；升级前可备份 `database.db`。
- 涉及 FRB API 变更时先跑 codegen.ps1，content hash 不一致会导致桥接运行时报错（SETUP.md 已记录）。
- 每阶段独立归档，回滚只影响该阶段。
