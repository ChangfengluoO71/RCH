import 'dart:async';
import 'dart:io';

import 'package:app/store/eh_daily_coordinator.dart';
import 'package:app/store/cloud115_bt_batch.dart' as bt_batch;
import 'package:app/store/cloud115_bt_daily_workflow.dart' as bt_daily;
import 'package:app/store/eh_subscription_store.dart';
import 'package:app/store/cloud115_session.dart';
import 'package:app/store/cloud115_bt_monitor.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/store/storage_access.dart';
import 'package:app/src/rust/source/cloud115/offline.dart' as cloud115_offline;
import 'package:app/ui/common.dart';
import 'package:file_selector/file_selector.dart';
import 'package:app/ui/rch_overlay.dart';
import 'package:flutter/material.dart';

typedef EhHandoffProcessRunner =
    Future<ProcessResult> Function(String executable, List<String> arguments);
typedef EhAndroidDirectoryOpener = Future<bool> Function(String directory);

enum _Missing115SourceChoice { addSource, saveSeedsOnly }

const _localTorrentOnlyValue = '__local_torrent_only__';

/// Opens only the configured output directory. Tests can inject the platform
/// and process boundary without starting Explorer.
Future<bool> openEhOutputDirectory(
  String directory, {
  bool? isWindows,
  bool? isAndroid,
  EhHandoffProcessRunner? runProcess,
  EhAndroidDirectoryOpener? openAndroidDirectory,
}) async {
  if (directory.trim().isEmpty) return false;
  if (isWindows ?? Platform.isWindows) {
    try {
      final runner =
          runProcess ??
          ((executable, arguments) => Process.run(executable, arguments));
      final result = await runner('explorer', [
        directory.replaceAll('/', r'\'),
      ]);
      return result.exitCode == 0;
    } catch (_) {
      return false;
    }
  }
  if (!(isAndroid ?? Platform.isAndroid)) return false;
  try {
    return await (openAndroidDirectory ?? openAndroidDirectoryPicker)(
      directory,
    );
  } catch (_) {
    return false;
  }
}

/// 设置页「EH 订阅与自动化」面板（可选插件）。
///
/// 边界：只写用户指定的保存目录（`.torrent` + `manifest.json`），
/// 不触碰书源、目录库与阅读数据；必须手动点击才会运行。
class EhSubscriptionPanel extends StatefulWidget {
  const EhSubscriptionPanel({
    super.key,
    this.storeOverride,
    this.btSourcesOverride,
    this.onAdd115Source,
    this.btFolderListRunner,
    this.btTargetSelectionOverride,
    this.btPreflightRunner,
    this.btUploadRunner,
    this.btUploadRecoveryRunner,
    this.btSubmitRunner,
    this.btTaskStatusRunner,
    this.btBatchRunner,
    this.btBatchLedgerOverride,
    this.btBatchPreferencesLoader,
    this.btBatchPreferencesSaver,
  });

  /// 仅测试使用：注入预置 store（避免触碰 FFI / 磁盘）。
  final EhSubscriptionStore? storeOverride;
  @visibleForTesting
  final List<BookSource>? btSourcesOverride;
  final Future<void> Function()? onAdd115Source;
  @visibleForTesting
  final Eh115BtFolderListRunner? btFolderListRunner;
  @visibleForTesting
  final cloud115_offline.Cloud115BtFolderSelection? btTargetSelectionOverride;
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
  @visibleForTesting
  final Eh115BtBatchRunner? btBatchRunner;
  @visibleForTesting
  final bt_batch.Cloud115BtBatchLedger? btBatchLedgerOverride;
  @visibleForTesting
  final Eh115BtBatchPreferencesLoader? btBatchPreferencesLoader;
  @visibleForTesting
  final Eh115BtBatchPreferencesSaver? btBatchPreferencesSaver;

  @override
  State<EhSubscriptionPanel> createState() => _EhSubscriptionPanelState();
}

class _EhSubscriptionPanelState extends State<EhSubscriptionPanel> {
  final _rulesKey = GlobalKey<_RulesEditorState>();
  final _pageCountKey = GlobalKey<_PageCountFieldState>();
  final _cookieBtKey = GlobalKey<_CookieBtPreflightBlockState>();
  bool _fullWorkflowBusy = false;
  String? _foregroundWorkflowProgress;

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

  Future<bt_batch.Cloud115BtBatchSelectionPreferences>
  _loadBatchSelectionPreferences() async {
    final loader = widget.btBatchPreferencesLoader;
    if (loader != null) return loader();
    final ledger =
        widget.btBatchLedgerOverride ??
        await bt_batch.Cloud115BtBatchLedger.inApplicationSupportDirectory();
    return ledger.selectionPreferences();
  }

  Future<void> _runFullWorkflowNow() async {
    final currentStore = store;
    if (_fullWorkflowBusy) return;
    if (!currentStore.hasOutDir) {
      _snack('请先在高级选项中选择 EH 种子保存目录', error: true);
      return;
    }
    if (currentStore.busy) {
      _snack('EH 收集任务正在运行，请等待或使用现有取消入口', error: true);
      return;
    }
    if (_cookieBtKey.currentState?.processing == true) {
      _snack('115 下载方式或批次设置正在保存，请稍后再运行', error: true);
      return;
    }

    bool runCloudStage;
    try {
      final preferences = await _loadBatchSelectionPreferences();
      runCloudStage =
          preferences.mode == bt_batch.Cloud115BtDeliveryMode.cloud115;
    } catch (error) {
      _snack('读取云下载方式失败：$error', error: true);
      return;
    }
    if (runCloudStage) {
      final sources = widget.btSourcesOverride ?? LibraryStore.instance.sources;
      final hasCookie115Source = sources.any(
        (source) =>
            source.is115 &&
            !source.remoteOnly &&
            (source.cookie ?? '').trim().isNotEmpty,
      );
      if (!hasCookie115Source) {
        final choice = await _showMissing115SourceChoices();
        if (!mounted) return;
        if (choice == _Missing115SourceChoice.addSource) {
          final onAdd115Source = widget.onAdd115Source;
          if (onAdd115Source == null) {
            _snack('请先在“书源”中添加 115 Cookie 书源', error: true);
          } else {
            await onAdd115Source();
          }
          return;
        }
        if (choice != _Missing115SourceChoice.saveSeedsOnly) return;
        runCloudStage = false;
      }
    }

    _pageCountKey.currentState?.flush();
    final batchState = runCloudStage ? _cookieBtKey.currentState : null;
    if (runCloudStage && batchState == null) {
      _snack('批次设置尚未加载，请展开高级选项后重试', error: true);
      return;
    }
    setState(() {
      _fullWorkflowBusy = true;
      _foregroundWorkflowProgress = '正在检查工作流设置';
    });
    try {
      if (runCloudStage) {
        final configuredBatch = batchState!;
        final configurationIssue = await configuredBatch
            .batchConfigurationIssue();
        if (!mounted) return;
        if (configurationIssue != null) {
          _snack(configurationIssue, error: true);
          return;
        }
        if (configuredBatch.processing) {
          _snack('115 批次正在处理，请等待当前批次结束', error: true);
          return;
        }
      }

      _rulesKey.currentState?.flush();
      setState(
        () => _foregroundWorkflowProgress = runCloudStage
            ? 'EH 订阅扫描进行中'
            : '仅运行到保存种子：EH 订阅扫描进行中',
      );
      final outcome = await currentStore.run(source: EhDailyRunSource.manual);
      if (!mounted) return;
      if (!outcome.started) {
        _snack(outcome.error ?? 'EH 检查未启动，请查看当前任务状态', error: true);
        return;
      }
      final scanCompleted = outcome.status == EhDailyRunStatus.completed;
      if (!scanCompleted) {
        setState(() => _foregroundWorkflowProgress = 'EH 扫描未完整结束，正在检查已保存种子');
      }
      if (runCloudStage) {
        await batchState!.refreshTorrentInventory();
      } else {
        await currentStore.refreshTorrentDirectory();
      }
      final diskInventory = currentStore.torrentDirectoryReport;
      if (diskInventory.status != EhManifestReadStatus.ready &&
          diskInventory.status != EhManifestReadStatus.empty) {
        await _recordManualWorkflowResult(
          currentStore,
          outcome,
          status: EhDailyRunStatus.partial,
          error:
              '${scanCompleted ? '' : '${outcome.error ?? 'EH 扫描未完整结束'}；'}'
              '种子保存目录未能读取或校验；未启动 115 批次。',
        );
        if (!mounted) return;
        _snack(
          '${scanCompleted ? '' : '${outcome.error ?? 'EH 扫描未完整结束'}；'}'
          '本地种子目录不可用；未启动 115 批次',
          error: true,
        );
        return;
      }

      if (!runCloudStage) {
        final verifiedCount = diskInventory.entries
            .where(
              (entry) =>
                  entry.fileStatus == EhSavedFileStatus.present &&
                  entry.validationStatus == EhTorrentValidationStatus.valid,
            )
            .length;
        const cloudStage = EhDailyCloudStageResult(
          status: EhDailyCloudStageStatus.notRun,
          message: '按选择仅运行到本地种子保存；本次未访问或写入 115。',
        );
        final partialError = <String>[
          if (!scanCompleted) outcome.error ?? 'EH 扫描未完整结束',
        ].join('；');
        final runStatus = scanCompleted
            ? EhDailyRunStatus.completed
            : EhDailyRunStatus.partial;
        await _recordManualWorkflowResult(
          currentStore,
          outcome,
          status: runStatus,
          error: partialError.isEmpty ? null : partialError,
          cloudStage: cloudStage,
        );
        if (mounted) {
          final summary =
              '${scanCompleted ? 'EH 检查完成' : 'EH 扫描未完整结束'}；'
              '本轮新增 ${outcome.saved} 个，目录中有 $verifiedCount 个已校验 Torrent；'
              '115 阶段未运行。';
          _snack(summary, error: !scanCompleted);
        }
        return;
      }

      final entries = batchState!.eligibleEntries;
      if (entries.isEmpty) {
        final noTorrentMessage = scanCompleted
            ? 'EH 检查完成，没有可提交的已校验 Torrent。'
            : '${outcome.error ?? 'EH 扫描未完整结束'}；已检查磁盘种子目录，没有可提交的已校验 Torrent。';
        await _recordManualWorkflowResult(
          currentStore,
          outcome,
          status: scanCompleted
              ? EhDailyRunStatus.completed
              : EhDailyRunStatus.partial,
          cloudStage: EhDailyCloudStageResult(
            status: EhDailyCloudStageStatus.completed,
            message: noTorrentMessage,
          ),
        );
        if (mounted) _snack(noTorrentMessage, error: !scanCompleted);
        return;
      }

      setState(
        () => _foregroundWorkflowProgress = scanCompleted
            ? 'EH 已完成，等待确认 115 批次'
            : 'EH 扫描未完整结束，已找到 ${entries.length} 个有效种子，等待确认处理',
      );
      final report = await batchState.runLatestEligibleBatch(
        confirmationNotice: scanCompleted
            ? null
            : '${outcome.error ?? 'EH 扫描未完整结束'}；将继续处理磁盘上已保存并通过校验的 Torrent。',
      );
      if (!mounted) return;
      if (report == null) {
        final issue = batchState.batchError;
        final message = [
          if (!scanCompleted) outcome.error ?? 'EH 扫描未完整结束',
          if (issue == null) '115 批次未确认，未执行云端写入' else '115 批次未执行：$issue',
        ].join('；');
        await _recordManualWorkflowResult(
          currentStore,
          outcome,
          status: EhDailyRunStatus.partial,
          error: message,
        );
        if (mounted) _snack(message, error: issue != null);
        return;
      }

      final cloudStage = bt_daily.Cloud115BtDailyWorkflow.summarizeBatchReport(
        report,
        entries.length,
      );
      final runStatus =
          scanCompleted &&
              cloudStage.status == EhDailyCloudStageStatus.completed
          ? EhDailyRunStatus.completed
          : EhDailyRunStatus.partial;
      final finalMessage = [
        if (!scanCompleted) outcome.error ?? 'EH 扫描未完整结束；本次仅处理了已保存并校验的种子',
        if (cloudStage.message?.trim().isNotEmpty == true)
          cloudStage.message!.trim(),
        if (cloudStage.message?.trim().isNotEmpty != true &&
            report.error != null)
          report.error!,
      ].join('；');
      await _recordManualWorkflowResult(
        currentStore,
        outcome,
        status: runStatus,
        error: runStatus == EhDailyRunStatus.completed ? null : finalMessage,
        cloudStage: cloudStage,
      );
      if (!mounted) return;
      _snack(
        finalMessage.isEmpty ? '完整工作流处理结束' : finalMessage,
        error: runStatus != EhDailyRunStatus.completed,
      );
    } catch (error) {
      if (mounted) _snack('完整工作流未完成：$error', error: true);
    } finally {
      if (mounted) {
        setState(() {
          _fullWorkflowBusy = false;
          _foregroundWorkflowProgress = null;
        });
      }
    }
  }

  Future<_Missing115SourceChoice?> _showMissing115SourceChoices() =>
      showRchDialog<_Missing115SourceChoice>(
        context: context,
        builder: (dialogContext) => AlertDialog(
          title: const Text('还没有可用的 115 Cookie 书源'),
          content: const Text(
            '添加 115 书源后可继续配置云端目录；也可以只运行 EH 订阅，把 Torrent 保存到本地。\n\n'
            '“只运行到保存种子”不会连接或写入 115。',
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(dialogContext).pop(),
              child: const Text('取消'),
            ),
            TextButton(
              onPressed: () => Navigator.of(
                dialogContext,
              ).pop(_Missing115SourceChoice.saveSeedsOnly),
              child: const Text('只运行到保存种子'),
            ),
            FilledButton(
              onPressed: () => Navigator.of(
                dialogContext,
              ).pop(_Missing115SourceChoice.addSource),
              child: const Text('添加115书源'),
            ),
          ],
        ),
      );

  void _onBatchProgressChanged(String? progress) {
    if (!mounted) return;
    setState(() => _foregroundWorkflowProgress = progress);
  }

  Future<void> _recordManualWorkflowResult(
    EhSubscriptionStore currentStore,
    EhRunOutcome outcome, {
    required EhDailyRunStatus status,
    String? error,
    EhDailyCloudStageResult cloudStage = const EhDailyCloudStageResult(),
  }) async {
    final previous = currentStore.dailyAutomation.lastRun;
    final EhDailyRunRecord run;
    if (previous != null && previous.source == EhDailyRunSource.manual) {
      run = previous.copyWith(
        status: status,
        saved: outcome.saved,
        error: error,
        cloudStage: cloudStage,
      );
    } else {
      run = EhDailyRunRecord(
        date: ehLocalDateKey(DateTime.now()),
        source: EhDailyRunSource.manual,
        status: status,
        saved: outcome.saved,
        error: error,
        cloudStage: cloudStage,
      );
    }
    try {
      await currentStore.updateDailyRun(run);
    } catch (error) {
      if (mounted) {
        _snack('工作流结果未能写入最近运行状态：$error', error: true);
      }
    }
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
      final message = Platform.isAndroid
          ? isAndroidSharedStoragePath(dir)
                ? '无法定位该 Android 保存目录；请确认目录仍存在且位于手机共享存储中。'
                : '该保存目录位于 Android 应用私有空间，系统文件浏览器无法打开；请在高级选项中改选共享存储目录。'
          : Platform.isWindows
          ? '无法打开保存目录'
          : '当前平台暂不支持打开保存目录';
      _snack(message, error: true);
    }
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
            title: 'EH 订阅与自动化',
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
            title: 'EH 订阅与自动化',
            subtitle: '正在加载规则…',
            children: [LinearProgressIndicator()],
          );
        }
        return _Section(
          title: 'EH 订阅与自动化',
          subtitle: '手动可随时运行 EH 检查与 115 批次；自动工作流仅在 Windows 应用运行期间执行。',
          trailing: _StatusPill(progress: store.progress),
          children: [
            _DailyAutomationBlock(
              store: store,
              coordinator: EhDailyCoordinator.maybeInstance,
              workflowBusy: _fullWorkflowBusy,
              workflowProgress: _foregroundWorkflowProgress,
              onRunNow: _runFullWorkflowNow,
              onStop: store.cancel,
              pageCountKey: _pageCountKey,
            ),
            const SizedBox(height: 12),
            Container(
              width: double.infinity,
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
              decoration: BoxDecoration(
                color: theme.colorScheme.surfaceContainerHighest.withValues(
                  alpha: 0.45,
                ),
                borderRadius: BorderRadius.circular(8),
              ),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Icon(
                    Icons.info_outline,
                    size: 18,
                    color: theme.colorScheme.primary,
                  ),
                  const SizedBox(width: 8),
                  const Expanded(
                    child: Text(
                      '非 115 云下载：在下方“高级选项”将“115 Cookie 书源”设为“非 115 云下载”。工作流只扫描、校验并保存 Torrent 到 EH 保存目录，不连接 115。',
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(height: 12),
            KeyedSubtree(
              key: const ValueKey('eh-seed-handoff'),
              child: _ManifestBlock(
                store: store,
                onOpenDir: _openOutDir,
                onRefresh: store.refreshManifest,
              ),
            ),
            const SizedBox(height: 12),
            ExpansionTile(
              key: const ValueKey('eh-advanced-options'),
              maintainState: true,
              title: const Text('高级选项'),
              subtitle: const Text('筛选规则、保存目录、115 目标与清理策略、手动批次'),
              tilePadding: EdgeInsets.zero,
              childrenPadding: const EdgeInsets.fromLTRB(12, 0, 12, 12),
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
                _CookieBtPreflightBlock(
                  key: _cookieBtKey,
                  store: store,
                  sourcesOverride: widget.btSourcesOverride,
                  folderListRunner: widget.btFolderListRunner,
                  targetSelectionOverride: widget.btTargetSelectionOverride,
                  preflightRunner: widget.btPreflightRunner,
                  uploadRunner: widget.btUploadRunner,
                  uploadRecoveryRunner: widget.btUploadRecoveryRunner,
                  submitRunner: widget.btSubmitRunner,
                  taskStatusRunner: widget.btTaskStatusRunner,
                  batchRunner: widget.btBatchRunner,
                  batchLedgerOverride: widget.btBatchLedgerOverride,
                  batchPreferencesLoader: widget.btBatchPreferencesLoader,
                  batchPreferencesSaver: widget.btBatchPreferencesSaver,
                  onBatchProgressChanged: _onBatchProgressChanged,
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
                      _pageCountKey.currentState?.reloadFromStore();
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
      cloud115_offline.Cloud115BtFolderSelection target,
    );

typedef Eh115BtFolderListRunner =
    Future<List<cloud115_offline.Cloud115BtFolder>> Function(
      BookSource source,
      String parentId,
    );

typedef Eh115BtUploadRunner =
    Future<cloud115_offline.Cloud115BtUploadParseResult> Function(
      BookSource source,
      String outputDir,
      String relativeFile,
      String infoHash,
      cloud115_offline.Cloud115BtFolderSelection target,
    );

typedef Eh115BtUploadRecoveryRunner =
    Future<cloud115_offline.Cloud115BtUploadParseResult> Function(
      BookSource source,
      String outputDir,
      String relativeFile,
      String infoHash,
      cloud115_offline.Cloud115BtFolderSelection target,
      String? pickCode,
    );

typedef Eh115BtSubmitRunner =
    Future<cloud115_offline.Cloud115BtSubmitResult> Function(
      BookSource source,
      String outputDir,
      String relativeFile,
      String infoHash,
      cloud115_offline.Cloud115BtFolderSelection target,
      List<int> wantedIndices,
    );

typedef Eh115BtTaskStatusRunner =
    Future<cloud115_offline.Cloud115BtTaskStatus> Function(
      BookSource source,
      String infoHash,
      cloud115_offline.Cloud115BtFolderSelection target,
    );

typedef Eh115BtBatchRunner =
    Future<bt_batch.Cloud115BtBatchReport> Function(
      BookSource source,
      List<EhCheckedManifestEntry> entries,
      cloud115_offline.Cloud115BtFolderSelection target,
      bool cleanupAfterSubmit,
    );

typedef Eh115BtBatchPreferencesLoader =
    Future<bt_batch.Cloud115BtBatchSelectionPreferences> Function();

typedef Eh115BtBatchPreferencesSaver =
    Future<void> Function(
      String? sourceId,
      cloud115_offline.Cloud115BtFolderSelection? target,
      bt_batch.Cloud115BtDeliveryMode mode,
    );

/// Experimental one-item Cookie-mode preflight. The card is intentionally
/// inert until the user presses the read-only preflight button.
class _CookieBtPreflightBlock extends StatefulWidget {
  const _CookieBtPreflightBlock({
    super.key,
    required this.store,
    this.sourcesOverride,
    this.folderListRunner,
    this.targetSelectionOverride,
    this.preflightRunner,
    this.uploadRunner,
    this.uploadRecoveryRunner,
    this.submitRunner,
    this.taskStatusRunner,
    this.batchRunner,
    this.batchLedgerOverride,
    this.batchPreferencesLoader,
    this.batchPreferencesSaver,
    this.onBatchProgressChanged,
  });

  final EhSubscriptionStore store;
  @visibleForTesting
  final List<BookSource>? sourcesOverride;
  final Eh115BtFolderListRunner? folderListRunner;
  final cloud115_offline.Cloud115BtFolderSelection? targetSelectionOverride;
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
  final Eh115BtBatchRunner? batchRunner;
  final bt_batch.Cloud115BtBatchLedger? batchLedgerOverride;
  final Eh115BtBatchPreferencesLoader? batchPreferencesLoader;
  final Eh115BtBatchPreferencesSaver? batchPreferencesSaver;
  final ValueChanged<String?>? onBatchProgressChanged;

  @override
  State<_CookieBtPreflightBlock> createState() =>
      _CookieBtPreflightBlockState();
}

class _CookieBtPreflightBlockState extends State<_CookieBtPreflightBlock> {
  String? _selectedSourceId;
  bt_batch.Cloud115BtDeliveryMode _deliveryMode =
      bt_batch.Cloud115BtDeliveryMode.cloud115;
  cloud115_offline.Cloud115BtFolderSelection? _targetFolder;
  bool _busy = false;
  bool _confirmingWrite = false;
  String? _error;
  cloud115_offline.Cloud115BtPreflight? _result;
  cloud115_offline.Cloud115BtTaskStatus? _taskStatus;
  bool _batchBusy = false;
  bool _cleanupAfterSubmit = false;
  bool _cleanupPreferenceLoaded = false;
  bool _preferenceSaving = false;
  Future<void>? _batchPreferencesLoad;
  bt_batch.Cloud115BtBatchReport? _batchReport;
  String? _batchProgress;
  String? _batchError;
  bt_batch.Cloud115BtBatchLedger? _batchLedger;
  String? _observedTorrentDirectory;

  @override
  void initState() {
    super.initState();
    _targetFolder = widget.targetSelectionOverride;
    _attachTorrentDirectoryStore(widget.store);
    if (widget.store.hasOutDir && widget.store.autoRefreshTorrentDirectory) {
      unawaited(widget.store.refreshTorrentDirectory());
    }
    if (widget.batchRunner != null &&
        widget.batchLedgerOverride == null &&
        widget.batchPreferencesLoader == null) {
      _cleanupPreferenceLoaded = true;
      _batchPreferencesLoad = Future<void>.value();
      return;
    }
    _batchPreferencesLoad = _loadBatchCleanupPreference();
    unawaited(_batchPreferencesLoad!);
  }

  @override
  void didUpdateWidget(covariant _CookieBtPreflightBlock oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.store != widget.store) {
      oldWidget.store.removeListener(_onTorrentDirectoryStoreChanged);
      _attachTorrentDirectoryStore(widget.store);
      if (widget.store.hasOutDir && widget.store.autoRefreshTorrentDirectory) {
        unawaited(widget.store.refreshTorrentDirectory());
      }
    }
  }

  @override
  void dispose() {
    widget.store.removeListener(_onTorrentDirectoryStoreChanged);
    super.dispose();
  }

  void _attachTorrentDirectoryStore(EhSubscriptionStore store) {
    _observedTorrentDirectory = store.outDir;
    store.addListener(_onTorrentDirectoryStoreChanged);
  }

  void _onTorrentDirectoryStoreChanged() {
    final directory = widget.store.outDir;
    if (directory == _observedTorrentDirectory) return;
    _observedTorrentDirectory = directory;
    if (widget.store.hasOutDir && widget.store.autoRefreshTorrentDirectory) {
      unawaited(widget.store.refreshTorrentDirectory());
    }
  }

  Future<void> refreshTorrentInventory() =>
      widget.store.refreshTorrentDirectory();

  Future<bt_batch.Cloud115BtBatchLedger> _getBatchLedger() async =>
      _batchLedger ??=
          widget.batchLedgerOverride ??
          await bt_batch.Cloud115BtBatchLedger.inApplicationSupportDirectory();

  Future<void> _loadBatchCleanupPreference() async {
    try {
      final preferencesLoader = widget.batchPreferencesLoader;
      final enabled = preferencesLoader == null
          ? await (await _getBatchLedger()).cleanupAfterSubmit()
          : false;
      final preferences = preferencesLoader == null
          ? await (await _getBatchLedger()).selectionPreferences()
          : await preferencesLoader();
      final savedSourceId =
          preferences.sourceId != null &&
              _cookieSources.any((source) => source.id == preferences.sourceId)
          ? preferences.sourceId
          : null;
      if (!mounted) return;
      setState(() {
        _cleanupAfterSubmit = enabled;
        _selectedSourceId = savedSourceId;
        _deliveryMode = preferences.mode;
        _targetFolder =
            widget.targetSelectionOverride ??
            (savedSourceId == null ? null : preferences.target);
        _cleanupPreferenceLoaded = true;
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _cleanupPreferenceLoaded = true;
        _batchError = _safeBtError(error);
      });
    }
  }

  Future<void> _setCleanupAfterSubmit(bool enabled) async {
    if (_preferenceSaving || _batchBusy) return;
    setState(() {
      _preferenceSaving = true;
      _batchError = null;
    });
    try {
      await (await _getBatchLedger()).setCleanupAfterSubmit(enabled);
      if (!mounted) return;
      setState(() => _cleanupAfterSubmit = enabled);
    } catch (error) {
      if (!mounted) return;
      setState(() => _batchError = _safeBtError(error));
    } finally {
      if (mounted) setState(() => _preferenceSaving = false);
    }
  }

  Future<bool> _saveSelectionPreferences({
    required String? sourceId,
    required cloud115_offline.Cloud115BtFolderSelection? target,
    bt_batch.Cloud115BtDeliveryMode? mode,
  }) async {
    if (_preferenceSaving || _batchBusy) return false;
    setState(() {
      _preferenceSaving = true;
      _batchError = null;
    });
    var saved = false;
    try {
      final saver = widget.batchPreferencesSaver;
      final selectedMode = mode ?? _deliveryMode;
      if (saver != null) {
        await saver(sourceId, target, selectedMode);
      } else {
        await (await _getBatchLedger()).setSelectionPreferences(
          sourceId: sourceId,
          target: target,
          mode: selectedMode,
        );
      }
      saved = true;
    } catch (error) {
      if (mounted) setState(() => _batchError = _safeBtError(error));
    } finally {
      if (mounted) setState(() => _preferenceSaving = false);
    }
    return saved;
  }

  List<EhCheckedManifestEntry> get _eligibleEntries => widget
      .store
      .torrentDirectoryReport
      .entries
      .where(
        (entry) =>
            entry.fileStatus == EhSavedFileStatus.present &&
            entry.validationStatus == EhTorrentValidationStatus.valid &&
            entry.item.file.isNotEmpty,
      )
      .toList()
      .reversed
      .toList();

  List<EhCheckedManifestEntry> get eligibleEntries => _eligibleEntries;

  bool get processing =>
      _busy || _batchBusy || _confirmingWrite || _preferenceSaving;

  String? get batchError => _batchError;

  Future<void> _ensureBatchPreferencesLoaded() async {
    if (_cleanupPreferenceLoaded) return;
    await (_batchPreferencesLoad ??= _loadBatchCleanupPreference());
  }

  Future<String?> batchConfigurationIssue() async {
    await _ensureBatchPreferencesLoaded();
    if (!_cleanupPreferenceLoaded) return '115 批次设置尚未加载完成';
    if (_sourceFor(_cookieSources) == null) {
      return '请在高级选项中选择一个已登录的 115 Cookie 书源';
    }
    if (_targetFolder == null) {
      return '请在高级选项中选择并保存 115 目标文件夹';
    }
    if (processing) return '115 批次正在处理或设置正在保存';
    return null;
  }

  Future<bt_batch.Cloud115BtBatchReport?> runLatestEligibleBatch({
    String? confirmationNotice,
  }) async {
    await _ensureBatchPreferencesLoaded();
    if (_deliveryMode != bt_batch.Cloud115BtDeliveryMode.cloud115) return null;
    final source = _sourceFor(_cookieSources);
    final target = _targetFolder;
    final entries = _eligibleEntries;
    if (source == null || target == null || entries.isEmpty) return null;
    return _runBatch(
      source,
      entries,
      target,
      confirmationNotice: confirmationNotice,
    );
  }

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
    return entries.isEmpty ? null : entries.first;
  }

  BookSource? _sourceFor(List<BookSource> sources) {
    if (sources.isEmpty) return null;
    for (final source in sources) {
      if (source.id == _selectedSourceId) return source;
    }
    return sources.first;
  }

  Future<void> _selectDeliveryOption(String? value) async {
    if (value == null || _preferenceSaving || _batchBusy) return;
    final oldMode = _deliveryMode;
    final oldSourceId = _selectedSourceId;
    final oldTarget = _targetFolder;
    if (value == _localTorrentOnlyValue) {
      if (_deliveryMode == bt_batch.Cloud115BtDeliveryMode.localTorrentOnly) {
        return;
      }
      setState(() {
        _deliveryMode = bt_batch.Cloud115BtDeliveryMode.localTorrentOnly;
        _resetOperationForSelection();
      });
      final saved = await _saveSelectionPreferences(
        sourceId: _selectedSourceId,
        target: _targetFolder,
        mode: bt_batch.Cloud115BtDeliveryMode.localTorrentOnly,
      );
      if (!saved && mounted) {
        setState(() {
          _deliveryMode = oldMode;
          _selectedSourceId = oldSourceId;
          _targetFolder = oldTarget;
        });
      }
      return;
    }

    final sameSource = value == _selectedSourceId;
    setState(() {
      _deliveryMode = bt_batch.Cloud115BtDeliveryMode.cloud115;
      _selectedSourceId = value;
      if (!sameSource) _targetFolder = null;
      _resetOperationForSelection();
    });
    final saved = await _saveSelectionPreferences(
      sourceId: value,
      target: sameSource ? _targetFolder : null,
      mode: bt_batch.Cloud115BtDeliveryMode.cloud115,
    );
    if (!saved && mounted) {
      setState(() {
        _deliveryMode = oldMode;
        _selectedSourceId = oldSourceId;
        _targetFolder = oldTarget;
      });
    }
  }

  void _resetOperationForSelection() {
    _error = null;
    _result = null;
    _taskStatus = null;
    _batchReport = null;
    _batchProgress = null;
    _batchError = null;
  }

  Future<List<cloud115_offline.Cloud115BtFolder>> _listTargetFolders(
    BookSource source,
    String parentId,
  ) {
    final runner = widget.folderListRunner;
    return runner == null
        ? cloud115CookieBtListChildFoldersFor(source, parentId: parentId)
        : runner(source, parentId);
  }

  Future<void> _chooseTargetFolder(BookSource source) async {
    if (_busy || _batchBusy || _confirmingWrite) {
      return;
    }
    final selection =
        await showRchDialog<cloud115_offline.Cloud115BtFolderSelection>(
          context: context,
          builder: (context) => _BtFolderPickerDialog(
            source: source,
            loadChildren: _listTargetFolders,
          ),
        );
    if (!mounted || selection == null) return;
    setState(() {
      _targetFolder = selection;
      _resetOperationForSelection();
    });
    await _saveSelectionPreferences(sourceId: source.id, target: selection);
  }

  Future<void> _preflight(
    BookSource source,
    EhCheckedManifestEntry entry,
  ) async {
    final target = _targetFolder;
    if (target == null || _busy || _batchBusy || _confirmingWrite) {
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
      _result = null;
      _taskStatus = null;
    });
    try {
      final runner = widget.preflightRunner;
      final result = runner == null
          ? await cloud115CookieBtReadonlyPreflightFor(
              source,
              outputDir: widget.store.outDir,
              relativeFile: entry.item.file,
              expectedInfoHash: entry.item.infohash,
              target: target,
            )
          : await runner(
              source,
              widget.store.outDir,
              entry.item.file,
              entry.item.infohash,
              target,
            );
      if (!mounted) return;
      if (result.infoHash.toLowerCase() != entry.item.infohash.toLowerCase() ||
          result.targetFolderPath != target.path ||
          result.targetFolderId != target.id ||
          !RegExp(r'^\d+$').hasMatch(result.targetFolderId)) {
        throw StateError('115 预检结果与当前 Torrent 或目标目录不一致');
      }
      setState(() {
        _result = result;
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
    if (_busy || _batchBusy || _confirmingWrite) return false;
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

  Future<void> _queryTask(
    BookSource source,
    cloud115_offline.Cloud115BtPreflight preflight,
  ) async {
    final target = _targetFolder;
    if (_busy || _confirmingWrite || target == null) return;
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
              target: target,
            )
          : await runner(source, preflight.infoHash, target);
      if (!mounted) return;
      if (status.infoHash.toLowerCase() != preflight.infoHash.toLowerCase() ||
          status.targetFolderId != preflight.targetFolderId) {
        throw StateError('115 任务状态与当前 Torrent 或目标目录不一致');
      }
      setState(() {
        _taskStatus = status;
      });
    } catch (error) {
      if (!mounted) return;
      setState(() => _error = _safeBtError(error));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<bt_batch.Cloud115BtBatchReport> _processBatch(
    BookSource source,
    List<EhCheckedManifestEntry> entries,
    cloud115_offline.Cloud115BtFolderSelection target,
    bool cleanupAfterSubmit, {
    bt_batch.Cloud115BtBatchProgress? onProgress,
  }) async {
    final injectedRunner = widget.batchRunner;
    if (injectedRunner != null) {
      return injectedRunner(source, entries, target, cleanupAfterSubmit);
    }
    final processor = bt_batch.Cloud115BtBatchProcessor(
      ledger: await _getBatchLedger(),
    );
    return processor.run(
      entries: entries,
      target: target,
      sourceId: source.id,
      cleanupAfterSubmit: cleanupAfterSubmit,
      onProgress: onProgress,
      preflight: (entry) {
        final runner = widget.preflightRunner;
        return runner == null
            ? cloud115CookieBtReadonlyPreflightFor(
                source,
                outputDir: widget.store.outDir,
                relativeFile: entry.item.file,
                expectedInfoHash: entry.item.infohash,
                target: target,
              )
            : runner(
                source,
                widget.store.outDir,
                entry.item.file,
                entry.item.infohash,
                target,
              );
      },
      upload: (entry) {
        final runner = widget.uploadRunner;
        return runner == null
            ? cloud115CookieBtUploadAndParseFor(
                source,
                outputDir: widget.store.outDir,
                relativeFile: entry.item.file,
                expectedInfoHash: entry.item.infohash,
                target: target,
              )
            : runner(
                source,
                widget.store.outDir,
                entry.item.file,
                entry.item.infohash,
                target,
              );
      },
      recoverUpload: (entry, pickCode) {
        final runner = widget.uploadRecoveryRunner;
        return runner == null
            ? cloud115CookieBtConfirmUploadParseFor(
                source,
                outputDir: widget.store.outDir,
                relativeFile: entry.item.file,
                expectedInfoHash: entry.item.infohash,
                target: target,
                pickCode: pickCode,
              )
            : runner(
                source,
                widget.store.outDir,
                entry.item.file,
                entry.item.infohash,
                target,
                pickCode,
              );
      },
      submit: (entry, wanted) {
        final runner = widget.submitRunner;
        return runner == null
            ? cloud115CookieBtSubmitOnceFor(
                source,
                outputDir: widget.store.outDir,
                relativeFile: entry.item.file,
                expectedInfoHash: entry.item.infohash,
                target: target,
                wantedIndices: wanted,
              )
            : runner(
                source,
                widget.store.outDir,
                entry.item.file,
                entry.item.infohash,
                target,
                wanted,
              );
      },
      queryTask: (infoHash) {
        final runner = widget.taskStatusRunner;
        return runner == null
            ? cloud115CookieBtTaskStatusFor(
                source,
                infoHash: infoHash,
                target: target,
              )
            : runner(source, infoHash, target);
      },
      deleteTorrent: (entry, accountId, selectedTarget) =>
          cloud115CookieBtDeleteLocalIfRemoteTaskFoundFor(
            source,
            outputDir: widget.store.outDir,
            relativeFile: entry.item.file,
            expectedInfoHash: entry.item.infohash,
            expectedAccountId: accountId,
            target: selectedTarget,
          ),
    );
  }

  Future<bt_batch.Cloud115BtBatchReport?> _runBatch(
    BookSource source,
    List<EhCheckedManifestEntry> entries,
    cloud115_offline.Cloud115BtFolderSelection target, {
    String? confirmationNotice,
  }) async {
    if (_busy ||
        _batchBusy ||
        _confirmingWrite ||
        _preferenceSaving ||
        entries.isEmpty) {
      return null;
    }
    setState(() => _batchError = null);
    final confirmed = await _confirmWrite(
      title: '确认批量处理 ${entries.length} 个 Torrent',
      message: [
        if (confirmationNotice?.trim().isNotEmpty == true)
          confirmationNotice!.trim(),
        '账号书源：${source.name}\n'
            '目标目录：${target.path}（ID ${target.id}）\n'
            '候选数量：${entries.length}\n\n'
            '批次先逐项校验本地种子、账号和目标目录；不在提交前扫描完整离线任务列表。用户确认后，每项 Torrent 上传、解析并向 115 提交一次；已有记录交由 115 在提交时处理。单项请求结果未知时会记为待核对、保留种子且不重发，然后继续下一项；账号级鉴权失败、限流或 WAF 阻断会停止整批。接口接受请求不代表云端下载已完成。\n\n'
            'Cookie Web / SSP 写入路线的官方授权及当前线上兼容性尚未由 RCH 实测确认；此确认将授权本批次逐项执行，只对你有权操作的账号和内容继续。\n\n'
            '${_cleanupAfterSubmit ? '仅在只读核实到远端同哈希任务后按设置删除本地 .torrent；无法核实时保留。' : '保留本地 .torrent。'}'
            '此操作不会删除漫画文件、清单或其他本地文件。',
      ].join('\n\n'),
      confirmLabel: '确认开始批次',
    );
    if (!confirmed || !mounted) return null;

    setState(() {
      _busy = true;
      _batchBusy = true;
      _batchProgress = '准备处理 ${entries.length} 项';
      _batchReport = null;
      _batchError = null;
    });
    widget.onBatchProgressChanged?.call(_batchProgress);
    try {
      final report = await _processBatch(
        source,
        List.unmodifiable(entries),
        target,
        _cleanupAfterSubmit,
        onProgress: (current, total, relativeFile, stage) {
          if (!mounted) return;
          setState(() {
            final file = relativeFile.isEmpty ? '' : ' · $relativeFile';
            _batchProgress =
                '$current/$total · ${_batchProgressLabel(stage)}$file';
          });
          widget.onBatchProgressChanged?.call(_batchProgress);
        },
      );
      if (!mounted) return null;
      setState(() {
        _batchReport = report;
        _batchError = report.error;
        _batchProgress = report.stopped
            ? '批次已停止：${report.error ?? ''}'
            : '批次处理完成';
      });
      widget.onBatchProgressChanged?.call(_batchProgress);
      if (Cloud115BtMonitor.instance.isStarted) {
        await Cloud115BtMonitor.instance.reconcileFromLedger();
        await Cloud115BtMonitor.instance.pollDueOnce();
      }
      if (widget.batchRunner == null) {
        await widget.store.refreshTorrentDirectory();
        await widget.store.refreshManifest();
      }
      return report;
    } catch (error) {
      if (!mounted) return null;
      setState(() {
        _batchError = _safeBtError(error);
        _batchProgress = '批次失败：$_batchError';
      });
      widget.onBatchProgressChanged?.call(_batchProgress);
      return null;
    } finally {
      if (mounted) {
        setState(() {
          _batchBusy = false;
          _busy = false;
        });
      }
    }
  }

  String _batchStateLabel(String state) => switch (state) {
    'submitted' => '115 已接受提交请求',
    'local_torrent_removed' => '远端同哈希任务存在，已删除本地种子',
    'duplicate_found_delete_failed' => '远端任务存在，本地种子删除失败',
    'duplicate_lookup_unknown' => '本项远端查重未确认，已保留种子并继续其他项',
    'unknown_remote_state' => '本项提交结果未知，禁止重试；已继续其他种子',
    'upload_outcome_unknown' => '本项上传结果未知，禁止重传；已继续其他种子',
    'upload_parse_pending' => '本项已上传、解析待恢复；保留种子用于恢复',
    'waf_blocked' => '115 风控拦截，批次停止',
    'rate_limited' => '115 限流，批次停止',
    'auth_expired' => '115 登录失效，批次停止',
    'access_denied' => '115 权限拒绝，批次停止',
    'challenge_required' => '115 要求人工验证，批次停止',
    'rejected' => '115 拒绝本项，已继续其他种子',
    'preflight_failed' => '远端预检失败',
    'local_validation_failed' => '本地校验失败，已跳过',
    'account_changed' => '账号变化，批次停止',
    'torrent_invalid' => 'Torrent 清单无效',
    _ => state,
  };

  String _batchProgressLabel(String stage) => switch (stage) {
    'preflight' => '检查本地种子、账号和目标目录',
    'duplicate' => '核实远端任务并按设置清理种子',
    'reconciling' => '只读核对此前提交结果',
    'recovering_upload' => '只读核对 Torrent 上传',
    'uploading' => '上传并解析 Torrent',
    'submitting' => '提交 BT 任务',
    'completed' => '批次完成',
    _ => stage,
  };

  @override
  Widget build(BuildContext context) {
    final entries = _eligibleEntries;
    final sources = _cookieSources;
    final selectedEntry = _entryFor(entries);
    final selectedSource = _sourceFor(sources);
    final targetFolder = _targetFolder;
    final theme = Theme.of(context);
    final preflight = _result;
    final files = preflight?.files ?? const <cloud115_offline.Cloud115BtFile>[];
    final canProcessSelected =
        !_busy &&
        !_batchBusy &&
        !_confirmingWrite &&
        !_preferenceSaving &&
        _cleanupPreferenceLoaded &&
        preflight?.writeGate == 'REQUIRES_EXPLICIT_CONFIRMATION' &&
        selectedEntry != null &&
        selectedSource != null &&
        targetFolder != null;
    final busyLabel = _batchBusy ? '批量处理中…' : '正在只读核对…';
    return Card(
      margin: EdgeInsets.zero,
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              _deliveryMode == bt_batch.Cloud115BtDeliveryMode.localTorrentOnly
                  ? '下载方式'
                  : '115 云端下载',
              style: theme.textTheme.titleSmall,
            ),
            DropdownButtonFormField<String>(
              key: ValueKey(
                'eh115-bt-source-${_deliveryMode.name}-${selectedSource?.id ?? "none"}',
              ),
              initialValue:
                  _deliveryMode ==
                      bt_batch.Cloud115BtDeliveryMode.localTorrentOnly
                  ? _localTorrentOnlyValue
                  : selectedSource?.id,
              isExpanded: true,
              decoration: const InputDecoration(
                labelText: '115 Cookie 书源',
                border: OutlineInputBorder(),
                isDense: true,
              ),
              items: [
                const DropdownMenuItem(
                  value: _localTorrentOnlyValue,
                  child: Text('非 115 云下载'),
                ),
                for (final source in sources)
                  DropdownMenuItem(
                    value: source.id,
                    child: Text(source.name, overflow: TextOverflow.ellipsis),
                  ),
              ],
              onChanged:
                  !_cleanupPreferenceLoaded ||
                      _preferenceSaving ||
                      widget.store.busy ||
                      _busy ||
                      _batchBusy ||
                      _confirmingWrite
                  ? null
                  : _selectDeliveryOption,
            ),
            const SizedBox(height: 4),
            Text(
              '下载方式、115 Cookie 书源和目标目录保存在本机；Cookie 本身不保存。',
              style: theme.textTheme.bodySmall,
            ),
            if (_deliveryMode ==
                bt_batch.Cloud115BtDeliveryMode.localTorrentOnly)
              Padding(
                padding: const EdgeInsets.only(top: 10),
                child: Text(
                  '当前模式只执行 EH 扫描、本地校验并保存 Torrent；不会向 115 发请求、查询目录、上传文件、创建 BT 任务或清理本地种子。',
                  style: theme.textTheme.bodyMedium,
                ),
              )
            else ...[
              const SizedBox(height: 10),
              Row(
                children: [
                  Expanded(
                    child: Text(
                      '保存目录实存 Torrent：${entries.length} 个有效，'
                      '${widget.store.torrentDirectoryReport.entries.length - entries.length} 个未通过校验。批次以此目录为准，不依赖 manifest。',
                      style: theme.textTheme.bodySmall,
                    ),
                  ),
                  IconButton(
                    key: const ValueKey('eh115-bt-refresh-directory'),
                    tooltip: '重新检查保存目录',
                    onPressed:
                        _batchBusy || widget.store.refreshingTorrentDirectory
                        ? null
                        : refreshTorrentInventory,
                    icon: widget.store.refreshingTorrentDirectory
                        ? const SizedBox(
                            width: 18,
                            height: 18,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : const Icon(Icons.refresh),
                  ),
                ],
              ),
              ExpansionTile(
                tilePadding: EdgeInsets.zero,
                childrenPadding: const EdgeInsets.only(bottom: 8),
                title: const Text('115 写入与安全说明'),
                subtitle: const Text('每批需确认；不确定的结果只读核对，不自动重发'),
                children: const [
                  Text(
                    '此入口复用已有 Cookie 会话，不新建登录、不扫码、不刷新 Cookie，也不使用 Open API。手动批次开始前需要确认；Daily 自动处理由上方开关控制。每项写入使用持久化幂等账本，结果不确定时先只读核对，不会自动重发。',
                  ),
                ],
              ),
              const SizedBox(height: 10),
              Row(
                crossAxisAlignment: CrossAxisAlignment.center,
                children: [
                  Icon(
                    Icons.folder_outlined,
                    color: theme.colorScheme.onSurfaceVariant,
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text('115 保存目录', style: theme.textTheme.labelMedium),
                        Text(
                          targetFolder == null
                              ? '未选择；打开目录选择器浏览已有文件夹'
                              : '${targetFolder.path}（ID ${targetFolder.id}）',
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(width: 8),
                  OutlinedButton.icon(
                    onPressed:
                        selectedSource == null ||
                            _busy ||
                            _confirmingWrite ||
                            _batchBusy ||
                            !_cleanupPreferenceLoaded ||
                            _preferenceSaving
                        ? null
                        : () => _chooseTargetFolder(selectedSource),
                    icon: const Icon(Icons.folder_open_outlined),
                    label: const Text('选择目录'),
                  ),
                ],
              ),
              if (targetFolder == null)
                Padding(
                  padding: const EdgeInsets.only(top: 6),
                  child: Text(
                    '从当前账号的已有目录中选择；RCH 不会自动创建云端文件夹。',
                    style: theme.textTheme.bodySmall,
                  ),
                ),
              SwitchListTile.adaptive(
                contentPadding: EdgeInsets.zero,
                value: _cleanupAfterSubmit,
                title: const Text('确认远端任务后删除本地 .torrent'),
                subtitle: const Text(
                  '默认关闭；只删除已校验的种子文件。115 已接受请求不代表任务已核实；无法只读确认时保留种子。',
                ),
                onChanged:
                    !_cleanupPreferenceLoaded ||
                        _preferenceSaving ||
                        _busy ||
                        _confirmingWrite
                    ? null
                    : _setCleanupAfterSubmit,
              ),
              Align(
                alignment: Alignment.centerLeft,
                child: FilledButton.icon(
                  key: const ValueKey('eh115-bt-batch-run'),
                  onPressed:
                      _busy ||
                          _confirmingWrite ||
                          _preferenceSaving ||
                          !_cleanupPreferenceLoaded ||
                          entries.isEmpty ||
                          selectedSource == null ||
                          targetFolder == null
                      ? null
                      : () async {
                          await _runBatch(
                            selectedSource,
                            entries,
                            targetFolder,
                          );
                        },
                  icon: _batchBusy
                      ? const SizedBox(
                          width: 16,
                          height: 16,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                      : const Icon(Icons.playlist_add_check_circle_outlined),
                  label: Text(
                    _batchBusy ? '批次处理中…' : '批量上传并提交 BT 任务（${entries.length}）',
                  ),
                ),
              ),
              if (_batchBusy && _batchProgress != null)
                Padding(
                  padding: const EdgeInsets.only(top: 6),
                  child: Text(
                    _batchProgress!,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.bodySmall,
                  ),
                ),
              AnimatedBuilder(
                animation: Cloud115BtMonitor.instance,
                builder: (context, _) {
                  final monitor = Cloud115BtMonitor.instance;
                  final summary = monitor.summaryValue;
                  if (summary.trackedCount == 0 &&
                      monitor.lastLocalError == null) {
                    return const SizedBox.shrink();
                  }
                  final next = summary.nextCheckAt;
                  final nextText = next == null
                      ? '等待可用的现有 Cookie 会话'
                      : '下次只读核对：${next.toLocal().toString().substring(0, 16)}';
                  return Padding(
                    padding: const EdgeInsets.only(top: 8),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          monitor.lastLocalError == null
                              ? '115 任务监视：关联 ${summary.trackedCount} 项 · '
                                    '任务列表发现 ${summary.foundCount} 项 · '
                                    '状态待确认 ${summary.unknownCount} 项'
                              : '115 任务监视暂不可用：${monitor.lastLocalError}',
                          style: theme.textTheme.bodySmall,
                        ),
                        if (monitor.lastLocalError == null)
                          Text(
                            '$nextText。目标目录和下载完成状态未经验证；监视不会上传、提交或删种子。',
                            style: theme.textTheme.bodySmall,
                          ),
                      ],
                    ),
                  );
                },
              ),
              if (_batchError != null)
                Padding(
                  padding: const EdgeInsets.only(top: 8),
                  child: Text(
                    _batchError!,
                    style: TextStyle(color: theme.colorScheme.error),
                  ),
                ),
              if (_batchReport case final report?) ...[
                const SizedBox(height: 8),
                ExpansionTile(
                  tilePadding: EdgeInsets.zero,
                  initiallyExpanded: report.stopped,
                  title: Text('批次结果：${report.results.length} 项'),
                  subtitle: Text(
                    report.stopped
                        ? '已停止；未完成项保留本地种子'
                        : report.hasPendingRecovery
                        ? '本轮已遍历；未确认项保留本地 Torrent，后续只读恢复且不重传'
                        : report.results.any(
                            (result) =>
                                result.state == 'rejected' ||
                                result.state == 'torrent_invalid' ||
                                result.state == 'local_validation_failed',
                          )
                        ? '已遍历全部种子；有拒绝或无效项，请查看明细'
                        : '批次处理完成',
                  ),
                  children: [
                    for (final result in report.results)
                      ListTile(
                        dense: true,
                        leading: Icon(
                          result.state == 'submitted' ||
                                  result.state == 'local_torrent_removed'
                              ? Icons.check_circle_outline
                              : Icons.info_outline,
                        ),
                        title: Text(
                          result.relativeFile.isEmpty
                              ? result.infoHash
                              : result.relativeFile,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                        ),
                        subtitle: Text(
                          '${_batchStateLabel(result.state)}'
                          '${result.message.isEmpty ? '' : ' · ${result.message}'}',
                          maxLines: 4,
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                  ],
                ),
              ],
              const SizedBox(height: 10),
              Align(
                alignment: Alignment.centerLeft,
                child: FilledButton.icon(
                  onPressed:
                      _busy ||
                          _confirmingWrite ||
                          _batchBusy ||
                          selectedEntry == null ||
                          selectedSource == null ||
                          targetFolder == null
                      ? null
                      : () => _preflight(selectedSource, selectedEntry),
                  icon: _busy
                      ? const SizedBox(
                          width: 16,
                          height: 16,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                      : const Icon(Icons.fact_check_outlined),
                  label: Text(_busy ? busyLabel : '检查最新有效种子'),
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
                  label: '重复任务处理',
                  value: '不扫描完整离线任务列表；按 115 BT 提交结果处理重复项。结果未知时不重发并保留本地种子。',
                ),
                if (files.isNotEmpty) ...[
                  const SizedBox(height: 10),
                  Text('本地校验文件清单', style: theme.textTheme.titleSmall),
                  const SizedBox(height: 4),
                  Text(
                    '通过远端解析校验后，将把全部文件加入 BT 任务。',
                    style: theme.textTheme.bodySmall,
                  ),
                ],
                for (final file in files)
                  ListTile(
                    dense: true,
                    contentPadding: EdgeInsets.zero,
                    title: Text(
                      '${file.index} · ${file.path}',
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                    ),
                    subtitle: Text(fmtSize(file.size)),
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
                        : status.statusText.trim().isEmpty
                        ? '无法确认'
                        : '无法确认：${status.statusText.trim()}',
                  ),
                  if (status.statusCode != null)
                    _BtPreflightFact(
                      label: '状态码',
                      value: status.statusCode.toString(),
                    ),
                  if (status.percentDone != null)
                    _BtPreflightFact(
                      label: '远端进度',
                      value:
                          '${(status.percentDone! * 100).toStringAsFixed(0)}%',
                    ),
                ],
                const SizedBox(height: 8),
                if (canProcessSelected)
                  FilledButton.icon(
                    onPressed: () => _runBatch(
                      selectedSource,
                      [selectedEntry],
                      targetFolder,
                      confirmationNotice:
                          '这是单项操作。上传、解析、查重恢复和 BT 提交均使用同一持久化幂等账本；已有未知结果时先只读核对，不重复发送写请求。',
                    ),
                    icon: const Icon(Icons.cloud_upload_outlined),
                    label: Text(_batchBusy ? '正在处理…' : '幂等处理此 Torrent'),
                  ),
                OutlinedButton.icon(
                  onPressed:
                      _busy ||
                          _batchBusy ||
                          _confirmingWrite ||
                          selectedSource == null
                      ? null
                      : () => _queryTask(selectedSource, result),
                  icon: const Icon(Icons.search),
                  label: Text(_busy ? '正在查询…' : '查询 BT 任务'),
                ),
              ],
            ],
          ],
        ),
      ),
    );
  }
}

class _BtFolderPickerDialog extends StatefulWidget {
  const _BtFolderPickerDialog({
    required this.source,
    required this.loadChildren,
  });

  final BookSource source;
  final Future<List<cloud115_offline.Cloud115BtFolder>> Function(
    BookSource source,
    String parentId,
  )
  loadChildren;

  @override
  State<_BtFolderPickerDialog> createState() => _BtFolderPickerDialogState();
}

class _BtFolderPickerDialogState extends State<_BtFolderPickerDialog> {
  final List<String> _folderIds = ['0'];
  final List<String> _folderNames = [];
  List<cloud115_offline.Cloud115BtFolder> _folders = const [];
  bool _loading = true;
  String? _error;

  @override
  void initState() {
    super.initState();
    _loadChildren('0');
  }

  String get _currentPath =>
      _folderNames.isEmpty ? '/' : '/${_folderNames.join('/')}';

  Future<void> _loadChildren(String parentId) async {
    try {
      final folders = await widget.loadChildren(widget.source, parentId);
      if (!mounted) return;
      setState(() {
        _folders = folders;
        _loading = false;
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _folders = const [];
        _loading = false;
        _error = _safeBtError(error);
      });
    }
  }

  Future<void> _enterFolder(cloud115_offline.Cloud115BtFolder folder) async {
    if (_loading || _folderIds.contains(folder.id)) return;
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final folders = await widget.loadChildren(widget.source, folder.id);
      if (!mounted) return;
      setState(() {
        _folderIds.add(folder.id);
        _folderNames.add(folder.name);
        _folders = folders;
        _loading = false;
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _loading = false;
        _error = _safeBtError(error);
      });
    }
  }

  void _goUp() {
    if (_loading || _folderIds.length <= 1) return;
    setState(() {
      _folderIds.removeLast();
      _folderNames.removeLast();
      _loading = true;
      _error = null;
    });
    _loadChildren(_folderIds.last);
  }

  void _selectCurrentFolder() {
    final isRoot = _folderNames.isEmpty;
    Navigator.of(context).pop(
      cloud115_offline.Cloud115BtFolderSelection(
        id: _folderIds.last,
        parentId: isRoot ? '' : _folderIds[_folderIds.length - 2],
        name: isRoot ? '' : _folderNames.last,
        path: _currentPath,
      ),
    );
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('选择 115 目标目录'),
    content: SizedBox(
      width: 440,
      height: 360,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              IconButton(
                tooltip: '返回上级目录',
                onPressed: _folderIds.length > 1 && !_loading ? _goUp : null,
                icon: const Icon(Icons.arrow_back),
              ),
              Expanded(
                child: Text(
                  _currentPath,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
            ],
          ),
          const Divider(height: 8),
          Expanded(
            child: _loading
                ? const Center(child: CircularProgressIndicator())
                : _error != null
                ? Center(child: Text(_error!))
                : _folders.isEmpty
                ? const Center(child: Text('此目录下没有子文件夹'))
                : ListView.builder(
                    itemCount: _folders.length,
                    itemBuilder: (context, index) {
                      final folder = _folders[index];
                      return ListTile(
                        dense: true,
                        leading: const Icon(Icons.folder_outlined),
                        title: Text(
                          folder.name,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                        subtitle: Text('目录 ID ${folder.id}'),
                        trailing: const Icon(Icons.chevron_right),
                        onTap: () => _enterFolder(folder),
                      );
                    },
                  ),
          ),
        ],
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.of(context).pop(),
        child: const Text('取消'),
      ),
      FilledButton(
        onPressed: _loading || _error != null ? null : _selectCurrentFolder,
        child: const Text('使用此目录'),
      ),
    ],
  );
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

class _PageCountField extends StatefulWidget {
  const _PageCountField({super.key, required this.store});

  final EhSubscriptionStore store;

  @override
  State<_PageCountField> createState() => _PageCountFieldState();
}

class _PageCountFieldState extends State<_PageCountField> {
  late final TextEditingController _controller;
  Timer? _saveTimer;

  @override
  void initState() {
    super.initState();
    _controller = TextEditingController(text: '${widget.store.pages}');
  }

  void _scheduleSave() {
    final value = int.tryParse(_controller.text.trim());
    if (value == null) return;
    _saveTimer?.cancel();
    _saveTimer = Timer(const Duration(milliseconds: 500), () {
      _persist(value.clamp(1, 500));
    });
  }

  void flush() {
    _saveTimer?.cancel();
    final value = int.tryParse(_controller.text.trim());
    if (value == null) {
      _controller.text = '${widget.store.pages}';
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(const SnackBar(content: Text('扫描页数请输入 1 到 500 的整数。')));
      return;
    }
    final pages = value.clamp(1, 500);
    _controller.value = TextEditingValue(
      text: '$pages',
      selection: TextSelection.collapsed(offset: '$pages'.length),
    );
    _persist(pages);
  }

  void reloadFromStore() {
    _saveTimer?.cancel();
    _controller.text = '${widget.store.pages}';
  }

  Future<void> _persist(int pages) async {
    widget.store.patch('pages', pages);
    try {
      await widget.store.saveRules();
    } catch (error) {
      if (!mounted) return;
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('保存扫描页数失败：$error')));
    }
  }

  @override
  void dispose() {
    _saveTimer?.cancel();
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => SizedBox(
    width: 116,
    child: TextField(
      key: const ValueKey('eh-scan-pages'),
      controller: _controller,
      keyboardType: TextInputType.number,
      decoration: const InputDecoration(
        isDense: true,
        border: OutlineInputBorder(),
        labelText: '扫描页数',
        suffixText: '页',
      ),
      onChanged: (_) => _scheduleSave(),
      onSubmitted: (_) => flush(),
    ),
  );
}

class _DailyAutomationBlock extends StatelessWidget {
  const _DailyAutomationBlock({
    required this.store,
    required this.coordinator,
    required this.workflowBusy,
    required this.workflowProgress,
    required this.onRunNow,
    required this.onStop,
    required this.pageCountKey,
  });

  final EhSubscriptionStore store;
  final EhDailyCoordinator? coordinator;
  final bool workflowBusy;
  final String? workflowProgress;
  final VoidCallback onRunNow;
  final Future<void> Function() onStop;
  final GlobalKey<_PageCountFieldState> pageCountKey;

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
        final isCollecting = store.busy || store.progress.running;
        return Card(
          key: const ValueKey('eh-daily-workflow-card'),
          margin: EdgeInsets.zero,
          child: Padding(
            padding: const EdgeInsets.all(14),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        '自动每日完整工作流',
                        style: Theme.of(context).textTheme.titleSmall,
                      ),
                    ),
                    Switch.adaptive(
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
                                    content: Text('保存自动化设置失败：$error'),
                                    backgroundColor: Theme.of(
                                      context,
                                    ).colorScheme.error,
                                  ),
                                );
                              }
                            }
                          : null,
                    ),
                  ],
                ),
                Text(
                  supported
                      ? 'Windows 运行期间每天自动检查一次；手动工作流可随时运行。'
                      : '自动每日检查仅支持 Windows。',
                  style: Theme.of(context).textTheme.bodySmall,
                ),
                const SizedBox(height: 12),
                LayoutBuilder(
                  builder: (context, constraints) {
                    final runButton = FilledButton.icon(
                      key: const ValueKey('eh-full-workflow-run'),
                      onPressed: workflowBusy || store.busy ? null : onRunNow,
                      icon: workflowBusy || store.busy
                          ? const SizedBox(
                              width: 16,
                              height: 16,
                              child: CircularProgressIndicator(strokeWidth: 2),
                            )
                          : const Icon(Icons.refresh),
                      label: Text(
                        store.busy
                            ? '扫描中…'
                            : workflowBusy
                            ? '工作流运行中…'
                            : '立即运行完整工作流',
                      ),
                    );
                    final stopButton = isCollecting
                        ? OutlinedButton.icon(
                            onPressed: onStop,
                            icon: const Icon(Icons.stop, size: 18),
                            label: const Text('停止扫描'),
                          )
                        : const SizedBox.shrink();
                    final pages = _PageCountField(
                      key: pageCountKey,
                      store: store,
                    );
                    if (constraints.maxWidth < 560) {
                      return Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          runButton,
                          const SizedBox(height: 8),
                          Wrap(
                            spacing: 8,
                            crossAxisAlignment: WrapCrossAlignment.center,
                            children: [stopButton, pages],
                          ),
                        ],
                      );
                    }
                    return Row(
                      children: [
                        Expanded(child: runButton),
                        if (isCollecting) ...[
                          const SizedBox(width: 8),
                          stopButton,
                        ],
                        const SizedBox(width: 8),
                        pages,
                      ],
                    );
                  },
                ),
                const SizedBox(height: 8),
                Text(
                  run == null
                      ? '最近运行：尚无记录'
                      : '最近运行：${_dailySourceLabel(run.source)} · ${_dailyRunLabel(run.status)} · 保存 ${run.saved} 个',
                  style: Theme.of(context).textTheme.bodySmall,
                ),
                if (state.dailyEnabled)
                  Text(
                    '下次自动条件：${_nextDailyEligibility(state, store.hasOutDir, supported)}',
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                if (run != null &&
                    (run.cloudStage.status != EhDailyCloudStageStatus.notRun ||
                        run.cloudStage.message?.trim().isNotEmpty == true))
                  Text(
                    run.cloudStage.status == EhDailyCloudStageStatus.notRun
                        ? '保存模式：${_dailyCloudStageSummary(run.cloudStage)}'
                        : '115：${_dailyCloudStageSummary(run.cloudStage)}',
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                if (workflowBusy && !store.busy && workflowProgress != null)
                  Text(
                    '工作流：$workflowProgress',
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                if (coordinator?.status ==
                    EhDailyCoordinatorStatus.waitingForCurrentRun)
                  Text(
                    'Daily 正在等待当前任务结束。',
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                if (runError != null && runError.isNotEmpty)
                  Padding(
                    padding: const EdgeInsets.only(top: 3),
                    child: Text(
                      '最近错误：$runError',
                      style: TextStyle(
                        color: Theme.of(context).colorScheme.error,
                      ),
                    ),
                  ),
                if (coordinator?.status ==
                    EhDailyCoordinatorStatus.persistenceFailed)
                  Text(
                    coordinator?.error ?? '调度状态保存失败，自动检查已停止。',
                    style: TextStyle(
                      color: Theme.of(context).colorScheme.error,
                    ),
                  ),
                if (store.progress.stage != 'idle' || store.busy) ...[
                  const SizedBox(height: 10),
                  const Divider(height: 1),
                  const SizedBox(height: 10),
                  _ProgressBlock(progress: store.progress),
                ],
              ],
            ),
          ),
        );
      },
    );
  }
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
  EhDailyRunStatus.partial => '部分完成',
  EhDailyRunStatus.failed => '失败',
  EhDailyRunStatus.cancelled => '已取消',
  EhDailyRunStatus.interruptedUnknown => '中断，远端结果未知',
  EhDailyRunStatus.unknown => '状态未知',
};

