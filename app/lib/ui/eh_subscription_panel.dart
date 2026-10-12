import 'dart:async';
import 'dart:io';

import 'package:app/store/eh_daily_coordinator.dart';
import 'package:app/store/eh_subscription_store.dart';
import 'package:app/store/cloud115_session.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/src/rust/source/cloud115/offline.dart' as cloud115_offline;
import 'package:app/ui/common.dart';
import 'package:file_selector/file_selector.dart';
import 'package:app/ui/rch_overlay.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

typedef EhHandoffProcessRunner =
    Future<ProcessResult> Function(String executable, List<String> arguments);

/// Opens only the configured output directory. Tests can inject the platform
/// and process boundary without starting Explorer.
Future<bool> openEhOutputDirectory(
  String directory, {
  bool? isWindows,
  EhHandoffProcessRunner? runProcess,
}) async {
  if (!(isWindows ?? Platform.isWindows) || directory.trim().isEmpty) {
    return false;
  }
  try {
    final runner =
        runProcess ??
        ((executable, arguments) => Process.run(executable, arguments));
    final result = await runner('explorer', [directory.replaceAll('/', r'\')]);
    return result.exitCode == 0;
  } catch (_) {
    return false;
  }
}

/// 设置页「EH 订阅」面板（可选插件）。
///
/// 边界：只写用户指定的保存目录（`.torrent` + `manifest.json`），
/// 不触碰书源、目录库与阅读数据；必须手动点击才会运行。
class EhSubscriptionPanel extends StatefulWidget {
  const EhSubscriptionPanel({
    super.key,
    this.storeOverride,
    this.btSourcesOverride,
    this.btPreflightRunner,
    this.btUploadRunner,
    this.btUploadRecoveryRunner,
    this.btSubmitRunner,
    this.btTaskStatusRunner,
  });

  /// 仅测试使用：注入预置 store（避免触碰 FFI / 磁盘）。
  final EhSubscriptionStore? storeOverride;
  @visibleForTesting
  final List<BookSource>? btSourcesOverride;
  @visibleForTesting
  final Eh115BtPreflightRunner? btPreflightRunner;
  @visibleForTesting
  final Eh115BtUploadRunner? btUploadRunner;
  @visibleForTesting
  final Eh115BtUploadRecoveryRunner? btUploadRecoveryRunner;
  @visibleForTesting
  final Eh115BtSubmitRunner? btSubmitRunner;
  @visibleForTesting
  final Eh115BtTaskStatusRunner? btTaskStatusRunner;

  @override
  State<EhSubscriptionPanel> createState() => _EhSubscriptionPanelState();
}

class _EhSubscriptionPanelState extends State<EhSubscriptionPanel> {
  final _rulesKey = GlobalKey<_RulesEditorState>();

  EhSubscriptionStore get store =>
      widget.storeOverride ?? EhSubscriptionStore.instance;

  @override
  void initState() {
    super.initState();
    if (widget.storeOverride != null) return; // 测试注入：不初始化
    final s = EhSubscriptionStore.instance;
    if (s.progress.stage == 'idle' && s.loadError == null) {
      // 首次挂载时初始化（读规则 + 读清单）
      WidgetsBinding.instance.addPostFrameCallback((_) async {
        await s.init();
        await EhDailyCoordinator.maybeInstance?.start();
      });
    }
  }

  Future<void> _pickDir() async {
    final store = EhSubscriptionStore.instance;
    final picked = await getDirectoryPath(
      initialDirectory: store.hasOutDir ? store.outDir : null,
      confirmButtonText: '选择此目录',
    );
    if (picked == null) return;
    _rulesKey.currentState?.setOutDir(picked);
    store.patch('out_dir', picked);
    await store.saveRules();
    await store.refreshManifest();
    await EhDailyCoordinator.maybeInstance?.settingsChanged();
  }

  Future<void> _run() async {
    final editor = _rulesKey.currentState;
    editor?.flush();
    final store = this.store;
    if (!store.hasOutDir) {
      _snack('请先选择保存目录', error: true);
      return;
    }
    await store.run();
    if (!mounted) return;
    final p = store.progress;
    _snack(
      p.error ?? (p.message.isEmpty ? '扫描结束' : p.message),
      error: p.error != null,
    );
  }

  void _snack(String msg, {bool error = false}) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(msg),
        backgroundColor: error ? Theme.of(context).colorScheme.error : null,
      ),
    );
  }

  Future<bool> _confirm(String title, String body) async {
    final ok = await showRchDialog<bool>(
      context: context,
      builder: (c) => AlertDialog(
        title: Text(title),
        content: Text(body),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(c).pop(false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(c).pop(true),
            child: const Text('确定'),
          ),
        ],
      ),
    );
    return ok ?? false;
  }

  Future<void> _openOutDir() async {
    final dir = store.outDir;
    if (dir.isEmpty) return;
    final opened = await openEhOutputDirectory(dir);
    if (!opened) {
      _snack(Platform.isWindows ? '无法打开保存目录' : '当前平台暂不支持打开保存目录', error: true);
    }
  }

  Future<void> _showManualSubmitInstructions() async {
    await showRchDialog<void>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('115 手动提交说明'),
        content: const SingleChildScrollView(
          child: Text(
            '1. 选择并核对本地已保存的 .torrent 文件。\n'
            '2. 仅使用你当前账号可用的 115 官方客户端或网页功能手动操作。\n'
            '3. 若官方界面没有 Torrent 文件提交入口，请停止，不要改用未公开接口。\n'
            '4. 本地 Infohash 和文件校验只证明本地种子状态，不证明已上传或已创建云下载任务。\n\n'
            'RCH 不会自动上传、提交任务、打开第三方 BT 客户端或验证云端结果。',
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('关闭'),
          ),
        ],
      ),
    );
  }

  Future<void> _copyTorrentPath(String path) async {
    await Clipboard.setData(ClipboardData(text: path));
    _snack('已复制 Torrent 完整路径');
  }

  @override
  Widget build(BuildContext context) {
    final store = this.store;
    final theme = Theme.of(context);
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        if (store.loadError != null) {
          return _Section(
            title: 'EH 订阅',
            subtitle: '加载失败：${store.loadError}',
            children: [
              FilledButton.icon(
                onPressed: () async {
                  await store.init();
                  await EhDailyCoordinator.maybeInstance?.start();
                },
                icon: const Icon(Icons.refresh),
                label: const Text('重试'),
              ),
            ],
          );
        }
        if (!store.rulesLoaded) {
          return const _Section(
            title: 'EH 订阅',
            subtitle: '正在加载规则…',
            children: [LinearProgressIndicator()],
          );
        }
        return _Section(
          title: 'EH 订阅',
          subtitle:
              '按规则筛选 E-Hentai 画廊种子并保存到指定文件夹。可选插件：只写该文件夹，'
              '不改动书源、目录与阅读数据；需手动点击运行。',
          trailing: _StatusPill(progress: store.progress),
          children: [
            Card(
              margin: EdgeInsets.zero,
              child: Padding(
                padding: const EdgeInsets.all(16),
                child: _RulesEditor(
                  key: _rulesKey,
                  store: store,
                  onPickDir: _pickDir,
                ),
              ),
            ),
            const SizedBox(height: 12),
            _DailyAutomationBlock(
              store: store,
              coordinator: EhDailyCoordinator.maybeInstance,
            ),
            const SizedBox(height: 12),
            _RunBar(store: store, onRun: _run, onOpenDir: _openOutDir),
            const SizedBox(height: 12),
            _ProgressBlock(progress: store.progress),
            const SizedBox(height: 16),
            _ManifestBlock(
              store: store,
              onOpenDir: _openOutDir,
              onRefresh: store.refreshManifest,
              onShowInstructions: _showManualSubmitInstructions,
              onCopyPath: _copyTorrentPath,
            ),
            const SizedBox(height: 12),
            _CookieBtPreflightBlock(
              store: store,
              sourcesOverride: widget.btSourcesOverride,
              preflightRunner: widget.btPreflightRunner,
              uploadRunner: widget.btUploadRunner,
              uploadRecoveryRunner: widget.btUploadRecoveryRunner,
              submitRunner: widget.btSubmitRunner,
              taskStatusRunner: widget.btTaskStatusRunner,
            ),
            const SizedBox(height: 8),
            Align(
              alignment: Alignment.centerLeft,
              child: TextButton.icon(
                onPressed: () async {
                  if (!await _confirm(
                    '恢复默认规则',
                    '将把搜索语法、评分、标记与分档阈值恢复为默认值（已保存的种子与清单不受影响）。',
                  )) {
                    return;
                  }
                  await store.resetToDefaults();
                  await EhDailyCoordinator.maybeInstance?.settingsChanged();
                  _rulesKey.currentState?.reloadFromStore();
                  _snack('已恢复默认规则');
                },
                icon: const Icon(Icons.restart_alt),
                label: const Text('恢复默认规则'),
              ),
            ),
            Align(
              alignment: Alignment.centerLeft,
              child: Text(
                r'标签语法示例：language:"chinese"$ · other:"full color"$ · uncensored',
                style: theme.textTheme.bodySmall,
              ),
            ),
          ],
        );
      },
    );
  }
}

typedef Eh115BtPreflightRunner =
    Future<cloud115_offline.Cloud115BtPreflight> Function(
      BookSource source,
      String outputDir,
      String relativeFile,
      String infoHash,
    );

