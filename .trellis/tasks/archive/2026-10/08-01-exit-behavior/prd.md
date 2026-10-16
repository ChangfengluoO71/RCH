# Active scope — Conditional exit prompt for long-running work (approved 2026-10-14)

## Goal

When the user requests application exit while RCH has active local work or resumable RCH monitoring, show a concise confirmation that explains what will pause and what continues remotely. With no active/resumable work, exit immediately without prompting.

## Requirements

- Cover all long-running RCH work after an implementation-time inventory: EH scans, Cloud115 batch writes and read-only monitoring, remote scans, sync/automation, AI tasks, cover work, and other existing import/export or update jobs that can remain active beyond a short UI action.
- Use an in-memory task registry for currently executing work and read each subsystem's existing persisted state for resumable work. The exit UI is an aggregation only; it must not become a second task-state database.
- Show the active task names and phases. Explain that closing RCH pauses local work and 115 status polling while accepted 115 cloud jobs continue remotely and can be checked again after the next launch.
- Offer only “Continue using RCH” and “Exit and pause local work” for this scope. If no work is active or resumable, preserve silent normal exit.
- Exit through the normal Flutter/Windows lifecycle after existing task cancellation/flush handling. Do not kill unrelated processes or erase durable work state.
- No system tray, background service, Windows Task Scheduler job, or promise of continued local work while the app is closed.
- Do not change Daily scheduling, 115 write authorization, or existing task retry/idempotency behavior.

## Acceptance

- With no active task and no resumable monitor, a close request exits without a dialog.
- With one or more active/resumable tasks, a close request shows accurate task summaries; continuing cancels the exit request and leaves work running.
- Choosing exit pauses local work through its existing cancellation/flush path, retains durable monitor state, and accurately says accepted remote 115 work continues.
- A relaunch restores persisted monitoring without repeating any upload/submit write.
- The prompt appears once per close request, handles a second close request while visible, and remains readable at normal and narrow Windows window sizes.
- Tests cover registry lease lifecycle and all inventoried task categories; Windows GUI verification uses an isolated configuration and creates no real network work.

## Deferred historical proposal (not part of this implementation)

The original three-mode exit setting, system-tray residency, and per-close choice behavior are retained below as historical context only. They are not approved scope for this follow-up.

### Original proposal

## Goal

全局设置新增"退出应用行为"选项，用户可决定关闭窗口时：彻底退出、后台运行（最小化到系统托盘常驻）、或每次询问。

## Requirements

- **R1** 全局设置增加"退出应用行为"设置项，三选一：彻底退出 / 后台运行 / 每次询问。
- **R2** 彻底退出：关闭主窗口即结束进程。
- **R3** 后台运行：关闭主窗口后最小化到系统托盘常驻，托盘菜单可恢复窗口/彻底退出；AI 后台任务（见 `ai-background-upscale`）在后台运行时不因关窗中断。
- **R4** 每次询问：关窗时弹确认（退出 / 后台运行），并记录本次选择。

## Acceptance Criteria

- [ ] 三种模式行为分别符合上述定义
- [ ] 托盘图标与恢复/退出菜单可用（Windows）
- [ ] 后台运行模式下退出时数据 flush 正常（衔接 `flushPendingSave`）
- [ ] 设置持久化，重启后保持
- [ ] `flutter analyze` 0 issues

## Open Questions（开工前确认）

- **O1** "后台运行"是否依赖新增系统托盘实现（当前应用无托盘）？推荐：本任务一并实现最小托盘（图标 + 恢复/退出）。
- **O2** "每次询问"是否记忆本次选择直到下次关窗？推荐记忆当次运行。
- **O3** 默认值：推荐"每次询问"。
