import 'dart:async';
import 'dart:convert';

import 'package:app/store/eh_daily_coordinator.dart';
import 'package:app/src/rust/api/eh_subscription.dart' as eh_api;
import 'package:app/store/long_task_registry.dart';
import 'package:flutter/foundation.dart';
import 'package:path_provider/path_provider.dart';

typedef EhRulesWriter = Future<void> Function(String rulesJson);
typedef EhCollectRunner = Future<String> Function(String rulesJson);
typedef EhDailyCloudStageRunner =
    Future<EhDailyCloudStageResult> Function(
      List<EhCheckedManifestEntry> entries,
    );

/// 一条已保存的种子（对应 Rust 侧 `EhSavedItem` 的 JSON 形状）。
class EhSavedItem {
  const EhSavedItem({
    required this.infohash,
    required this.titleJpn,
    required this.rating,
    required this.downloads,
    required this.requiredDl,
    required this.postedUtc,
    required this.ageYears,
    required this.category,
    required this.filecount,
    required this.filesize,
    required this.tags,
    required this.file,
    required this.bytes,
    this.torrentName = '',
    this.savedAtUtc = '未知',
  });

  final String infohash;
  final String titleJpn;
  final double rating;
  final int downloads;
  final int requiredDl;
  final String postedUtc;
  final double? ageYears;
  final String category;
  final int? filecount;
  final int? filesize;
  final List<String> tags;
  final String file;
  final int bytes;
  final String torrentName;
  final String savedAtUtc;

  static EhSavedItem fromJson(Map<String, dynamic> j) => EhSavedItem(
    infohash: j['infohash'] as String? ?? '',
    titleJpn: j['title_jpn'] as String? ?? '',
    rating: (j['rating'] as num?)?.toDouble() ?? 0,
    downloads: (j['downloads'] as num?)?.toInt() ?? 0,
    requiredDl: (j['required_dl'] as num?)?.toInt() ?? 0,
    postedUtc: j['posted_utc'] as String? ?? '未知',
    ageYears: (j['age_years'] as num?)?.toDouble(),
    category: j['category'] as String? ?? '',
    filecount: (j['filecount'] as num?)?.toInt(),
    filesize: (j['filesize'] as num?)?.toInt(),
    tags: (j['tags'] as List?)?.map((e) => e.toString()).toList() ?? const [],
    file: j['file'] as String? ?? '',
    bytes: (j['bytes'] as num?)?.toInt() ?? 0,
    torrentName: j['torrent_name'] as String? ?? '',
    savedAtUtc: j['saved_at_utc'] as String? ?? '未知',
  );
}

enum EhManifestReadStatus {
  unconfigured,
  missing,
  empty,
  ready,
  damaged,
  unreadable,
}

enum EhSavedFileStatus {
  present,
  missing,
  unsafePath,
  notRegularFile,
  unreadable,
  notChecked,
}

enum EhTorrentValidationStatus {
  valid,
  sizeMismatch,
  invalidTorrent,
  infohashMismatch,
  tooLarge,
  unreadable,
  notChecked,
}

class EhCheckedManifestEntry {
  const EhCheckedManifestEntry({
    required this.item,
    required this.fileStatus,
    required this.validationStatus,
    required this.fullPath,
  });

  final EhSavedItem item;
  final EhSavedFileStatus fileStatus;
  final EhTorrentValidationStatus validationStatus;
  final String? fullPath;

  static EhCheckedManifestEntry fromJson(Map<String, dynamic> json) =>
      EhCheckedManifestEntry(
        item: EhSavedItem.fromJson(
          Map<String, dynamic>.from(json['item'] as Map),
        ),
        fileStatus: switch (json['file_status']) {
          'present' => EhSavedFileStatus.present,
          'missing' => EhSavedFileStatus.missing,
          'unsafe_path' => EhSavedFileStatus.unsafePath,
          'not_regular_file' => EhSavedFileStatus.notRegularFile,
          'unreadable' => EhSavedFileStatus.unreadable,
          _ => EhSavedFileStatus.notChecked,
        },
        validationStatus: switch (json['validation_status']) {
          'valid' => EhTorrentValidationStatus.valid,
          'size_mismatch' => EhTorrentValidationStatus.sizeMismatch,
          'invalid_torrent' => EhTorrentValidationStatus.invalidTorrent,
          'infohash_mismatch' => EhTorrentValidationStatus.infohashMismatch,
          'too_large' => EhTorrentValidationStatus.tooLarge,
          'unreadable' => EhTorrentValidationStatus.unreadable,
          _ => EhTorrentValidationStatus.notChecked,
        },
        fullPath: json['full_path'] as String?,
      );
}

class EhManifestReport {
  const EhManifestReport({
    required this.status,
    this.entries = const [],
    this.invalidEntries = 0,
    this.errorCode,
  });