typedef Eh115BtUploadRunner =
    Future<cloud115_offline.Cloud115BtUploadParseResult> Function(
      BookSource source,
      String outputDir,
      String relativeFile,
      String infoHash,
      String targetFolderId,
    );

typedef Eh115BtUploadRecoveryRunner =
    Future<cloud115_offline.Cloud115BtUploadParseResult> Function(
      BookSource source,
      String outputDir,
      String relativeFile,
      String infoHash,
      String targetFolderId,
      String? pickCode,
    );

typedef Eh115BtSubmitRunner =
    Future<cloud115_offline.Cloud115BtSubmitResult> Function(
      BookSource source,
      String outputDir,
      String relativeFile,
      String infoHash,
      String targetFolderId,
      List<int> wantedIndices,
    );

typedef Eh115BtTaskStatusRunner =
    Future<cloud115_offline.Cloud115BtTaskStatus> Function(
      BookSource source,
      String infoHash,
      String targetFolderId,
    );

/// Experimental one-item Cookie-mode preflight. The card is intentionally
/// inert until the user presses the read-only preflight button.
class _CookieBtPreflightBlock extends StatefulWidget {
  const _CookieBtPreflightBlock({
    required this.store,
    this.sourcesOverride,
    this.preflightRunner,
    this.uploadRunner,
    this.uploadRecoveryRunner,
    this.submitRunner,
    this.taskStatusRunner,
  });

  final EhSubscriptionStore store;
  @visibleForTesting
  final List<BookSource>? sourcesOverride;
  @visibleForTesting
  final Eh115BtPreflightRunner? preflightRunner;
  @visibleForTesting
  final Eh115BtUploadRunner? uploadRunner;
  @visibleForTesting
  final Eh115BtUploadRecoveryRunner? uploadRecoveryRunner;
  @visibleForTesting
  final Eh115BtSubmitRunner? submitRunner;
  @visibleForTesting
  final Eh115BtTaskStatusRunner? taskStatusRunner;

  @override
  State<_CookieBtPreflightBlock> createState() =>
      _CookieBtPreflightBlockState();
}

class _CookieBtPreflightBlockState extends State<_CookieBtPreflightBlock> {
  String? _selectedFile;
  String? _selectedSourceId;
  bool _busy = false;
  bool _confirmingWrite = false;
  String? _error;
  cloud115_offline.Cloud115BtPreflight? _result;
  cloud115_offline.Cloud115BtUploadParseResult? _uploadResult;
  cloud115_offline.Cloud115BtSubmitResult? _submitResult;
  cloud115_offline.Cloud115BtTaskStatus? _taskStatus;
  bool _uploadAttempted = false;
  bool _submitAttempted = false;
  bool _uploadOutcomeUnknown = false;
  bool _submitOutcomeUnknown = false;
  Set<int> _wanted = {};

  List<EhCheckedManifestEntry> get _eligibleEntries => widget
      .store
      .manifestReport
      .entries
      .where(
        (entry) =>
            entry.fileStatus == EhSavedFileStatus.present &&
            entry.validationStatus == EhTorrentValidationStatus.valid &&
            entry.item.file.isNotEmpty,
      )
      .toList();

  List<BookSource> get _cookieSources =>
      (widget.sourcesOverride ?? LibraryStore.instance.sources)
          .where(
            (source) =>
                source.is115 &&
                !source.remoteOnly &&
                (source.cookie ?? '').trim().isNotEmpty,
          )
          .toList();

  EhCheckedManifestEntry? _entryFor(List<EhCheckedManifestEntry> entries) {
    if (entries.isEmpty) return null;
    for (final entry in entries) {
      if (entry.item.file == _selectedFile) return entry;
    }
    return entries.first;
  }

  BookSource? _sourceFor(List<BookSource> sources) {
    if (sources.isEmpty) return null;
    for (final source in sources) {
      if (source.id == _selectedSourceId) return source;
    }
    return sources.first;
  }

  void _resetOperationForSelection() {
    _error = null;
    _result = null;
    _uploadResult = null;
    _submitResult = null;
    _taskStatus = null;
    _uploadAttempted = false;
    _submitAttempted = false;
    _uploadOutcomeUnknown = false;
    _submitOutcomeUnknown = false;
    _wanted = {};
  }