String _dailyCloudStageSummary(EhDailyCloudStageResult result) {
  final message = result.message?.trim();
  if (result.status == EhDailyCloudStageStatus.notRun) {
    return message == null || message.isEmpty ? '本轮未运行云端阶段' : message;
  }
  final status = switch (result.status) {
    EhDailyCloudStageStatus.notRun => '未运行',
    EhDailyCloudStageStatus.p0Blocked => '旧版门禁记录',
    EhDailyCloudStageStatus.needsConfiguration => '等待配置',
    EhDailyCloudStageStatus.sessionUnavailable => '等待现有书源会话',
    EhDailyCloudStageStatus.completed => '本轮提交已处理',
    EhDailyCloudStageStatus.partial => '部分完成',
    EhDailyCloudStageStatus.failed => '失败',
  };
  final counts =
      '${result.submitted} 已确认 · ${result.skipped} 跳过 · '
      '${result.pending} 待核对 · ${result.removed} 已清理本地种子 · '
      '${result.failed} 失败';
  return '$status（${result.total} 项；$counts）'
      '${message == null || message.isEmpty ? '' : ' · $message'}';
}

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
              'EH 订阅与自动化',
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
    : useDays = years < 1,
      ageCtrl = TextEditingController(
        text: _fmtNum(years < 1 ? years * 365.25 : years),
      ),
      downloadsCtrl = TextEditingController(text: '$downloads');

  bool useDays;
  final TextEditingController ageCtrl;
  final TextEditingController downloadsCtrl;

  double get minAgeYears {
    final value = double.tryParse(ageCtrl.text.trim()) ?? 0;
    return useDays ? value / 365.25 : value;
  }

  void setAgeUnit(bool days) {
    if (useDays == days) return;
    final years = minAgeYears;
    useDays = days;
    ageCtrl.text = _fmtNum(days ? years * 365.25 : years);
  }

  void dispose() {
    ageCtrl.dispose();
    downloadsCtrl.dispose();
  }
}