  final EhManifestReadStatus status;
  final List<EhCheckedManifestEntry> entries;
  final int invalidEntries;
  final String? errorCode;

  factory EhManifestReport.fromJson(Map<String, dynamic> json) {
    final rawEntries = json['entries'] as List? ?? const [];
    return EhManifestReport(
      status: switch (json['status']) {
        'unconfigured' => EhManifestReadStatus.unconfigured,
        'missing' => EhManifestReadStatus.missing,
        'empty' => EhManifestReadStatus.empty,
        'ready' => EhManifestReadStatus.ready,
        'damaged' => EhManifestReadStatus.damaged,
        _ => EhManifestReadStatus.unreadable,
      },
      entries: rawEntries
          .map(
            (entry) => EhCheckedManifestEntry.fromJson(
              Map<String, dynamic>.from(entry as Map),
            ),
          )
          .toList(),
      invalidEntries: (json['invalid_entries'] as num?)?.toInt() ?? 0,
      errorCode: json['error_code'] as String?,
    );
  }

  factory EhManifestReport.fromItems(Iterable<EhSavedItem> items) {
    final entries = items
        .map(
          (item) => EhCheckedManifestEntry(
            item: item,
            fileStatus: EhSavedFileStatus.notChecked,
            validationStatus: EhTorrentValidationStatus.notChecked,
            fullPath: null,
          ),
        )
        .toList();
    return EhManifestReport(
      status: entries.isEmpty
          ? EhManifestReadStatus.empty
          : EhManifestReadStatus.ready,
      entries: entries,
    );
  }
}

typedef EhCheckedManifestLoader = Future<String> Function(String outDir);
typedef EhTorrentDirectoryLoader = Future<String> Function(String outDir);

/// 连通性预检结果（对应 Rust 侧 `EhProbe`）。
class EhProbe {
  const EhProbe({
    this.mode = 'system',
    this.checkedAtUnixMs = 0,
    required this.host,
    required this.hostOk,
    required this.hostDetail,
    this.hostElapsedMs = 0,
    this.hostChallengeSuspected = false,
    required this.tracker,
    required this.trackerOk,
    required this.trackerDetail,
    this.trackerElapsedMs = 0,
    this.trackerChallengeSuspected = false,
    this.trackerTorrentValidated = false,
  });

  final String mode;
  final int checkedAtUnixMs;
  final String host;
  final bool hostOk;
  final String hostDetail;
  final int hostElapsedMs;
  final bool hostChallengeSuspected;
  final String tracker;
  final bool trackerOk;
  final String trackerDetail;
  final int trackerElapsedMs;
  final bool trackerChallengeSuspected;
  final bool trackerTorrentValidated;

  static EhProbe fromJson(Map<String, dynamic> j) => EhProbe(
    mode: j['mode'] as String? ?? 'system',
    checkedAtUnixMs: (j['checked_at_unix_ms'] as num?)?.toInt() ?? 0,
    host: j['host'] as String? ?? '',
    hostOk: j['host_ok'] as bool? ?? false,
    hostDetail: j['host_detail'] as String? ?? '',
    hostElapsedMs: (j['host_elapsed_ms'] as num?)?.toInt() ?? 0,
    hostChallengeSuspected: j['host_challenge_suspected'] as bool? ?? false,
    tracker: j['tracker'] as String? ?? '',
    trackerOk: j['tracker_ok'] as bool? ?? false,
    trackerDetail: j['tracker_detail'] as String? ?? '',
    trackerElapsedMs: (j['tracker_elapsed_ms'] as num?)?.toInt() ?? 0,
    trackerChallengeSuspected:
        j['tracker_challenge_suspected'] as bool? ?? false,
    trackerTorrentValidated: j['tracker_torrent_validated'] as bool? ?? false,
  );
}

/// 一轮扫描的进度/统计（对应 Rust 侧 `EhProgress`）。
class EhProgress {
  const EhProgress({
    this.running = false,
    this.stage = 'idle',
    this.message = '',
    this.page = 0,
    this.pages = 0,
    this.candidates = 0,
    this.processed = 0,
    this.workTotal = 0,
    this.checked = 0,
    this.saved = 0,
    this.noRating = 0,
    this.noMarker = 0,
    this.noTorrent = 0,
    this.noDownloads = 0,
    this.unmapped = 0,
    this.activityStartedAtUnixMs = 0,
    this.error,
  });

  final bool running;
  final String stage;
  final String message;
  final int page;
  final int pages;
  final int candidates;
  final int processed;
  final int workTotal;
  final int checked;
  final int saved;
  final int noRating;
  final int noMarker;
  final int noTorrent;
  final int noDownloads;
  final int unmapped;
  final int activityStartedAtUnixMs;
  final String? error;

