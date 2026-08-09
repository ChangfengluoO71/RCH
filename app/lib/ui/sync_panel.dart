// 同步设置（Phase 6.5 收敛版）。
//
// 只保留：WebDAV 配置 + 测试连接 + 设备名称 + 自动同步 + 最后同步/状态 + 立即同步 +
// 参与设备 + 同步历史。旧概念（Push/Pull/Export/Import/Archive/rchbundle）本阶段从面板移除，
// 正式删除在 Phase 7。

// ignore_for_file: unused_element

import 'package:app/src/rust/api/sync.dart' as syncapi;
import 'package:app/store/sync_engine.dart';
import 'package:app/store/sync_manager.dart';
import 'package:flutter/material.dart';

class SyncPanel extends StatefulWidget {
  const SyncPanel({super.key});

  @override
  State<SyncPanel> createState() => _SyncPanelState();
}

class _SyncPanelState extends State<SyncPanel> {
  final _urlCtrl = TextEditingController();
  final _userCtrl = TextEditingController();
  final _passCtrl = TextEditingController();
  final _dirCtrl = TextEditingController();
  final _deviceNameCtrl = TextEditingController();

  String _lastSyncText = '从未';
  List<syncapi.SyncHistoryDto> _history = [];
  List<syncapi.SyncDeviceDto> _devices = [];

  @override
  void initState() {
    super.initState();
    final mgr = SyncManager.instance;
    _urlCtrl.text = mgr.webdavUrl;
    _userCtrl.text = mgr.webdavUsername;
    _passCtrl.text = mgr.webdavPassword;
    _dirCtrl.text = mgr.webdavDir;
    _deviceNameCtrl.text = mgr.deviceName;
    _refreshMeta();
  }

  @override
  void dispose() {
    _urlCtrl.dispose();
    _userCtrl.dispose();
    _passCtrl.dispose();
    _dirCtrl.dispose();
    _deviceNameCtrl.dispose();
    super.dispose();
  }

  Future<void> _refreshMeta() async {
    try {
      final st = await syncapi.syncStatus();
      if (st.lastError.isNotEmpty) {
        _lastSyncText = '上次失败: ${st.lastError}';
      } else if (!st.initialized) {
        _lastSyncText = '从未';
      } else {
        final t = DateTime.fromMillisecondsSinceEpoch(st.lastSyncAt).toLocal();
        final s = t.toString();
        _lastSyncText = '${s.substring(0, s.length > 19 ? 19 : s.length)}（v${st.revision}）';
      }
      _history = await syncapi.syncHistoryRecent(limit: 8);
      _devices = await syncapi.syncDevicesList();
    } catch (_) {}
    if (mounted) setState(() {});
  }

  Future<void> _saveWebdav() async {
    await SyncManager.instance.setWebdavConfig(
      url: _urlCtrl.text,
      username: _userCtrl.text,
      password: _passCtrl.text,
      dir: _dirCtrl.text,
    );
  }

  Future<void> _saveDeviceName() async {
    SyncManager.instance.deviceName = _deviceNameCtrl.text.trim();
    await SyncManager.instance.save();
  }

  Future<void> _testWebdav() async {
    await _saveWebdav();
    final r = await SyncManager.instance.testWebdavConnection();
    if (mounted) _snack(r);
  }

  Future<void> _syncNow() async {
    final m = await SyncEngine.instance.syncNow();
    if (mounted) {
      setState(() {});
      await _refreshMeta();
      _snack(m);
    }
  }

  Widget _configField(String label, TextEditingController ctrl,
      {bool obscure = false, String hint = '', required void Function(String) onChanged}) {
    return Padding(
      padding: const EdgeInsets.only(top: 6),
      child: Row(children: [
        SizedBox(width: 88, child: Text(label, style: const TextStyle(fontSize: 13))),
        Expanded(
          child: TextField(
            controller: ctrl,
            obscureText: obscure,
            onChanged: onChanged,
            decoration: InputDecoration(
              isDense: true,
              hintText: hint,
              border: const OutlineInputBorder(),
            ),
            style: const TextStyle(fontSize: 13),
          ),
        ),
      ]),
    );
  }

