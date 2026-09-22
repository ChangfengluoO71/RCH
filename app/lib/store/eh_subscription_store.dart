import 'dart:async';
import 'dart:convert';

import 'package:app/src/rust/api/eh_subscription.dart' as eh_api;
import 'package:flutter/foundation.dart';
import 'package:path_provider/path_provider.dart';

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
  );
}

/// 连通性预检结果（对应 Rust 侧 `EhProbe`）。
class EhProbe {
  const EhProbe({
    required this.host,
    required this.hostOk,
    required this.hostDetail,
    required this.tracker,
    required this.trackerOk,
    required this.trackerDetail,
  });

  final String host;
  final bool hostOk;
  final String hostDetail;
  final String tracker;
  final bool trackerOk;
  final String trackerDetail;

  static EhProbe fromJson(Map<String, dynamic> j) => EhProbe(
    host: j['host'] as String? ?? '',
    hostOk: j['host_ok'] as bool? ?? false,
    hostDetail: j['host_detail'] as String? ?? '',
    tracker: j['tracker'] as String? ?? '',
    trackerOk: j['tracker_ok'] as bool? ?? false,
    trackerDetail: j['tracker_detail'] as String? ?? '',
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
    this.checked = 0,
    this.saved = 0,
    this.noRating = 0,
    this.noMarker = 0,
    this.noTorrent = 0,
    this.noDownloads = 0,
    this.unmapped = 0,
    this.error,
  });

  final bool running;
  final String stage;
  final String message;
  final int page;
  final int pages;
  final int candidates;
  final int checked;
  final int saved;
  final int noRating;
  final int noMarker;
  final int noTorrent;
  final int noDownloads;
  final int unmapped;
  final String? error;

  static EhProgress fromJson(Map<String, dynamic> j) => EhProgress(
    running: j['running'] as bool? ?? false,
    stage: j['stage'] as String? ?? 'idle',
    message: j['message'] as String? ?? '',
    page: (j['page'] as num?)?.toInt() ?? 0,
    pages: (j['pages'] as num?)?.toInt() ?? 0,
    candidates: (j['candidates'] as num?)?.toInt() ?? 0,
    checked: (j['checked'] as num?)?.toInt() ?? 0,
    saved: (j['saved'] as num?)?.toInt() ?? 0,
    noRating: (j['no_rating'] as num?)?.toInt() ?? 0,
    noMarker: (j['no_marker'] as num?)?.toInt() ?? 0,
    noTorrent: (j['no_torrent'] as num?)?.toInt() ?? 0,
    noDownloads: (j['no_downloads'] as num?)?.toInt() ?? 0,
    unmapped: (j['unmapped'] as num?)?.toInt() ?? 0,
    error: j['error'] as String?,
  );

  /// 规则明细里"是否被挡住"的可读标签。
  String get stageLabel => switch (stage) {
    'searching' => '搜索候选',
    'metadata' => '拉取元数据',
    'filtering' => '筛选与保存',
    'probing' => '查询下载数',
    'done' => '已完成',
    'failed' => '已失败',
    _ => '待运行',
  };
}

/// EH 订阅面板的状态：
/// 规则以 **Rust 侧的 JSON** 为准（单一事实来源），这里只做读写与轮询。
class EhSubscriptionStore extends ChangeNotifier {
  EhSubscriptionStore._();

  static final EhSubscriptionStore instance = EhSubscriptionStore._();

  /// 仅测试使用：构造一个不触碰磁盘/FFI 的实例，用给定规则与清单预置状态，
  /// 便于在无设备环境下渲染面板做视觉核对。
  @visibleForTesting
  factory EhSubscriptionStore.forTest({
    Map<String, dynamic> rules = const {},
    List<EhSavedItem> manifest = const [],
    EhProgress progress = const EhProgress(),
  }) {
    final store = EhSubscriptionStore._();
    if (rules.isNotEmpty) {
      store._rules = Map<String, dynamic>.from(rules);
      store._rulesJson = const JsonEncoder.withIndent('  ').convert(rules);
    }
    store.manifest = manifest;
    store.progress = progress;
    return store;
  }

  static const _pollInterval = Duration(milliseconds: 600);

  String? _rulesPath;
  String _rulesJson = '';
  Map<String, dynamic> _rules = const {};