  static EhProgress fromJson(Map<String, dynamic> j) => EhProgress(
    running: j['running'] as bool? ?? false,
    stage: j['stage'] as String? ?? 'idle',
    message: j['message'] as String? ?? '',
    page: (j['page'] as num?)?.toInt() ?? 0,
    pages: (j['pages'] as num?)?.toInt() ?? 0,
    candidates: (j['candidates'] as num?)?.toInt() ?? 0,
    processed: (j['processed'] as num?)?.toInt() ?? 0,
    workTotal: (j['work_total'] as num?)?.toInt() ?? 0,
    checked: (j['checked'] as num?)?.toInt() ?? 0,
    saved: (j['saved'] as num?)?.toInt() ?? 0,
    noRating: (j['no_rating'] as num?)?.toInt() ?? 0,
    noMarker: (j['no_marker'] as num?)?.toInt() ?? 0,
    noTorrent: (j['no_torrent'] as num?)?.toInt() ?? 0,
    noDownloads: (j['no_downloads'] as num?)?.toInt() ?? 0,
    unmapped: (j['unmapped'] as num?)?.toInt() ?? 0,
    activityStartedAtUnixMs:
        (j['activity_started_at_unix_ms'] as num?)?.toInt() ?? 0,
    error: j['error'] as String?,
  );

  EhProgress copyWith({
    bool? running,
    String? stage,
    String? message,
    String? error,
  }) => EhProgress(
    running: running ?? this.running,
    stage: stage ?? this.stage,
    message: message ?? this.message,
    page: page,
    pages: pages,
    candidates: candidates,
    processed: processed,
    workTotal: workTotal,
    checked: checked,
    saved: saved,
    noRating: noRating,
    noMarker: noMarker,
    noTorrent: noTorrent,
    noDownloads: noDownloads,
    unmapped: unmapped,
    activityStartedAtUnixMs: activityStartedAtUnixMs,
    error: error ?? this.error,
  );

  /// 规则明细里"是否被挡住"的可读标签。
  String get stageLabel => switch (stage) {
    'preparing' => '准备扫描',
    'searching' => '搜索候选',
    'metadata' => '拉取元数据',
    'filtering' => '筛选与保存',
    'probing' => '查询下载数',
    'done' => '已完成',
    'failed' => '已失败',
    'cancelled' => '已取消',
    'downloading_torrent' => '下载 Torrent',
    _ => '待运行',
  };
}

/// EH 订阅面板的状态：
/// 规则以 **Rust 侧的 JSON** 为准（单一事实来源），这里只做读写与轮询。
class EhSubscriptionStore extends ChangeNotifier implements EhDailyHost {
  EhSubscriptionStore._();

  static const officialHost = 'e-hentai.org';

  static final EhSubscriptionStore instance = EhSubscriptionStore._();

  /// 仅测试使用：构造一个不触碰磁盘/FFI 的实例，用给定规则与清单预置状态，
  /// 便于在无设备环境下渲染面板做视觉核对。
  @visibleForTesting
  factory EhSubscriptionStore.forTest({
    Map<String, dynamic> rules = const {},
    List<EhSavedItem> manifest = const [],
    EhManifestReport? manifestReport,
    EhManifestReport? torrentDirectoryReport,
    EhCheckedManifestLoader? checkedManifestLoader,
    EhTorrentDirectoryLoader? torrentDirectoryLoader,
    bool autoRefreshTorrentDirectory = false,
    VoidCallback? onScanRequested,
    EhRulesWriter? rulesWriter,
    EhCollectRunner? collectRunner,
    EhProgress progress = const EhProgress(),
    String? rulesPath,
    bool dailyWorkflowReady = true,
    String dailyWorkflowGateMessage = 'P0_REAL_PASS required',
    EhDailyCloudStageRunner? dailyCloudStageRunner,
  }) {
    final store = EhSubscriptionStore._();
    if (rules.isNotEmpty) {
      store._rules = Map<String, dynamic>.from(rules);
      store._normalizeHost();
      store._encodeRules();
    }
    store.manifestReport =
        manifestReport ?? EhManifestReport.fromItems(manifest.reversed);
    store.torrentDirectoryReport =
        torrentDirectoryReport ??
        const EhManifestReport(status: EhManifestReadStatus.unconfigured);
    store.manifest = manifestReport == null
        ? manifest
        : manifestReport.entries
              .map((entry) => entry.item)
              .toList()
              .reversed
              .toList();
    store._checkedManifestLoader = checkedManifestLoader;
    store._torrentDirectoryLoader = torrentDirectoryLoader;
    store.autoRefreshTorrentDirectory = autoRefreshTorrentDirectory;
    store._onScanRequested = onScanRequested;
    store._rulesWriter = rulesWriter;
    store._collectRunner = collectRunner;
    store._dailyWorkflowReadyOverride = dailyWorkflowReady;
    store._dailyWorkflowGateMessageOverride = dailyWorkflowGateMessage;
    store._dailyCloudStageRunner = dailyCloudStageRunner;
    store.progress = progress;
    if (rulesPath != null) {
      store._rulesPath = rulesPath;
      store._persistedRulesJson = store._rulesJson;
      store._initialized = store._rules.isNotEmpty;
    }
    return store;
  }

