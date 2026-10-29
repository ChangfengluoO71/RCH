import 'dart:io';

import 'package:app/src/rust/source/cloud115/offline.dart' as cloud115;
import 'package:app/store/cloud115_bt_batch.dart';
import 'package:app/store/cloud115_bt_monitor.dart';
import 'package:app/store/cloud115_session.dart';
import 'package:app/store/eh_daily_coordinator.dart';
import 'package:app/store/eh_subscription_store.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:path_provider/path_provider.dart';

typedef Cloud115BtDailyBatchRunner =
    Future<Cloud115BtBatchReport> Function(
      BookSource source,
      List<EhCheckedManifestEntry> entries,
      cloud115.Cloud115BtFolderSelection target,
      bool cleanupAfterSubmit,
    );

/// Daily adapter over the same per-item processor used by the foreground UI.
/// The persisted delivery mode can stop the workflow after local Torrent
/// validation without contacting 115.
class Cloud115BtDailyWorkflow {
  Cloud115BtDailyWorkflow({
    this.ledger,
    List<BookSource> Function()? sources,
    bool Function(BookSource source)? cookieSessionIsCached,
    this.restoreCookieSession,
    this.batchRunner,
  }) : _sources = sources ?? (() => LibraryStore.instance.sources),
       _cookieSessionIsCached =
           cookieSessionIsCached ?? hasCachedCloud115CookieSession;

  final Cloud115BtBatchLedger? ledger;
  Cloud115BtBatchLedger? _resolvedLedger;
  final List<BookSource> Function() _sources;
  final bool Function(BookSource source) _cookieSessionIsCached;

  /// Only a background caller opts into restoring the saved Cookie. The
  /// foreground path retains its existing explicit-session behavior.
  final Future<void> Function(BookSource source)? restoreCookieSession;
  final Cloud115BtDailyBatchRunner? batchRunner;

  Future<EhDailyCloudStageResult> run({
    required List<EhCheckedManifestEntry> entries,
    required String outputDir,
  }) async {
    if (entries.isEmpty) {
      return const EhDailyCloudStageResult(
        status: EhDailyCloudStageStatus.completed,
      );
    }

    try {
      final ledger = await _getLedger();
      final preferences = await ledger.selectionPreferences();
      if (preferences.mode == Cloud115BtDeliveryMode.localTorrentOnly) {
        return EhDailyCloudStageResult(
          status: EhDailyCloudStageStatus.notRun,
          total: entries.length,
          message: '已选择非 115 云下载；Torrent 保存在本地目录，本轮未访问 115。',
        );
      }
      final sourceId = preferences.sourceId;
      final target = preferences.target;
      if (sourceId == null || target == null) {
        return EhDailyCloudStageResult(
          status: EhDailyCloudStageStatus.needsConfiguration,
          total: entries.length,
          message: '请在 EH 高级选项中保存 115 Cookie 书源和目标目录。',
        );
      }

      BookSource? source;
      for (final candidate in _sources()) {
        if (candidate.id == sourceId) {
          source = candidate;
          break;
        }
      }
      if (source == null ||
          !source.is115 ||
          source.remoteOnly ||
          (source.cookie ?? '').trim().isEmpty) {
        return EhDailyCloudStageResult(
          status: EhDailyCloudStageStatus.needsConfiguration,
          total: entries.length,
          message: '已保存的 115 Cookie 书源不可用；未发起云端请求。',
        );
      }
      if (!_cookieSessionIsCached(source)) {
        final restore = restoreCookieSession;
        if (restore != null) {
          try {
            await restore(source);
          } catch (_) {
            return EhDailyCloudStageResult(
              status: EhDailyCloudStageStatus.sessionUnavailable,
              total: entries.length,
              message: '无法使用保存的 115 会话；请打开应用检查网络和登录状态。',
            );
          }
        }
      }
      if (!_cookieSessionIsCached(source)) {
        return EhDailyCloudStageResult(
          status: EhDailyCloudStageStatus.sessionUnavailable,
          total: entries.length,
          message: '当前 RCH 进程没有该书源已连接的 Cookie 会话；不会重新登录。',
        );
      }

      final cleanup = await ledger.cleanupAfterSubmit();
      final report = batchRunner == null
          ? await _runExistingBatch(source, entries, target, cleanup, outputDir)
          : await batchRunner!(source, entries, target, cleanup);
      if (batchRunner == null && Cloud115BtMonitor.instance.isStarted) {
        await Cloud115BtMonitor.instance.reconcileFromLedger();
        await Cloud115BtMonitor.instance.pollDueOnce();
      }
      return summarizeBatchReport(report, entries.length);
    } catch (error) {
      return EhDailyCloudStageResult(
        status: EhDailyCloudStageStatus.failed,
        total: entries.length,
        message: _safeSummary(error),
      );
    }
  }

