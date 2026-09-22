import 'package:app/store/library_store.dart';
import 'package:app/store/update_manager.dart';
import 'package:flutter/material.dart';
import 'package:url_launcher/url_launcher.dart';

/// 设置页「关于与更新」面板：展示当前版本、检查/下载/安装入口与状态。
class UpdatePanel extends StatefulWidget {
  const UpdatePanel({super.key});
  @override
  State<UpdatePanel> createState() => _UpdatePanelState();
}

class _UpdatePanelState extends State<UpdatePanel> {
  static const String _customKey = '__custom__';

  late String _mirror;
  late bool _customMode;
  late final TextEditingController _customCtrl;
  bool _refreshing = false;

  @override
  void initState() {
    super.initState();
    _mirror = LibraryStore.instance.settings.updateMirror;
    _customCtrl = TextEditingController(text: _mirror);
    _customMode = !_isPreset(_mirror);
    _maybeAutoRefreshMirrors();
  }

  @override
  void dispose() {
    _customCtrl.dispose();
    super.dispose();
  }

  bool _isPreset(String v) =>
      UpdateManager.instance.effectiveMirrors.any((p) => p.value == v);

  /// 打开面板时若镜像列表超过 24h 未更新，自动拉取一次（失败静默，保留旧列表）。
  Future<void> _maybeAutoRefreshMirrors() async {
    final m = UpdateManager.instance;
    if (!m.remoteMirrorsStale) return;
    await m.refreshRemoteMirrors();
    if (mounted) setState(() {});
  }

  Future<void> _refreshMirrors() async {
    if (_refreshing) return;
    setState(() => _refreshing = true);
    final ok = await UpdateManager.instance.refreshRemoteMirrors();
    if (!mounted) return;
    setState(() => _refreshing = false);
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(
      content: Text(ok ? '镜像列表已更新' : '镜像列表拉取失败（继续使用已有列表）'),
      duration: const Duration(seconds: 2),
    ));
  }

  void _applyMirror(String value) {
    final v = value.trim();
    setState(() {
      _mirror = v;
      _customMode = !_isPreset(v);
      if (_customMode) _customCtrl.text = v;
    });
    final s = LibraryStore.instance.settings;
    s.updateMirror = v;
    LibraryStore.instance.updateSettings(s);
  }

  Future<void> _openReleases() async {
    await launchUrl(Uri.parse(UpdateManager.releasesUrl),
        mode: LaunchMode.externalApplication);
  }

  Widget _buildMirrorSelector() {
    final m = UpdateManager.instance;
    final inList = m.effectiveMirrors.any((p) => p.value == _mirror);
    // 已选镜像仍在列表中才直接显示；否则（自定义或已被刷新移除）落到「自定义…」编辑态
    final selected = (inList && !_customMode) ? _mirror : _customKey;
    final fetchedAt = LibraryStore.instance.settings.updateMirrorFetchedAt;
    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      Row(children: [
        const Expanded(
          child: Text('下载通道',
              style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)),
        ),
        TextButton.icon(
          onPressed: _refreshing ? null : _refreshMirrors,
          icon: _refreshing
              ? const SizedBox(
                  width: 14,
                  height: 14,
                  child: CircularProgressIndicator(strokeWidth: 2))
              : const Icon(Icons.refresh, size: 16),
          label: const Text('刷新镜像列表'),
        ),
      ]),
      const SizedBox(height: 4),
      Text(
        '官方 GitHub 直连在国内可能较慢；镜像为第三方前缀代理。应用会自动从 CDN 拉取最新镜像列表，'
        '下载失败自动切换下一个通道；也可手动填写自定义镜像前缀。',
        style: Theme.of(context).textTheme.bodySmall,
      ),
      const SizedBox(height: 10),
      DropdownButton<String>(
        value: selected,
        isExpanded: true,
        items: [
          ...m.effectiveMirrors
              .map((p) => DropdownMenuItem(value: p.value, child: Text(p.key))),
          const DropdownMenuItem(value: _customKey, child: Text('自定义…')),
        ],
        onChanged: (v) {
          if (v == null) return;
          if (v == _customKey) {
            setState(() => _customMode = true);
          } else {
            _applyMirror(v);
          }
        },
      ),
      if (_customMode) ...[
        const SizedBox(height: 8),
        TextField(
          controller: _customCtrl,
          decoration: const InputDecoration(
            labelText: '镜像前缀（如 https://ghfast.top/）',
            border: OutlineInputBorder(),
            isDense: true,
          ),
        ),
        const SizedBox(height: 4),
        Align(
          alignment: Alignment.centerRight,
          child: TextButton(
            onPressed: () => _applyMirror(_customCtrl.text),
            child: const Text('应用'),
          ),
        ),
      ],
      if (fetchedAt > 0) ...[
        const SizedBox(height: 6),
        Text(
          '镜像列表更新于 ${DateTime.fromMillisecondsSinceEpoch(fetchedAt).toLocal().toString().substring(0, 16)}'
          '（来源：jsDelivr CDN）',
          style: Theme.of(context).textTheme.bodySmall,
        ),
      ],
      const SizedBox(height: 10),
    ]);
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
      const SizedBox(height: 24),
      _buildMirrorSelector(),
    ]);
  }


  /// 点「下载并安装」：先打开**可见的**下载与安装界面（模态进度），
  /// 下载完成后**自动**启动安装程序（Windows 打开安装向导；Android 打开系统安装界面）。
  ///
  /// 2026-09-22（用户反馈）：此前这里只调用 `download()` 静默后台下载 ⇒
  /// 用户既看不到进度，下载完还得自己去临时目录找安装包 ✗。
  Future<void> _downloadAndInstall(BuildContext context) async {
    final m = UpdateManager.instance;
    final dialog = showDialog<void>(
      context: context,
      barrierDismissible: false,
      builder: (_) => const _UpdateProgressDialog(),
    );
    try {
      await m.download();
      if (m.status.value == UpdateStatus.downloaded) {
        // 自动进入安装：Windows 弹出安装向导（可见），Android 弹系统安装界面。
        await m.install();
      }
    } finally {
      if (context.mounted) {
        Navigator.of(context, rootNavigator: true).maybePop();
      }
      await dialog;
    }
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
                  onPressed: () => _downloadAndInstall(context),
                  icon: const Icon(Icons.download, size: 18),
                  label: const Text('下载并安装'),
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
          label: const Text('下载并安装'),
        ),
      ],
    ),
  );
}


