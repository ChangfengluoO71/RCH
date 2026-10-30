// 同步配置与备份管理（Phase 7 收敛版）。
//
// 日常同步由 SyncEngine（Sync State + 三方合并）负责；本类只保留：
// WebDAV 传输配置、设备名、跨设备搜索开关、rchpkg 备份导出/恢复。
// 旧的 Push/Pull/归档/模式已删除（ADR-025）。

import 'package:app/src/rust/api/db.dart';
import 'package:app/src/rust/api/package.dart';
import 'package:app/src/rust/api/source.dart';
import 'package:app/store/credential_vault.dart';
import 'package:app/store/long_task_registry.dart';
import 'package:flutter/foundation.dart';

const _kWebdavUrl = 'sync_webdav_url';
const _kWebdavUsername = 'sync_webdav_username';
const _kWebdavPassword = 'sync_webdav_password';
const _kWebdavPasswordRef = 'sync_webdav_password_ref';
const _kWebdavDir = 'sync_webdav_dir';
const _kDeviceName = 'sync_device_name';
const _kCrossDeviceSearch = 'sync_cross_device_search';

/// 清理不可见控制字符：复制粘贴 WebDAV 地址/目录时可能带入 \x00-\x1F
/// （实测：MuMu 上目录变成 `RCH<0x14>同步` → 坚果云 409 AncestorsNotFound）。
String sanitizeConfig(String s) => s.replaceAll(RegExp(r'[\x00-\x1F\x7F]'), '');

/// Normalize a persisted vault reference without allowing control characters
/// or an empty value to cross the storage boundary.
String? normalizeCredentialRef(String? value) {
  final normalized = value == null ? '' : sanitizeConfig(value).trim();
  return normalized.isEmpty ? null : normalized;
}

/// 同步配置 + 备份管理（单例）。
class SyncManager extends ChangeNotifier {
  SyncManager._({
    this._credentialVault,
    Future<List<SettingEntryDto>> Function()? loadSettings,
    Future<void> Function({required String key, required String value})?
    saveSetting,
    Future<List<DeviceDto>> Function()? listDevices,
  }) : _loadSettings = loadSettings ?? dbLoadAllSettings,
       _saveSetting = saveSetting ?? dbSaveSetting,
       _listDevices = listDevices ?? dbListDevices;

  @visibleForTesting
  SyncManager.forTesting({
    required CredentialVault credentialVault,
    required Future<List<SettingEntryDto>> Function() loadSettings,
    required Future<void> Function({required String key, required String value})
    saveSetting,
    required Future<List<DeviceDto>> Function() listDevices,
  }) : this._(
         credentialVault: credentialVault,
         loadSettings: loadSettings,
         saveSetting: saveSetting,
         listDevices: listDevices,
       );

  static final SyncManager instance = SyncManager._();

  String webdavUrl = '';
  String webdavUsername = '';
  String webdavPassword = '';
  String webdavDir = 'RCH/sync';
  String deviceName = '';
  int lastAt = 0;
  String lastStatus = '尚未同步';
  bool busy = false;
  bool crossDeviceSearch = true;
  BigInt? _webdavSession;
  CredentialVault? _credentialVault;
  String? _webdavPasswordRef;
  bool _credentialVaultReady = false;
  bool _webdavPasswordDirty = false;
  final Future<List<SettingEntryDto>> Function() _loadSettings;
  final Future<void> Function({required String key, required String value})
  _saveSetting;
  final Future<List<DeviceDto>> Function() _listDevices;

  bool get _usesCredentialVault => usesNativeCredentialVault(isWeb: kIsWeb);

  CredentialVault get _vault => _credentialVault ??= platformCredentialVault();

  /// 设备 id → 名称（幽灵书源来源展示；Phase 6 起由 syncDevicesList 提供主数据）。
  final Map<String, String> deviceNames = {};

  Future<void> init() async {
    await load();
  }