String _fmtNum(double v) =>
    v.toStringAsFixed(3).replaceFirst(RegExp(r'\.?0+$'), '');

class _RulesEditorState extends State<_RulesEditor> {
  late final TextEditingController _search;
  late final TextEditingController _markers;
  late final TextEditingController _exclude;
  late final TextEditingController _interval;
  List<_TierRow> _tiers = [];

  double _minRating = EhSubscriptionStore.minimumRating;
  Timer? _debounce;

  EhSubscriptionStore get store => widget.store;

  @override
  void initState() {
    super.initState();
    reloadFromStore();
  }

  void reloadFromStore() {
    final s = store;
    _search = TextEditingController(text: s.search);
    _markers = TextEditingController(text: s.titleMarkers);
    _exclude = TextEditingController(text: s.excludeMarkers.join(', '));
    _interval = TextEditingController(text: '${s.intervalSecs}');
    _minRating = s.minRating;
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
            'min_age_years': t.minAgeYears,
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
          '评分下限：≥${_minRating.toStringAsFixed(1)}',
          '最低评分固定为 4.4；可提高门槛，低于 4.4 的作品不会下载',
        ),
        Slider(
          value: _minRating
              .clamp(EhSubscriptionStore.minimumRating, 5)
              .toDouble(),
          min: EhSubscriptionStore.minimumRating,
          max: 5,
          divisions: 6,
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
                  _tiers.insert(0, _TierRow(7 / 365.25, 100));
                });
                flush();
              },
              icon: const Icon(Icons.add, size: 18),
              label: const Text('添加档位'),
            );
            const label = _FieldLabel(
              '分时间段下载数要求',
              '画龄越久要求越高；发布时间不超过 3 天（含）一律跳过，小于 1 年的档位按天显示',
            );
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
                      child: Text('画龄 ≥', style: theme.textTheme.bodySmall),
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
                          controller: _tiers[i].ageCtrl,
                          onChanged: (_) => _debouncedFlush(),
                          keyboardType: const TextInputType.numberWithOptions(
                            decimal: true,
                          ),
                          decoration: InputDecoration(
                            isDense: true,
                            border: const OutlineInputBorder(),
                            suffix: DropdownButtonHideUnderline(
                              child: DropdownButton<bool>(
                                value: _tiers[i].useDays,
                                isDense: true,
                                items: const [
                                  DropdownMenuItem(
                                    value: true,
                                    child: Text('天'),
                                  ),
                                  DropdownMenuItem(
                                    value: false,
                                    child: Text('年'),
                                  ),
                                ],
                                onChanged: (days) {
                                  if (days == null) return;
                                  setState(() => _tiers[i].setAgeUnit(days));
                                  _debouncedFlush();
                                },
                              ),
                            ),
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
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        directory,
        const SizedBox(height: 12),
        SizedBox(width: 220, child: requestInterval),
      ],
    );
  }
}

