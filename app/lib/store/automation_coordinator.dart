import 'dart:async';
import 'dart:convert';

import 'package:app/src/rust/api/db.dart' as dbapi;
import 'package:app/src/rust/api/scraper.dart' as scraperapi;
import 'package:app/store/library_index_service.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/sync_engine.dart';
import 'package:app/store/foreground_work_coordinator.dart';
import 'package:app/store/long_task_registry.dart';
import 'package:flutter/foundation.dart';

/// Owns the automatic sync -> catalog scrape pipeline.
///
/// SyncEngine remains the WebDAV transport implementation, but its timers are
/// disabled in coordinator mode. This class is the only owner of startup,
/// debounce and periodic triggers, and always runs catalog scraping after the
/// optional sync step. Scraping itself can only query the local SQLite index.
class AutomationCoordinator {
  AutomationCoordinator._();
  static final instance = AutomationCoordinator._();

  static const ruleVersion = 'catalog-rules-v3';
  static const ancestorDepth = 4;
  static const generatedTagProjectionVersionKey =
      'generated_tag_projection_version';
  static const generatedTagProjectionVersion = 'v3';
  // Production safe-auto is enabled only for Ready, conflict-free proposals;
  // provider enrichment and remote book-source I/O remain separate lanes.
  static const automaticMaterializationEnabled = true;
  static const _pollInterval = Duration(seconds: 60);
  static const _debounceDelay = Duration(seconds: 2);

  Timer? _poll;
  Timer? _debounce;
  bool _started = false;
  bool _running = false;
  bool _enabled = true;
  int _dirtyGeneration = 0;
  bool _pendingSyncPush = false;
  String? _pendingCycleTrigger;
  bool _pendingManualScrape = false;
  Completer<scraperapi.ScrapeRunDto?>? _pendingScrapeCompleter;

  scraperapi.ScrapeRunDto? lastRun;
  int lastAutoApplied = 0;
  int lastMaterializationSkipped = 0;
  int lastMaterializationReview = 0;
  int lastMaterializationStale = 0;
  int lastMaterializationErrors = 0;
  String lastStatus = '尚未运行自动流程';

  bool get autoRun => _enabled;
  bool get running => _running;

  @visibleForTesting
  static bool needsGeneratedTagProjectionMigration(
    Iterable<dbapi.SettingEntryDto> settings,
  ) => !settings.any(
    (entry) =>
        entry.key == generatedTagProjectionVersionKey &&
        entry.value == generatedTagProjectionVersion,
  );

  Future<void> init() async {
    if (_started) return;
    _started = true;
    await SyncEngine.instance.init(managedByCoordinator: true);
    _enabled = SyncEngine.instance.autoSync;
    LibraryStore.instance.addCommittedMutationListener(markDirty);
    _poll?.cancel();
    _poll = Timer.periodic(_pollInterval, (_) {
      if (_enabled) unawaited(runCycle(trigger: 'timer', quickPoll: true));
    });
    if (_enabled) await runCycle(trigger: 'startup');
  }

  Future<void> setAutoRun(bool value) async {
    _enabled = value;
    await SyncEngine.instance.setAutoSync(value);
    if (value) unawaited(runCycle(trigger: 'enabled'));
  }

  void markDirty() {
    if (!_enabled) return;
    _dirtyGeneration++;
    if (_running) return;
    _debounce?.cancel();
    _debounce = Timer(_debounceDelay, () {
      unawaited(runCycle(trigger: 'local-change'));
    });
  }