  static const _pollInterval = Duration(milliseconds: 600);

  String? _rulesPath;
  String _rulesJson = '';
  String? _persistedRulesJson;
  Map<String, dynamic> _rules = const {};
  Future<void> _rulesWriteTail = Future<void>.value();
  Future<void>? _initTask;
  bool _initialized = false;

  List<EhSavedItem> manifest = const [];
  EhManifestReport manifestReport = const EhManifestReport(
    status: EhManifestReadStatus.unconfigured,
  );

  /// Direct inventory of top-level `.torrent` files under the configured
  /// output directory. It is independent of `manifest.json`.
  EhManifestReport torrentDirectoryReport = const EhManifestReport(
    status: EhManifestReadStatus.unconfigured,
  );
  bool refreshingManifest = false;
  bool refreshingTorrentDirectory = false;
  bool autoRefreshTorrentDirectory = true;
  EhProgress progress = const EhProgress();
  EhProbe? probe;
  bool probing = false;
  @override
  bool busy = false;
  String? loadError;
  EhCheckedManifestLoader? _checkedManifestLoader;
  EhTorrentDirectoryLoader? _torrentDirectoryLoader;
  VoidCallback? _onScanRequested;
  EhRulesWriter? _rulesWriter;
  EhCollectRunner? _collectRunner;
  bool _cancelRequested = false;
  bool? _dailyWorkflowReadyOverride;
  String? _dailyWorkflowGateMessageOverride;
  bool Function()? _dailyWorkflowReadiness;
  String Function()? _dailyWorkflowBlockedMessage;
  EhDailyCloudStageRunner? _dailyCloudStageRunner;

  Timer? _poll;

  // ---- 规则读写 ----

  /// 规则字段（UI 直接读写这几个键）。
  String get search => _str('search');
  double get minRating => _num('min_rating', 4.0).toDouble();
  String get titleMarkers => _str('title_markers');
  List<String> get excludeMarkers =>
      (_rules['exclude_markers'] as List?)?.map((e) => e.toString()).toList() ??
      const [];
  List<Map<String, dynamic>> get ageTiers =>
      (_rules['age_tiers'] as List?)
          ?.map((e) => Map<String, dynamic>.from(e as Map))
          .toList() ??
      const [];
  String get outDir => _str('out_dir');
  int get pages => _num('pages', 2).toInt();
  String get host => officialHost;
  double get intervalSecs => _num('request_interval_secs', 2.5).toDouble();
  Map<String, dynamic> get networkSettings =>
      Map<String, dynamic>.from(_rules['network'] as Map? ?? const {});
  String get proxyMode {
    final value = networkSettings['mode'] as String? ?? 'system';
    return const {'system', 'direct', 'manual'}.contains(value)
        ? value
        : 'system';
  }

  String get proxyUrl => networkSettings['proxy_url'] as String? ?? '';

  @override
  bool get dailyReady => _initialized && loadError == null;

  @override
  bool get dailyWorkflowReady =>
      _dailyWorkflowReadiness?.call() ?? _dailyWorkflowReadyOverride ?? false;

  @override
  String get dailyWorkflowGateMessage =>
      _dailyWorkflowBlockedMessage?.call() ??
      _dailyWorkflowGateMessageOverride ??
      '115 每日自动写入仍被 P0_REAL_PASS 门禁锁定。';

  void configureDailyWorkflowGate({
    required bool Function() isReady,
    required String Function() blockedMessage,
    required EhDailyCloudStageRunner runCloudStage,
  }) {
    _dailyWorkflowReadiness = isReady;
    _dailyWorkflowBlockedMessage = blockedMessage;
    _dailyCloudStageRunner = runCloudStage;
    notifyListeners();
  }

  @override
  EhDailyAutomationState get dailyAutomation {
    final raw = _rules['automation'];
    return raw is Map
        ? EhDailyAutomationState.fromJson(Map<String, Object?>.from(raw))
        : const EhDailyAutomationState();
  }

  String _str(String k) => _rules[k] as String? ?? '';
  num _num(String k, num fallback) => (_rules[k] as num?) ?? fallback;

  bool _normalizeHost() {
    if (_rules['host'] == officialHost) return false;
    _rules = {..._rules, 'host': officialHost};
    _encodeRules();
    return true;
  }

