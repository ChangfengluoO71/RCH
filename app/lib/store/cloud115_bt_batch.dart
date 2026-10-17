import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:app/src/rust/source/cloud115/offline.dart' as cloud115;
import 'package:app/store/eh_subscription_store.dart';
import 'package:app/store/long_task_registry.dart';
import 'package:path_provider/path_provider.dart';

const _persistedBatchStates = {
  'upload_started',
  'upload_outcome_unknown',
  'upload_parse_pending',
  'torrent_parsed',
  'submit_started',
  'submitted',
  'unknown_remote_state',
  'duplicate_lookup_unknown',
  'duplicate_found',
  'duplicate_found_delete_failed',
  'local_torrent_removed',
  'waf_blocked',
  'rate_limited',
  'auth_expired',
  'access_denied',
  'challenge_required',
  'rejected',
};

enum Cloud115BtDeliveryMode { cloud115, localTorrentOnly }

bool _isSafeRelativeTorrent(String value) {
  if (value.isEmpty ||
      value.startsWith('/') ||
      value.startsWith('\\') ||
      RegExp(r'^[a-zA-Z]:').hasMatch(value)) {
    return false;
  }
  final components = value.replaceAll('\\', '/').split('/');
  if (components.any(
    (component) => component.isEmpty || component == '.' || component == '..',
  )) {
    return false;
  }
  return components.last.toLowerCase().endsWith('.torrent');
}

typedef Cloud115BtBatchPreflight =
    Future<cloud115.Cloud115BtPreflight> Function(EhCheckedManifestEntry entry);
typedef Cloud115BtBatchUpload =
    Future<cloud115.Cloud115BtUploadParseResult> Function(
      EhCheckedManifestEntry entry,
    );
typedef Cloud115BtBatchUploadRecovery =
    Future<cloud115.Cloud115BtUploadParseResult> Function(
      EhCheckedManifestEntry entry,
      String? pickCode,
    );
typedef Cloud115BtBatchSubmit =
    Future<cloud115.Cloud115BtSubmitResult> Function(
      EhCheckedManifestEntry entry,
      List<int> wantedIndices,
    );
typedef Cloud115BtBatchTaskQuery =
    Future<cloud115.Cloud115BtTaskStatus> Function(String infoHash);
typedef Cloud115BtBatchTorrentDelete =
    Future<void> Function(
      EhCheckedManifestEntry entry,
      String accountId,
      cloud115.Cloud115BtFolderSelection target,
    );
typedef Cloud115BtBatchProgress =
    void Function(int current, int total, String relativeFile, String stage);

/// Credential-free, append-only operation ledger for foreground 115 batches.
/// It lives in application support, outside the EH output directory/manifest.
class Cloud115BtBatchLedger {
  Cloud115BtBatchLedger(this.file);

  final File file;

  static final Map<String, Future<void>> _localQueues = {};

  static Future<Cloud115BtBatchLedger> inApplicationSupportDirectory() async {
    final support = await getApplicationSupportDirectory();
    final directory = Directory('${support.path}${Platform.pathSeparator}RCH');
    await directory.create(recursive: true);
    return Cloud115BtBatchLedger(
      File('${directory.path}${Platform.pathSeparator}cloud115_bt_batch.jsonl'),
    );
  }

  Future<bool> cleanupAfterSubmit() =>
      withExclusiveLock((transaction) async => transaction.cleanupAfterSubmit);

  Future<Cloud115BtBatchSelectionPreferences> selectionPreferences() =>
      withExclusiveLock(
        (transaction) async => transaction.selectionPreferences,
      );

  Future<Cloud115BtBatchLedgerSnapshot> snapshotForMonitoring() =>
      withExclusiveLock(
        (transaction) async => Cloud115BtBatchLedgerSnapshot(
          records: transaction.records,
          selectionPreferences: transaction.selectionPreferences,
        ),
      );

  Future<void> setSelectionPreferences({
    required String? sourceId,
    required cloud115.Cloud115BtFolderSelection? target,
    Cloud115BtDeliveryMode mode = Cloud115BtDeliveryMode.cloud115,
  }) => withExclusiveLock((transaction) async {
    await transaction.setSelectionPreferences(
      sourceId: sourceId,
      target: target,
      mode: mode,
    );
  });

  Future<void> setCleanupAfterSubmit(bool enabled) =>
      withExclusiveLock((transaction) async {
        await transaction.setCleanupAfterSubmit(enabled);
      });

  /// Serializes batches across isolates and RCH processes. The OS file lock is
  /// held for the entire batch so a second process cannot race a write intent.
  Future<T> withExclusiveLock<T>(
    Future<T> Function(Cloud115BtBatchLedgerTransaction transaction) action,
  ) async {
    final absolutePath = file.absolute.path;
    final queueKey = Platform.isWindows
        ? absolutePath.toLowerCase()
        : absolutePath;
    final previous = _localQueues[queueKey] ?? Future<void>.value();
    final released = Completer<void>();
    final queued = released.future;
    _localQueues[queueKey] = queued;
    await previous;

    try {
      await file.parent.create(recursive: true);
      final lockFile = File('${file.path}.lock');
      final lockHandle = await lockFile.open(mode: FileMode.append);
      try {
        await lockHandle.lock(FileLock.exclusive);
        final transaction = await Cloud115BtBatchLedgerTransaction.open(file);
        return await action(transaction);
      } finally {
        await lockHandle.unlock();
        await lockHandle.close();
      }
    } finally {
      released.complete();
      if (identical(_localQueues[queueKey], queued)) {
        _localQueues.remove(queueKey);
      }
    }
  }
}

class Cloud115BtBatchLedgerTransaction {
  Cloud115BtBatchLedgerTransaction._(
    this._file,
    this._records,
    this._cleanup,
    this._selectionPreferences,
  ) : _sourceId = null,
      _batchId = null;

  final File _file;
  final Map<String, Cloud115BtBatchLedgerRecord> _records;
  bool _cleanup;
  Cloud115BtBatchSelectionPreferences _selectionPreferences;
  String? _sourceId;
  String? _batchId;