  Future<void> _preflight(
    BookSource source,
    EhCheckedManifestEntry entry,
  ) async {
    if (_busy || _confirmingWrite || _uploadAttempted || _submitAttempted) {
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
      _result = null;
      _uploadResult = null;
      _submitResult = null;
      _taskStatus = null;
      _wanted = {};
    });
    try {
      final runner = widget.preflightRunner;
      final result = runner == null
          ? await cloud115CookieBtReadonlyPreflightFor(
              source,
              outputDir: widget.store.outDir,
              relativeFile: entry.item.file,
              expectedInfoHash: entry.item.infohash,
            )
          : await runner(
              source,
              widget.store.outDir,
              entry.item.file,
              entry.item.infohash,
            );
      if (!mounted) return;
      if (result.infoHash.toLowerCase() != entry.item.infohash.toLowerCase() ||
          result.targetFolderPath != '/漫画/EH订阅' ||
          !RegExp(r'^\d+$').hasMatch(result.targetFolderId)) {
        throw StateError('115 预检结果与当前 Torrent 或目标目录不一致');
      }
      setState(() {
        _result = result;
        _wanted = result.files.map((file) => file.index).toSet();
      });
    } catch (error) {
      if (!mounted) return;
      setState(() => _error = _safeBtError(error));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<bool> _confirmWrite({
    required String title,
    required String message,
    required String confirmLabel,
  }) async {
    if (_busy || _confirmingWrite) return false;
    setState(() => _confirmingWrite = true);
    try {
      final confirmed = await showRchDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          title: Text(title),
          content: SingleChildScrollView(child: Text(message)),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(context).pop(false),
              child: const Text('取消'),
            ),
            FilledButton(
              onPressed: () => Navigator.of(context).pop(true),
              child: Text(confirmLabel),
            ),
          ],
        ),
      );
      return confirmed ?? false;
    } finally {
      if (mounted) setState(() => _confirmingWrite = false);
    }
  }

  Future<void> _upload(
    BookSource source,
    EhCheckedManifestEntry entry,
    cloud115_offline.Cloud115BtPreflight result,
  ) async {
    if (_busy ||
        _confirmingWrite ||
        _uploadAttempted ||
        result.duplicateStatus != 'not_found') {
      return;
    }
    final confirmed = await _confirmWrite(
      title: '确认上传这一个 Torrent',
      message:
          '账号：${result.accountId}\n'
          'Torrent：${result.torrentName}\n'
          '文件 SHA-1：${result.torrentSha1}\n'
          'BT Infohash：${result.infoHash}\n'
          '目标：${result.targetFolderPath}（ID ${result.targetFolderId}）\n\n'
          '此操作使用第三方实现的 115 Cookie Web 上传协议。115 官方授权和当前线上兼容性尚未由 RCH 验证。请只对你有权操作的账号及测试资源继续。请求结果不确定时，RCH 只做只读核对，不会自动重传。',
      confirmLabel: '确认上传',
    );
    if (!confirmed || !mounted) return;

    setState(() {
      _busy = true;
      _uploadAttempted = true;
      _uploadOutcomeUnknown = true;
      _error = null;
      _uploadResult = null;
      _taskStatus = null;
    });
    try {
      final runner = widget.uploadRunner;
      final uploaded = runner == null
          ? await cloud115CookieBtUploadAndParseFor(
              source,
              outputDir: widget.store.outDir,
              relativeFile: entry.item.file,
              expectedInfoHash: entry.item.infohash,
              expectedTargetFolderId: result.targetFolderId,
            )
          : await runner(
              source,
              widget.store.outDir,
              entry.item.file,
              entry.item.infohash,
              result.targetFolderId,
            );
      if (!mounted) return;
      if (uploaded.infoHash.toLowerCase() !=
              entry.item.infohash.toLowerCase() ||
          uploaded.targetFolderId != result.targetFolderId) {
        throw StateError('115 上传解析结果与当前 Torrent 或目标目录不一致');
      }
      setState(() {
        _uploadResult = uploaded;
        _uploadOutcomeUnknown =
            uploaded.state == 'UNKNOWN_UPLOAD_RESULT' ||
            uploaded.state == 'UPLOADED_PARSE_PENDING';
        if (uploaded.state == 'PARSED') {
          _wanted = uploaded.files.map((file) => file.index).toSet();
        }
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _uploadOutcomeUnknown = true;
        _error = _safeBtError(error);
      });
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _recoverUpload(
    BookSource source,
    EhCheckedManifestEntry entry,
    cloud115_offline.Cloud115BtPreflight preflight,
  ) async {
    if (_busy || _confirmingWrite || !_uploadAttempted) return;
    setState(() {
      _busy = true;
      _error = null;
      _taskStatus = null;
    });
    try {
      final runner = widget.uploadRecoveryRunner;
      final result = runner == null
          ? await cloud115CookieBtConfirmUploadParseFor(
              source,
              outputDir: widget.store.outDir,
              relativeFile: entry.item.file,
              expectedInfoHash: entry.item.infohash,
              expectedTargetFolderId: preflight.targetFolderId,
              pickCode: _uploadResult?.pickCode,
            )
          : await runner(
              source,
              widget.store.outDir,
              entry.item.file,
              entry.item.infohash,
              preflight.targetFolderId,
              _uploadResult?.pickCode,
            );
      if (!mounted) return;
      if (result.infoHash.toLowerCase() != entry.item.infohash.toLowerCase() ||
          result.targetFolderId != preflight.targetFolderId) {
        throw StateError('115 上传核对结果与当前 Torrent 或目标目录不一致');
      }
      setState(() {
        _uploadResult = result;
        _uploadOutcomeUnknown = result.state != 'PARSED';
        if (result.state == 'PARSED') {
          _wanted = result.files.map((file) => file.index).toSet();
        }
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _uploadOutcomeUnknown = true;
        _error = _safeBtError(error);
      });
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _submit(
    BookSource source,
    EhCheckedManifestEntry entry,
    cloud115_offline.Cloud115BtPreflight preflight,
  ) async {
    if (_busy ||
        _confirmingWrite ||
        _submitAttempted ||
        _uploadResult?.state != 'PARSED' ||
        _wanted.isEmpty) {
      return;
    }
    final indices = _wanted.toList()..sort();
    final confirmed = await _confirmWrite(
      title: '确认创建一个 BT 离线任务',
      message:
          '账号：${preflight.accountId}\n'
          'Infohash：${entry.item.infohash}\n'
          '下载目录：${preflight.targetFolderPath}（ID ${preflight.targetFolderId}）\n'
          'wanted 文件索引：${indices.join(', ')}\n\n'
          '将使用 SSP 加密后提交一次。提交超时或结果不确定时，RCH 会先只读查询远端任务，不会再次 POST。此 Cookie Web 写入路线的官方授权及当前线上兼容性尚未验证。',
      confirmLabel: '确认提交一次',
    );
    if (!confirmed || !mounted) return;

    setState(() {
      _busy = true;
      _submitAttempted = true;
      _submitOutcomeUnknown = true;
      _error = null;
      _taskStatus = null;
    });
    try {
      final runner = widget.submitRunner;
      final submitted = runner == null
          ? await cloud115CookieBtSubmitOnceFor(
              source,
              outputDir: widget.store.outDir,
              relativeFile: entry.item.file,
              expectedInfoHash: entry.item.infohash,
              expectedTargetFolderId: preflight.targetFolderId,
              wantedIndices: indices,
            )
          : await runner(
              source,
              widget.store.outDir,
              entry.item.file,
              entry.item.infohash,
              preflight.targetFolderId,
              indices,
            );
      if (!mounted) return;
      if (submitted.infoHash.toLowerCase() !=
              entry.item.infohash.toLowerCase() ||
          submitted.targetFolderId != preflight.targetFolderId) {
        throw StateError('115 提交结果与当前 Torrent 或目标目录不一致');
      }
      setState(() {
        _submitResult = submitted;
        _submitOutcomeUnknown = submitted.state == 'UNKNOWN_REMOTE_STATE';
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _submitOutcomeUnknown = true;
        _error = _safeBtError(error);
      });
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _queryTask(
    BookSource source,
    cloud115_offline.Cloud115BtPreflight preflight,
  ) async {
    if (_busy || _confirmingWrite) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final runner = widget.taskStatusRunner;
      final status = runner == null
          ? await cloud115CookieBtTaskStatusFor(
              source,
              infoHash: preflight.infoHash,
              expectedTargetFolderId: preflight.targetFolderId,
            )
          : await runner(source, preflight.infoHash, preflight.targetFolderId);
      if (!mounted) return;
      if (status.infoHash.toLowerCase() != preflight.infoHash.toLowerCase() ||
          status.targetFolderId != preflight.targetFolderId) {
        throw StateError('115 任务状态与当前 Torrent 或目标目录不一致');
      }
      setState(() {
        _taskStatus = status;
        if (status.state == 'found') _submitOutcomeUnknown = false;
      });
    } catch (error) {
      if (!mounted) return;
      setState(() => _error = _safeBtError(error));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final entries = _eligibleEntries;
    final sources = _cookieSources;
    final selectedEntry = _entryFor(entries);
    final selectedSource = _sourceFor(sources);
    final theme = Theme.of(context);
    final preflight = _result;
    final upload = _uploadResult;
    final uploadParsed = upload?.state == 'PARSED';
    final files = uploadParsed
        ? upload!.files
        : preflight?.files ?? const <cloud115_offline.Cloud115BtFile>[];
    final canUpload =
        !_busy &&
        !_confirmingWrite &&
        !_uploadAttempted &&
        preflight?.writeGate == 'REQUIRES_EXPLICIT_CONFIRMATION' &&
        preflight?.duplicateStatus == 'not_found' &&
        selectedEntry != null &&
        selectedSource != null;
    final canRecoverUpload =
        !_busy &&
        !_confirmingWrite &&
        _uploadAttempted &&
        preflight != null &&
        !uploadParsed &&
        selectedEntry != null &&
        selectedSource != null;
    final canSubmit =
        !_busy &&
        !_confirmingWrite &&
        !_submitAttempted &&
        uploadParsed &&
        preflight?.duplicateStatus == 'not_found' &&
        _taskStatus?.state != 'found' &&
        _wanted.isNotEmpty &&
        selectedEntry != null &&
        selectedSource != null;
    final busyLabel = _submitAttempted
        ? '正在提交或核对…'
        : _uploadAttempted
        ? '正在上传或核对…'
        : '正在只读核对…';
    return Card(
      margin: EdgeInsets.zero,
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('115 Cookie BT 实验入口', style: theme.textTheme.titleSmall),
            const SizedBox(height: 4),
            Text(
              '只处理 H0 已校验的单个 Torrent。初始化不会连接 115；只读预检、上传解析和 BT 提交都由你分别点击触发。',
              style: theme.textTheme.bodySmall,
            ),
            const SizedBox(height: 8),
            Text(
              '实验性 Cookie Web 写入：不使用 Open API，也不会新建登录、扫码或刷新 Cookie。第三方协议实现及当前 115 线上兼容性未获 RCH 实测确认；请先确认账号、目录和测试种子。上传与 BT 提交需要两次独立确认，结果不确定时只提供只读核对。',
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.error,
              ),
            ),
            const SizedBox(height: 12),
            DropdownButtonFormField<String>(
              key: ValueKey(
                'eh115-bt-file-${selectedEntry?.item.file ?? "none"}',
              ),
              initialValue: selectedEntry?.item.file,
              isExpanded: true,
              decoration: const InputDecoration(
                labelText: '已校验 Torrent',
                border: OutlineInputBorder(),
                isDense: true,
              ),
              items: [
                for (final entry in entries)
                  DropdownMenuItem(
                    value: entry.item.file,
                    child: Text(
                      '${entry.item.titleJpn.isEmpty ? entry.item.torrentName : entry.item.titleJpn} · ${entry.item.file}',
                      overflow: TextOverflow.ellipsis,
                    ),
                  ),
              ],
              onChanged:
                  entries.isEmpty ||
                      _busy ||
                      _confirmingWrite ||
                      _uploadAttempted ||
                      _submitAttempted
                  ? null
                  : (file) => setState(() {
                      _selectedFile = file;
                      _resetOperationForSelection();
                    }),
            ),
            const SizedBox(height: 10),
            DropdownButtonFormField<String>(
              key: ValueKey('eh115-bt-source-${selectedSource?.id ?? "none"}'),
              initialValue: selectedSource?.id,
              isExpanded: true,
              decoration: const InputDecoration(
                labelText: '已有的 115 Cookie 书源',
                border: OutlineInputBorder(),
                isDense: true,
              ),
              items: [
                for (final source in sources)
                  DropdownMenuItem(
                    value: source.id,
                    child: Text(source.name, overflow: TextOverflow.ellipsis),
                  ),
              ],
              onChanged:
                  sources.isEmpty ||
                      _busy ||
                      _confirmingWrite ||
                      _uploadAttempted ||
                      _submitAttempted
                  ? null
                  : (id) => setState(() {
                      _selectedSourceId = id;
                      _resetOperationForSelection();
                    }),
            ),
            const SizedBox(height: 10),
            Align(
              alignment: Alignment.centerLeft,
              child: FilledButton.icon(
                onPressed:
                    _busy ||
                        _confirmingWrite ||
                        _uploadAttempted ||
                        selectedEntry == null ||
                        selectedSource == null
                    ? null
                    : () => _preflight(selectedSource, selectedEntry),
                icon: _busy
                    ? const SizedBox(
                        width: 16,
                        height: 16,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Icon(Icons.fact_check_outlined),
                label: Text(_busy ? busyLabel : '检查账号、目录与重复任务'),
              ),
            ),
            if (entries.isEmpty)
              const Padding(
                padding: EdgeInsets.only(top: 8),
                child: Text('没有通过 H0 文件存在性和 Torrent 校验的种子。'),
              ),
            if (sources.isEmpty)
              const Padding(
                padding: EdgeInsets.only(top: 8),
                child: Text('没有可用的 115 Cookie 书源；不会改用 Open 模式。'),
              ),
            if (_error != null)
              Padding(
                padding: const EdgeInsets.only(top: 8),
                child: Text(
                  _error!,
                  style: TextStyle(color: theme.colorScheme.error),
                ),
              ),
            if (preflight case final result?) ...[
              const Divider(height: 24),
              _BtPreflightFact(label: '115 账号标识', value: result.accountId),
              _BtPreflightFact(
                label: '目标目录',
                value:
                    '${result.targetFolderPath}（ID ${result.targetFolderId}）',
              ),
              _BtPreflightFact(
                label: 'Torrent',
                value:
                    '${result.torrentName} · ${fmtSize(result.torrentSize)} · SHA-1 ${result.torrentSha1}',
              ),
              _BtPreflightFact(label: 'Infohash', value: result.infoHash),
              _BtPreflightFact(
                label: '同 Infohash 任务',
                value: switch (result.duplicateStatus) {
                  'found' => '已找到；不得重复提交',
                  'not_found' => '只读接口报告未找到',
                  _ => '未知；必须停止写入',
                },
              ),
              if (result.duplicateStatus == 'unknown')
                Padding(
                  padding: const EdgeInsets.only(top: 8),
                  child: Text(
                    '远端重复检查结果未知，已停止写入。请稍后只读查询任务。',
                    style: TextStyle(color: theme.colorScheme.error),
                  ),
                ),
              if (result.duplicateStatus == 'found')
                const Padding(
                  padding: EdgeInsets.only(top: 8),
                  child: Text('远端已存在同 Infohash 任务；本面板不会再次上传或提交。'),
                ),
              if (files.isNotEmpty) ...[
                const SizedBox(height: 10),
                Text(
                  uploadParsed ? '远端解析文件清单' : '本地校验文件清单',
                  style: theme.textTheme.titleSmall,
                ),
                const SizedBox(height: 4),
                Text('选择 wanted 文件（零基索引）', style: theme.textTheme.bodySmall),
              ],
              for (final file in files)
                CheckboxListTile(
                  dense: true,
                  contentPadding: EdgeInsets.zero,
                  value: _wanted.contains(file.index),
                  title: Text(
                    '${file.index} · ${file.path}',
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                  ),
                  subtitle: Text(fmtSize(file.size)),
                  onChanged:
                      _busy ||
                          _confirmingWrite ||
                          _submitAttempted ||
                          !uploadParsed
                      ? null
                      : (checked) => setState(() {
                          if (checked == true) {
                            _wanted.add(file.index);
                          } else {
                            _wanted.remove(file.index);
                          }
                        }),
                ),
              if (upload != null) ...[
                const Divider(height: 24),
                Text(
                  uploadParsed ? '远端 Torrent 已解析' : '上传状态：${upload.state}',
                  style: theme.textTheme.titleSmall,
                ),
                Padding(
                  padding: const EdgeInsets.only(top: 4),
                  child: Text(upload.message),
                ),
                if (uploadParsed)
                  _BtPreflightFact(
                    label: '上传结果',
                    value: upload.uploadReused
                        ? '目标目录中已有的 Torrent 已匹配并通过解析'
                        : 'Torrent 已上传并通过解析',
                  ),
                _BtPreflightFact(label: '远端 Infohash', value: upload.infoHash),
                _BtPreflightFact(
                  label: '远端 Torrent SHA-1',
                  value: upload.torrentSha1,
                ),
              ],
              if (_uploadOutcomeUnknown)
                Padding(
                  padding: const EdgeInsets.only(top: 8),
                  child: Text(
                    '上传结果尚未确认。请使用下面的只读核对；不会再次上传。',
                    style: TextStyle(color: theme.colorScheme.error),
                  ),
                ),
              if (_submitResult case final submitted?) ...[
                const Divider(height: 24),
                Text(
                  'BT 提交结果：${submitted.state}',
                  style: theme.textTheme.titleSmall,
                ),
                Padding(
                  padding: const EdgeInsets.only(top: 4),
                  child: Text(submitted.message),
                ),
              ],
              if (_submitOutcomeUnknown)
                Padding(
                  padding: const EdgeInsets.only(top: 8),
                  child: Text(
                    '提交结果尚未确认。请查询远端任务；本面板不会再次 POST。',
                    style: TextStyle(color: theme.colorScheme.error),
                  ),
                ),
              if (_taskStatus case final status?) ...[
                const Divider(height: 24),
                Text('远端 BT 任务', style: theme.textTheme.titleSmall),
                _BtPreflightFact(
                  label: '任务状态',
                  value: status.state == 'found'
                      ? status.statusText
                      : status.state == 'not_found'
                      ? '任务列表未找到'
                      : '无法确认',
                ),
                if (status.statusCode != null)
                  _BtPreflightFact(
                    label: '状态码',
                    value: status.statusCode.toString(),
                  ),
                if (status.percentDone != null)
                  _BtPreflightFact(
                    label: '远端进度',
                    value: '${(status.percentDone! * 100).toStringAsFixed(0)}%',
                  ),
                if (status.state == 'not_found' && _submitAttempted)
                  const Padding(
                    padding: EdgeInsets.only(top: 4),
                    child: Text('提交请求已经发出；列表暂未找到任务，禁止在本轮重复提交。'),
                  ),
              ],
              const SizedBox(height: 8),
              if (canUpload)
                FilledButton.icon(
                  onPressed: () =>
                      _upload(selectedSource, selectedEntry, result),
                  icon: const Icon(Icons.cloud_upload_outlined),
                  label: const Text('上传 Torrent 并解析'),
                ),
              if (canRecoverUpload)
                OutlinedButton.icon(
                  onPressed: () =>
                      _recoverUpload(selectedSource, selectedEntry, result),
                  icon: const Icon(Icons.fact_check_outlined),
                  label: const Text('核对 Torrent 上传结果'),
                ),
              if (uploadParsed)
                FilledButton.icon(
                  onPressed: canSubmit
                      ? () => _submit(selectedSource, selectedEntry, result)
                      : null,
                  icon: const Icon(Icons.playlist_add_check),
                  label: const Text('提交 BT 任务（一次）'),
                ),
              OutlinedButton.icon(
                onPressed: _busy || _confirmingWrite || selectedSource == null
                    ? null
                    : () => _queryTask(selectedSource, result),
                icon: const Icon(Icons.search),
                label: Text(_busy ? '正在查询…' : '查询 BT 任务'),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

class _BtPreflightFact extends StatelessWidget {
  const _BtPreflightFact({required this.label, required this.value});

  final String label;
  final String value;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: 4),
    child: SelectableText(
      '$label：$value',
      style: Theme.of(context).textTheme.bodySmall,
    ),
  );
}

String _safeBtError(Object error) {
  final value = error.toString();
  if (value.contains('WAF') || value.contains('405')) {
    return '115 返回 WAF 405，已停止本轮核对并进入会话冷却。';
  }
  if (value.contains('429') || value.contains('限流')) {
    return '115 请求受到限流，本轮只读核对已停止。';
  }
  if (value.contains('当前没有已连接') ||
      value.contains('会话不存在') ||
      value.contains('不会重新连接')) {
    return '没有可复用的已连接 115 Cookie 会话。请先通过现有书源正常连接；此入口不会重新登录或刷新 Cookie。';
  }
  if (value.contains('登录失效') || value.contains('鉴权失败')) {
    return '现有 115 Cookie 会话未通过验证。请检查书源登录状态；此入口不会自动刷新或替换会话。';
  }
  if (value.contains('登录') || value.contains('Cookie')) {
    return '115 Cookie 会话未通过核对，请检查现有书源登录状态；此入口不会自动刷新或替换会话。';
  }
  return value.replaceAll(RegExp(r'\s+'), ' ').trim();
}

class _DailyAutomationBlock extends StatelessWidget {
  const _DailyAutomationBlock({required this.store, required this.coordinator});

  final EhSubscriptionStore store;
  final EhDailyCoordinator? coordinator;

  @override
  Widget build(BuildContext context) {
    final listenable = coordinator == null
        ? store
        : Listenable.merge([store, coordinator]);
    return ListenableBuilder(
      listenable: listenable,
      builder: (context, _) {
        final state = store.dailyAutomation;
        final run = state.lastRun;
        final runError = run?.error;
        final supported = Platform.isWindows;
        return Card(
          margin: EdgeInsets.zero,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 4),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      '自动每日检查',
                      style: Theme.of(context).textTheme.titleSmall,
                    ),
                    const SizedBox(width: 8),
                    Semantics(
                      label: '自动每日检查',
                      child: Switch.adaptive(
                        key: const ValueKey('eh-daily-toggle'),
                        value: state.dailyEnabled,
                        onChanged: supported
                            ? (enabled) async {
                                try {
                                  await store.setDailyEnabled(enabled);
                                  await coordinator?.settingsChanged();
                                } catch (error) {
                                  if (!context.mounted) return;
                                  ScaffoldMessenger.of(context).showSnackBar(
                                    SnackBar(
                                      content: Text('保存 Daily 设置失败：$error'),
                                      backgroundColor: Theme.of(
                                        context,
                                      ).colorScheme.error,
                                    ),
                                  );
                                }
                              }
                            : null,
                      ),
                    ),
                  ],
                ),
                Padding(
                  padding: const EdgeInsets.only(bottom: 8),
                  child: Text(
                    supported
                        ? '仅在 Windows 版 RCH 运行期间检查；每天最多尝试一次，不固定执行时刻。'
                        : 'Daily 自动检查仅支持 Windows。',
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                ),
                _DailyInfoRow(
                  label: '上次自动尝试日期',
                  value: state.lastAutoAttemptDate ?? '尚未尝试',
                ),
                _DailyInfoRow(
                  label: '最近一次运行',
                  value: run == null
                      ? '尚无记录'
                      : '${_dailySourceLabel(run.source)} · ${_dailyRunLabel(run.status)}',
                ),
                _DailyInfoRow(
                  label: '下次符合条件',
                  value: _nextDailyEligibility(
                    state,
                    store.hasOutDir,
                    supported,
                  ),
                ),
                if (coordinator?.status ==
                    EhDailyCoordinatorStatus.waitingForCurrentRun)
                  const _DailyInfoRow(
                    label: '当前调度状态',
                    value: '等待当前收集完成；Daily 不会抢占或中断当前任务。',
                  ),
                if (run != null)
                  _DailyInfoRow(
                    label: '最近保存数量',
                    value: '${run.saved} 个 Torrent',
                  ),
                if (runError != null && runError.isNotEmpty)
                  Padding(
                    padding: const EdgeInsets.only(top: 4, bottom: 8),
                    child: Text(
                      '最近错误：$runError',
                      style: TextStyle(
                        color: Theme.of(context).colorScheme.error,
                      ),
                    ),
                  ),
                if (coordinator?.status ==
                    EhDailyCoordinatorStatus.persistenceFailed)
                  Padding(
                    padding: const EdgeInsets.only(top: 4, bottom: 8),
                    child: Text(
                      coordinator?.error ?? '调度状态保存失败，自动检查已停止。',
                      style: TextStyle(
                        color: Theme.of(context).colorScheme.error,
                      ),
                    ),
                  ),
              ],
            ),
          ),
        );
      },
    );
  }
}

class _DailyInfoRow extends StatelessWidget {
  const _DailyInfoRow({required this.label, required this.value});

  final String label;
  final String value;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: 6),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        SizedBox(
          width: 126,
          child: Text(label, style: Theme.of(context).textTheme.bodySmall),
        ),
        Expanded(
          child: Text(value, style: Theme.of(context).textTheme.bodySmall),
        ),
      ],
    ),
  );
}