  void _finishActiveWork(int startingGeneration) {
    _running = false;
    final pendingScrape = _pendingScrapeCompleter;
    if (_pendingManualScrape && pendingScrape != null) {
      _pendingManualScrape = false;
      _pendingScrapeCompleter = null;
      scheduleMicrotask(() async {
        try {
          pendingScrape.complete(await runScrapeNow());
        } catch (error, stackTrace) {
          pendingScrape.completeError(error, stackTrace);
        }
      });
      return;
    }

    final trigger = _pendingCycleTrigger;
    final dirtyDuringRun = _dirtyGeneration > startingGeneration;
    if (trigger == null && !dirtyDuringRun) return;
    _pendingCycleTrigger = null;
    _debounce?.cancel();
    _debounce = Timer(_debounceDelay, () {
      unawaited(runCycle(trigger: trigger ?? 'local-change'));
    });
  }

  /// Run the complete pipeline. The sync step may be a no-op when WebDAV is
  /// not configured; local catalog scraping still runs in that case.
  Future<scraperapi.ScrapeRunDto?> runCycle({
    required String trigger,
    bool quickPoll = false,
  }) async {
    if (_running) {
      if (trigger != 'timer') _pendingCycleTrigger = trigger;
      return lastRun;
    }
    _running = true;
    final startingGeneration = _dirtyGeneration;
    final taskLease = LongTaskRegistry.instance.register(
      kind: 'automation_cycle',
      label: '同步与本地目录整理',
      phase: quickPoll ? '检查同步状态' : '刷新目录快照',
    );
    try {
      final quickPollOutcome = quickPoll
          ? await SyncEngine.instance.probeQuickPoll(background: true)
          : null;
      final preSyncCatalog = await LibraryIndexService.instance
          .refreshCatalogSnapshots(
            sources: LibraryStore.instance.sources,
            trigger: trigger,
            background: true,
          );
      taskLease.update(phase: '同步配置');
      final syncOutcome =
          quickPollOutcome ??
          await SyncEngine.instance.syncNowWithOutcome(
            refreshCatalog: false,
            background: true,
          );
      final syncMessage = syncOutcome.message;
      if (!syncOutcome.changesKnown) {
        // Failure or an overlapping sync leaves the local projection unknown;
        // reload conservatively before catalog processing.
        await LibraryStore.instance.load(force: true, persist: false);
      }
      var postSyncChanged = 0;
      if (!syncOutcome.changesKnown || syncOutcome.hasChanges) {
        // Local paths are refreshed after a sync that may have applied catalog
        // data. A confirmed no-op quick poll is probed before the local scan,
        // so it needs only that one scan and does not imply local paths are
        // unchanged.
        final postSyncCatalog = await LibraryIndexService.instance
            .refreshCatalogSnapshots(
              sources: LibraryStore.instance.sources,
              trigger: '$trigger-post-sync',
              background: true,
            );
        postSyncChanged = postSyncCatalog.changed;
      }
      taskLease.update(phase: '整理本地目录索引');
      // Catalog replacement marks deleted assets and clears their local
      // proposal/metadata/tag state in Rust. The Dart pass removes stale
      // records/caches from memory as well. M8 automation deliberately stays
      // offline against remote book sources; remote changes enter through a
      // persisted snapshot or the existing sync/discovery lane.
      await ForegroundWorkCoordinator.instance.waitForMaintenanceTurn();
      final purge = await LibraryStore.instance.purgeStaleData(
        alignRemote: false,
        background: true,
      );
      if (purge.$1 > 0 || purge.$2 > 0) {
        await LibraryStore.instance.load(force: true, persist: false);
      }
      await ForegroundWorkCoordinator.instance.waitForMaintenanceTurn();
      final run = await scraperapi.dbRunCatalogScrape(
        trigger: trigger,
        ancestorDepth: ancestorDepth,
        ruleVersion: ruleVersion,
      );
      taskLease.update(phase: '应用已确认的本地整理');
      lastRun = run;
      final materialization = automaticMaterializationEnabled
          ? await _materializeReadyProposals(background: true)
          : _MaterializationSummary.held();
      var pushMessage = syncMessage;
      final initialSyncCommitted =
          syncOutcome.status == SyncRunStatus.completed ||
          (syncOutcome.status == SyncRunStatus.remoteUnchanged &&
              syncOutcome.revision != null);
      if (initialSyncCommitted) _pendingSyncPush = false;
      if (syncOutcome.status == SyncRunStatus.failed ||
          !syncOutcome.changesKnown) {
        _pendingSyncPush = true;
      }
      if (materialization.syncDirty ||
          postSyncChanged > 0 ||
          purge.$1 > 0 ||
          purge.$2 > 0 ||
          (preSyncCatalog.changed > 0 && !initialSyncCommitted)) {
        _pendingSyncPush = true;
      }
      if (_pendingSyncPush &&
          syncOutcome.status != SyncRunStatus.failed &&
          syncOutcome.changesKnown) {
        // The projection transaction has already committed. Sync is deliberately
        // scheduled from this coordinator boundary, never from Rust/SQLite.
        final pushOutcome = await SyncEngine.instance.syncNowWithOutcome(
          refreshCatalog: false,
          background: true,
        );
        pushMessage = pushOutcome.message;
        _pendingSyncPush =
            pushOutcome.status != SyncRunStatus.completed &&
            !(pushOutcome.status == SyncRunStatus.remoteUnchanged &&
                pushOutcome.revision != null);
      }
      lastStatus = automaticMaterializationEnabled
          ? '刮削完成 ${run.processed}/${run.total}，可确认 ${run.ready}，自动写入 ${materialization.applied}，需复核 ${run.ambiguous + materialization.reviewRequired}（同步：$pushMessage）'
          : '刮削完成 ${run.processed}/${run.total}，待人工确认 ${run.ready}，自动写入已暂停（同步：$pushMessage）';
      return run;
    } catch (error) {
      lastStatus = '自动流程失败：$error';
      debugPrint('[AutomationCoordinator] $error');
      return null;
    } finally {
      taskLease.dispose();
      _finishActiveWork(startingGeneration);
    }
  }