  bool get cleanupAfterSubmit => _cleanup;
  Cloud115BtBatchSelectionPreferences get selectionPreferences =>
      _selectionPreferences;
  List<Cloud115BtBatchLedgerRecord> get records =>
      List.unmodifiable(_records.values);

  Future<void> beginBatchContext({
    String? sourceId,
    required String batchId,
  }) async {
    final normalizedSourceId = sourceId?.trim();
    if (normalizedSourceId != null && normalizedSourceId.isEmpty) {
      throw ArgumentError.value(sourceId, 'sourceId', '必须是非空 ID 或 null');
    }
    if (batchId.trim().isEmpty) throw ArgumentError.value(batchId, 'batchId');
    await _append({
      'type': 'batch_context',
      'sourceId': normalizedSourceId,
      'batchId': batchId,
    });
    _sourceId = normalizedSourceId;
    _batchId = batchId;
  }

  Cloud115BtBatchLedgerRecord? recordFor({
    required String accountId,
    required String infoHash,
    required String targetFolderId,
  }) => _records[_key(accountId, infoHash, targetFolderId)];

  Future<void> setCleanupAfterSubmit(bool enabled) async {
    await _append({'type': 'setting', 'cleanupAfterSubmit': enabled});
    _cleanup = enabled;
  }

  Future<void> setSelectionPreferences({
    required String? sourceId,
    required cloud115.Cloud115BtFolderSelection? target,
    Cloud115BtDeliveryMode mode = Cloud115BtDeliveryMode.cloud115,
  }) async {
    final normalizedSourceId = sourceId?.trim();
    if (normalizedSourceId != null && normalizedSourceId.isEmpty) {
      throw ArgumentError.value(sourceId, 'sourceId', '必须是非空 ID 或 null');
    }
    if (target != null && normalizedSourceId == null) {
      throw ArgumentError('保存目标目录时必须同时保存 Cookie 书源 ID');
    }
    if (target != null) _validateSavedTarget(target);
    final preferences = Cloud115BtBatchSelectionPreferences(
      sourceId: normalizedSourceId,
      target: target,
      mode: mode,
    );
    await _append({
      'type': 'selection',
      'sourceId': normalizedSourceId,
      'mode': _deliveryModeToJson(mode),
      'target': target == null
          ? null
          : {
              'id': target.id,
              'parentId': target.parentId,
              'name': target.name,
              'path': target.path,
            },
    });
    _selectionPreferences = preferences;
  }

  Future<void> recordItem({
    required String accountId,
    required String infoHash,
    required String targetFolderId,
    required String relativeFile,
    required String state,
  }) async {
    final normalizedHash = infoHash.toLowerCase();
    final updatedAtUtc = DateTime.now().toUtc().toIso8601String();
    final record = Cloud115BtBatchLedgerRecord(
      accountId: accountId,
      infoHash: normalizedHash,
      targetFolderId: targetFolderId,
      relativeFile: relativeFile,
      state: state,
      updatedAtUtc: updatedAtUtc,
      sourceId: _sourceId,
      batchId: _batchId,
    );
    await _append({
      'type': 'item',
      'accountId': accountId,
      'infoHash': normalizedHash,
      'targetFolderId': targetFolderId,
      'relativeFile': relativeFile,
      'state': state,
      'updatedAtUtc': updatedAtUtc,
      'sourceId': _sourceId,
      'batchId': _batchId,
    });
    _records[_key(accountId, normalizedHash, targetFolderId)] = record;
  }

  Future<void> _append(Map<String, Object?> event) async {
    final handle = await _file.open(mode: FileMode.append);
    try {
      await handle.writeString(jsonEncode(event));
      await handle.writeString('\n');
      await handle.flush();
    } finally {
      await handle.close();
    }
  }