String _nextDailyEligibility(
  EhDailyAutomationState state,
  bool hasOutDir,
  bool supported,
) {
  if (!state.dailyEnabled) return '未启用';
  if (!supported) return '仅 Windows 支持';
  if (!hasOutDir) return '等待选择 Torrent 保存目录';
  final today = ehLocalDateKey(DateTime.now());
  final lastAttempt = state.lastAutoAttemptDate;
  if (lastAttempt == null || today.compareTo(lastAttempt) > 0) {
    return '本地日期 $today 尚未尝试；运行期间首次满足条件时启动';
  }
  if (today.compareTo(lastAttempt) < 0) {
    final next = DateTime.tryParse(lastAttempt)?.add(const Duration(days: 1));
    return next == null
        ? '系统日期早于已记录尝试日期，自动检查暂停'
        : '系统日期回拨；待本地日期超过 $lastAttempt 后恢复（预计 ${ehLocalDateKey(next)}）';
  }
  final next = DateTime.tryParse(lastAttempt)?.add(const Duration(days: 1));
  return next == null
      ? '本日已领取，下一次等待新本地日期'
      : '本日已领取；下一次符合日期为 ${ehLocalDateKey(next)}';
}

String _dailySourceLabel(EhDailyRunSource source) => switch (source) {
  EhDailyRunSource.manual => '手动',
  EhDailyRunSource.dailyAuto => 'Daily',
  EhDailyRunSource.unknown => '来源未知',
};