  Future<scraperapi.ScrapeRunDto?> runScrapeNow() async {
    if (_running) {
      _pendingManualScrape = true;
      return (_pendingScrapeCompleter ??= Completer<scraperapi.ScrapeRunDto?>())
          .future;
    }
    _running = true;
    final startingGeneration = _dirtyGeneration;
    final taskLease = LongTaskRegistry.instance.register(
      kind: 'manual_catalog_scrape',
      label: '手动刮削与本地整理',
      phase: '刷新目录快照',
    );
    try {
      taskLease.update(phase: '刮削本地目录');
      return await ForegroundWorkCoordinator.instance.runForeground(
        _runManualScrape,
      );
    } finally {
      taskLease.dispose();
      _finishActiveWork(startingGeneration);
    }
  }

  Future<scraperapi.ScrapeRunDto?> _runManualScrape() async {
    try {
      final catalog = await LibraryIndexService.instance
          .refreshCatalogSnapshots(
            sources: LibraryStore.instance.sources,
            trigger: 'manual-scrape',
          );
      // Manual scrape also preserves the zero-remote-I/O scraper invariant;
      // online directory alignment remains an explicit discovery operation.
      final purge = await LibraryStore.instance.purgeStaleData(
        alignRemote: false,
      );
      if (purge.$1 > 0 || purge.$2 > 0) {
        await LibraryStore.instance.load(force: true, persist: false);
      }
      final run = await scraperapi.dbRunCatalogScrape(
        trigger: 'manual-scrape',
        ancestorDepth: ancestorDepth,
        ruleVersion: ruleVersion,
      );
      lastRun = run;
      final materialization = automaticMaterializationEnabled
          ? await _materializeReadyProposals()
          : _MaterializationSummary.held();
      if (_pendingSyncPush ||
          materialization.syncDirty ||
          catalog.changed > 0 ||
          purge.$1 > 0 ||
          purge.$2 > 0) {
        final syncOutcome = await SyncEngine.instance.syncNowWithOutcome(
          refreshCatalog: false,
        );
        _pendingSyncPush =
            syncOutcome.status != SyncRunStatus.completed &&
            !(syncOutcome.status == SyncRunStatus.remoteUnchanged &&
                syncOutcome.revision != null);
      }
      lastStatus = automaticMaterializationEnabled
          ? '刮削完成 ${run.processed}/${run.total}，可确认 ${run.ready}，自动写入 ${materialization.applied}，需复核 ${run.ambiguous + materialization.reviewRequired}'
          : '刮削完成 ${run.processed}/${run.total}，待人工确认 ${run.ready}，自动写入已暂停';
      return run;
    } catch (error) {
      lastStatus = '刮削失败：$error';
      debugPrint('[AutomationCoordinator] scrape $error');
      return null;
    }
  }