  static Future<Cloud115BtBatchLedgerTransaction> open(File file) async {
    final records = <String, Cloud115BtBatchLedgerRecord>{};
    var cleanup = false;
    var selectionPreferences = const Cloud115BtBatchSelectionPreferences();
    String? sourceId;
    String? batchId;
    if (!await file.exists()) {
      return Cloud115BtBatchLedgerTransaction._(
        file,
        records,
        cleanup,
        selectionPreferences,
      );
    }

    final bytes = await file.readAsBytes();
    var completeLength = bytes.length;
    if (bytes.isNotEmpty && bytes.last != 10) {
      completeLength = bytes.lastIndexOf(10) + 1;
      final handle = await file.open(mode: FileMode.append);
      try {
        await handle.truncate(completeLength);
        await handle.flush();
      } finally {
        await handle.close();
      }
    }
    if (completeLength == 0) {
      return Cloud115BtBatchLedgerTransaction._(
        file,
        records,
        cleanup,
        selectionPreferences,
      );
    }

    final content = utf8.decode(bytes.sublist(0, completeLength));
    for (final rawLine in content.split('\n')) {
      final line = rawLine.trim();
      if (line.isEmpty) continue;
      final decoded = jsonDecode(line);
      if (decoded is! Map<String, dynamic>) {
        throw const FormatException('115 BT 操作账本记录格式无效');
      }
      switch (decoded['type']) {
        case 'batch_context':
          final nextSourceId = decoded['sourceId'];
          final nextBatchId = decoded['batchId'];
          if ((nextSourceId != null &&
                  (nextSourceId is! String || nextSourceId.trim().isEmpty)) ||
              nextBatchId is! String ||
              nextBatchId.trim().isEmpty) {
            throw const FormatException('115 BT 批次上下文记录无效');
          }
          sourceId = nextSourceId as String?;
          batchId = nextBatchId;
        case 'setting':
          final value = decoded['cleanupAfterSubmit'];
          if (value is! bool) {
            throw const FormatException('115 BT 清理设置记录无效');
          }
          cleanup = value;
        case 'selection':
          final sourceId = decoded['sourceId'];
          if (sourceId != null &&
              (sourceId is! String || sourceId.trim().isEmpty)) {
            throw const FormatException('115 BT 来源选择记录无效');
          }
          final rawTarget = decoded['target'];
          if (rawTarget != null && sourceId == null) {
            throw const FormatException('115 BT 目标目录缺少来源绑定');
          }
          final mode = _deliveryModeFromJson(decoded['mode']);
          final target = _decodeSavedTarget(rawTarget);
          selectionPreferences = Cloud115BtBatchSelectionPreferences(
            sourceId: sourceId as String?,
            target: target,
            mode: mode,
          );
        case 'item':
          final accountId = decoded['accountId'];
          final infoHash = decoded['infoHash'];
          final targetFolderId = decoded['targetFolderId'];
          final relativeFile = decoded['relativeFile'];
          final state = decoded['state'];
          final updatedAtUtc = decoded['updatedAtUtc'];
          final rowSourceId = decoded['sourceId'] ?? sourceId;
          final rowBatchId = decoded['batchId'] ?? batchId;
          if (accountId is! String ||
              accountId.isEmpty ||
              infoHash is! String ||
              !RegExp(r'^[0-9a-fA-F]{40}$').hasMatch(infoHash) ||
              targetFolderId is! String ||
              targetFolderId.isEmpty ||
              relativeFile is! String ||
              !_isSafeRelativeTorrent(relativeFile) ||
              state is! String ||
              !_persistedBatchStates.contains(state) ||
              updatedAtUtc is! String ||
              DateTime.tryParse(updatedAtUtc) == null ||
              (rowSourceId != null &&
                  (rowSourceId is! String || rowSourceId.trim().isEmpty)) ||
              (rowBatchId != null &&
                  (rowBatchId is! String || rowBatchId.trim().isEmpty))) {
            throw const FormatException('115 BT 操作账本条目无效');
          }
          final record = Cloud115BtBatchLedgerRecord(
            accountId: accountId,
            infoHash: infoHash.toLowerCase(),
            targetFolderId: targetFolderId,
            relativeFile: relativeFile,
            state: state,
            updatedAtUtc: updatedAtUtc,
            sourceId: rowSourceId as String?,
            batchId: rowBatchId as String?,
          );
          records[_key(accountId, infoHash, targetFolderId)] = record;
        default:
          throw const FormatException('115 BT 操作账本事件类型未知');
      }
    }
    return Cloud115BtBatchLedgerTransaction._(
      file,
      records,
      cleanup,
      selectionPreferences,
    );
  }

  static cloud115.Cloud115BtFolderSelection? _decodeSavedTarget(Object? value) {
    if (value == null) return null;
    if (value is! Map<String, dynamic>) {
      throw const FormatException('115 BT 目标目录记录无效');
    }
    final id = value['id'];
    final parentId = value['parentId'];
    final name = value['name'];
    final path = value['path'];
    if (id is! String ||
        parentId is! String ||
        name is! String ||
        path is! String) {
      throw const FormatException('115 BT 目标目录记录字段无效');
    }
    final target = cloud115.Cloud115BtFolderSelection(
      id: id,
      parentId: parentId,
      name: name,
      path: path,
    );
    _validateSavedTarget(target);
    return target;
  }

  static void _validateSavedTarget(cloud115.Cloud115BtFolderSelection target) {
    final isRoot =
        target.id == '0' &&
        target.parentId.isEmpty &&
        target.name.isEmpty &&
        target.path == '/';
    final validId = RegExp(r'^\d+$').hasMatch(target.id);
    final validParent =
        target.parentId.isEmpty || RegExp(r'^\d+$').hasMatch(target.parentId);
    final validPath =
        target.path.startsWith('/') &&
        target.path.length >= 2 &&
        !target.path.endsWith('/') &&
        target.path.split('/').last == target.name &&
        !target.path
            .split('/')
            .skip(1)
            .any((part) => part.isEmpty || part == '.' || part == '..');
    if (!isRoot &&
        (!validId ||
            target.id == '0' ||
            !validParent ||
            target.parentId.isEmpty ||
            target.name.trim().isEmpty ||
            !validPath)) {
      throw const FormatException('115 BT 目标目录信息无效');
    }
  }

  static String _key(
    String accountId,
    String infoHash,
    String targetFolderId,
  ) => jsonEncode([accountId, infoHash.toLowerCase(), targetFolderId]);
}

class Cloud115BtBatchSelectionPreferences {
  const Cloud115BtBatchSelectionPreferences({
    this.sourceId,
    this.target,
    this.mode = Cloud115BtDeliveryMode.cloud115,
  });

  final String? sourceId;
  final cloud115.Cloud115BtFolderSelection? target;
  final Cloud115BtDeliveryMode mode;
}

String _deliveryModeToJson(Cloud115BtDeliveryMode mode) => switch (mode) {
  Cloud115BtDeliveryMode.cloud115 => 'cloud115',
  Cloud115BtDeliveryMode.localTorrentOnly => 'local_torrent_only',
};

Cloud115BtDeliveryMode _deliveryModeFromJson(Object? value) => switch (value) {
  null || 'cloud115' => Cloud115BtDeliveryMode.cloud115,
  'local_torrent_only' => Cloud115BtDeliveryMode.localTorrentOnly,
  _ => throw const FormatException('115 BT 下载方式设置无效'),
};

class Cloud115BtBatchLedgerSnapshot {
  const Cloud115BtBatchLedgerSnapshot({
    required this.records,
    required this.selectionPreferences,
  });

  final List<Cloud115BtBatchLedgerRecord> records;
  final Cloud115BtBatchSelectionPreferences selectionPreferences;
}

class Cloud115BtBatchLedgerRecord {
  const Cloud115BtBatchLedgerRecord({
    required this.accountId,
    required this.infoHash,
    required this.targetFolderId,
    required this.relativeFile,
    required this.state,
    required this.updatedAtUtc,
    this.sourceId,
    this.batchId,
  });

  final String accountId;
  final String infoHash;
  final String targetFolderId;
  final String relativeFile;
  final String state;
  final String updatedAtUtc;
  final String? sourceId;
  final String? batchId;
}