  Future<void> load() async {
    try {
      final entries = await _loadSettings();
      final map = {for (final e in entries) e.key: e.value};
      webdavUrl = sanitizeConfig(map[_kWebdavUrl] ?? '').trim();
      webdavUsername = sanitizeConfig(map[_kWebdavUsername] ?? '').trim();
      final legacyPassword = sanitizeConfig(map[_kWebdavPassword] ?? '');
      _webdavPasswordRef = normalizeCredentialRef(map[_kWebdavPasswordRef]);
      webdavPassword = legacyPassword;
      _credentialVaultReady = false;
      _webdavPasswordDirty = false;
      if (_usesCredentialVault) {
        final ref = _webdavPasswordRef ?? 'sync:webdav-password';
        try {
          final stored = await _vault.get(ref);
          if (stored != null) {
            webdavPassword = stored;
          } else if (legacyPassword.isNotEmpty) {
            // One-time migration from the old app_settings row. Verify the
            // read-back before save() redacts that row.
            await _vault.put(ref, legacyPassword);
            if (await _vault.get(ref) != legacyPassword) {
              throw StateError('同步凭据保险库回读校验失败');
            }
          }
          _webdavPasswordRef = ref;
          _credentialVaultReady = true;
        } catch (error, stack) {
          // Keep the legacy value in memory and leave the DB row intact; a
          // vault failure must never silently erase the sync password.
          debugPrint(
            '[SyncManager] credential vault migration failed (${error.runtimeType})',
          );
          debugPrintStack(stackTrace: stack);
        }
      }
      webdavDir = sanitizeConfig(map[_kWebdavDir] ?? 'RCH/sync').trim();
      deviceName = map[_kDeviceName] ?? '';
      lastAt = int.tryParse(map['sync_last_at'] ?? '') ?? 0;
      lastStatus = map['sync_last_status'] ?? '尚未同步';
      crossDeviceSearch = map[_kCrossDeviceSearch] != 'false';
      _webdavSession = null;
      deviceNames.clear();
      for (final d in await _listDevices()) {
        deviceNames[d.id] = d.name;
      }
      // 清理后的配置写回，避免隐藏控制字符残留在 DB
      await save();
    } catch (e) {
      debugPrint('[SyncManager] load failed: $e');
    }
    notifyListeners();
  }

  Future<void> save() async {
    String? persistedPassword = webdavPassword;
    String? persistedPasswordRef;
    if (_usesCredentialVault &&
        (_credentialVaultReady || _webdavPasswordDirty)) {
      final ref = _webdavPasswordRef ?? 'sync:webdav-password';
      if (webdavPassword.isNotEmpty) {
        await _vault.put(ref, webdavPassword);
        if (await _vault.get(ref) != webdavPassword) {
          throw StateError('同步凭据保险库回读校验失败');
        }
        _webdavPasswordRef = ref;
        persistedPassword = '';
      } else {
        await _vault.delete(ref);
        if (await _vault.get(ref) != null) {
          throw StateError('sync credential vault delete verification failed');
        }
        _webdavPasswordRef = ref;
        persistedPassword = '';
      }
      _credentialVaultReady = true;
      _webdavPasswordDirty = false;
      persistedPasswordRef = ref;
    } else if (_usesCredentialVault) {
      // A prior read/migration failed. Preserve the old settings row by not
      // writing either the password or its reference until the vault works.
      persistedPassword = null;
    }

    await _saveSetting(key: _kWebdavUrl, value: webdavUrl);
    await _saveSetting(key: _kWebdavUsername, value: webdavUsername);
    if (persistedPassword != null) {
      await _saveSetting(key: _kWebdavPassword, value: persistedPassword);
    }
    if (persistedPasswordRef != null) {
      await _saveSetting(key: _kWebdavPasswordRef, value: persistedPasswordRef);
    }
    await _saveSetting(key: _kWebdavDir, value: webdavDir);
    await _saveSetting(key: _kDeviceName, value: deviceName);
    await _saveSetting(key: 'sync_last_at', value: '$lastAt');
    await _saveSetting(key: 'sync_last_status', value: lastStatus);
    await _saveSetting(key: _kCrossDeviceSearch, value: '$crossDeviceSearch');
    notifyListeners();
  }

  Future<void> setWebdavConfig({
    required String url,
    required String username,
    required String password,
    required String dir,
  }) async {
    webdavUrl = sanitizeConfig(url).trim();
    webdavUsername = sanitizeConfig(username).trim();
    webdavPassword = sanitizeConfig(password);
    _webdavPasswordDirty = true;
    webdavDir = sanitizeConfig(dir).trim();
    _webdavSession = null;
    await save();
  }

  Future<void> setCrossDeviceSearch(bool v) async {
    crossDeviceSearch = v;
    await save();
  }