/// 连通性预检结果块：把"哪一段不通"讲清楚。
class EhNetworkSettingsPanel extends StatefulWidget {
  const EhNetworkSettingsPanel({super.key, this.storeOverride});

  @visibleForTesting
  final EhSubscriptionStore? storeOverride;

  @override
  State<EhNetworkSettingsPanel> createState() => _EhNetworkSettingsPanelState();
}

class _EhNetworkSettingsPanelState extends State<EhNetworkSettingsPanel> {
  late final TextEditingController _proxyUrl;
  bool _proxyEditing = false;

  EhSubscriptionStore get store =>
      widget.storeOverride ?? EhSubscriptionStore.instance;

  @override
  void initState() {
    super.initState();
    _proxyUrl = TextEditingController(text: store.proxyUrl);
    store.addListener(_syncProxyUrl);
    if (widget.storeOverride == null && !store.rulesLoaded) {
      WidgetsBinding.instance.addPostFrameCallback((_) => store.init());
    }
  }

  void _syncProxyUrl() {
    if (_proxyEditing || _proxyUrl.text == store.proxyUrl) return;
    _proxyUrl.value = TextEditingValue(
      text: store.proxyUrl,
      selection: TextSelection.collapsed(offset: store.proxyUrl.length),
    );
  }

  @override
  void dispose() {
    store.removeListener(_syncProxyUrl);
    _proxyUrl.dispose();
    super.dispose();
  }