  void _snack(String msg) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(msg), duration: const Duration(seconds: 3)),
    );
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Text('同步', style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600)),
        const SizedBox(height: 4),
        const Text(
          '通过 WebDAV 同步文件夹进行多设备同步；浏览使用离线索引，凭据仅保存在本机。',
          style: TextStyle(fontSize: 12, color: Colors.grey),
        ),
        const SizedBox(height: 8),
        _configField('WebDAV 地址', _urlCtrl,
            hint: 'https://dav.example.com/dav', onChanged: (_) => _saveWebdav()),
        _configField('账号', _userCtrl, onChanged: (_) => _saveWebdav()),
        _configField('密码', _passCtrl, obscure: true, onChanged: (_) => _saveWebdav()),
        _configField('远程目录', _dirCtrl,
            hint: 'RCH/sync（留空用默认）', onChanged: (_) => _saveWebdav()),
        _configField('设备名称', _deviceNameCtrl,
            hint: '我的 Windows（随同步传播）', onChanged: (_) => _saveDeviceName()),
        const SizedBox(height: 8),
        Row(children: [
          TextButton.icon(
            onPressed: _testWebdav,
            icon: const Icon(Icons.wifi_tethering, size: 18),
            label: const Text('测试连接'),
          ),
          TextButton.icon(
            onPressed: _syncNow,
            icon: const Icon(Icons.sync, size: 18),
            label: const Text('立即同步'),
          ),
          const Spacer(),
          const Text('自动同步', style: TextStyle(fontSize: 12)),
          Switch(
            value: SyncEngine.instance.autoSync,
            onChanged: (v) async {
              await SyncEngine.instance.setAutoSync(v);
              if (mounted) setState(() {});
            },
          ),
        ]),
        const Text(
          '同步间隔 60 秒；启动/回前台/本地变更（防抖 2 秒）自动触发；失败自动重试。',
          style: TextStyle(fontSize: 11, color: Colors.grey),
        ),
        const SizedBox(height: 8),
        Text('最后同步: $_lastSyncText', style: const TextStyle(fontSize: 12)),
        Text(
          '状态: ${SyncEngine.instance.lastStatus}',
          style: const TextStyle(fontSize: 12, color: Colors.blueGrey),
        ),
        const SizedBox(height: 12),
        const Text('参与设备', style: TextStyle(fontSize: 13, fontWeight: FontWeight.w600)),
        if (_devices.isEmpty)
          const Text('暂无', style: TextStyle(fontSize: 12, color: Colors.white38))
        else
          ..._devices.map((d) => Padding(
                padding: const EdgeInsets.only(top: 2),
                child: Text(
                  '${d.deviceName}（${d.platform}）· 最后同步 v${d.lastRevision}',
                  style: const TextStyle(fontSize: 12),
                ),
              )),
        const SizedBox(height: 12),
        const Text('同步历史', style: TextStyle(fontSize: 13, fontWeight: FontWeight.w600)),
        if (_history.isEmpty)
          const Text('暂无', style: TextStyle(fontSize: 12, color: Colors.white38))
        else
          ..._history.map((h) {
            final t = DateTime.fromMillisecondsSinceEpoch(h.startTime).toLocal();
            final s = t.toString();
            final time = s.substring(0, s.length > 19 ? 19 : s.length);
            final err = h.error.isEmpty ? '' : ' · 失败: ${h.error}';
            return Padding(
              padding: const EdgeInsets.only(top: 2),
              child: Text(
                '$time  v${h.revisionBefore}→v${h.revisionAfter}'
                '  拉${h.pullCount} 推${h.pushCount} 合${h.mergeCount} 冲突${h.conflictCount}$err',
                style: const TextStyle(fontSize: 11, color: Colors.white70),
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
              ),
            );
          }),
      ],
    );
  }
}
