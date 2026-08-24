import 'dart:async';
import 'dart:convert';

import 'package:app/src/rust/api/scraper.dart' as scraperapi;
import 'package:app/store/library_index_service.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/sync_engine.dart';
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

  scraperapi.ScrapeRunDto? lastRun;
  int lastAutoApplied = 0;
  int lastMaterializationSkipped = 0;
  int lastMaterializationReview = 0;
  int lastMaterializationStale = 0;
  int lastMaterializationErrors = 0;
  String lastStatus = '尚未运行自动流程';

  bool get autoRun => _enabled;
  bool get running => _running;

  Future<void> init() async {
    if (_started) return;
    _started = true;
    await SyncEngine.instance.init(managedByCoordinator: true);
    _enabled = SyncEngine.instance.autoSync;
    LibraryStore.instance.addListener(markDirty);
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
    if (!_enabled || _running) return;
    _debounce?.cancel();
    _debounce = Timer(_debounceDelay, () {
      unawaited(runCycle(trigger: 'local-change'));
    });
  }

  /// Run the complete pipeline. The sync step may be a no-op when WebDAV is
  /// not configured; local catalog scraping still runs in that case.
  Future<scraperapi.ScrapeRunDto?> runCycle({
    required String trigger,
    bool quickPoll = false,
  }) async {
    if (_running) return lastRun;
    _running = true;
    try {
      final preSyncCatalog = await LibraryIndexService.instance
          .refreshCatalogSnapshots(
            sources: LibraryStore.instance.sources,
            trigger: trigger,
          );
      late final String syncMessage;
      if (quickPoll) {
        await SyncEngine.instance.triggerAutomatic(
          quickPoll: true,
          refreshCatalog: false,
        );
        syncMessage = SyncEngine.instance.lastStatus;
      } else {
        syncMessage = await SyncEngine.instance.syncNow(refreshCatalog: false);
      }
      // Sync may have added/removed a source or applied source aliases in Rust;
      // refresh the Dart source list before rebuilding the post-sync catalog.
      await LibraryStore.instance.load(force: true, persist: false);
      // A sync pull may have changed library_index. Rebuild only from the
      // resulting local/persisted snapshot before parsing proposals.
      final postSyncCatalog = await LibraryIndexService.instance
          .refreshCatalogSnapshots(
            sources: LibraryStore.instance.sources,
            trigger: '$trigger-post-sync',
          );
      // Catalog replacement marks deleted assets and clears their local
      // proposal/metadata/tag state in Rust. The Dart pass removes stale
      // records/caches from memory as well. M8 automation deliberately stays
      // offline against remote book sources; remote changes enter through a
      // persisted snapshot or the existing sync/discovery lane.
      final purge = await LibraryStore.instance.purgeStaleData(
        alignRemote: false,
      );
      await LibraryStore.instance.load(force: true, persist: false);
      final run = await scraperapi.dbRunCatalogScrape(
        trigger: trigger,
        ancestorDepth: ancestorDepth,
        ruleVersion: ruleVersion,
      );
      lastRun = run;
      final materialization = automaticMaterializationEnabled
          ? await _materializeReadyProposals()
          : _MaterializationSummary.held();
      var pushMessage = syncMessage;
      final catalogDirty =
          preSyncCatalog.changed > 0 ||
          postSyncCatalog.changed > 0 ||
          purge.$1 > 0 ||
          purge.$2 > 0;
      if (materialization.syncDirty || catalogDirty) {
        // The projection transaction has already committed. Sync is deliberately
        // scheduled from this coordinator boundary, never from Rust/SQLite.
        pushMessage = await SyncEngine.instance.syncNow(refreshCatalog: false);
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
      _running = false;
    }
  }

  Future<scraperapi.ScrapeRunDto?> runScrapeNow() async {
    if (_running) return lastRun;
    _running = true;
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
      await LibraryStore.instance.load(force: true, persist: false);
      final run = await scraperapi.dbRunCatalogScrape(
        trigger: 'manual-scrape',
        ancestorDepth: ancestorDepth,
        ruleVersion: ruleVersion,
      );
      lastRun = run;
      final materialization = automaticMaterializationEnabled
          ? await _materializeReadyProposals()
          : _MaterializationSummary.held();
      if (materialization.syncDirty ||
          catalog.changed > 0 ||
          purge.$1 > 0 ||
          purge.$2 > 0) {
        await SyncEngine.instance.syncNow(refreshCatalog: false);
      }
      lastStatus = automaticMaterializationEnabled
          ? '刮削完成 ${run.processed}/${run.total}，可确认 ${run.ready}，自动写入 ${materialization.applied}，需复核 ${run.ambiguous + materialization.reviewRequired}'
          : '刮削完成 ${run.processed}/${run.total}，待人工确认 ${run.ready}，自动写入已暂停';
      return run;
    } catch (error) {
      lastStatus = '刮削失败：$error';
      debugPrint('[AutomationCoordinator] scrape $error');
      return null;
    } finally {
      _running = false;
    }
  }

  /// Materialize only proposals that satisfy the production safe-auto policy.
  /// Rust owns the projection transaction; Dart only drains typed results and
  /// reloads its in-memory repositories after a committed batch.
  Future<_MaterializationSummary> _materializeReadyProposals() async {
    final summary = _MaterializationSummary();
    final proposals = await scraperapi.dbLoadScrapeProposals(
      limit: 100000,
      state: 'ready',
    );
    for (final proposal in proposals) {
      if (proposal.materializationStatus == 'applied' ||
          proposal.inputRevision.trim().isEmpty ||
          !_isEmptyJsonArray(proposal.conflictsJson)) {
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

  bool _isEmptyJsonArray(String raw) {
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