class Cloud115BtBatchItemResult {
  const Cloud115BtBatchItemResult({
    required this.relativeFile,
    required this.infoHash,
    required this.state,
    this.message = '',
  });

  final String relativeFile;
  final String infoHash;
  final String state;
  final String message;
}

class Cloud115BtBatchReport {
  const Cloud115BtBatchReport({
    required this.results,
    required this.stopped,
    this.error,
  });

  final List<Cloud115BtBatchItemResult> results;
  final bool stopped;
  final String? error;

  bool get hasPendingRecovery => results.any(
    (result) => const {
      'upload_outcome_unknown',
      'upload_parse_pending',
      'unknown_remote_state',
      'duplicate_lookup_unknown',
      'duplicate_found_delete_failed',
    }.contains(result.state),
  );
}

/// Runs a user-started, sequential batch. Every write intent is flushed to the
/// private ledger before the corresponding network write is allowed.
class Cloud115BtBatchProcessor {
  Cloud115BtBatchProcessor({
    required this.ledger,
    LongTaskRegistry? taskRegistry,
  }) : _taskRegistry = taskRegistry ?? LongTaskRegistry.instance;

  final Cloud115BtBatchLedger ledger;
  final LongTaskRegistry _taskRegistry;

  Future<Cloud115BtBatchReport> run({
    required List<EhCheckedManifestEntry> entries,
    required cloud115.Cloud115BtFolderSelection target,
    String? sourceId,
    required Cloud115BtBatchPreflight preflight,
    required Cloud115BtBatchUpload upload,
    required Cloud115BtBatchUploadRecovery recoverUpload,
    required Cloud115BtBatchSubmit submit,
    required Cloud115BtBatchTaskQuery queryTask,
    required Cloud115BtBatchTorrentDelete deleteTorrent,
    bool? cleanupAfterSubmit,
    Cloud115BtBatchProgress? onProgress,
  }) => _taskRegistry.run(
    kind: 'cloud115_bt_batch',
    label: '115 批量 BT 任务',
    phase: '准备批次账本',
    // The ledger makes retry/reconciliation safe, but the batch is not
    // automatically restarted on launch; don't present it as auto-resumable.
    resumable: false,
    action: (lease) => _run(
      entries: entries,
      target: target,
      sourceId: sourceId,
      preflight: preflight,
      upload: upload,
      recoverUpload: recoverUpload,
      submit: submit,
      queryTask: queryTask,
      deleteTorrent: deleteTorrent,
      cleanupAfterSubmit: cleanupAfterSubmit,
      onProgress: (index, total, file, stage) {
        lease.update(phase: _safeExitPhase(stage));
        onProgress?.call(index, total, file, stage);
      },
    ),
  );