String _dailyRunLabel(EhDailyRunStatus status) => switch (status) {
  EhDailyRunStatus.claimed => '已领取执行资格',
  EhDailyRunStatus.running => '运行中',
  EhDailyRunStatus.completed => '已完成',
  EhDailyRunStatus.failed => '失败',
  EhDailyRunStatus.cancelled => '已取消',
  EhDailyRunStatus.interruptedUnknown => '中断，远端结果未知',
  EhDailyRunStatus.unknown => '状态未知',
};

/// 面板骨架：标题 + 说明 + 内容（与设置页其它面板一致）。
class _Section extends StatelessWidget {
  const _Section({
    required this.title,
    required this.subtitle,
    required this.children,
    this.trailing,
  });

  final String title;
  final String subtitle;
  final List<Widget> children;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            const Text(
              'EH 订阅',
              style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600),
            ),
            const Spacer(),
            ?trailing,
          ],
        ),
        const SizedBox(height: 4),
        Text(subtitle, style: theme.textTheme.bodySmall),
        const SizedBox(height: 12),
        ...children,
      ],
    );
  }
}

/// 顶部状态徽标：跑动中/上次结果。
class _StatusPill extends StatelessWidget {
  const _StatusPill({required this.progress});

  final EhProgress progress;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final (label, color) = switch (progress.stage) {
      'failed' => ('失败', scheme.error),
      'done' => ('就绪', scheme.primary),
      'idle' => ('未运行', scheme.outline),
      _ => (progress.stageLabel, scheme.tertiary),
    };
    final running = progress.running;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(999),
        border: Border.all(color: color.withValues(alpha: 0.5)),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (running) ...[
            SizedBox(
              width: 12,
              height: 12,
              child: CircularProgressIndicator(strokeWidth: 2, color: color),
            ),
            const SizedBox(width: 6),
          ] else ...[
            Icon(Icons.circle, size: 8, color: color),
            const SizedBox(width: 6),
          ],
          Text(
            label,
            style: TextStyle(
              fontSize: 12,
              color: color,
              fontWeight: FontWeight.w600,
            ),
          ),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// 规则编辑
// ---------------------------------------------------------------------------

class _RulesEditor extends StatefulWidget {
  const _RulesEditor({super.key, required this.store, required this.onPickDir});

  final EhSubscriptionStore store;
  final Future<void> Function() onPickDir;

  @override
  State<_RulesEditor> createState() => _RulesEditorState();
}

class _TierRow {
  _TierRow(double years, int downloads)
    : yearsCtrl = TextEditingController(text: _fmtNum(years)),
      downloadsCtrl = TextEditingController(text: '$downloads');

  final TextEditingController yearsCtrl;
  final TextEditingController downloadsCtrl;

  void dispose() {
    yearsCtrl.dispose();
    downloadsCtrl.dispose();
  }
}

String _fmtNum(double v) => v == v.roundToDouble() ? '${v.toInt()}' : '$v';

class _RulesEditorState extends State<_RulesEditor> {
  late final TextEditingController _search;
  late final TextEditingController _markers;
  late final TextEditingController _exclude;
  late final TextEditingController _interval;
  late final TextEditingController _pagesCtrl;
  late final TextEditingController _proxyUrl;
  List<_TierRow> _tiers = [];

  double _minRating = 4;
  int _pages = 2;
  Timer? _debounce;
  bool _proxyEditing = false;

  EhSubscriptionStore get store => widget.store;

  @override
  void initState() {
    super.initState();
    reloadFromStore();
    _proxyUrl = TextEditingController(text: store.proxyUrl);
    store.addListener(_syncProxyUrl);
  }

