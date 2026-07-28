# Contributing to RCH

感谢你的兴趣！RCH 是一个 Windows 优先、本地优先的流式漫画阅读器。以下规范帮助你我高效协作。

## 行为准则

本项目遵循 [Contributor Covenant 行为准则](CODE_OF_CONDUCT.md)。参与即表示同意遵守。

## 开始之前

1. **阅读文档**：[README](README.md)（当前状态）、[SPEC](SPEC.md)（目标设计）、[TODO](TODO.md)（任务看板）
2. **搜索已有 Issue/PR**：确认你的想法未被提出
3. **大改先讨论**：架构级修改请先开 Issue 描述方案，获得确认后再动手

## 分支规范

```
master          # 稳定分支，只接受 PR 合入
feat/<name>     # 功能分支
fix/<name>      # 修复分支
docs/<name>     # 文档分支
refactor/<name> # 重构分支
```

分支名使用英文小写 + 短横线，如 `feat/7z-streaming`。

## 提交规范

提交信息格式：

```
<type>: <简短描述>

<详细说明（可选）>
```

类型：
- `feat` — 新功能
- `fix` — Bug 修复
- `docs` — 文档变更
- `refactor` — 重构（非功能、非修复）
- `perf` — 性能优化
- `test` — 测试
- `chore` — 构建/工具/依赖
- `style` — 格式/空白（不影响逻辑）

示例：
```
feat: add 7z streaming read support

Implement block index parsing and lazy page extraction
for 7z archives without full decompression.
```

## PR 流程

1. **保持小粒度**：一个 PR 只做一件事
2. **先过本地检查**：
   ```bash
   cd app/rust && cargo check && cargo test
   cd app && flutter analyze
   ```
3. **更新文档**：涉及功能变更时更新 LOG.md、CHANGELOG.md
4. **描述清晰**：说明做了什么、为什么、怎么验证
5. **等待 Review**：至少一位维护者审核通过后合入

## 代码风格

### Dart
- 遵循 `flutter analyze` 零警告
- 命名：类名 `UpperCamelCase`，变量/方法 `lowerCamelCase`，文件 `lower_snake_case`

### Rust
- 遵循 `cargo fmt` 和 `cargo clippy`
- 模块结构：`api/`（桥接层）→ `document/`（格式）→ `source/`（书源）→ `cache/`（缓存）→ `util/`（工具）

## 目录约定

```
RCH/
├─ app/lib/       # Flutter 代码
├─ app/rust/      # Rust 核心引擎
├─ docs/          # 文档
├─ benchmarks/    # 性能基准（规划中）
└─ .github/       # CI 配置（规划中）
```

## 需要帮助？

开 Issue 即可，请附带：
- 环境信息（Windows 版本、Flutter 版本、Rust 版本）
- 复现步骤
- 期望行为 vs 实际行为
- 截图或日志（如有）