  Future<Cloud115BtBatchReport> _run({
    required List<EhCheckedManifestEntry> entries,
    required cloud115.Cloud115BtFolderSelection target,
    String? sourceId,
    required Cloud115BtBatchPreflight preflight,
    required Cloud115BtBatchUpload upload,
    required Cloud115BtBatchUploadRecovery recoverUpload,
    required Cloud115BtBatchSubmit submit,
    required Cloud115BtBatchTaskQuery queryTask,
    required Cloud115BtBatchTorrentDelete deleteTorrent,
    bool? cleanupAfterSubmit,
    Cloud115BtBatchProgress? onProgress,
  }) async {
    final batchId = 'bt-${DateTime.now().toUtc().microsecondsSinceEpoch}';
    try {
      return await ledger.withExclusiveLock((transaction) async {
        await transaction.beginBatchContext(
          sourceId: sourceId ?? transaction.selectionPreferences.sourceId,
          batchId: batchId,
        );
        if (cleanupAfterSubmit != null &&
            cleanupAfterSubmit != transaction.cleanupAfterSubmit) {
          await transaction.setCleanupAfterSubmit(cleanupAfterSubmit);
        }
        final results = <Cloud115BtBatchItemResult>[];
        String? batchAccount;

        Cloud115BtBatchReport stop(
          String file,
          String hash,
          String state,
          String message,
        ) => Cloud115BtBatchReport(
          results: [
            ...results,
            Cloud115BtBatchItemResult(
              relativeFile: file,
              infoHash: hash,
              state: state,
              message: message,
            ),
          ],
          stopped: true,
          error: message,
        );

        for (var entryIndex = 0; entryIndex < entries.length; entryIndex++) {
          final entry = entries[entryIndex];
          final item = entry.item;
          onProgress?.call(
            entryIndex + 1,
            entries.length,
            item.file,
            'preflight',
          );
          if (entry.fileStatus != EhSavedFileStatus.present ||
              entry.validationStatus != EhTorrentValidationStatus.valid ||
              !_isSafeRelativeTorrent(item.file) ||
              !RegExp(r'^[0-9a-fA-F]{40}$').hasMatch(item.infohash)) {
            results.add(
              Cloud115BtBatchItemResult(
                relativeFile: item.file,
                infoHash: item.infohash,
                state: 'local_validation_failed',
                message: '本地 Torrent 未通过 H0 校验，已跳过。',
              ),
            );
            continue;
          }

          late cloud115.Cloud115BtPreflight checked;
          try {
            checked = await preflight(entry);
            _verifyPreflight(checked, item.infohash, target);
          } catch (error) {
            return stop(
              item.file,
              item.infohash,
              'preflight_failed',
              _safeMessage(error),
            );
          }
          if (checked.accountId.trim().isEmpty) {
            return stop(
              item.file,
              item.infohash,
              'preflight_failed',
              '115 账号标识为空，已停止批次。',
            );
          }
          if (batchAccount != null && batchAccount != checked.accountId) {
            return stop(
              item.file,
              item.infohash,
              'account_changed',
              '批次期间 115 账号标识发生变化，已停止。',
            );
          }
          batchAccount ??= checked.accountId;

          final existing = transaction.recordFor(
            accountId: checked.accountId,
            infoHash: item.infohash,
            targetFolderId: target.id,
          );

          if (existing?.state == 'submitted') {
            var localRemoved = false;
            if (transaction.cleanupAfterSubmit) {
              localRemoved = await _deleteAfterRemoteProof(
                transaction: transaction,
                entry: entry,
                target: target,
                accountId: checked.accountId,
                deleteTorrent: deleteTorrent,
              );
            }
            results.add(
              Cloud115BtBatchItemResult(
                relativeFile: item.file,
                infoHash: item.infohash,
                state: localRemoved ? 'local_torrent_removed' : 'submitted',
                message: localRemoved
                    ? '账本已记录提交；只读核实远端同哈希任务后删除了本地 Torrent。'
                    : transaction.cleanupAfterSubmit
                    ? '账本已记录提交；远端任务尚未得到正向确认或本地删除失败，保留种子并继续核对。'
                    : '账本已记录该账号、Infohash 和目标目录的提交请求；不会重复提交。',
              ),
            );
            continue;
          }

          if (existing?.state == 'local_torrent_removed') {
            results.add(
              Cloud115BtBatchItemResult(
                relativeFile: item.file,
                infoHash: item.infohash,
                state: 'local_torrent_removed',
                message: '账本已记录该 Torrent 已处理并删除；不会重新提交。',
              ),
            );
            continue;
          }

          if (existing?.state == 'waf_blocked' ||
              existing?.state == 'rate_limited' ||
              existing?.state == 'auth_expired' ||
              existing?.state == 'access_denied' ||
              existing?.state == 'challenge_required') {
            return stop(
              item.file,
              item.infohash,
              existing!.state,
              '账本记录该账号或服务存在限流、风控、鉴权或访问拒绝；本批次停止写入。',
            );
          }

          if (existing?.state == 'rejected') {
            results.add(
              Cloud115BtBatchItemResult(
                relativeFile: item.file,
                infoHash: item.infohash,
                state: 'rejected',
                message: '115 曾明确拒绝本项；不重复提交，本轮继续处理其他种子。',
              ),
            );
            continue;
          }

          if (existing?.state == 'submit_started' ||
              existing?.state == 'unknown_remote_state' ||
              existing?.state == 'duplicate_lookup_unknown' ||
              existing?.state == 'duplicate_found' ||
              existing?.state == 'duplicate_found_delete_failed') {
            cloud115.Cloud115BtTaskStatus task;
            onProgress?.call(
              entryIndex + 1,
              entries.length,
              item.file,
              'reconciling',
            );
            try {
              task = await queryTask(item.infohash);
              _verifyTask(task, item.infohash, target);
            } catch (error) {
              await transaction.recordItem(
                accountId: checked.accountId,
                infoHash: item.infohash,
                targetFolderId: target.id,
                relativeFile: item.file,
                state: 'unknown_remote_state',
              );
              results.add(
                Cloud115BtBatchItemResult(
                  relativeFile: item.file,
                  infoHash: item.infohash,
                  state: 'unknown_remote_state',
                  message: '本项提交仍待只读核对；不会重发。本轮继续处理其他种子。${_safeMessage(error)}',
                ),
              );
              continue;
            }
            final blockedState = _taskStopState(task.state);
            if (blockedState != null) {
              await transaction.recordItem(
                accountId: checked.accountId,
                infoHash: item.infohash,
                targetFolderId: target.id,
                relativeFile: item.file,
                state: blockedState,
              );
              return stop(
                item.file,
                item.infohash,
                blockedState,
                task.statusText.trim().isEmpty
                    ? '115 查询返回账号级访问限制；本批次停止写入。'
                    : '${task.statusText.trim()} 本批次停止写入。',
              );
            }
            if (task.state == 'found') {
              if (!transaction.cleanupAfterSubmit) {
                await transaction.recordItem(
                  accountId: checked.accountId,
                  infoHash: item.infohash,
                  targetFolderId: target.id,
                  relativeFile: item.file,
                  state: 'duplicate_found',
                );
                results.add(
                  Cloud115BtBatchItemResult(
                    relativeFile: item.file,
                    infoHash: item.infohash,
                    state: 'duplicate_found',
                    message: '只读查询确认远端同哈希任务存在；按设置保留本地 Torrent，不再上传或提交。',
                  ),
                );
                continue;
              }
              final deleted = await _deleteAfterRemoteProof(
                transaction: transaction,
                entry: entry,
                target: target,
                accountId: checked.accountId,
                deleteTorrent: deleteTorrent,
              );
              if (!deleted) {
                await transaction.recordItem(
                  accountId: checked.accountId,
                  infoHash: item.infohash,
                  targetFolderId: target.id,
                  relativeFile: item.file,
                  state: 'duplicate_found_delete_failed',
                );
                results.add(
                  Cloud115BtBatchItemResult(
                    relativeFile: item.file,
                    infoHash: item.infohash,
                    state: 'duplicate_found_delete_failed',
                    message: '只读查询已确认远端任务，但本地种子删除失败。',
                  ),
                );
                continue;
              }
              results.add(
                Cloud115BtBatchItemResult(
                  relativeFile: item.file,
                  infoHash: item.infohash,
                  state: 'local_torrent_removed',
                  message: '恢复查询确认远端任务已存在；没有再次上传或提交。',
                ),
              );
              continue;
            }
            await transaction.recordItem(
              accountId: checked.accountId,
              infoHash: item.infohash,
              targetFolderId: target.id,
              relativeFile: item.file,
              state: 'unknown_remote_state',
            );
            results.add(
              Cloud115BtBatchItemResult(
                relativeFile: item.file,
                infoHash: item.infohash,
                state: 'unknown_remote_state',
                message:
                    task.state == 'unknown' && task.statusText.trim().isNotEmpty
                    ? '本项提交结果不确定；${task.statusText.trim()}。禁止重发，本轮继续处理其他种子。'
                    : '本项远端任务暂未确认；禁止重发，本轮继续处理其他种子。',
              ),
            );
            continue;
          }

          cloud115.Cloud115BtUploadParseResult parsed;
          if (existing?.state == 'upload_started' ||
              existing?.state == 'upload_outcome_unknown' ||
              existing?.state == 'upload_parse_pending' ||
              existing?.state == 'torrent_parsed') {
            onProgress?.call(
              entryIndex + 1,
              entries.length,
              item.file,
              'recovering_upload',
            );
            try {
              parsed = await recoverUpload(entry, null);
            } catch (error) {
              final parseWasPending = existing?.state == 'upload_parse_pending';
              final state = parseWasPending
                  ? 'upload_parse_pending'
                  : 'upload_outcome_unknown';
              await transaction.recordItem(
                accountId: checked.accountId,
                infoHash: item.infohash,
                targetFolderId: target.id,
                relativeFile: item.file,
                state: state,
              );
              results.add(
                Cloud115BtBatchItemResult(
                  relativeFile: item.file,
                  infoHash: item.infohash,
                  state: state,
                  message: parseWasPending
                      ? '本项已确认上传；只读解析恢复失败，不会重传。本轮继续处理其他种子。${_safeMessage(error)}'
                      : '本项上传结果未确认；只读恢复失败，禁止重传。本轮继续处理其他种子。${_safeMessage(error)}',
                ),
              );
              continue;
            }
          } else {
            onProgress?.call(
              entryIndex + 1,
              entries.length,
              item.file,
              'uploading',
            );
            await transaction.recordItem(
              accountId: checked.accountId,
              infoHash: item.infohash,
              targetFolderId: target.id,
              relativeFile: item.file,
              state: 'upload_started',
            );
            try {
              parsed = await upload(entry);
            } catch (error) {
              await transaction.recordItem(
                accountId: checked.accountId,
                infoHash: item.infohash,
                targetFolderId: target.id,
                relativeFile: item.file,
                state: 'upload_outcome_unknown',
              );
              results.add(
                Cloud115BtBatchItemResult(
                  relativeFile: item.file,
                  infoHash: item.infohash,
                  state: 'upload_outcome_unknown',
                  message: '本项上传结果未确认；禁止重传。本轮继续处理其他种子。${_safeMessage(error)}',
                ),
              );
              continue;
            }
          }

          if (parsed.state == 'UPLOADED_PARSE_PENDING') {
            await transaction.recordItem(
              accountId: checked.accountId,
              infoHash: item.infohash,
              targetFolderId: target.id,
              relativeFile: item.file,
              state: 'upload_parse_pending',
            );
            results.add(
              Cloud115BtBatchItemResult(
                relativeFile: item.file,
                infoHash: item.infohash,
                state: 'upload_parse_pending',
                message: parsed.message.trim().isEmpty
                    ? 'Torrent 已上传，115 解析待恢复；为支持恢复保留本地种子，不会重传。本轮继续处理其他种子。'
                    : '${_safeMessage(parsed.message)} 本轮继续处理其他种子；本地种子保留用于恢复。',
              ),
            );
            continue;
          }
          if (parsed.state != 'PARSED') {
            await transaction.recordItem(
              accountId: checked.accountId,
              infoHash: item.infohash,
              targetFolderId: target.id,
              relativeFile: item.file,
              state: 'upload_outcome_unknown',
            );
            if (parsed.state == 'WAF_BLOCKED' ||
                parsed.state == 'RATE_LIMITED' ||
                parsed.state == 'AUTH_EXPIRED' ||
                parsed.state == 'ACCESS_DENIED' ||
                parsed.state == 'CHALLENGE_REQUIRED') {
              final state = switch (parsed.state) {
                'WAF_BLOCKED' => 'waf_blocked',
                'RATE_LIMITED' => 'rate_limited',
                'AUTH_EXPIRED' => 'auth_expired',
                'ACCESS_DENIED' => 'access_denied',
                _ => 'challenge_required',
              };
              await transaction.recordItem(
                accountId: checked.accountId,
                infoHash: item.infohash,
                targetFolderId: target.id,
                relativeFile: item.file,
                state: state,
              );
              return stop(item.file, item.infohash, state, parsed.message);
            }
            results.add(
              Cloud115BtBatchItemResult(
                relativeFile: item.file,
                infoHash: item.infohash,
                state: 'upload_outcome_unknown',
                message: parsed.message.trim().isEmpty
                    ? '115 未确认本项 Torrent 上传与解析结果；禁止重传。本轮继续处理其他种子。'
                    : '${_safeMessage(parsed.message)} 本轮继续处理其他种子。',
              ),
            );
            continue;
          }

          final mismatches = <String>[];
          if (parsed.infoHash.toLowerCase() != item.infohash.toLowerCase()) {
            mismatches.add('Infohash');
          }
          if (parsed.targetFolderId != target.id) {
            mismatches.add('目标目录 ID');
          }
          if (parsed.targetFolderPath != target.path) {
            mismatches.add('目标目录路径');
          }
          if (parsed.accountId != checked.accountId) {
            mismatches.add('账号标识');
          }
          if (mismatches.isNotEmpty) {
            await transaction.recordItem(
              accountId: checked.accountId,
              infoHash: item.infohash,
              targetFolderId: target.id,
              relativeFile: item.file,
              state: 'upload_outcome_unknown',
            );
            return stop(
              item.file,
              item.infohash,
              'upload_outcome_unknown',
              '上传响应的${mismatches.join('、')}与当前任务不一致；保留本地种子，只读恢复前不会重传。',
            );
          }
          await transaction.recordItem(
            accountId: checked.accountId,
            infoHash: item.infohash,
            targetFolderId: target.id,
            relativeFile: item.file,
            state: 'torrent_parsed',
          );

          final wanted = parsed.files.map((file) => file.index).toSet().toList()
            ..sort();
          if (wanted.isEmpty || wanted.any((index) => index < 0)) {
            results.add(
              Cloud115BtBatchItemResult(
                relativeFile: item.file,
                infoHash: item.infohash,
                state: 'torrent_invalid',
                message: '115 解析没有返回可提交的文件索引；已跳过本项并继续。',
              ),
            );
            continue;
          }
          await transaction.recordItem(
            accountId: checked.accountId,
            infoHash: item.infohash,
            targetFolderId: target.id,
            relativeFile: item.file,
            state: 'submit_started',
          );
          onProgress?.call(
            entryIndex + 1,
            entries.length,
            item.file,
            'submitting',
          );

          late cloud115.Cloud115BtSubmitResult submitted;
          try {
            submitted = await submit(entry, wanted);
          } catch (error) {
            await transaction.recordItem(
              accountId: checked.accountId,
              infoHash: item.infohash,
              targetFolderId: target.id,
              relativeFile: item.file,
              state: 'unknown_remote_state',
            );
            results.add(
              Cloud115BtBatchItemResult(
                relativeFile: item.file,
                infoHash: item.infohash,
                state: 'unknown_remote_state',
                message: '本项 BT 提交结果未确认；禁止重发。本轮继续处理其他种子。${_safeMessage(error)}',
              ),
            );
            continue;
          }
          if (submitted.infoHash.toLowerCase() != item.infohash.toLowerCase() ||
              submitted.targetFolderId != target.id) {
            await transaction.recordItem(
              accountId: checked.accountId,
              infoHash: item.infohash,
              targetFolderId: target.id,
              relativeFile: item.file,
              state: 'unknown_remote_state',
            );
            return stop(
              item.file,
              item.infohash,
              'unknown_remote_state',
              'BT 提交响应与当前 Infohash 或目标目录不一致；禁止重发。',
            );
          }

          if (submitted.state == 'ACCEPTED' ||
              submitted.state == 'REMOTE_TASK_FOUND') {
            await transaction.recordItem(
              accountId: checked.accountId,
              infoHash: item.infohash,
              targetFolderId: target.id,
              relativeFile: item.file,
              state: 'submitted',
            );
            if (transaction.cleanupAfterSubmit) {
              final deleted = await _deleteAfterRemoteProof(
                transaction: transaction,
                entry: entry,
                target: target,
                accountId: checked.accountId,
                deleteTorrent: deleteTorrent,
              );
              results.add(
                Cloud115BtBatchItemResult(
                  relativeFile: item.file,
                  infoHash: item.infohash,
                  state: deleted ? 'local_torrent_removed' : 'submitted',
                  message: deleted
                      ? '远端同哈希任务已核实；按设置删除了本地 Torrent。'
                      : '尚未能只读确认远端任务或本地删除失败；已保留 Torrent。',
                ),
              );
            } else {
              results.add(
                Cloud115BtBatchItemResult(
                  relativeFile: item.file,
                  infoHash: item.infohash,
                  state: 'submitted',
                  message: submitted.state == 'REMOTE_TASK_FOUND'
                      ? '已只读核实到远端同 Infohash 任务；本地 Torrent 已保留。'
                      : '115 已接受 BT 提交请求；云端下载状态尚未确认，本地 Torrent 已保留。',
                ),
              );
            }
            continue;
          }
          if (submitted.state == 'ALREADY_PRESENT') {
            await transaction.recordItem(
              accountId: checked.accountId,
              infoHash: item.infohash,
              targetFolderId: target.id,
              relativeFile: item.file,
              state: 'duplicate_found',
            );
            final deleted = transaction.cleanupAfterSubmit
                ? await _deleteAfterRemoteProof(
                    transaction: transaction,
                    entry: entry,
                    target: target,
                    accountId: checked.accountId,
                    deleteTorrent: deleteTorrent,
                  )
                : false;
            if (transaction.cleanupAfterSubmit && !deleted) {
              await transaction.recordItem(
                accountId: checked.accountId,
                infoHash: item.infohash,
                targetFolderId: target.id,
                relativeFile: item.file,
                state: 'duplicate_found_delete_failed',
              );
            }
            results.add(
              Cloud115BtBatchItemResult(
                relativeFile: item.file,
                infoHash: item.infohash,
                state: deleted
                    ? 'local_torrent_removed'
                    : transaction.cleanupAfterSubmit
                    ? 'duplicate_found_delete_failed'
                    : 'duplicate_found',
                message: deleted
                    ? '115 报告同哈希任务已存在，并只读核实后删除了本地 Torrent。'
                    : transaction.cleanupAfterSubmit
                    ? '115 报告同哈希任务已存在；本地删除未完成，保留种子并可继续核对。'
                    : '115 报告同哈希任务已存在；按设置保留本地 Torrent，不再提交。',
              ),
            );
            continue;
          }

          if (submitted.state == 'REJECTED') {
            cloud115.Cloud115BtTaskStatus task;
            try {
              task = await queryTask(item.infohash);
              _verifyTask(task, item.infohash, target);
            } catch (error) {
              await transaction.recordItem(
                accountId: checked.accountId,
                infoHash: item.infohash,
                targetFolderId: target.id,
                relativeFile: item.file,
                state: 'duplicate_lookup_unknown',
              );
              results.add(
                Cloud115BtBatchItemResult(
                  relativeFile: item.file,
                  infoHash: item.infohash,
                  state: 'duplicate_lookup_unknown',
                  message:
                      '115 拒绝了提交；只读查重暂时无法确认，保留本地种子、不重试并继续下一项。${_safeMessage(error)}',
                ),
              );
              continue;
            }
            final blockedState = _taskStopState(task.state);
            if (blockedState != null) {
              await transaction.recordItem(
                accountId: checked.accountId,
                infoHash: item.infohash,
                targetFolderId: target.id,
                relativeFile: item.file,
                state: blockedState,
              );
              return stop(
                item.file,
                item.infohash,
                blockedState,
                task.statusText.trim().isEmpty
                    ? '115 查询返回账号级访问限制；本批次停止写入。'
                    : '${task.statusText.trim()} 本批次停止写入。',
              );
            }
            if (task.state == 'found') {
              await transaction.recordItem(
                accountId: checked.accountId,
                infoHash: item.infohash,
                targetFolderId: target.id,
                relativeFile: item.file,
                state: 'duplicate_found',
              );
              final deleted = transaction.cleanupAfterSubmit
                  ? await _deleteAfterRemoteProof(
                      transaction: transaction,
                      entry: entry,
                      target: target,
                      accountId: checked.accountId,
                      deleteTorrent: deleteTorrent,
                    )
                  : false;
              final state = deleted
                  ? 'local_torrent_removed'
                  : transaction.cleanupAfterSubmit
                  ? 'duplicate_found_delete_failed'
                  : 'duplicate_found';
              if (state == 'duplicate_found_delete_failed') {
                await transaction.recordItem(
                  accountId: checked.accountId,
                  infoHash: item.infohash,
                  targetFolderId: target.id,
                  relativeFile: item.file,
                  state: state,
                );
              }
              results.add(
                Cloud115BtBatchItemResult(
                  relativeFile: item.file,
                  infoHash: item.infohash,
                  state: state,
                  message: deleted
                      ? '提交被拒绝，但只读查询确认远端同哈希任务存在；已删除本地 Torrent。'
                      : transaction.cleanupAfterSubmit
                      ? '提交被拒绝，远端同哈希任务已确认；本地删除未完成，保留种子并继续下一项。'
                      : '提交被拒绝，远端同哈希任务已确认；按设置保留本地种子。',
                ),
              );
              continue;
            }
            if (task.state != 'not_found') {
              await transaction.recordItem(
                accountId: checked.accountId,
                infoHash: item.infohash,
                targetFolderId: target.id,
                relativeFile: item.file,
                state: 'duplicate_lookup_unknown',
              );
              results.add(
                Cloud115BtBatchItemResult(
                  relativeFile: item.file,
                  infoHash: item.infohash,
                  state: 'duplicate_lookup_unknown',
                  message: task.statusText.trim().isEmpty
                      ? '115 拒绝了提交；远端同哈希状态未知，保留本地种子、不重试并继续下一项。'
                      : '115 拒绝了提交；${task.statusText.trim()}。保留本地种子、不重试并继续下一项。',
                ),
              );
              continue;
            }
          }

          final state = switch (submitted.state) {
            'WAF_BLOCKED' => 'waf_blocked',
            'RATE_LIMITED' => 'rate_limited',
            'AUTH_EXPIRED' => 'auth_expired',
            'ACCESS_DENIED' => 'access_denied',
            'CHALLENGE_REQUIRED' => 'challenge_required',
            'REJECTED' => 'rejected',
            _ => 'unknown_remote_state',
          };
          await transaction.recordItem(
            accountId: checked.accountId,
            infoHash: item.infohash,
            targetFolderId: target.id,
            relativeFile: item.file,
            state: state,
          );
          if (const {
            'waf_blocked',
            'rate_limited',
            'auth_expired',
            'access_denied',
            'challenge_required',
          }.contains(state)) {
            return stop(item.file, item.infohash, state, submitted.message);
          }
          results.add(
            Cloud115BtBatchItemResult(
              relativeFile: item.file,
              infoHash: item.infohash,
              state: state,
              message: state == 'rejected'
                  ? '${submitted.message} 已跳过本项并继续处理其他种子。'
                  : '${submitted.message} 本项不会重发；本轮继续处理其他种子。',
            ),
          );
          continue;
        }
        onProgress?.call(entries.length, entries.length, '', 'completed');
        return Cloud115BtBatchReport(results: results, stopped: false);
      });
    } catch (error) {
      return Cloud115BtBatchReport(
        results: const [],
        stopped: true,
        error: '批次账本不可用，已失败关闭；没有继续写入。${_safeMessage(error)}',
      );
    }
  }