  Future<void> _save({bool showSuccess = true}) async {
    store.patchNetwork(proxyUrl: _proxyUrl.text, notify: false);
    _proxyEditing = false;
    try {
      await store.saveRules();
      if (showSuccess && mounted) _snack('EH 网络设置已保存');
    } catch (error) {
      if (mounted) _snack('$error', error: true);
    }
  }

  Future<void> _checkEh() async {
    store.patchNetwork(proxyUrl: _proxyUrl.text, notify: false);
    _proxyEditing = false;
    try {
      await store.saveRules();
    } catch (error) {
      if (mounted) _snack('$error', error: true);
      return;
    }
    final probe = await store.runProbe();
    if (!mounted || probe == null) return;
    final reachable = probe.hostOk && probe.trackerOk;
    _snack(
      'EH 主站 ${probe.hostOk ? "可用" : "不可用（${probe.hostDetail}）"} · '
      'Tracker ${probe.trackerOk ? "有响应" : "不可达（${probe.trackerDetail}）"}；'
      'Torrent 数据${probe.trackerTorrentValidated ? "已验证" : "未验证"}',
      error: !reachable,
    );
  }

  void _snack(String message, {bool error = false}) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(message),
        backgroundColor: error ? Theme.of(context).colorScheme.error : null,
        duration: const Duration(seconds: 6),
      ),
    );
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: store,
    builder: (context, _) {
      final theme = Theme.of(context);
      return Card(
        margin: EdgeInsets.zero,
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text('EH 网络', style: theme.textTheme.titleMedium),
              const SizedBox(height: 4),
              Text(
                '此处只配置 EH 主站与 Tracker 路径。115 书源继续使用原有连接策略，不继承 EH 代理。',
                style: theme.textTheme.bodySmall,
              ),
              const SizedBox(height: 12),
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
                  DropdownMenuItem(
                    value: 'system',
                    child: Text('system（系统代理）'),
                  ),
                  DropdownMenuItem(value: 'direct', child: Text('direct（直连）')),
                  DropdownMenuItem(
                    value: 'manual',
                    child: Text('manual（手动代理）'),
                  ),
                ],
                onChanged: (mode) {
                  if (mode == null) return;
                  store.patchNetwork(mode: mode);
                  _save(showSuccess: false);
                },
              ),
              if (store.proxyMode == 'manual') ...[
                const SizedBox(height: 12),
                _FieldLabel(
                  '代理地址',
                  '支持 HTTP(S)、SOCKS5 和 SOCKS5H（代理端 DNS）。地址仅保存在本地设置中，不写入诊断日志。',
                ),
                TextField(
                  key: const ValueKey('eh-proxy-url'),
                  controller: _proxyUrl,
                  onChanged: (value) {
                    _proxyEditing = true;
                    store.patchNetwork(proxyUrl: value, notify: false);
                  },
                  onSubmitted: (_) => _save(showSuccess: false),
                  decoration: const InputDecoration(
                    isDense: true,
                    border: OutlineInputBorder(),
                    hintText:
                        'http://127.0.0.1:7890 或 socks5h://127.0.0.1:7891',
                  ),
                ),
              ],
              const SizedBox(height: 10),
              Wrap(
                spacing: 8,
                runSpacing: 8,
                children: [
                  OutlinedButton.icon(
                    onPressed: () => _save(),
                    icon: const Icon(Icons.save_outlined, size: 18),
                    label: const Text('保存 EH 网络设置'),
                  ),
                  OutlinedButton.icon(
                    onPressed: store.probing ? null : _checkEh,
                    icon: store.probing
                        ? const SizedBox(
                            width: 14,
                            height: 14,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : const Icon(Icons.wifi_tethering_outlined, size: 18),
                    label: const Text('检测 EH 网络'),
                  ),
                ],
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
          ),
        ),
      );
    },
  );
}

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
// EH 进度 + 已保存清单
// ---------------------------------------------------------------------------

