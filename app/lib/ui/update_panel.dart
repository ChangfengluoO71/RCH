import 'package:app/store/update_manager.dart';
import 'package:flutter/material.dart';
import 'package:url_launcher/url_launcher.dart';

/// 设置页「关于与更新」面板：展示当前版本、检查/下载/安装入口与状态。
class UpdatePanel extends StatelessWidget {
  const UpdatePanel({super.key});

  Future<void> _openReleases() async {
    await launchUrl(Uri.parse(UpdateManager.releasesUrl),
        mode: LaunchMode.externalApplication);
  }

  @override
  Widget build(BuildContext context) {
    final m = UpdateManager.instance;
    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      const Text('关于与更新', style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)),
      const SizedBox(height: 4),
      ValueListenableBuilder<String?>(
        valueListenable: m.localVersion,
        builder: (c, v, _) => Text(
          '当前版本：v${v ?? '…'}',
          style: Theme.of(c).textTheme.bodySmall,
        ),
      ),
      const SizedBox(height: 10),
      ValueListenableBuilder<UpdateStatus>(
        valueListenable: m.status,
        builder: (c, status, _) => _buildStatusArea(c, m, status),
      ),
    ]);
  }

  Widget _buildStatusArea(BuildContext context, UpdateManager m, UpdateStatus status) {
    switch (status) {
      case UpdateStatus.checking:
        return const Row(children: [
          SizedBox(width: 16, height: 16, child: CircularProgressIndicator(strokeWidth: 2)),
          SizedBox(width: 10),
          Text('正在检查更新…'),
        ]);
      case UpdateStatus.upToDate:
        return Row(children: [
          const Icon(Icons.check_circle, color: Colors.green, size: 18),
          const SizedBox(width: 8),
          const Expanded(child: Text('当前已是最新版本')),
          TextButton(onPressed: () => m.check(), child: const Text('重新检查')),
        ]);
      case UpdateStatus.updateAvailable:
        final i = m.info;
        if (i == null) return const SizedBox.shrink();
        final sizeMb = i.asset.size / (1024 * 1024);
        return Card(
          margin: EdgeInsets.zero,
          child: Padding(
            padding: const EdgeInsets.all(12),
            child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Text('发现新版本 v${i.version}',
                  style: const TextStyle(fontWeight: FontWeight.w600)),
              const SizedBox(height: 4),
              Text('${i.asset.name}（${sizeMb.toStringAsFixed(1)} MB）',
                  style: Theme.of(context).textTheme.bodySmall),
              const SizedBox(height: 8),
              Row(children: [
                FilledButton.icon(
                  onPressed: () => m.download(),
                  icon: const Icon(Icons.download, size: 18),
                  label: const Text('下载更新'),
                ),
                const SizedBox(width: 8),
                TextButton(
                  onPressed: () => showUpdateDialog(context),
                  child: const Text('详情'),
                ),
              ]),
            ]),
          ),
        );
      case UpdateStatus.downloading:
        return ValueListenableBuilder<double>(
          valueListenable: m.progress,
          builder: (c, p, _) => Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            LinearProgressIndicator(value: p),
            const SizedBox(height: 6),
            Text('正在下载 ${(p * 100).toStringAsFixed(0)}%',
                style: Theme.of(c).textTheme.bodySmall),
          ]),
        );
      case UpdateStatus.downloaded:
        return Row(children: [
          const Expanded(child: Text('安装包已下载，可以开始安装')),
          FilledButton(onPressed: () => m.install(), child: const Text('立即安装')),
        ]);
      case UpdateStatus.installing:
        return const Row(children: [
          SizedBox(width: 16, height: 16, child: CircularProgressIndicator(strokeWidth: 2)),
          SizedBox(width: 10),
          Expanded(child: Text('正在启动安装…（Windows 将自动关闭应用）')),
        ]);
      case UpdateStatus.error:
        return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
          Text('操作失败：${m.error.value ?? '未知错误'}',
              style: TextStyle(color: Theme.of(context).colorScheme.error)),
          const SizedBox(height: 4),
          Wrap(spacing: 8, children: [
            TextButton(onPressed: () => m.check(), child: const Text('重试')),
            TextButton(onPressed: _openReleases, child: const Text('打开 GitHub Releases')),
          ]),
        ]);
      case UpdateStatus.idle:
        return Wrap(spacing: 8, children: [
          FilledButton.tonalIcon(
            onPressed: () async {
              await m.init();
              if (context.mounted) m.check();
            },
            icon: const Icon(Icons.system_update_alt, size: 18),
            label: const Text('检查更新'),
          ),
          TextButton(onPressed: _openReleases, child: const Text('GitHub Releases')),
        ]);
    }
  }
}

/// 弹出版本详情与更新操作对话框。
Future<void> showUpdateDialog(BuildContext context) async {
  final m = UpdateManager.instance;
  final i = m.info;
  if (i == null) return;
  final sizeMb = i.asset.size / (1024 * 1024);
  await showDialog<void>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text('发现新版本 v${i.version}'),
      content: SizedBox(
        width: 420,
        child: SingleChildScrollView(
          child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text('当前版本：v${m.localVersion.value ?? '?'}',
                style: Theme.of(ctx).textTheme.bodySmall),
            Text('安装包：${i.asset.name}（${sizeMb.toStringAsFixed(1)} MB）',
                style: Theme.of(ctx).textTheme.bodySmall),
            if (i.publishedAt != null)
              Text('发布时间：${i.publishedAt!.toLocal().toString().substring(0, 16)}',
                  style: Theme.of(ctx).textTheme.bodySmall),
            if (i.notes != null && i.notes!.trim().isNotEmpty) ...[
              const SizedBox(height: 12),
              const Text('更新内容：', style: TextStyle(fontWeight: FontWeight.w600)),
              const SizedBox(height: 4),
              Text(i.notes!),
            ],
          ]),
        ),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.of(ctx).pop(), child: const Text('稍后')),
        FilledButton.icon(
          onPressed: () {
            Navigator.of(ctx).pop();
            m.download();
          },
          icon: const Icon(Icons.download, size: 18),
          label: const Text('下载更新'),
        ),
      ],
    ),
  );
}