  Future<Cloud115BtBatchLedger> _getLedger() async {
    final existing = _resolvedLedger ?? ledger;
    if (existing != null) return existing;
    final support = await getApplicationSupportDirectory();
    final directory = Directory('${support.path}${Platform.pathSeparator}RCH');
    await directory.create(recursive: true);
    final created = Cloud115BtBatchLedger(
      File('${directory.path}${Platform.pathSeparator}cloud115_bt_batch.jsonl'),
    );
    _resolvedLedger = created;
    return created;
  }

  Future<Cloud115BtBatchReport> _runExistingBatch(
    BookSource source,
    List<EhCheckedManifestEntry> entries,
    cloud115.Cloud115BtFolderSelection target,
    bool cleanupAfterSubmit,
    String outputDir,
  ) async {
    final ledger = await _getLedger();
    return Cloud115BtBatchProcessor(ledger: ledger).run(
      entries: entries,
      target: target,
      sourceId: source.id,
      cleanupAfterSubmit: cleanupAfterSubmit,
      preflight: (entry) => cloud115CookieBtReadonlyPreflightFor(
        source,
        outputDir: outputDir,
        relativeFile: entry.item.file,
        expectedInfoHash: entry.item.infohash,
        target: target,
      ),
      upload: (entry) => cloud115CookieBtUploadAndParseFor(
        source,
        outputDir: outputDir,
        relativeFile: entry.item.file,
        expectedInfoHash: entry.item.infohash,
        target: target,
      ),
      recoverUpload: (entry, pickCode) => cloud115CookieBtConfirmUploadParseFor(
        source,
        outputDir: outputDir,
        relativeFile: entry.item.file,
        expectedInfoHash: entry.item.infohash,
        target: target,
        pickCode: pickCode,
      ),
      submit: (entry, wanted) => cloud115CookieBtSubmitOnceFor(
        source,
        outputDir: outputDir,
        relativeFile: entry.item.file,
        expectedInfoHash: entry.item.infohash,
        target: target,
        wantedIndices: wanted,
      ),
      queryTask: (infoHash) => cloud115CookieBtTaskStatusFor(
        source,
        infoHash: infoHash,
        target: target,
      ),
      deleteTorrent: (entry, accountId, selectedTarget) =>
          cloud115CookieBtDeleteLocalIfRemoteTaskFoundFor(
            source,
            outputDir: outputDir,
            relativeFile: entry.item.file,
            expectedInfoHash: entry.item.infohash,
            expectedAccountId: accountId,
            target: selectedTarget,
          ),
    );
  }

  static EhDailyCloudStageResult summarizeBatchReport(
    Cloud115BtBatchReport report,
    int total,
  ) {
    var submitted = 0;
    var skipped = 0;
    var pending = 0;
    var removed = 0;
    var failed = 0;
    for (final result in report.results) {
      switch (result.state) {
        case 'submitted':
        case 'duplicate_found':
          submitted++;
        case 'local_torrent_removed':
          removed++;
        case 'unknown_remote_state':
        case 'upload_outcome_unknown':
        case 'upload_parse_pending':
        case 'duplicate_lookup_unknown':
        case 'duplicate_found_delete_failed':
        case 'submitted_cleanup_pending':
          pending++;
        case 'local_validation_failed':
        case 'torrent_invalid':
        case 'rejected':
          skipped++;
        case 'preflight_failed':
        case 'waf_blocked':
        case 'rate_limited':
        case 'auth_expired':
        case 'access_denied':
        case 'challenge_required':
          failed++;
        default:
          break;
      }
    }
    final complete =
        !report.stopped &&
        pending == 0 &&
        failed == 0 &&
        skipped == 0 &&
        report.results.length == total;
    return EhDailyCloudStageResult(
      status: complete
          ? EhDailyCloudStageStatus.completed
          : EhDailyCloudStageStatus.partial,
      total: total,
      submitted: submitted,
      skipped: skipped,
      pending: pending,
      removed: removed,
      failed: failed,
      message: complete
          ? '本轮 BT 提交处理已结束；115 云端下载完成与 RCH 书源识别尚未确认。'
          : report.stopped
          ? '115 阶段按安全门禁暂停；未完成 Torrent 保留在本地，其他项目状态保留在幂等账本。'
          : '115 阶段已处理可独立项目；待确认或失败项目已保留，未知写入不会自动重发。',
    );
  }

  String _safeSummary(Object error) {
    final text = error.toString().replaceAll(RegExp(r'\s+'), ' ').trim();
    if (text.isEmpty) return '115 Daily 阶段失败；凭据不会写入状态记录。';
    final redacted = text
        .replaceAll(
          RegExp(r'cookie\s*[:=]\s*[^ ]+', caseSensitive: false),
          'Cookie=[redacted]',
        )
        .replaceAll(
          RegExp(r'authorization\s*[:=]\s*[^ ]+', caseSensitive: false),
          'Authorization=[redacted]',
        )
        .replaceAll(
          RegExp(r'token\s*[:=]\s*[^ ]+', caseSensitive: false),
          'token=[redacted]',
        );
    return redacted.length <= 180 ? redacted : '${redacted.substring(0, 180)}…';
  }
}