class _ProgressBlock extends StatelessWidget {
  const _ProgressBlock({required this.progress});

  final EhProgress progress;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = progress;
    final total = p.pages <= 0 ? 1 : p.pages;
    final elapsedSeconds = p.activityStartedAtUnixMs <= 0
        ? 0
        : ((DateTime.now().millisecondsSinceEpoch -
                      p.activityStartedAtUnixMs) ~/
                  1000)
              .clamp(0, 24 * 60 * 60);
    final elapsedLabel =
        '${(elapsedSeconds ~/ 60).toString().padLeft(2, '0')}:'
        '${(elapsedSeconds % 60).toString().padLeft(2, '0')}';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (p.running) ...[
          LinearProgressIndicator(
            value: p.stage == 'searching' ? (p.page / total).clamp(0, 1) : null,
          ),
          const SizedBox(height: 8),
        ],
        Text(
          p.error != null || p.message.isEmpty ? p.stageLabel : p.message,
          maxLines: 2,
          overflow: TextOverflow.ellipsis,
          style: theme.textTheme.bodyMedium,
        ),
        const SizedBox(height: 4),
        Wrap(
          spacing: 14,
          runSpacing: 4,
          children: [
            if (p.pages > 0)
              Text('搜索页 ${p.page}/$total', style: theme.textTheme.bodySmall),
            if (p.workTotal > 0)
              Text(
                '处理 ${p.processed}/${p.workTotal} 项',
                style: theme.textTheme.bodySmall,
              ),
            Text('本轮新增 ${p.saved} 个', style: theme.textTheme.bodySmall),
            if (p.tooRecent > 0)
              Text(
                '3 天内跳过 ${p.tooRecent} 个种子',
                style: theme.textTheme.bodySmall,
              ),
          ],
        ),
        if (p.running && p.activityStartedAtUnixMs > 0)
          Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Text(
              '当前步骤已运行 $elapsedLabel',
              style: theme.textTheme.bodySmall,
            ),
          ),
        if (p.error != null)
          Text(
            '${p.stage == 'cancelled' ? '已取消' : '失败'}：${p.error}',
            style: TextStyle(color: theme.colorScheme.error, fontSize: 12),
          ),
      ],
    );
  }
}

