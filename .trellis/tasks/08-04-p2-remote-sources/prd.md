# P2 远程书源安卓适配

## Goal

WebDAV / SFTP / 百度网盘 / 115 / 夸克 五类远程书源在安卓上可用,打开策略、下载缓存、token 刷新与桌面端一致(SMB 不在首版);WebDAV 同步通道(备份/同步)可推/拉/恢复。

## Requirements

- 联网能力:INTERNET 权限(由 p0 提供)+ 弱网 / 失败提示复用下载器策略。
- WebDAV / SFTP:连接、浏览、打开策略(auto / download / stream)、下载到缓存、进度显示。
- 百度网盘:OAuth 浏览器授权 + 回调处理(桌面为 localhost 回调,安卓需 deep link 或复制 code 方案)。
- 115:扫码授权或手动输入 + token 自动刷新回写。
- 夸克:Cookie 认证(无 OAuth),复用桌面会话逻辑,凭据持久化回写。
- WebDAV 同步:复用 v0.3.5 备份/同步面板,支持 WebDAV 通道推/拉/恢复/归档清理;网盘同步盘本地目录通道后置(Android 不支持 getDirectoryPath)。
- 远程书源配置可保存,重启不丢。

## Acceptance Criteria

- [ ] 真机上四类书源各完成一次"连接 → 浏览 → 打开 → 阅读 → 重启秒开(缓存生效)"。
- [ ] 夸克完成一次同闭环;WebDAV 同步推/拉一次(标签/书源/进度),重启后数据一致。
- [ ] token 自动刷新,重启应用无需重新授权。
- [ ] 打开策略三种行为与桌面端一致。

## Dependencies

- 前置:p0-android-buildchain + p1-local-reader(阅读闭环)。