  /// Materialize only proposals that satisfy the production safe-auto policy.
  /// Rust owns the projection transaction; Dart only drains typed results and
  /// reloads its in-memory repositories after a committed batch.
  Future<_MaterializationSummary> _materializeReadyProposals({
    bool background = false,
  }) async {
    final summary = _MaterializationSummary();
    if (background) {
      await ForegroundWorkCoordinator.instance.waitForMaintenanceTurn();
    }
    final projectionMigration = needsGeneratedTagProjectionMigration(
      await dbapi.dbLoadAllSettings(),
    );
    final proposals = await scraperapi.dbLoadScrapeProposals(
      limit: 100000,
      state: 'ready',
    );
    for (final proposal in proposals) {
      if (background) {
        await ForegroundWorkCoordinator.instance.waitForMaintenanceTurn();
      }
      // Do not use the persisted materialization status as a skip gate.
      // Re-scraping can update semantic_json while retaining the same catalog
      // input revision, and Rust's transaction is responsible for returning
      // `skipped` when the canonical projection is already complete.  In
      // particular, this lets an applied proposal repair a missing generated
      // tag after a vocabulary/projection migration.
      if (!shouldAttemptMaterialization(
        inputRevision: proposal.inputRevision,
        conflictsJson: proposal.conflictsJson,
      )) {
        continue;
      }
      try {
        final result = await scraperapi.dbMaterializeReadyProposal(
          assetKey: proposal.assetKey,
          expectedRevision: proposal.inputRevision,
        );
        switch (result.status) {
          case 'applied':
            summary.applied++;
          case 'skipped':
            summary.skipped++;
          case 'review-required':
            summary.reviewRequired++;
          case 'stale':
            summary.stale++;
          default:
            summary.errors++;
        }
        summary.syncDirty = summary.syncDirty || result.syncDirty;
      } catch (error) {
        summary.errors++;
        debugPrint('[AutomationCoordinator] materialization $error');
      }
    }
    if (projectionMigration && summary.errors == 0) {
      await dbapi.dbSaveSetting(
        key: generatedTagProjectionVersionKey,
        value: generatedTagProjectionVersion,
      );
    }
    lastAutoApplied = summary.applied;
    lastMaterializationSkipped = summary.skipped;
    lastMaterializationReview = summary.reviewRequired;
    lastMaterializationStale = summary.stale;
    lastMaterializationErrors = summary.errors;
    if (summary.applied > 0 || summary.syncDirty) {
      await LibraryStore.instance.load(force: true, persist: false);
    }
    return summary;
  }

  @visibleForTesting
  static bool shouldAttemptMaterialization({
    required String inputRevision,
    required String conflictsJson,
  }) {
    if (inputRevision.trim().isEmpty) return false;
    return _isEmptyJsonArray(conflictsJson);
  }

  static bool _isEmptyJsonArray(String raw) {
    try {
      final value = jsonDecode(raw);
      return value is List && value.isEmpty;
    } catch (_) {
      return false;
    }
  }
}

class _MaterializationSummary {
  _MaterializationSummary();

  int applied = 0;
  int skipped = 0;
  int reviewRequired = 0;
  int stale = 0;
  int errors = 0;
  bool syncDirty = false;

  _MaterializationSummary.held();
}