class _ManifestBlock extends StatefulWidget {
  const _ManifestBlock({
    required this.store,
    required this.onOpenDir,
    required this.onRefresh,
  });

  final EhSubscriptionStore store;
  final Future<void> Function() onOpenDir;
  final Future<void> Function() onRefresh;

  @override
  State<_ManifestBlock> createState() => _ManifestBlockState();
}

class _ManifestBlockState extends State<_ManifestBlock> {
  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final report = widget.store.manifestReport;
    final entries = report.entries.reversed.toList();
    final shown = entries.take(1).toList();
    return Column(
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
          ),
        if (entries.length > 1)
          Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Text('仅显示最近 1 个', style: theme.textTheme.bodySmall),
          ),
      ],
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
  const _ManifestHandoffTile({super.key, required this.entry});

  final EhCheckedManifestEntry entry;

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
          Text(
            title,
            style: const TextStyle(fontSize: 13, fontWeight: FontWeight.w600),
          ),
          Padding(
            padding: const EdgeInsets.only(top: 3),
            child: Text('${item.file} · $torrentSize'),
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
          Padding(
            padding: const EdgeInsets.only(left: 12, top: 4),
            child: Text(
              '本地清单状态不代表已上传或已提交 115。',
              style: theme.textTheme.bodySmall?.copyWith(color: scheme.outline),
            ),
          ),
        ],
      ),
    );
  }
}