  void _encodeRules() {
    _rulesJson = const JsonEncoder.withIndent('  ').convert(_rules);
  }

  @override
  bool get hasOutDir => outDir.trim().isNotEmpty;

  /// 规则是否已从磁盘/默认值加载完成。
  bool get rulesLoaded => _rules.isNotEmpty;

  Future<void> init() {
    if (_initialized) return Future<void>.value();
    final active = _initTask;
    if (active != null) return active;
    final task = _initInternal();
    _initTask = task;
    return task.whenComplete(() {
      if (identical(_initTask, task)) _initTask = null;
    });
  }

  Future<void> _initInternal() async {
    try {
      final support = await getApplicationSupportDirectory();
      final path = '${support.path}/RCH/data/eh_subscription_rules.json';
      _rulesPath = path;
      try {
        _rulesJson = await eh_api.ehLoadRules(path: path);
        _persistedRulesJson = _rulesJson;
        _rules = Map<String, dynamic>.from(jsonDecode(_rulesJson) as Map);
        if (_normalizeHost()) await saveRules();
        loadError = null;
      } catch (e) {
        loadError = '$e';
      }
      await refreshManifest();
      _initialized = loadError == null && _rules.isNotEmpty;
    } catch (e) {
      loadError = '$e';
      _initialized = false;
    }
    notifyListeners();
  }

  /// 局部更新规则字段（保持其余字段不变，符合"字段即用户可编辑键"的设计）。
  void patch(String key, Object? value) {
    _rules = {..._rules, key: key == 'host' ? officialHost : value};
    _normalizeHost();
    _encodeRules();
    notifyListeners();
  }

  void patchNetwork({String? mode, String? proxyUrl, bool notify = true}) {
    final network = networkSettings;
    _rules = {
      ..._rules,
      'network': {...network, 'mode': ?mode, 'proxy_url': ?proxyUrl},
    };
    _encodeRules();
    if (notify) notifyListeners();
  }

  Future<void> saveRules() async {
    _normalizeHost();
    _encodeRules();
    final path = _rulesPath;
    final writer = _rulesWriter;
    if (path == null && writer == null) return;

    final write = _rulesWriteTail.catchError((Object _) {}).then((_) async {
      final currentJson = _rulesJson;
      if (writer != null) {
        await writer(currentJson);
        _persistedRulesJson = currentJson;
      } else {
        final savedJson = await eh_api.ehSaveRules(
          path: path!,
          baseJson: _persistedRulesJson ?? currentJson,
          rulesJson: currentJson,
        );
        _applyPersistedRules(savedJson, submittedJson: currentJson);
      }
    });
    _rulesWriteTail = write.catchError((Object _) {});
    await write;
  }

  Future<void> saveDailyAutomation(EhDailyAutomationState state) async {
    if (_rulesPath != null && _rulesWriter == null) {
      throw StateError('生产 Daily 状态必须通过专用的原子操作写入。');
    }
    final previous = _rules;
    final updated = {..._rules, 'automation': state.toJson()};
    _rules = updated;
    _encodeRules();
    notifyListeners();
    try {
      await saveRules();
    } catch (_) {
      if (identical(_rules, updated)) {
        _rules = previous;
        _encodeRules();
        notifyListeners();
      }
      rethrow;
    }
  }

  @override
  Future<void> setDailyEnabled(bool enabled) async {
    if (enabled && !dailyWorkflowReady) {
      throw StateError(dailyWorkflowGateMessage);
    }
    final path = _rulesPath;
    if (path == null || _rulesWriter != null) {
      await saveDailyAutomation(
        dailyAutomation.copyWith(dailyEnabled: enabled),
      );
      return;
    }
    final savedJson = await eh_api.ehSetDailyEnabled(
      path: path,
      enabled: enabled,
    );
    _applyPersistedRules(savedJson);
  }

  @override
  Future<bool> claimDailyRun(String date) async {
    if (!dailyWorkflowReady) return false;
    final path = _rulesPath;
    if (path == null || _rulesWriter != null) {
      final current = dailyAutomation;
      if (!current.dailyEnabled ||
          (current.lastAutoAttemptDate != null &&
              date.compareTo(current.lastAutoAttemptDate!) <= 0)) {
        return false;
      }
      await saveDailyAutomation(
        current.copyWith(
          lastAutoAttemptDate: date,
          lastRun: EhDailyRunRecord(
            date: date,
            source: EhDailyRunSource.dailyAuto,
            status: EhDailyRunStatus.claimed,
          ),
        ),
      );
      return true;
    }

    final result =
        jsonDecode(await eh_api.ehClaimDaily(path: path, date: date)) as Map;
    _applyPersistedRules(result['rules_json'] as String);
    return result['claimed'] as bool? ?? false;
  }