/// 「下载并安装」的可见进度界面（2026-09-22 用户反馈新增）。
///
/// 设计要点：
/// - **不再后台静默下载**：打开即显示版本、文件名、进度与安装包保存位置；
/// - 下载完成 ⇒ 由 `_downloadAndInstall` 自动调用 `install()` 进入安装界面；
/// - 失败 ⇒ 显示原因（可关闭后重试），而不是静默失败。
class _UpdateProgressDialog extends StatelessWidget {
  const _UpdateProgressDialog();

  @override
  Widget build(BuildContext context) {
    final m = UpdateManager.instance;
    return AlertDialog(
      title: const Text('下载并安装更新'),
      content: ValueListenableBuilder<UpdateStatus>(
        valueListenable: m.status,
        builder: (context, status, _) {
          final info = m.info;
          final failed = status == UpdateStatus.error;
          return Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              if (info != null) ...[
                Text('版本 v${info.version}'),
                const SizedBox(height: 4),
                Text(info.asset.name, style: Theme.of(context).textTheme.bodySmall),
                const SizedBox(height: 12),
              ],
              if (status == UpdateStatus.downloading)
                ValueListenableBuilder<double>(
                  valueListenable: m.progress,
                  builder: (context, p, _) => Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      LinearProgressIndicator(value: p <= 0 ? null : p),
                      const SizedBox(height: 6),
                      Text('已下载 ${(p * 100).toStringAsFixed(0)}%'),
                    ],
                  ),
                )
              else if (status == UpdateStatus.installing)
                const Text('下载完成，正在打开安装程序…（Windows 会关闭本应用）')
              else if (failed)
                Text(
                  '下载失败：${m.error.value ?? '未知错误'}',
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                )
              else
                const Text('准备中…'),
              const SizedBox(height: 12),
              Text(
                '安装包保存位置：${m.downloadPath ?? '（尚未确定）'}',
                style: Theme.of(context).textTheme.bodySmall,
              ),
            ],
          );
        },
      ),
      actions: [
        ValueListenableBuilder<UpdateStatus>(
          valueListenable: m.status,
          builder: (context, status, _) {
            final busy = status == UpdateStatus.downloading ||
                status == UpdateStatus.installing;
            return TextButton(
              onPressed: busy
                  ? null
                  : () => Navigator.of(context, rootNavigator: true).maybePop(),
              child: Text(status == UpdateStatus.error ? '关闭并重试' : '关闭'),
            );
          },
        ),
      ],
    );
  }
}