  @override
  void didUpdateWidget(covariant _RulesEditor oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.store, widget.store)) {
      oldWidget.store.removeListener(_syncProxyUrl);
      widget.store.addListener(_syncProxyUrl);
      _syncProxyUrl();
    }
  }

  void _syncProxyUrl() {
    if (_proxyEditing || _proxyUrl.text == store.proxyUrl) return;
    _proxyUrl.value = TextEditingValue(
      text: store.proxyUrl,
      selection: TextSelection.collapsed(offset: store.proxyUrl.length),
    );
  }

  void reloadFromStore() {
    final s = store;
    _search = TextEditingController(text: s.search);
    _markers = TextEditingController(text: s.titleMarkers);
    _exclude = TextEditingController(text: s.excludeMarkers.join(', '));
    _interval = TextEditingController(text: '${s.intervalSecs}');
    _pagesCtrl = TextEditingController(text: '${s.pages}');
    _minRating = s.minRating;
    _pages = s.pages;
    for (final t in _tiers) {
      t.dispose();
    }
    _tiers = s.ageTiers
        .map(
          (t) => _TierRow(
            (t['min_age_years'] as num?)?.toDouble() ?? 0,
            (t['min_downloads'] as num?)?.toInt() ?? 0,
          ),
        )
        .toList();
    setState(() {});
  }

  /// 目录选择后由外部写入（控制器不在本编辑器内）。
  void setOutDir(String dir) {
    store.patch('out_dir', dir);
    setState(() {});
  }

  /// 把编辑器内容统一写回 store（运行/保存前调用）。
  void flush() {
    store.patch('search', _search.text.trim());
    store.patch('title_markers', _markers.text.trim());
    store.patch(
      'exclude_markers',
      _exclude.text
          .split(RegExp(r'[,，]'))
          .map((e) => e.trim())
          .where((e) => e.isNotEmpty)
          .toList(),
    );
    store.patch('min_rating', _minRating);
    final custom = int.tryParse(_pagesCtrl.text.trim());
    if (custom != null && custom > 0) {
      _pages = custom.clamp(1, 500);
    }
    store.patch('pages', _pages);
    store.patch(
      'request_interval_secs',
      double.tryParse(_interval.text.trim()) ?? 2.5,
    );
    store.patch('age_tiers', _collectTiers());
  }

  List<Map<String, dynamic>> _collectTiers() {
    final rows = _tiers
        .map(
          (t) => {
            'min_age_years': double.tryParse(t.yearsCtrl.text.trim()) ?? 0,
            'min_downloads': int.tryParse(t.downloadsCtrl.text.trim()) ?? 0,
          },
        )
        .toList();
    rows.sort(
      (a, b) => (b['min_age_years'] as double).compareTo(
        a['min_age_years'] as double,
      ),
    );
    return rows;
  }

  /// 输入防抖写回：用户停手 700ms 后更新 store（不触发运行）。
  void _debouncedFlush() {
    _debounce?.cancel();
    _debounce = Timer(const Duration(milliseconds: 700), flush);
  }

  @override
  void dispose() {
    _debounce?.cancel();
    _search.dispose();
    _markers.dispose();
    _exclude.dispose();
    _interval.dispose();
    _pagesCtrl.dispose();
    store.removeListener(_syncProxyUrl);
    _proxyUrl.dispose();
    for (final t in _tiers) {
      t.dispose();
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final store = widget.store;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _FieldLabel('搜索语法', 'EH 服务端唯一能筛的维度：语言与标签（评分/有无种子只能本地筛）'),
        TextField(
          controller: _search,
          onChanged: (_) => _debouncedFlush(),
          decoration: const InputDecoration(
            isDense: true,
            border: OutlineInputBorder(),
            hintText: r'language:chinese$ uncensored',
            prefixIcon: Icon(Icons.search),
          ),
        ),
        const SizedBox(height: 16),
        _FieldLabel(
          '评分下限：${_minRating.toStringAsFixed(1)}',
          '0 票的画廊评分为 0.00，会被自动排除',
        ),
        Slider(
          value: _minRating.clamp(0, 5),
          min: 0,
          max: 5,
          divisions: 50,
          label: _minRating.toStringAsFixed(1),
          onChanged: (v) => setState(() => _minRating = v),
          onChangeEnd: (_) => flush(),
        ),
        const SizedBox(height: 8),
        _FieldLabel(
          '标题标记（白名单）',
          '用 | 分隔；实测 [Digital]/[DL版] 出现在标题而非标签里。填 any 表示不筛',
        ),
        TextField(
          controller: _markers,
          onChanged: (_) => _debouncedFlush(),
          decoration: const InputDecoration(
            isDense: true,
            border: OutlineInputBorder(),
            hintText: 'Digital|DL版|DL',
          ),
        ),
        const SizedBox(height: 12),
        _FieldLabel('排除标记', '命中即排除，用逗号分隔'),
        TextField(
          controller: _exclude,
          onChanged: (_) => _debouncedFlush(),
          decoration: const InputDecoration(
            isDense: true,
            border: OutlineInputBorder(),
            hintText: 'AI Generated',
          ),
        ),
        const SizedBox(height: 20),
        LayoutBuilder(
          builder: (context, constraints) {
            final addTierButton = TextButton.icon(
              onPressed: () {
                setState(() {
                  _tiers.insert(0, _TierRow(5, 800));
                });
                flush();
              },
              icon: const Icon(Icons.add, size: 18),
              label: const Text('添加档位'),
            );
            const label = _FieldLabel('分时间段下载数要求', '画龄越久要求越高；新发不设门槛，靠重复扫描累积达标');
            if (constraints.maxWidth < 500) {
              return Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  label,
                  Align(alignment: Alignment.centerRight, child: addTierButton),
                ],
              );
            }
            return Row(children: [label, const Spacer(), addTierButton]);
          },
        ),
        const SizedBox(height: 4),
        Container(
          decoration: BoxDecoration(
            border: Border.all(color: theme.dividerColor),
            borderRadius: BorderRadius.circular(8),
          ),
          child: Column(
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(12, 8, 12, 0),
                child: Row(
                  children: [
                    Expanded(
                      flex: 4,
                      child: Text('画龄 ≥（年）', style: theme.textTheme.bodySmall),
                    ),
                    Expanded(
                      flex: 4,
                      child: Text('下载数 ≥', style: theme.textTheme.bodySmall),
                    ),
                    const SizedBox(width: 40),
                  ],
                ),
              ),
              for (var i = 0; i < _tiers.length; i++)
                Padding(
                  padding: const EdgeInsets.fromLTRB(12, 4, 4, 4),
                  child: Row(
                    children: [
                      Expanded(
                        flex: 4,
                        child: TextField(
                          controller: _tiers[i].yearsCtrl,
                          onChanged: (_) => _debouncedFlush(),
                          keyboardType: const TextInputType.numberWithOptions(
                            decimal: true,
                          ),
                          decoration: const InputDecoration(
                            isDense: true,
                            border: OutlineInputBorder(),
                          ),
                        ),
                      ),
                      const SizedBox(width: 8),
                      Expanded(
                        flex: 4,
                        child: TextField(
                          controller: _tiers[i].downloadsCtrl,
                          onChanged: (_) => _debouncedFlush(),
                          keyboardType: TextInputType.number,
                          decoration: const InputDecoration(
                            isDense: true,
                            border: OutlineInputBorder(),
                          ),
                        ),
                      ),
                      IconButton(
                        tooltip: '删除该档位',
                        onPressed: _tiers.length <= 1
                            ? null
                            : () {
                                setState(() {
                                  _tiers.removeAt(i).dispose();
                                });
                                flush();
                              },
                        icon: const Icon(Icons.remove_circle_outline, size: 20),
                      ),
                    ],
                  ),
                ),
            ],
          ),
        ),
        const SizedBox(height: 20),
        _subscriptionFields(theme, store),
      ],
    );
  }

  Widget _subscriptionFields(ThemeData theme, EhSubscriptionStore store) {
    final directory = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _FieldLabel('保存目录', '种子与 manifest.json 落在这里'),
        Row(
          children: [
            Expanded(
              child: Text(
                store.hasOutDir ? store.outDir : '（未选择）',
                style: theme.textTheme.bodyMedium?.copyWith(
                  color: store.hasOutDir ? null : theme.colorScheme.error,
                ),
                overflow: TextOverflow.ellipsis,
              ),
            ),
            const SizedBox(width: 8),
            OutlinedButton.icon(
              onPressed: widget.onPickDir,
              icon: const Icon(Icons.folder_open, size: 18),
              label: const Text('选择'),
            ),
          ],
        ),
      ],
    );
    final pageCount = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _FieldLabel('每轮页数', '每页 25 条（约 40~60 秒/页）；上限 500。覆盖越广耗时越长'),
        SizedBox(
          width: 140,
          child: TextField(
            controller: _pagesCtrl,
            keyboardType: TextInputType.number,
            decoration: const InputDecoration(
              isDense: true,
              border: OutlineInputBorder(),
              suffixText: '页',
            ),
            onChanged: (_) => _debouncedFlush(),
          ),
        ),
      ],
    );
    final requestInterval = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _FieldLabel('请求间隔（秒）', '过小会触发 EH 限流，建议 ≥2'),
        TextField(
          controller: _interval,
          onChanged: (_) => _debouncedFlush(),
          keyboardType: const TextInputType.numberWithOptions(decimal: true),
          decoration: const InputDecoration(
            isDense: true,
            border: OutlineInputBorder(),
          ),
        ),
      ],
    );
    final networkAndAutomation = ExpansionTile(
      key: const ValueKey('eh-network-automation'),
      title: const Text('网络与自动化'),
      subtitle: Text('EH：${store.proxyMode}'),
      childrenPadding: const EdgeInsets.fromLTRB(16, 0, 16, 12),
      children: [
        _FieldLabel(
          'EH 网络模式',
          'system 使用 Rust 支持的系统/环境代理；direct 显式直连；manual 只使用下方代理，不会静默切换路线。',
        ),
        DropdownButtonFormField<String>(
          key: const ValueKey('eh-proxy-mode'),
          initialValue: store.proxyMode,
          decoration: const InputDecoration(
            isDense: true,
            border: OutlineInputBorder(),
          ),
          items: const [
            DropdownMenuItem(value: 'system', child: Text('system（系统代理）')),
            DropdownMenuItem(value: 'direct', child: Text('direct（直连）')),
            DropdownMenuItem(value: 'manual', child: Text('manual（手动代理）')),
          ],
          onChanged: (mode) {
            if (mode == null) return;
            store.patchNetwork(mode: mode);
            _saveNetworkSettings(showSuccess: false);
          },
        ),
        if (store.proxyMode == 'manual') ...[
          const SizedBox(height: 12),
          _FieldLabel(
            '代理地址',
            '支持 HTTP(S)、SOCKS5 和 SOCKS5H（代理端 DNS）。地址只保存在本地设置中，不写入诊断日志。',
          ),
          TextField(
            key: const ValueKey('eh-proxy-url'),
            controller: _proxyUrl,
            onChanged: (value) {
              _proxyEditing = true;
              store.patchNetwork(proxyUrl: value, notify: false);
            },
            onSubmitted: (_) => _saveNetworkSettings(showSuccess: false),
            decoration: const InputDecoration(
              isDense: true,
              border: OutlineInputBorder(),
              hintText: 'http://127.0.0.1:7890 或 socks5h://127.0.0.1:7891',
            ),
          ),
        ],
        const SizedBox(height: 10),
        Align(
          alignment: Alignment.centerLeft,
          child: OutlinedButton.icon(
            onPressed: _saveNetworkSettings,
            icon: const Icon(Icons.save_outlined, size: 18),
            label: const Text('保存网络设置'),
          ),
        ),
        const SizedBox(height: 4),
        Align(
          alignment: Alignment.centerLeft,
          child: OutlinedButton.icon(
            onPressed: store.probing ? null : _checkConnectivity,
            icon: store.probing
                ? const SizedBox(
                    width: 14,
                    height: 14,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  )
                : const Icon(Icons.wifi_tethering_outlined, size: 18),
            label: const Text('检测 EH 网络'),
          ),
        ),
        const SizedBox(height: 4),
        Text(
          'Tracker 检测只请求主机根路径；没有授权测试种子时，不会声称 Torrent 数据已验证。',
          style: theme.textTheme.bodySmall,
        ),
        if (store.probe != null) ...[
          const SizedBox(height: 12),
          _ProbeBlock(probe: store.probe!),
        ],
      ],
    );

    return LayoutBuilder(
      builder: (context, constraints) {
        final compact = constraints.maxWidth < 680;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            compact
                ? Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      directory,
                      const SizedBox(height: 12),
                      pageCount,
                      const SizedBox(height: 12),
                      requestInterval,
                    ],
                  )
                : Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Expanded(flex: 3, child: directory),
                      const SizedBox(width: 16),
                      Expanded(flex: 2, child: pageCount),
                      const SizedBox(width: 16),
                      Expanded(flex: 2, child: requestInterval),
                    ],
                  ),
            const SizedBox(height: 12),
            networkAndAutomation,
          ],
        );
      },
    );
  }

  Future<void> _saveNetworkSettings({bool showSuccess = true}) async {
    store.patchNetwork(proxyUrl: _proxyUrl.text, notify: false);
    _proxyEditing = false;
    try {
      await store.saveRules();
      if (showSuccess && mounted) _snack('EH 网络设置已保存');
    } catch (error) {
      if (mounted) _snack('$error', error: true);
    }
  }

  void _snack(String message, {bool error = false}) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(message),
        backgroundColor: error ? Theme.of(context).colorScheme.error : null,
      ),
    );
  }

  Future<void> _checkConnectivity() async {
    flush();
    store.patchNetwork(proxyUrl: _proxyUrl.text, notify: false);
    _proxyEditing = false;
    try {
      await store.saveRules();
    } catch (error) {
      if (mounted) _snack('$error', error: true);
      return;
    }
    final p = await widget.store.runProbe();
    if (!mounted || p == null) return;
    final ok = p.hostOk && p.trackerOk;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          'EH 主站 ${p.hostOk ? "可用" : "不可用（${p.hostDetail}）"} ｜ '
          'Tracker 主机 ${p.trackerOk ? "有响应" : "不可达（${p.trackerDetail}）"}；'
          'Torrent 数据${p.trackerTorrentValidated ? "已验证" : "未验证"}',
        ),
        backgroundColor: ok ? null : Theme.of(context).colorScheme.error,
        duration: const Duration(seconds: 6),
      ),
    );
  }
}