  @override
  Future<void> updateDailyRun(EhDailyRunRecord run) async {
    final path = _rulesPath;
    if (path == null || _rulesWriter != null) {
      await saveDailyAutomation(dailyAutomation.copyWith(lastRun: run));
      return;
    }
    final savedJson = await eh_api.ehUpdateDailyRun(
      path: path,
      runJson: jsonEncode(run.toJson()),
    );
    _applyPersistedRules(savedJson);
  }

  @override
  Future<void> recoverDailyRun() async {
    final path = _rulesPath;
    if (path == null || _rulesWriter != null) {
      final current = dailyAutomation.lastRun;
      if (current == null ||
          (current.status != EhDailyRunStatus.claimed &&
              current.status != EhDailyRunStatus.running)) {
        return;
      }
      await saveDailyAutomation(
        dailyAutomation.copyWith(
          lastRun: current.copyWith(
            status: EhDailyRunStatus.interruptedUnknown,
            error: '上次进程退出时任务尚未结束，远端结果未知；本轮不会自动重试。',
          ),
        ),
      );
      return;
    }
    final savedJson = await eh_api.ehRecoverDailyRun(path: path);
    _applyPersistedRules(savedJson);
  }

  void _applyPersistedRules(String savedJson, {String? submittedJson}) {
    final persisted = Map<String, dynamic>.from(jsonDecode(savedJson) as Map);
    final submitted = Map<String, dynamic>.from(
      jsonDecode(submittedJson ?? _persistedRulesJson ?? savedJson) as Map,
    );
    final merged = Map<String, dynamic>.from(_rules);
    for (final key in {...submitted.keys, ...persisted.keys, ...merged.keys}) {
      final liveMatchesSubmitted =
          merged.containsKey(key) == submitted.containsKey(key) &&
          jsonEncode(merged[key]) == jsonEncode(submitted[key]);
      if (!liveMatchesSubmitted) continue;
      if (persisted.containsKey(key)) {
        merged[key] = persisted[key];
      } else {
        merged.remove(key);
      }
    }
    _rules = merged;
    _persistedRulesJson = savedJson;
    _encodeRules();
    notifyListeners();
  }

  Future<void> resetToDefaults() async {
    _rulesJson = await eh_api.ehDefaultRules();
    _rules = Map<String, dynamic>.from(jsonDecode(_rulesJson) as Map);
    _normalizeHost();
    _encodeRules();
    await saveRules();
    notifyListeners();
  }

  // ---- 运行 ----

  /// 影子模式：规划一次"从 manifest 导入"（**不写库**），返回计划 JSON 解码后的 Map。
  ///
  /// 候选来自已落盘的 manifest，因此离线可复现；`snapshot` 为本地现状
  /// （`{author, series, summary, tags}`），用于"只填空、不覆盖"的判断。
  Future<Map<String, dynamic>?> planImport({
    required String workTitle,
    required List<String> creators,
    required Map<String, dynamic> snapshot,
  }) async {
    if (!rulesLoaded) await init();
    if (!hasOutDir) return null;
    final raw = await eh_api.ehPlanImport(
      manifestDir: outDir,
      workTitle: workTitle,
      creatorsJson: jsonEncode(creators),
      snapshotJson: jsonEncode(snapshot),
    );
    return Map<String, dynamic>.from(jsonDecode(raw) as Map);
  }

  /// **实时搜索**规划一本书的导入（影子模式，不写库）。
  ///
  /// 与 [planImport]（读 manifest、离线）不同：候选来自实时搜索 E 站，
  /// 不受"manifest 只含订阅命中项"的限制——这是识别率的关键。
  Future<Map<String, dynamic>?> planImportLive({
    required String workTitle,
    required List<String> creators,
    required Map<String, dynamic> snapshot,
    String number = '',
  }) async {
    if (!rulesLoaded) await init();
    _normalizeHost();
    _encodeRules();
    final raw = await eh_api.ehPlanBookLive(
      rulesJson: _rulesJson,
      workTitle: workTitle,
      creatorsJson: jsonEncode(creators),
      snapshotJson: jsonEncode(snapshot),
      number: number,
    );
    return Map<String, dynamic>.from(jsonDecode(raw) as Map);
  }

  Future<void> refreshManifest() async {
    if (!hasOutDir) {
      manifest = const [];
      manifestReport = const EhManifestReport(
        status: EhManifestReadStatus.unconfigured,
      );
      notifyListeners();
      return;
    }
    if (refreshingManifest) return;
    refreshingManifest = true;
    notifyListeners();
    try {
      final loader = _checkedManifestLoader;
      final raw = loader == null
          ? await eh_api.ehManifestChecked(outDir: outDir)
          : await loader(outDir);
      manifestReport = EhManifestReport.fromJson(
        Map<String, dynamic>.from(jsonDecode(raw) as Map),
      );
      manifest = manifestReport.entries
          .map((entry) => entry.item)
          .toList()
          .reversed
          .toList();
    } catch (_) {
      manifest = const [];
      manifestReport = const EhManifestReport(
        status: EhManifestReadStatus.unreadable,
        errorCode: 'checked_read_failed',
      );
    } finally {
      refreshingManifest = false;
    }
    notifyListeners();
  }