  List<EhSavedItem> manifest = const [];
  EhProgress progress = const EhProgress();
  EhProbe? probe;
  bool probing = false;
  bool busy = false;
  String? loadError;

  Timer? _poll;

  // ---- 规则读写 ----

  /// 规则字段（UI 直接读写这几个键）。
  String get search => _str('search');
  double get minRating => _num('min_rating', 4.0).toDouble();
  String get titleMarkers => _str('title_markers');
  List<String> get excludeMarkers =>
      (_rules['exclude_markers'] as List?)?.map((e) => e.toString()).toList() ?? const [];
  List<Map<String, dynamic>> get ageTiers =>
      (_rules['age_tiers'] as List?)?.map((e) => Map<String, dynamic>.from(e as Map)).toList() ??
      const [];
  String get outDir => _str('out_dir');
  int get pages => _num('pages', 2).toInt();
  String get host => _str('host');
  double get intervalSecs => _num('request_interval_secs', 2.5).toDouble();

  String _str(String k) => _rules[k] as String? ?? '';
  num _num(String k, num fallback) => (_rules[k] as num?) ?? fallback;

  bool get hasOutDir => outDir.trim().isNotEmpty;

  /// 规则是否已从磁盘/默认值加载完成。
  bool get rulesLoaded => _rules.isNotEmpty;

  Future<void> init() async {
    final support = await getApplicationSupportDirectory();
    final path = '${support.path}/RCH/data/eh_subscription_rules.json';
    _rulesPath = path;
    try {
      _rulesJson = await eh_api.ehLoadRules(path: path);
      _rules = Map<String, dynamic>.from(jsonDecode(_rulesJson) as Map);
      loadError = null;
    } catch (e) {
      loadError = '$e';
    }
    await refreshManifest();
    notifyListeners();
  }

  /// 局部更新规则字段（保持其余字段不变，符合"字段即用户可编辑键"的设计）。
  void patch(String key, Object? value) {
    _rules = {..._rules, key: value};
    _rulesJson = const JsonEncoder.withIndent('  ').convert(_rules);
    notifyListeners();
  }

  Future<void> saveRules() async {
    final path = _rulesPath;
    if (path == null) return;
    await eh_api.ehSaveRules(path: path, rulesJson: _rulesJson);
  }

  Future<void> resetToDefaults() async {
    _rulesJson = await eh_api.ehDefaultRules();
    _rules = Map<String, dynamic>.from(jsonDecode(_rulesJson) as Map);
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

  Future<void> refreshManifest() async {
    if (!hasOutDir) {
      manifest = const [];
      notifyListeners();
      return;
    }
    try {
      final raw = await eh_api.ehManifest(outDir: outDir);
      final list = jsonDecode(raw) as List;
      manifest = list
          .map((e) => EhSavedItem.fromJson(Map<String, dynamic>.from(e as Map)))
          .toList()
          .reversed
          .toList();
    } catch (_) {
      manifest = const [];
    }
    notifyListeners();
  }

  /// 连通性预检（主站 / tracker 分别判定，便于定位"到底是哪一段不通"）。
  Future<EhProbe?> runProbe() async {
    if (probing) return probe;
    probing = true;
    notifyListeners();
    try {
      final raw = await eh_api.ehProbe(rulesJson: _rulesJson);
      probe = EhProbe.fromJson(Map<String, dynamic>.from(jsonDecode(raw) as Map));
    } catch (e) {
      probe = EhProbe(
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

  Future<void> run() async {
    if (busy || !hasOutDir) return;
    busy = true;
    progress = const EhProgress(running: true, stage: 'searching', message: '准备中…');
    notifyListeners();
    await saveRules();
    _startPolling();
    try {
      final raw = await eh_api.ehCollect(rulesJson: _rulesJson);
      progress = EhProgress.fromJson(Map<String, dynamic>.from(jsonDecode(raw) as Map));
    } catch (e) {
      progress = EhProgress(stage: 'failed', message: '$e', error: '$e');
    } finally {
      _stopPolling();
      busy = false;
      await refreshManifest();
      notifyListeners();
    }
  }

  Future<void> cancel() async {
    await eh_api.ehCancel();
  }

  void _startPolling() {
    _poll?.cancel();
    _poll = Timer.periodic(_pollInterval, (_) async {
      try {
        final raw = await eh_api.ehProgress();
        progress = EhProgress.fromJson(Map<String, dynamic>.from(jsonDecode(raw) as Map));
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
