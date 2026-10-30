import 'package:app/ui/rch_overlay.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

/// Ask before trusting an SFTP server key. No credentials have been sent yet.
Future<bool> confirmSftpHostKeyTrust(
  BuildContext context, {
  required String host,
  required int port,
  required String expectedFingerprint,
  required String actualFingerprint,
  required String expectedEndpoint,
  required String actualEndpoint,
}) async {
  final changed =
      expectedFingerprint.trim().isNotEmpty ||
      expectedEndpoint.trim().isNotEmpty;
  return await showRchDialog<bool>(
        context: context,
        builder: (dialogContext) => AlertDialog(
          title: Text(changed ? 'SFTP 主机密钥已变化' : '确认 SFTP 主机身份'),
          content: SelectableText(
            '服务器：$host:$port\n\n'
            '${expectedEndpoint.trim().isNotEmpty ? '已保存 endpoint：\n$expectedEndpoint\n\n' : ''}'
            '${changed ? '已保存指纹：\n$expectedFingerprint\n\n' : ''}'
            '当前 endpoint：\n$actualEndpoint\n\n'
            '当前 SHA-256 指纹：\n$actualFingerprint\n\n'
            '${changed ? '只有确认服务器已更换密钥且来源可信时才继续。' : '核对该指纹确实属于你的服务器后，才会发送登录凭据。'}',
          ),
          actions: [
            TextButton.icon(
              onPressed: () =>
                  Clipboard.setData(ClipboardData(text: actualFingerprint)),
              icon: const Icon(Icons.copy, size: 16),
              label: const Text('复制指纹'),
            ),
            TextButton(
              onPressed: () => Navigator.of(dialogContext).pop(false),
              child: const Text('取消'),
            ),
            FilledButton(
              onPressed: () => Navigator.of(dialogContext).pop(true),
              child: Text(changed ? '更新信任并继续' : '信任此主机并继续'),
            ),
          ],
        ),
      ) ??
      false;
}
