# Backend Development Guidelines

> Best practices for backend development in this project.

---

## Overview

This directory contains guidelines for backend development. Fill in each file with your project's specific conventions.

---

## Guidelines Index

| Guide | Description | Status |
|-------|-------------|--------|
| [Directory Structure](./directory-structure.md) | Module organization and file layout | To fill |
| [Database Guidelines](./database-guidelines.md) | ORM patterns, queries, migrations | To fill |
| [Error Handling](./error-handling.md) | Error types, handling strategies | To fill |
| [Quality Guidelines](./quality-guidelines.md) | Code standards, forbidden patterns | To fill |
| [Logging Guidelines](./logging-guidelines.md) | Structured logging, log levels | To fill |
| [SFTP 书源规范](./sftp-source.md) | russh 选型、会话模式、API 契约 | Filled |
| [网盘书源规范](./netdisk-source.md) | 百度/115 官方 API 鉴权、直链、缓存、已知坑 | Filled |
| [夸克网盘书源规范](./quark-source.md) | 非官方 Web API、Cookie 认证、fid 路径约定、已知坑 | Filled |
| [115 网页扫码 Cookie 书源规范](./115-web-source.md) | 非官方 Web API、扫码获取 Cookie、pickcode 路径约定、已知坑 | Filled |
| [Local-First Scraping Boundary](./local-first-scraping.md) | M8 禁止远程书源 I/O 的资产、Provider、确认与测试合同 | Filled |
| [Automation Pipeline Contracts](./automation-pipeline.md) | M8 自动刮削与现有 SyncEngine 的任务通道、顺序、去重与失败隔离 | Filled |

---

| [Source Refresh and Stale-Data Cleanup](./source-refresh-cleanup.md) | Source CRUD completion, 115/Quark effective roots, and safe cleanup boundaries | Filled |

## How to Fill These Guidelines

For each guideline file:

1. Document your project's **actual conventions** (not ideals)
2. Include **code examples** from your codebase
3. List **forbidden patterns** and why
4. Add **common mistakes** your team has made

The goal is to help AI assistants and new team members understand how YOUR project works.

---

**Language**: All documentation should be written in **English**.