  /// Enumerate actual top-level Torrent files under the configured output
  /// directory. This read never parses or depends on `manifest.json`.
  Future<void> refreshTorrentDirectory() async {
    if (!hasOutDir) {
      torrentDirectoryReport = const EhManifestReport(
        status: EhManifestReadStatus.unconfigured,
      );
      notifyListeners();
      return;
    }
    if (refreshingTorrentDirectory) return;
    refreshingTorrentDirectory = true;
    notifyListeners();
    try {
      final loader = _torrentDirectoryLoader;
      final raw = loader == null
          ? await eh_api.ehTorrentsChecked(outDir: outDir)
          : await loader(outDir);
      torrentDirectoryReport = EhManifestReport.fromJson(
        Map<String, dynamic>.from(jsonDecode(raw) as Map),
      );
    } catch (_) {
      torrentDirectoryReport = const EhManifestReport(
        status: EhManifestReadStatus.unreadable,
        errorCode: 'torrent_directory_check_failed',
      );
    } finally {
      refreshingTorrentDirectory = false;
    }
    notifyListeners();
  }

  /// 连通性预检（主站 / tracker 分别判定，便于定位"到底是哪一段不通"）。
  Future<EhProbe?> runProbe() async {
    if (probing) return probe;
    if (!rulesLoaded) await init();
    _normalizeHost();
    _encodeRules();
    probing = true;
    notifyListeners();
    try {
      final raw = await eh_api.ehProbe(rulesJson: _rulesJson);
      probe = EhProbe.fromJson(
        Map<String, dynamic>.from(jsonDecode(raw) as Map),
      );
    } catch (e) {
      probe = EhProbe(
        mode: proxyMode,
        host: host,
        hostOk: false,
        hostDetail: '$e',
        tracker: 'ehtracker.org',
        trackerOk: false,
        trackerDetail: '未探测',
      );
    } finally {
      probing = false;
      notifyListeners();
    }
    return probe;
  }

