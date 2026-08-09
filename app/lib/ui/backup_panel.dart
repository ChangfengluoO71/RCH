// 备份（Phase 7 / ADR-025）：rchpkg 导出/导入，与日常同步完全独立。

import 'dart:io';

import 'package:app/store/storage_access.dart';
import 'package:app/store/sync_manager.dart';
import 'package:app/ui/common.dart';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';

class BackupPanel extends StatelessWidget {
  const BackupPanel({super.key});

  Future<String?> _askPassphrase(BuildContext context, {required bool export}) async {
    final ctrl = TextEditingController();
    return showDialog<String>(
      context: context,
      builder: (c) => AlertDialog(
        title: Text(export ? '导出备份' : '导入备份'),
        content: Column(mainAxisSize: MainAxisSize.min, children: [
          Text(
            export ? '设置口令可把书源凭据加密写入备份；留空则不含凭据。'
                : '若备份包含加密凭据，请输入导出时的口令；否则留空。',
            style: const TextStyle(fontSize: 12),
          ),
          const SizedBox(height: 8),
          TextField(
            controller: ctrl,
            obscureText: true,
            autofocus: true,
            decoration: const InputDecoration(
                labelText: '口令（可选）', border: OutlineInputBorder(), isDense: true),
          ),
        ]),
        actions: [
          TextButton(onPressed: () => Navigator.pop(c), child: const Text('取消')),
          FilledButton(
              onPressed: () => Navigator.pop(c, ctrl.text.trim()), child: const Text('确定')),
        ],
      ),
    );
  }

  Future<void> _export(BuildContext context) async {
    final pass = await _askPassphrase(context, export: true);
    if (pass == null || !context.mounted) return;
    final stamp = DateTime.now().millisecondsSinceEpoch;
    final fileName = 'rch_backup_$stamp.rchpkg';
    final String destPath;
    if (isAndroidPlatform) {
      if (!await ensureAllFilesAccess(context)) return;
      final dir = await getDirectoryPath(confirmButtonText: '选择此目录');
      if (dir == null || !context.mounted) return;
      destPath = '$dir${Platform.pathSeparator}$fileName';
    } else {
      final loc = await getSaveLocation(
        suggestedName: fileName,
        acceptedTypeGroups: const [
          XTypeGroup(label: 'RCH 备份', extensions: ['rchpkg']),
        ],
      );
      if (loc == null || !context.mounted) return;
      destPath = loc.path;
    }
    final msg = await SyncManager.instance.exportToFile(destPath, passphrase: pass);
    if (context.mounted) _snack(context, msg);
  }

  Future<void> _import(BuildContext context) async {
    final file = await openFile(
      acceptedTypeGroups: const [
        XTypeGroup(label: 'RCH 备份', extensions: ['rchpkg']),
      ],
    );
    if (file == null || !context.mounted) return;
    final pass = await _askPassphrase(context, export: false);
    if (pass == null || !context.mounted) return;
    final msg = pass.isEmpty
        ? await SyncManager.instance.restoreFrom(file.path)
        : await SyncManager.instance.restoreFromWithCredentials(file.path, pass);
    if (context.mounted) _snack(context, msg);
  }

  void _snack(BuildContext context, String msg) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(msg), duration: const Duration(seconds: 3)),
    );
  }

  @override
  Widget build(BuildContext context) {
    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      const Text('备份', style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)),
      const SizedBox(height: 4),
      const Text(
        '导出/导入完整 .rchpkg 备份（书源、目录索引、元数据、标签、进度；可选加密凭据）。与日常同步相互独立。',
        style: TextStyle(fontSize: 12, color: Colors.grey),
      ),
      const SizedBox(height: 8),
      Row(children: [
        TextButton.icon(
          onPressed: () => _export(context),
          icon: const Icon(Icons.save_alt, size: 18),
          label: const Text('导出备份'),
        ),
        const SizedBox(width: 8),
        TextButton.icon(
          onPressed: () => _import(context),
          icon: const Icon(Icons.restore, size: 18),
          label: const Text('导入备份'),
        ),
      ]),
    ]);
  }
}