/// 连通性预检结果块：把"哪一段不通"讲清楚。
class _ProbeBlock extends StatelessWidget {
  const _ProbeBlock({required this.probe});

  final EhProbe probe;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    Widget row(bool ok, String label, String detail) {
      final color = ok ? scheme.primary : scheme.error;
      return Padding(
        padding: const EdgeInsets.only(top: 4),
        child: Row(
          children: [
            Icon(
              ok ? Icons.check_circle_outline : Icons.error_outline,
              size: 14,
              color: color,
            ),
            const SizedBox(width: 6),
            Text(
              '$label：${ok ? "可达" : "不可达"}',
              style: TextStyle(fontSize: 12, color: color),
            ),
            const SizedBox(width: 8),
            Expanded(
              child: Text(
                detail,
                style: Theme.of(
                  context,
                ).textTheme.bodySmall?.copyWith(fontSize: 11),
                overflow: TextOverflow.ellipsis,
              ),
            ),
          ],
        ),
      );
    }

    return Container(
      padding: const EdgeInsets.fromLTRB(12, 8, 12, 10),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: scheme.outlineVariant),
        color: scheme.surfaceContainerHighest.withValues(alpha: 0.3),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            '连通性检查 · ${probe.mode}',
            style: TextStyle(
              fontSize: 12,
              fontWeight: FontWeight.w600,
              color: scheme.outline,
            ),
          ),
          if (probe.checkedAtUnixMs > 0)
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Text(
                '检测时间：${DateTime.fromMillisecondsSinceEpoch(probe.checkedAtUnixMs).toLocal()}',
                style: Theme.of(
                  context,
                ).textTheme.bodySmall?.copyWith(fontSize: 11),
              ),
            ),
          row(probe.hostOk, probe.host, probe.hostDetail),
          row(probe.trackerOk, '${probe.tracker}（主机）', probe.trackerDetail),
          Text(
            'EH 用时 ${probe.hostElapsedMs} ms；Tracker 用时 ${probe.trackerElapsedMs} ms；'
            'Torrent 数据${probe.trackerTorrentValidated ? "已验证" : "未验证"}',
            style: Theme.of(
              context,
            ).textTheme.bodySmall?.copyWith(fontSize: 11),
          ),
          if (!probe.hostOk)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Text(
                '主站不可达时，请检查网络连接或系统代理后重试。',
                style: Theme.of(
                  context,
                ).textTheme.bodySmall?.copyWith(fontSize: 11),
              ),
            ),
        ],
      ),
    );
  }
}

class _FieldLabel extends StatelessWidget {
  const _FieldLabel(this.title, this.hint);

  final String title;
  final String hint;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            title,
            style: const TextStyle(fontWeight: FontWeight.w600, fontSize: 13),
          ),
          Text(hint, style: theme.textTheme.bodySmall?.copyWith(fontSize: 11)),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// 运行条 + 进度 + 已保存清单
// ---------------------------------------------------------------------------

class _RunBar extends StatelessWidget {
  const _RunBar({
    required this.store,
    required this.onRun,
    required this.onOpenDir,
  });

  final EhSubscriptionStore store;
  final Future<void> Function() onRun;
  final Future<void> Function() onOpenDir;

  @override
  Widget build(BuildContext context) {
    final running = store.progress.running || store.busy;
    return Row(
      children: [
        FilledButton.icon(
          onPressed: running ? null : () => onRun(),
          icon: running
              ? const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              : const Icon(Icons.download),
          label: Text(running ? '扫描中…' : '开始扫描'),
        ),
        const SizedBox(width: 8),
        if (running)
          OutlinedButton.icon(
            onPressed: () => store.cancel(),
            icon: const Icon(Icons.stop, size: 18),
            label: const Text('停止'),
          ),
        if (!running)
          OutlinedButton.icon(
            onPressed: store.hasOutDir ? () => onOpenDir() : null,
            icon: const Icon(Icons.folder_open, size: 18),
            label: const Text('打开目录'),
          ),
        const Spacer(),
        Text(
          '已保存 ${store.manifest.length} 个',
          style: Theme.of(context).textTheme.bodySmall,
        ),
      ],
    );
  }
}

class _ProgressBlock extends StatelessWidget {
  const _ProgressBlock({required this.progress});

  final EhProgress progress;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = progress;
    if (p.stage == 'idle') {
      return Text('尚未运行。点击「开始扫描」按当前规则抓取一轮。', style: theme.textTheme.bodySmall);
    }
    final total = p.pages <= 0 ? 1 : p.pages;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (p.running) ...[
          LinearProgressIndicator(
            value: p.stage == 'searching' ? (p.page / total).clamp(0, 1) : null,
          ),
          const SizedBox(height: 8),
        ],
        Wrap(
          spacing: 8,
          runSpacing: 8,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            _Metric(
              icon: Icons.travel_explore,
              label: '候选',
              value: '${p.candidates}',
            ),
            _Metric(
              icon: Icons.download_done,
              label: '本轮新增',
              value: '${p.saved}',
              highlight: true,
            ),
            _Metric(
              icon: Icons.star_border,
              label: '评分不足',
              value: '${p.noRating}',
            ),
            _Metric(
              icon: Icons.label_off_outlined,
              label: '标记排除',
              value: '${p.noMarker}',
            ),
            _Metric(
              icon: Icons.cloud_off,
              label: '无种子',
              value: '${p.noTorrent}',
            ),
            _Metric(
              icon: Icons.trending_down,
              label: '下载数不达标',
              value: '${p.noDownloads}',
            ),
            _Metric(
              icon: Icons.help_outline,
              label: '映射未确认',
              value: '${p.unmapped}',
            ),
          ],
        ),
        const SizedBox(height: 8),
        if (p.error != null)
          Text(
            '失败：${p.error}',
            style: TextStyle(color: theme.colorScheme.error, fontSize: 12),
          )
        else if (p.message.isNotEmpty)
          Text(p.message, style: theme.textTheme.bodySmall),
      ],
    );
  }
}