  @override
  Future<EhRunOutcome> run({
    EhDailyRunSource source = EhDailyRunSource.manual,
  }) async {
    if (busy || !hasOutDir) return EhRunOutcome.busy;
    if (source == EhDailyRunSource.dailyAuto && !dailyWorkflowReady) {
      return EhRunOutcome(
        started: false,
        status: EhDailyRunStatus.failed,
        error: dailyWorkflowGateMessage,
      );
    }
    final initialAutomation = dailyAutomation;
    if (source == EhDailyRunSource.manual &&
        initialAutomation.lastRun?.source == EhDailyRunSource.dailyAuto &&
        initialAutomation.lastRun?.status == EhDailyRunStatus.claimed) {
      return EhRunOutcome.busy;
    }
    final longTask = LongTaskRegistry.instance.register(
      kind: source == EhDailyRunSource.dailyAuto
          ? 'eh_daily_subscription'
          : 'eh_subscription_scan',
      label: source == EhDailyRunSource.dailyAuto ? 'EH 每日订阅扫描' : 'EH 订阅扫描',
      phase: '准备扫描',
      prepareForExit: _cancelAndWaitForScan,
    );
    _onScanRequested?.call();
    _normalizeHost();
    _encodeRules();
    busy = true;
    progress = const EhProgress(
      running: true,
      stage: 'searching',
      message: '准备中…',
    );
    notifyListeners();
    var started = false;
    var outcome = EhRunOutcome.busy;
    var cloudStage = const EhDailyCloudStageResult();
    var manifestRefreshed = false;
    EhDailyRunRecord? activeRun;
    try {
      final previous = dailyAutomation;
      final runDate =
          source == EhDailyRunSource.dailyAuto &&
              previous.lastRun?.status == EhDailyRunStatus.claimed
          ? previous.lastRun!.date
          : ehLocalDateKey(DateTime.now());
      activeRun = EhDailyRunRecord(
        date: runDate,
        source: source,
        status: EhDailyRunStatus.running,
      );
      await updateDailyRun(activeRun);
      started = true;
      longTask.update(phase: '检索 EH 订阅');
      _startPolling();
      final runner = _collectRunner;
      final raw = runner == null
          ? await eh_api.ehCollect(
              path: _rulesPath!,
              rulesJson: _rulesJson,
              source: source == EhDailyRunSource.dailyAuto
                  ? 'daily_auto'
                  : 'manual',
              date: activeRun.date,
            )
          : await runner(_rulesJson);
      progress = EhProgress.fromJson(
        Map<String, dynamic>.from(jsonDecode(raw) as Map),
      );
      final status = progress.error == null && progress.stage != 'failed'
          ? EhDailyRunStatus.completed
          : EhDailyRunStatus.failed;
      outcome = EhRunOutcome(
        started: true,
        status: status,
        saved: progress.saved,
        error: progress.error,
      );
      longTask.update(phase: '核对本地 Torrent 清单');
      if (source == EhDailyRunSource.dailyAuto &&
          status == EhDailyRunStatus.completed) {
        await refreshManifest();
        manifestRefreshed = true;
        if (manifestReport.status != EhManifestReadStatus.ready &&
            manifestReport.status != EhManifestReadStatus.empty) {
          cloudStage = EhDailyCloudStageResult(
            status: EhDailyCloudStageStatus.failed,
            message: 'EH 清单未通过检查，115 阶段未启动（${manifestReport.status.name}）。',
          );
        } else {
          final cloudRunner = _dailyCloudStageRunner;
          if (cloudRunner == null) {
            cloudStage = EhDailyCloudStageResult(
              status: EhDailyCloudStageStatus.p0Blocked,
              message: dailyWorkflowGateMessage,
            );
          } else {
            // The batch processor owns the durable Cloud115 portion. Release
            // the scan lease before handing work to it so the exit prompt
            // reports one active owner at a time.
            longTask.dispose();
            try {
              cloudStage = await cloudRunner(manifestReport.entries);
            } catch (error) {
              cloudStage = EhDailyCloudStageResult(
                status: EhDailyCloudStageStatus.failed,
                total: manifestReport.entries.length,
                message: _safeDailyError(error),
              );
            }
          }
          if (cloudStage.status != EhDailyCloudStageStatus.completed) {
            outcome = EhRunOutcome(
              started: true,
              status: EhDailyRunStatus.partial,
              saved: progress.saved,
              error: cloudStage.message,
            );
          }
        }
      }
    } catch (e) {
      var latest = progress;
      if (_collectRunner == null) {
        try {
          final rawProgress = await eh_api.ehProgress();
          latest = EhProgress.fromJson(
            Map<String, dynamic>.from(jsonDecode(rawProgress) as Map),
          );
        } catch (_) {
          // Preserve the last polled counters if the final snapshot is unavailable.
        }
      }
      final safeError = _safeDailyError(e);
      progress = latest.copyWith(
        running: false,
        stage: _cancelRequested ? 'cancelled' : 'failed',
        message: safeError,
        error: safeError,
      );
      outcome = EhRunOutcome(
        started: started,
        status: _cancelRequested
            ? EhDailyRunStatus.cancelled
            : EhDailyRunStatus.failed,
        saved: progress.saved,
        error: safeError,
      );
    } finally {
      longTask.dispose();
      _stopPolling();
      busy = false;
      if (!manifestRefreshed) await refreshManifest();
      if (started) {
        try {
          await updateDailyRun(
            activeRun!.copyWith(
              status: outcome.status,
              saved: outcome.saved,
              error: outcome.error,
              cloudStage: cloudStage,
            ),
          );
        } catch (e) {
          // If completion state cannot be persisted, the durable `running`
          // marker remains and startup will recover it as unknown.
          loadError ??= '无法保存 EH 自动化运行结果：$e';
        }
      }
      _cancelRequested = false;
      notifyListeners();
    }
    return outcome;
  }

  Future<void> _cancelAndWaitForScan() async {
    if (!busy) return;
    await cancel();
    final deadline = DateTime.now().add(const Duration(seconds: 10));
    while (busy && DateTime.now().isBefore(deadline)) {
      await Future<void>.delayed(const Duration(milliseconds: 100));
    }
    if (busy) {
      throw TimeoutException(
        'EH cancellation did not reach a saved checkpoint',
      );
    }
  }

  String _safeDailyError(Object error) {
    final value = error.toString().replaceAll(RegExp(r'\s+'), ' ').trim();
    final redacted = value
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
    return redacted.length <= 240 ? redacted : '${redacted.substring(0, 240)}…';
  }

  Future<void> cancel() async {
    _cancelRequested = true;
    await eh_api.ehCancel();
  }

  void _startPolling() {
    _poll?.cancel();
    _poll = Timer.periodic(_pollInterval, (_) async {
      try {
        final raw = await eh_api.ehProgress();
        progress = EhProgress.fromJson(
          Map<String, dynamic>.from(jsonDecode(raw) as Map),
        );
        notifyListeners();
      } catch (_) {
        // 轮询失败不打断主流程
      }
    });
  }

  void _stopPolling() {
    _poll?.cancel();
    _poll = null;
  }

  @override
  void dispose() {
    _stopPolling();
    super.dispose();
  }
}
