// 设置页"备份 / 同步"面板（P2）：模式选择、目录/书源配置、手动同步与恢复。

import 'package:app/store/library_store.dart';
import 'package:app/store/sync_manager.dart';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';

class SyncPanel extends StatefulWidget {
  const SyncPanel({super.key});

  @override
  State<SyncPanel> createState() => _SyncPanelState();
}

class _SyncPanelState extends State<SyncPanel> {
  @override
  void initState() {
    super.initState();
    SyncManager.instance.addListener(_onChanged);
  }

  @override
  void dispose() {
    SyncManager.instance.removeListener(_onChanged);
    super.dispose();
  }

  void _onChanged() {
    if (mounted) setState(() {});
  }

  Future<void> _pickDir() async {
    final picked = await getDirectoryPath();
    if (picked == null || !mounted) return;
    await SyncManager.instance.setDir(picked);
  }

  Future<void> _restore() async {
    final file = await openFile(
      acceptedTypeGroups: const [
        XTypeGroup(label: 'RCH 同步包', extensions: ['rchpkg']),
      ],
    );
    if (file == null || !mounted) return;
    final result = await SyncManager.instance.restoreFrom(file.path);
    if (mounted) _snack(result);
  }

  void _snack(String msg) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(msg), duration: const Duration(seconds: 3)),
    );
  }

  @override
  Widget build(BuildContext context) {
    final mgr = SyncManager.instance;
    final webdavSources =
        LibraryStore.instance.sources.where((s) => s.isWebDav).toList();
    final last = mgr.lastAt == 0
        ? '从未'
        : DateTime.fromMillisecondsSinceEpoch(mgr.lastAt)
            .toLocal()
            .toString()
            .substring(0, 19);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Text('备份 / 同步', style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)),
        const SizedBox(height: 4),
        const Text(
          '无服务器，通过你自己的 WebDAV 或网盘同步盘双向同步；敏感凭据不会写入同步包。',
          style: TextStyle(fontSize: 12, color: Colors.grey),
        ),
        const SizedBox(height: 8),
        Row(children: [
          const Text('模式: '),
          DropdownButton<SyncMode>(
            value: mgr.mode,
            items: SyncMode.values
                .map((m) => DropdownMenuItem(value: m, child: Text(m.label)))
                .toList(),
            onChanged: (v) {
              if (v != null) mgr.setMode(v);
            },
          ),
        ]),
        if (mgr.mode == SyncMode.folder) ...[
          Row(children: [
            Expanded(
              child: Text(
                mgr.dir.isEmpty ? '未选择同步目录' : mgr.dir,
                style: const TextStyle(fontSize: 12),
                overflow: TextOverflow.ellipsis,
              ),
            ),
            TextButton(onPressed: _pickDir, child: const Text('选择目录')),
          ]),
          const Text(
            '提示：选择网盘同步盘（OneDrive / 坚果云 / 百度同步空间等）的本地文件夹。',
            style: TextStyle(fontSize: 12, color: Colors.grey),
          ),
        ],
        if (mgr.mode == SyncMode.webdav) ...[
          Row(children: [
            const Text('WebDAV 书源: '),
            DropdownButton<String>(
              value: webdavSources.any((s) => s.id == mgr.webdavSourceId)
                  ? mgr.webdavSourceId
                  : null,
              hint: const Text('选择书源'),
              items: webdavSources
                  .map((s) => DropdownMenuItem(
                        value: s.id,
                        child: Text(s.name.isEmpty ? s.url ?? s.id : s.name),
                      ))
                  .toList(),
              onChanged: (v) {
                if (v != null) mgr.setWebdavSourceId(v);
              },
            ),
          ]),
          const Text(
            '提示：同步包将写入所选书源的 RCH/sync/ 目录。',
            style: TextStyle(fontSize: 12, color: Colors.grey),
          ),
        ],
        if (mgr.mode != SyncMode.off) ...[
          Row(children: [
            const Expanded(child: Text('跨设备搜索', style: TextStyle(fontSize: 14))),
            Switch(
              value: mgr.crossDeviceSearch,
              onChanged: (v) => mgr.setCrossDeviceSearch(v),
            ),
          ]),
          const Text(
            '开启后，全局搜索包含其他设备的本地书源（仅元数据，可编辑不可阅读）。',
            style: TextStyle(fontSize: 12, color: Colors.grey),
          ),
          const SizedBox(height: 4),
          Row(children: [
            const Text('定时同步: '),
            DropdownButton<int>(
              value: const [0, 30, 60, 180].contains(mgr.intervalMinutes)
                  ? mgr.intervalMinutes
                  : 0,
              items: const [
                DropdownMenuItem(value: 0, child: Text('关闭')),
                DropdownMenuItem(value: 30, child: Text('每 30 分钟')),
                DropdownMenuItem(value: 60, child: Text('每小时')),
                DropdownMenuItem(value: 180, child: Text('每 3 小时')),
              ],
              onChanged: (v) {
                if (v != null) mgr.setInterval(v);
              },
            ),
          ]),
          Row(children: [
            TextButton.icon(
              onPressed: mgr.busy
                  ? null
                  : () => mgr.pushNow().then((m) {
                        if (mounted) _snack(m);
                      }),
              icon: const Icon(Icons.upload),
              label: const Text('立即推送'),
            ),
            TextButton.icon(
              onPressed: mgr.busy
                  ? null
                  : () => mgr.pullNow().then((m) {
                        if (mounted) _snack(m);
                      }),
              icon: const Icon(Icons.download),
              label: const Text('立即拉取'),
            ),
            TextButton.icon(
              onPressed: mgr.busy ? null : _restore,
              icon: const Icon(Icons.restore),
              label: const Text('从文件恢复'),
            ),
          ]),
          Text('最近同步: $last', style: const TextStyle(fontSize: 12)),
          Text(
            mgr.lastStatus,
            style: const TextStyle(fontSize: 12, color: Colors.blueGrey),
          ),
          if (mgr.ignoredCopies > 0)
            Text(
              '检测到 ${mgr.ignoredCopies} 个冲突/临时副本，自动同步已忽略',
              style: const TextStyle(fontSize: 12, color: Colors.orange),
            ),
        ],
      ],
    );
  }
}