class _Metric extends StatelessWidget {
  const _Metric({
    required this.icon,
    required this.label,
    required this.value,
    this.highlight = false,
  });

  final IconData icon;
  final String label;
  final String value;
  final bool highlight;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final color = highlight && value != '0' ? scheme.primary : scheme.outline;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.10),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: color.withValues(alpha: 0.35)),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 14, color: color),
          const SizedBox(width: 6),
          Text('$label $value', style: TextStyle(fontSize: 12, color: color)),
        ],
      ),
    );
  }
}

class _ManifestBlock extends StatefulWidget {
  const _ManifestBlock({
    required this.store,
    required this.onOpenDir,
    required this.onRefresh,
    required this.onShowInstructions,
    required this.onCopyPath,
  });

  final EhSubscriptionStore store;
  final Future<void> Function() onOpenDir;
  final Future<void> Function() onRefresh;
  final Future<void> Function() onShowInstructions;
  final Future<void> Function(String path) onCopyPath;

  @override
  State<_ManifestBlock> createState() => _ManifestBlockState();
}

class _ManifestBlockState extends State<_ManifestBlock> {
  bool _showAll = false;
  String? _selectedFile;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final report = widget.store.manifestReport;
    final entries = report.entries.reversed.toList();
    final shown = _showAll ? entries : entries.take(5).toList();
    EhCheckedManifestEntry? selected;
    for (final entry in entries) {
      if (entry.item.file == _selectedFile) {
        selected = entry;
        break;
      }
    }
    final selectedPath = selected?.fullPath;
    return RadioGroup<String>(
      groupValue: _selectedFile,
      onChanged: (value) => setState(() => _selectedFile = value),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const Text(
                '已保存种子',
                style: TextStyle(fontWeight: FontWeight.w600, fontSize: 13),
              ),
              const SizedBox(width: 8),
              Text('共 ${entries.length} 个', style: theme.textTheme.bodySmall),
              const Spacer(),
              TextButton.icon(
                key: const ValueKey('eh-manifest-refresh'),
                onPressed: widget.store.refreshingManifest
                    ? null
                    : () => widget.onRefresh(),
                icon: widget.store.refreshingManifest
                    ? const SizedBox(
                        width: 16,
                        height: 16,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Icon(Icons.refresh, size: 16),
                label: const Text('刷新清单'),
              ),
              TextButton.icon(
                onPressed: widget.store.hasOutDir
                    ? () => widget.onOpenDir()
                    : null,
                icon: const Icon(Icons.folder_open, size: 16),
                label: const Text('打开保存目录'),
              ),
              if (entries.length > 5)
                TextButton(
                  onPressed: () => setState(() => _showAll = !_showAll),
                  child: Text(_showAll ? '收起' : '展开全部'),
                ),
            ],
          ),
          const SizedBox(height: 2),
          Text(
            _manifestReadStatusLabel(report),
            style: theme.textTheme.bodySmall,
          ),
          if (report.invalidEntries > 0)
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Text(
                '有 ${report.invalidEntries} 条清单记录无法解析；可读取记录仍显示，原 manifest 未修改。',
                style: TextStyle(color: theme.colorScheme.error, fontSize: 12),
              ),
            ),
          if (shown.isEmpty)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 12),
              child: Text(
                _manifestEmptyStateLabel(report),
                style: theme.textTheme.bodySmall,
              ),
            ),
          for (var index = 0; index < shown.length; index++)
            _ManifestHandoffTile(
              key: ValueKey('eh-handoff-select-$index'),
              entry: shown[index],
              selected: shown[index].item.file == _selectedFile,
            ),
          if (!_showAll && entries.length > 5)
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Text('仅显示最近 5 个', style: theme.textTheme.bodySmall),
            ),
          const SizedBox(height: 6),
          Wrap(
            spacing: 8,
            runSpacing: 4,
            children: [
              OutlinedButton.icon(
                key: const ValueKey('eh-handoff-copy-path'),
                onPressed: selectedPath == null
                    ? null
                    : () => widget.onCopyPath(selectedPath),
                icon: const Icon(Icons.content_copy, size: 16),
                label: const Text('复制 Torrent 完整路径'),
              ),
              TextButton.icon(
                key: const ValueKey('eh-handoff-instructions'),
                onPressed: () => widget.onShowInstructions(),
                icon: const Icon(Icons.info_outline, size: 16),
                label: const Text('查看 115 手动提交说明'),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

String _manifestReadStatusLabel(EhManifestReport report) =>
    switch (report.status) {
      EhManifestReadStatus.unconfigured => '本地清单状态：未配置保存目录',
      EhManifestReadStatus.missing => '本地清单状态：未找到 manifest.json',
      EhManifestReadStatus.empty => '本地清单状态：清单存在，但没有已保存种子',
      EhManifestReadStatus.ready => '本地清单状态：可读取',
      EhManifestReadStatus.damaged => '本地清单状态：内容损坏；未改写原文件',
      EhManifestReadStatus.unreadable => '本地清单状态：无法读取；未改写原文件',
    };

String _manifestEmptyStateLabel(EhManifestReport report) =>
    switch (report.status) {
      EhManifestReadStatus.unconfigured => '请先选择 EH 保存目录。',
      EhManifestReadStatus.missing => '当前保存目录没有 manifest.json。',
      EhManifestReadStatus.empty => 'manifest.json 中没有种子记录。',
      EhManifestReadStatus.damaged => '清单损坏，无法显示更多记录；原文件保持不变。',
      EhManifestReadStatus.unreadable => '无法读取清单或 Torrent 文件。',
      EhManifestReadStatus.ready => '没有可显示的清单记录。',
    };

String _savedFileStatusLabel(EhSavedFileStatus status) => switch (status) {
  EhSavedFileStatus.present => '文件存在',
  EhSavedFileStatus.missing => '文件缺失',
  EhSavedFileStatus.unsafePath => '路径不安全，已阻止访问',
  EhSavedFileStatus.notRegularFile => '目标不是普通文件',
  EhSavedFileStatus.unreadable => '文件无法读取',
  EhSavedFileStatus.notChecked => '尚未校验文件',
};

String _torrentValidationLabel(EhTorrentValidationStatus status) =>
    switch (status) {
      EhTorrentValidationStatus.valid => '校验通过：Bencode 与 Infohash 匹配',
      EhTorrentValidationStatus.sizeMismatch => '文件大小与清单记录不符',
      EhTorrentValidationStatus.invalidTorrent => 'Torrent 格式无效',
      EhTorrentValidationStatus.infohashMismatch => '计算出的 Infohash 与清单不符',
      EhTorrentValidationStatus.tooLarge => '文件超过 16 MiB 本地校验上限',
      EhTorrentValidationStatus.unreadable => 'Torrent 文件读取失败',
      EhTorrentValidationStatus.notChecked => '未执行 Torrent 校验',
    };

class _ManifestHandoffTile extends StatelessWidget {
  const _ManifestHandoffTile({
    super.key,
    required this.entry,
    required this.selected,
  });

  final EhCheckedManifestEntry entry;
  final bool selected;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final item = entry.item;
    final title = item.titleJpn.isEmpty ? item.torrentName : item.titleJpn;
    final torrentSize = item.bytes < 0
        ? '未知大小'
        : fmtSize(BigInt.from(item.bytes));
    final savedAt = DateTime.tryParse(item.savedAtUtc)?.toLocal();
    final savedAtText = savedAt == null
        ? item.savedAtUtc
        : savedAt.toString().split('.').first;
    return Container(
      margin: const EdgeInsets.only(bottom: 6),
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerHighest.withValues(alpha: 0.4),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Material(
            color: Colors.transparent,
            borderRadius: BorderRadius.circular(6),
            child: RadioListTile<String>(
              contentPadding: EdgeInsets.zero,
              dense: true,
              value: item.file,
              title: Text(
                title,
                style: const TextStyle(
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                ),
              ),
              subtitle: Text('${item.file} · $torrentSize'),
            ),
          ),
          Padding(
            padding: const EdgeInsets.only(left: 12, bottom: 3),
            child: Text('保存时间：$savedAtText', style: theme.textTheme.bodySmall),
          ),
          Padding(
            padding: const EdgeInsets.only(left: 12, bottom: 3),
            child: SelectableText(
              'Infohash：${item.infohash}',
              style: theme.textTheme.bodySmall,
            ),
          ),
          Padding(
            padding: const EdgeInsets.only(left: 12),
            child: Wrap(
              spacing: 12,
              runSpacing: 3,
              children: [
                Text(
                  _savedFileStatusLabel(entry.fileStatus),
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: entry.fileStatus == EhSavedFileStatus.present
                        ? scheme.primary
                        : scheme.error,
                  ),
                ),
                Text(
                  _torrentValidationLabel(entry.validationStatus),
                  style: theme.textTheme.bodySmall?.copyWith(
                    color:
                        entry.validationStatus ==
                            EhTorrentValidationStatus.valid
                        ? scheme.primary
                        : scheme.error,
                  ),
                ),
              ],
            ),
          ),
          if (selected)
            Padding(
              padding: const EdgeInsets.only(left: 12, top: 4),
              child: Text(
                '本地清单状态不代表已上传或已提交 115。',
                style: theme.textTheme.bodySmall?.copyWith(
                  color: scheme.outline,
                ),
              ),
            ),
        ],
      ),
    );
  }
}