  static String _safeExitPhase(String stage) => switch (stage) {
    'preflight' => '只读核对账号与远端任务',
    'recovering_upload' => '恢复已有上传结果',
    'uploading' => '上传 Torrent',
    'submitting' => '提交 BT 任务',
    'completed' => '批次完成',
    _ => '逐项处理 Torrent',
  };

  Future<bool> _deleteAfterRemoteProof({
    required Cloud115BtBatchLedgerTransaction transaction,
    required EhCheckedManifestEntry entry,
    required cloud115.Cloud115BtFolderSelection target,
    required String accountId,
    required Cloud115BtBatchTorrentDelete deleteTorrent,
  }) async {
    try {
      await deleteTorrent(entry, accountId, target);
    } catch (_) {
      return false;
    }
    await transaction.recordItem(
      accountId: accountId,
      infoHash: entry.item.infohash,
      targetFolderId: target.id,
      relativeFile: entry.item.file,
      state: 'local_torrent_removed',
    );
    return true;
  }

  static void _verifyPreflight(
    cloud115.Cloud115BtPreflight result,
    String expectedHash,
    cloud115.Cloud115BtFolderSelection target,
  ) {
    if (result.infoHash.toLowerCase() != expectedHash.toLowerCase() ||
        result.targetFolderId != target.id ||
        result.targetFolderPath != target.path) {
      throw StateError('115 预检结果与当前 Torrent 或目标目录不一致');
    }
  }

  static void _verifyTask(
    cloud115.Cloud115BtTaskStatus result,
    String expectedHash,
    cloud115.Cloud115BtFolderSelection target,
  ) {
    if (result.infoHash.toLowerCase() != expectedHash.toLowerCase() ||
        result.targetFolderId != target.id) {
      throw StateError('115 任务查询结果与当前 Torrent 或目标目录不一致');
    }
  }

  static String? _taskStopState(String state) => switch (state) {
    'WAF_BLOCKED' => 'waf_blocked',
    'RATE_LIMITED' => 'rate_limited',
    'AUTH_EXPIRED' => 'auth_expired',
    'ACCESS_DENIED' => 'access_denied',
    'CHALLENGE_REQUIRED' => 'challenge_required',
    _ => null,
  };

  static String _safeMessage(Object error) {
    final text = error.toString().replaceAll(RegExp(r'\s+'), ' ').trim();
    if (text.isEmpty) return '未知错误';
    return text.length <= 240 ? text : '${text.substring(0, 240)}…';
  }
}