  /// 幽灵书源来源设备显示名（旧数据兜底）。
  String deviceNameOf(String? deviceId) {
    if (deviceId == null || deviceId.isEmpty) return '其他设备';
    return deviceNames[deviceId] ?? '其他设备';
  }

  /// 测试 WebDAV 连接（使用当前配置建立会话）。
  Future<String> testWebdavConnection() async {
    try {
      await _webdavSessionFor();
      return '连接成功';
    } catch (e) {
      return '连接失败: $e';
    }
  }

  /// 从任意 `.rchpkg` 文件恢复（保留本地书源凭据）。
  Future<String> restoreFrom(String path) async {
    if (busy) return '正在处理，请稍候';
    final taskLease = LongTaskRegistry.instance.register(
      kind: 'backup_restore',
      label: '恢复 RCH 备份',
      phase: '读取备份并写入本地资料库',
    );
    busy = true;
    notifyListeners();
    try {
      final stats = await rchpkgImport(path: path, force: true);
      final msg =
          '恢复成功（${stats.sources.toInt()} 书源 / ${stats.metas.toInt()} 详情 / ${stats.tags.toInt()} 标签）'
          '${stats.ghosts.toInt() > 0 ? '，其中 ${stats.ghosts.toInt()} 个为其他设备书源' : ''}';
      await _finish(msg);
      return msg;
    } catch (e) {
      final msg = '恢复失败: $e';
      await _finish(msg);
      return msg;
    } finally {
      taskLease.dispose();
      busy = false;
      notifyListeners();
    }
  }

  /// 从任意 `.rchpkg` 文件恢复，并应用包内的加密书源凭据（需导出时设置的口令）。
  Future<String> restoreFromWithCredentials(
    String path,
    String passphrase,
  ) async {
    if (busy) return '正在处理，请稍候';
    final taskLease = LongTaskRegistry.instance.register(
      kind: 'backup_restore',
      label: '恢复 RCH 备份',
      phase: '恢复备份并应用加密凭据',
    );
    busy = true;
    notifyListeners();
    try {
      final stats = await rchpkgImportWithCredentials(
        path: path,
        passphrase: passphrase,
      );
      final msg =
          '恢复成功（${stats.sources.toInt()} 书源 / ${stats.metas.toInt()} 详情 / ${stats.tags.toInt()} 标签）· 加密凭据已应用';
      await _finish(msg);
      return msg;
    } catch (e) {
      final msg = '恢复失败: $e';
      await _finish(msg);
      return msg;
    } finally {
      taskLease.dispose();
      busy = false;
      notifyListeners();
    }
  }

  /// 导出标准 `.rchpkg` 备份包（全量快照；`passphrase` 非空时附带加密书源凭据）。
  Future<String> exportToFile(String path, {String passphrase = ''}) async {
    if (busy) return '正在处理，请稍候';
    final taskLease = LongTaskRegistry.instance.register(
      kind: 'backup_export',
      label: '导出 RCH 备份',
      phase: '生成本地备份文件',
    );
    busy = true;
    notifyListeners();
    try {
      final info = await rchpkgExportSnapshot(
        path: path,
        passphrase: passphrase,
      );
      final msg =
          '导出成功：${info.sources.toInt()} 书源 / ${info.metas.toInt()} 详情 / '
          '${info.tags.toInt()} 标签${passphrase.isNotEmpty ? '（含加密凭据）' : ''}';
      await _finish(msg);
      return msg;
    } catch (e) {
      final msg = '导出失败: $e';
      await _finish(msg);
      return msg;
    } finally {
      taskLease.dispose();
      busy = false;
      notifyListeners();
    }
  }

  Future<BigInt> _webdavSessionFor() async {
    if (_webdavSession != null) return _webdavSession!;
    final url = webdavUrl.trim();
    if (url.isEmpty) throw '未填写 WebDAV 地址';
    final s = await webdavConnect(
      url: url,
      username: webdavUsername.trim(),
      password: webdavPassword,
    );
    _webdavSession = s.id;
    return s.id;
  }

  Future<void> _finish(String msg) async {
    lastStatus = msg;
    lastAt = DateTime.now().millisecondsSinceEpoch;
    try {
      await save();
    } catch (e) {
      debugPrint('[SyncManager] 状态保存失败: $e');
    }
  }
}
