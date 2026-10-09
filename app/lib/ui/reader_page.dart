import 'dart:async';

import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/ai.dart';
import 'package:app/src/rust/api/source.dart' as frbsource;
import 'package:app/store/book_open_coordinator.dart';
import 'package:app/store/ai_upscale_manager.dart';
import 'package:app/src/rust/api/cache.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/library_catalog.dart';
import 'package:app/store/random_read_selector.dart';
import 'package:app/store/quark_session.dart';
import 'package:app/store/remote_scan_coordinator.dart';
import 'package:app/store/remote_scan_models.dart';
import 'package:app/ui/opener.dart';
import 'package:app/store/models.dart';
import 'package:app/store/remote_cache_cleanup.dart';
import 'package:app/ui/common.dart';
import 'package:app/ui/quark_qr_scan.dart';
import 'package:app/ui/webtoon_navigation.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart' show PointerDeviceKind;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:photo_view/photo_view.dart';

class ReaderPage extends StatefulWidget {
  final String path;
  final String title;
  final BigInt? webdavSession;
  final BookSource? source;
  final int initialPage;
  final bool skipAiCache;
  const ReaderPage({
    super.key,
    required this.path,
    required this.title,
    this.webdavSession,
    this.source,
    this.initialPage = 0,
    this.skipAiCache = false,
  });
  @override
  State<ReaderPage> createState() => _ReaderPageState();
}

enum _PageLoadStatus { loading, success, failed }

class _PageLoadState {
  const _PageLoadState({
    required this.status,
    required this.attempts,
    required this.requestGeneration,
    required this.bookHandle,
    this.error,
  });

  final _PageLoadStatus status;
  final int attempts;
  final int requestGeneration;
  final BigInt bookHandle;
  final String? error;
}

class _QueuedPageLoad {
  const _QueuedPageLoad({
    required this.page,
    required this.book,
    required this.openGeneration,
    required this.requestGeneration,
    required this.watch,
  });

  final int page;
  final BookInfo book;
  final int openGeneration;
  final int requestGeneration;
  final Stopwatch watch;
}

class _ReaderPageState extends State<ReaderPage> {
  /// 到达最后一屏后**延迟 3 秒**再提示（不打断最后一页的显示）；
  /// 离开末屏会取消计时并允许下次再提示。
  Timer? _endTimer;
  bool _endPrompted = false;
  BookInfo? _book;
  int _page = 0;
  String? _error;
  BookOpenStage _openStage = BookOpenStage.openingBook;
  BookOpenStage? _failedAtStage;
  int _openGeneration = 0;
  bool _quarkCookieRecoveryAttempted = false;
  bool _refreshingQuarkIndex = false;
  int _initialPageIndex = 0;
  bool _firstPageLogged = false;
  bool _readRecordStarted = false;
  Stopwatch? _firstPageWatch;
  bool _remoteImageFolder = false;
  String? _providerPath;
  final Object _aiReadingOwner = Object();
  bool _randomBookPicking = false;

  /// 下载进度: 0.0~1.0, null=非下载中或已完成。
  double? _downloadProgress;
  final Map<int, Uint8List> _bytes = {};
  final Map<int, _PageLoadState> _pageLoads = {};
  final Map<int, int> _pageRequestGenerations = {};
  final Map<int, _QueuedPageLoad> _queuedPageLoads = {};
  final Set<_QueuedPageLoad> _activePageLoads = {};
  final Set<_QueuedPageLoad> _activePrefetchLoads = {};
  static const int _maxConcurrentPageLoads = 3;
  bool _aiProcessing = false;
  bool _useAiVersion = true;
  bool _rotationMode = false; // 右键「界面旋转」进入旋转模式
  final Map<int, int> _rotations = {}; // pageIndex -> 度数(0/90/180/270)
  /// 单页模式下每个页面独立的缩放控制器。PageView 滑动时新旧页会同时挂载,
  /// 共用控制器会导致新页图片加载回写缩放时旧页跟着跳动。
  final Map<int, PhotoViewController> _photoCtrls = {};
  final Map<int, PhotoViewScaleStateController> _scaleStateCtrls = {};
  final TransformationController _dualZoomCtrl = TransformationController();
  final TransformationController _webtoonZoomCtrl = TransformationController();
  PageController? _pageCtrl;
  bool _dualZoomed = false; // 双页模式已放大(>1)时接管拖拽,否则让给 PageView 翻页
  final FocusNode _focus = FocusNode();
  final ScrollController _webtoonCtrl = ScrollController();

  /// 条漫模式各页实际渲染高度缓存(图片高度不一,滚动时据此定位视口页码)。
  final List<double> _webtoonHeights = [];
  final WebtoonNavigationModel _webtoonNavigation = WebtoonNavigationModel(
    pageCount: 0,
    estimatedHeight: webtoonPlaceholderHeight,
  );
  int _webtoonScrollGeneration = 0;

  /// 条漫滚动锚点守护:视口上方的页由占位高度收敛成真实高度时,等量补偿滚动偏移,
  /// 否则 SliverList 只保持像素偏移 ⇒ 可见内容被整体推走("划着划着突然跳回好几页前")。
  final WebtoonAnchorKeeper _webtoonAnchor = WebtoonAnchorKeeper();
  final GlobalKey _webtoonListKey = GlobalKey();

  /// 条漫程序化滚动(底部按钮/键盘)进行中:期间不做锚点补偿,避免与滚动动画互相拉扯。
  bool _webtoonProgrammaticScroll = false;

  /// 保留显式远跳目标，直到其首帧布局完成，避免布局期间把预取窗口移回列表起点。
  int? _webtoonLoadTarget;
  int? _webtoonPendingAttachPage;
  bool _webtoonAttachCallbackScheduled = false;
  late ReadMode _mode;
  late bool _invert;
  late DualPageMode _dual;
  late int _gap;
  late bool _skipCover;
  late KeyBinds _keys;

  /// 进入阅读器时是否为紧凑（手机）布局：退出时据此恢复竖屏锁定或保持可旋转。
  bool _compactAtOpen = true;
  bool _orientationCaptured = false;
  final ReadingCompletionState _completion = ReadingCompletionState(
    pageCount: 0,
  );
  RemoteBookUseLease? _cleanupLease;
  int _controllerCleanupGeneration = 0;

  // ---- 视口页 ↔ 真实页映射(双页模式一视口对应两页) ----
  ReaderPaging get _paging => ReaderPaging(
    dual: _dual != DualPageMode.off,
    skipCover: _skipCover,
    pageCount: _book?.pageCount ?? 1,
  );
  int _viewCount() => _paging.viewCount;
  int _viewOfPage(int p) => _paging.viewOfPage(p);
  int _pageOfView(int v) => _paging.pageOfView(v);

  /// 重建 PageController(书打开后、阅读设置变更后调用),保证初始视口与 _page 一致。
  void _recreatePageCtrl() {
    _webtoonScrollGeneration++;
    _webtoonProgrammaticScroll = false;
    _webtoonLoadTarget = _mode == ReadMode.webtoon && !_bytes.containsKey(_page)
        ? _page
        : null;
    _webtoonPendingAttachPage = null;
    _webtoonNavigation.cancelPendingForUserGesture();
    final old = _pageCtrl;
    final vc = _viewCount();
    _pageCtrl = PageController(
      initialPage: _book == null || vc <= 0
          ? 0
          : _viewOfPage(_page).clamp(0, vc - 1),
    );
    if (old != null) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) old.dispose();
      });
    }
  }

  PhotoViewController _photoCtrlOf(int page) =>
      _photoCtrls.putIfAbsent(page, () => PhotoViewController());
  PhotoViewScaleStateController _scaleStateCtrlOf(int page) =>
      _scaleStateCtrls.putIfAbsent(page, () => PhotoViewScaleStateController());

  /// Release distant PhotoView controllers only after the frame has detached
  /// their page subtree. Re-check the current keep window and object identity.
  void _disposeDistantPhotoCtrls() {
    final cleanupGeneration = ++_controllerCleanupGeneration;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || cleanupGeneration != _controllerCleanupGeneration) return;
      final keep = <int>{for (var i = _page - 1; i <= _page + 1; i++) i};
      for (final entry in _photoCtrls.entries.toList()) {
        if (keep.contains(entry.key) ||
            !identical(_photoCtrls[entry.key], entry.value)) {
          continue;
        }
        _photoCtrls.remove(entry.key);
        entry.value.dispose();
      }
      for (final entry in _scaleStateCtrls.entries.toList()) {
        if (keep.contains(entry.key) ||
            !identical(_scaleStateCtrls[entry.key], entry.value)) {
          continue;
        }
        _scaleStateCtrls.remove(entry.key);
        entry.value.dispose();
      }
    });
  }

  void _onDualZoomChanged() {
    final zoomed = _dualZoomCtrl.value.getMaxScaleOnAxis() > 1.01;
    if (zoomed != _dualZoomed) setState(() => _dualZoomed = zoomed);
  }

  /// 双击在 1x / 2x 之间切换（条漫、双页模式；单页 PhotoView 自带双击缩放）。
  void _toggleZoomByDoubleTap(TransformationController c) {
    final zoomed = c.value.getMaxScaleOnAxis() > 1.01;
    c.value = zoomed
        ? Matrix4.identity()
        : (Matrix4.identity()..scaleByDouble(2.0, 2.0, 2.0, 1.0));
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (!_orientationCaptured) {
      _orientationCaptured = true;
      _compactAtOpen = isCompact(context);
    }
  }

  @override
  void initState() {
    super.initState();
    _dualZoomCtrl.addListener(_onDualZoomChanged);
    _webtoonCtrl.addListener(_onWebtoonScroll);
    if (defaultTargetPlatform == TargetPlatform.android) {
      SystemChrome.setPreferredOrientations(DeviceOrientation.values);
    }
    final g = LibraryStore.instance.settings;
    _mode = g.readMode;
    _invert = g.invertTap;
    _dual = g.dualPageMode;
    _gap = g.dualPageGap;
    _skipCover = g.skipFrontCover;
    _keys = g.keys;
    final s0 = widget.source;
    _openStage = s0?.needsSession == true
        ? BookOpenStage.connecting
        : BookOpenStage.openingBook;
    if (s0 != null) {
      _rotations.addAll(
        LibraryStore.instance.metaOf(s0, widget.path).rotations,
      );
      _cleanupLease = RemoteBookUseRegistry.instance.acquire(
        source: s0,
        path: widget.path,
        enabled: true,
        strategy: g.bookOpenStrategy,
        isImageFolder: false,
      );
    }
    _open();
    AiUpscaleManager.instance.addListener(_onAiManager);
    final s = widget.source;
    // 延迟到本帧构建结束后再通知 AI 管理器：setReadingBook 会 notifyListeners，
    // 若在 initState（Navigator push 构建期间）同步触发，详情页监听器 setState 会
    // 抛 "setState() called during build"。
    final bk = s == null ? null : bookKeyOf(s.type, s.id, widget.path);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        AiUpscaleManager.instance.setReadingBook(bk, owner: _aiReadingOwner);
      }
    });
  }

  void _onAiManager() {
    final s = widget.source;
    if (s == null) return;
    final bookKey = bookKeyOf(s.type, s.id, widget.path);
    final m = AiUpscaleManager.instance;
    if (m.forceAiVersionBookKey == bookKey) {
      m.consumeForceAiVersion();
      if (!_useAiVersion) _toggleAiVersion();
    }
  }

  /// 原版 / 超分版本切换：清空当前视口页并重新加载，页码不变。
  void _toggleAiVersion() {
    final unknownHeightEstimate = _webtoonNavigation.estimatedUnknownHeight;
    setState(() {
      _useAiVersion = !_useAiVersion;
      _webtoonHeights.clear(); // 超分图 2x 分辨率,显示高度变化,页高缓存作废
      _webtoonAnchor.reset();
      _webtoonLoadTarget = _mode == ReadMode.webtoon ? _page : null;
      final book = _book;
      if (book != null) {
        _webtoonNavigation.reset(pageCount: book.pageCount, initialPage: _page);
        _webtoonNavigation.freezeUnknownHeight(unknownHeightEstimate);
      }
      for (var i = _page - 1; i <= _page + 2; i++) {
        if (i >= 0) {
          _bytes.remove(i);
          _pageLoads.remove(i);
          _pageRequestGenerations[i] = (_pageRequestGenerations[i] ?? 0) + 1;
        }
      }
    });
    _photoCtrlOf(_page).reset();
    _scaleStateCtrlOf(_page).reset();
    _dualZoomCtrl.value = Matrix4.identity();
    _webtoonZoomCtrl.value = Matrix4.identity();
    if (_mode == ReadMode.webtoon) {
      _ensureWebtoonWindow(_page);
    } else {
      _ensureVisiblePagedPages(_page);
    }
    _pumpPageLoadQueue();
  }

  // ---- 页面旋转(每页独立,右键「界面旋转」进入) ----
  int _rotationOf(int page) => _rotations[page] ?? 0;

  void _toggleRotationMode() {
    if (_mode == ReadMode.webtoon) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(const SnackBar(content: Text('条漫模式暂不支持旋转')));
      return;
    }
    setState(() => _rotationMode = !_rotationMode);
  }

  void _rotatePage(int page) {
    final next = (_rotationOf(page) + 90) % 360;
    setState(() {
      if (next == 0) {
        _rotations.remove(page);
      } else {
        _rotations[page] = next;
      }
    });
    final s = widget.source;
    if (s == null) return;
    final m = LibraryStore.instance.metaOf(s, widget.path);
    m.rotations
      ..clear()
      ..addAll(_rotations);
    LibraryStore.instance.updateMeta(m);
  }

  Future<void> _open() async {
    final generation = ++_openGeneration;
    _webtoonScrollGeneration++;
    _webtoonProgrammaticScroll = false;
    _webtoonLoadTarget = null;
    _webtoonPendingAttachPage = null;
    final source = widget.source;
    final strategy = LibraryStore.instance.settings.bookOpenStrategy;
    final oldBook = _book;
    _book = null;
    if (oldBook != null) {
      try {
        await closeBook(handle: oldBook.handle);
      } catch (_) {}
    }
    if (!mounted || generation != _openGeneration) return;
    setState(() {
      _error = null;
      _downloadProgress = null;
      _failedAtStage = null;
      _openStage = source?.needsSession == true
          ? BookOpenStage.connecting
          : BookOpenStage.openingBook;
      _bytes.clear();
      _pageLoads.clear();
      _pageRequestGenerations.clear();
      _queuedPageLoads.clear();
      _providerPath = null;
    });
    _firstPageLogged = false;
    _firstPageWatch = null;
    var failureStage = source?.needsSession == true
        ? BookOpenStage.connecting
        : BookOpenStage.openingBook;

    bool isCurrent() => mounted && generation == _openGeneration;

    try {
      final result = await BookOpenCoordinator.instance.open(
        source: source,
        path: widget.path,
        title: widget.title,
        strategy: strategy,
        existingSession: widget.webdavSession,
        isActive: isCurrent,
        onStage: (stage) {
          if (!isCurrent()) return;
          failureStage = stage;
          if (stage == BookOpenStage.openingBook &&
              source != null &&
              !_readRecordStarted) {
            _readRecordStarted = true;
            unawaited(
              LibraryStore.instance.recordRead(
                source: source,
                path: widget.path,
                title: widget.title,
              ),
            );
          }
          setState(() {
            _openStage = stage;
            if (stage == BookOpenStage.openingBook) {
              _downloadProgress = null;
            }
          });
        },
        onProgress: (progress) {
          if (isCurrent()) setState(() => _downloadProgress = progress);
        },
        onRemoteImageFolder: (isImageFolder) {
          if (!isCurrent()) return;
          _updateRemoteImageFolderLease(source, isImageFolder);
        },
        onProviderPathResolved: (providerPath) {
          if (isCurrent()) _providerPath = providerPath;
        },
      );
      if (!isCurrent()) {
        try {
          await closeBook(handle: result.book.handle);
        } catch (_) {}
        return;
      }
      if (result.book.pageCount <= 0) {
        try {
          await closeBook(handle: result.book.handle);
        } catch (_) {}
        throw StateError('这本漫画没有可读取的页面');
      }
      setState(() {
        _book = result.book;
        _remoteImageFolder = result.remoteImageFolder;
        _providerPath = result.providerPath;
        _page = widget.initialPage.clamp(0, result.book.pageCount - 1);
        _openStage = BookOpenStage.loadingFirstPage;
        _downloadProgress = null;
      });
      _initialPageIndex = _page;
      _firstPageWatch = Stopwatch()..start();
      _completion.reset(pageCount: result.book.pageCount, initialPage: _page);
      _webtoonNavigation.reset(
        pageCount: result.book.pageCount,
        initialPage: _page,
      );
      _webtoonLoadTarget = _mode == ReadMode.webtoon ? _page : null;
      _recreatePageCtrl();
      if (_mode == ReadMode.webtoon) {
        _ensureWebtoonWindow(_page);
      } else {
        _ensureVisiblePagedPages(_page);
      }
      if (_mode == ReadMode.webtoon && _page > 0) {
        final initialPage = _page;
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (isCurrent()) _scrollWebtoonToPage(initialPage);
        });
      }
    } catch (error) {
      if (!isCurrent()) return;
      final recovered = await _tryRecoverQuarkCookie(
        error,
        isCurrent: isCurrent,
      );
      if (recovered || !isCurrent()) return;
      final partialBook = _book;
      _book = null;
      if (partialBook != null) {
        unawaited(closeBook(handle: partialBook.handle).catchError((_) {}));
      }
      setState(() {
        _error = '$error';
        _downloadProgress = null;
        _failedAtStage = failureStage;
        _openStage = BookOpenStage.failed;
      });
    }
  }

  Future<bool> _tryRecoverQuarkCookie(
    Object error, {
    required bool Function() isCurrent,
  }) async {
    final source = widget.source;
    if (source == null ||
        !source.isQuark ||
        _quarkCookieRecoveryAttempted ||
        !isQuarkCookieExpiredError(error)) {
      return false;
    }

    _quarkCookieRecoveryAttempted = true;
    final messenger = ScaffoldMessenger.of(context);
    final cookie = await scanQuarkCookie(
      context,
      onError: (message) {
        if (!isCurrent()) return;
        messenger.showSnackBar(SnackBar(content: Text(message)));
      },
    );
    if (!isCurrent() || cookie == null || cookie.trim().isEmpty) return false;

    try {
      source.cookie = cookie;
      await LibraryStore.instance.updateSource(source.id, cookie: cookie);
      clearQuarkSession(source.id);
    } catch (saveError) {
      if (isCurrent()) {
        messenger.showSnackBar(
          SnackBar(content: Text('保存夸克登录状态失败：$saveError')),
        );
      }
      return false;
    }

    if (!isCurrent()) return false;
    await _open();
    return true;
  }

  void _retryOpen() {
    _quarkCookieRecoveryAttempted = false;
    unawaited(_open());
  }

  bool get _canRefreshQuarkIndex =>
      widget.source?.isQuark == true &&
      (_error?.contains('远程书籍索引缺少有效的文件映射') == true ||
          _error?.startsWith('刷新夸克书源失败：') == true);

  Future<void> _refreshQuarkIndexAndRetry() async {
    final source = widget.source;
    if (source == null || !source.isQuark || _refreshingQuarkIndex) return;

    final generation = ++_openGeneration;
    bool isCurrent() => mounted && generation == _openGeneration;
    final messenger = ScaffoldMessenger.of(context);
    setState(() => _refreshingQuarkIndex = true);

    try {
      final session = await quarkSessionFor(source);
      final scan = await RemoteScanCoordinator.instance.rescanFull(
        source,
        session,
      );
      if (!isCurrent()) return;
      final scanStatus = scan.status.trim().toLowerCase();
      if (scanStatus != 'complete' &&
          scanStatus != 'completed' &&
          scanStatus != 'succeeded') {
        throw StateError(scan.errorCode ?? '扫描状态：${scan.status}');
      }

      await LibraryCatalogStore.instance.loadTree();
      if (!isCurrent()) return;
      setState(() => _refreshingQuarkIndex = false);
      _quarkCookieRecoveryAttempted = false;
      await _open();
    } catch (error) {
      if (!isCurrent()) return;
      setState(() => _refreshingQuarkIndex = false);
      if (isQuarkCookieExpiredError(error)) {
        final recovered = await _tryRecoverQuarkCookie(
          error,
          isCurrent: isCurrent,
        );
        if (recovered || !isCurrent()) return;
      }
      if (!isCurrent()) return;
      final message = remoteErrorMessage(error, fallback: '夸克书源刷新失败，请稍后重试');
      setState(() => _error = '刷新夸克书源失败：$message');
      messenger.showSnackBar(SnackBar(content: Text(message)));
    }
  }

  void _updateRemoteImageFolderLease(BookSource? source, bool isImageFolder) {
    if (source == null) return;
    final wasImageFolder = _remoteImageFolder;
    _remoteImageFolder = isImageFolder;
    if (!isImageFolder || wasImageFolder) return;

    final oldLease = _cleanupLease;
    _cleanupLease = RemoteBookUseRegistry.instance.acquire(
      source: source,
      path: widget.path,
      enabled: true,
      strategy: LibraryStore.instance.settings.bookOpenStrategy,
      isImageFolder: true,
    );
    if (oldLease != null && oldLease.enabled) {
      unawaited(oldLease.release(completionCandidate: false));
    }
    if (mounted) setState(() {});
  }

  void _logReaderTiming(String result) {
    final watch = _firstPageWatch;
    if (watch == null || _firstPageLogged) return;
    _firstPageLogged = true;
    final sourceType =
        widget.source?.type ??
        (widget.webdavSession == null ? 'local' : 'webdav');
    unawaited(
      frbsource
          .logReaderTiming(
            stage: 'first_page',
            sourceType: sourceType,
            result: result,
            elapsedMs: watch.elapsedMilliseconds,
          )
          .catchError((Object _) {}),
    );
  }

  void _logPageLoadTiming(String result, int elapsedMs) {
    final sourceType =
        widget.source?.type ??
        (widget.webdavSession == null ? 'local' : 'webdav');
    unawaited(
      frbsource
          .logReaderTiming(
            stage: 'page_load',
            sourceType: sourceType,
            result: result,
            elapsedMs: elapsedMs,
          )
          .catchError((Object _) {}),
    );
  }

  void _markFirstPageReady(int page) {
    if (page != _initialPageIndex) return;
    _logReaderTiming('success');
    if (_openStage != BookOpenStage.ready && mounted) {
      setState(() => _openStage = BookOpenStage.ready);
    }
  }

  void _markFirstPageFailed(int page) {
    if (page != _initialPageIndex) return;
    _logReaderTiming('error');
    if (_openStage != BookOpenStage.failed && mounted) {
      setState(() => _openStage = BookOpenStage.failed);
    }
  }

  /// D7：把设置里的渲染宽度模式解析成像素宽。
  ///
  /// `null` = 沿用 Rust 侧默认（1600，页缓存路径与历史一致）；
  /// 省流档给 1080；跟随屏幕按"逻辑宽 × DPR"算（Rust 侧按宽度分目录，互不污染）。
  int? _renderWidthPixelsForDisplay() {
    final media = MediaQuery.of(context);
    return renderWidthPixels(
      LibraryStore.instance.settings.renderWidth,
      screenWidth: media.size.width,
      devicePixelRatio: media.devicePixelRatio,
    );
  }

  void _ensure(int page) {
    final book = _book;
    if (book == null || page < 0 || page >= book.pageCount) return;
    if (_bytes.containsKey(page)) return;
    final existing = _pageLoads[page];
    if (existing?.status == _PageLoadStatus.loading) {
      if (_isFocusedPageLoad(page)) _promotePrefetchPage(page, book, existing!);
      return;
    }
    if (existing != null) return;
    final requestGeneration = (_pageRequestGenerations[page] ?? 0) + 1;
    _pageRequestGenerations[page] = requestGeneration;
    final openGeneration = _openGeneration;
    _pageLoads[page] = _PageLoadState(
      status: _PageLoadStatus.loading,
      attempts: 1,
      requestGeneration: requestGeneration,
      bookHandle: book.handle,
    );
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted ||
          _openGeneration != openGeneration ||
          !identical(_book, book) ||
          _pageRequestGenerations[page] != requestGeneration) {
        return;
      }
      _enqueuePageLoad(
        _QueuedPageLoad(
          page: page,
          book: book,
          openGeneration: openGeneration,
          requestGeneration: requestGeneration,
          watch: Stopwatch()..start(),
        ),
      );
    });
  }

  /// A low-priority request may already be waiting in Rust when the user lands
  /// on that page. Issue a fresh foreground request so the target can jump the
  /// prefetch queue; the stale response is ignored by its request generation.
  void _promotePrefetchPage(int page, BookInfo book, _PageLoadState state) {
    final prefetchIsActive = _activePageLoads.any(
      (request) =>
          request.page == page &&
          request.requestGeneration == state.requestGeneration &&
          _activePrefetchLoads.contains(request),
    );
    if (!prefetchIsActive) return;
    final requestGeneration = state.requestGeneration + 1;
    _pageRequestGenerations[page] = requestGeneration;
    _pageLoads[page] = _PageLoadState(
      status: _PageLoadStatus.loading,
      attempts: 1,
      requestGeneration: requestGeneration,
      bookHandle: book.handle,
    );
    _enqueuePageLoad(
      _QueuedPageLoad(
        page: page,
        book: book,
        openGeneration: _openGeneration,
        requestGeneration: requestGeneration,
        watch: Stopwatch()..start(),
      ),
    );
  }

  bool _isCurrentQueuedPageLoad(_QueuedPageLoad request) {
    final state = _pageLoads[request.page];
    return mounted &&
        _openGeneration == request.openGeneration &&
        identical(_book, request.book) &&
        _pageRequestGenerations[request.page] == request.requestGeneration &&
        state?.status == _PageLoadStatus.loading &&
        state?.requestGeneration == request.requestGeneration &&
        state?.bookHandle == request.book.handle &&
        !_bytes.containsKey(request.page);
  }

  bool _pageLoadIsInActiveWindow(int page) {
    final book = _book;
    if (book == null || page < 0 || page >= book.pageCount) return false;
    if (page == _initialPageIndex &&
        _openStage == BookOpenStage.loadingFirstPage) {
      return true;
    }
    if (_mode == ReadMode.webtoon) {
      final requestedTarget = _webtoonLoadTarget;
      if (requestedTarget != null && !_bytes.containsKey(requestedTarget)) {
        return page == requestedTarget;
      }
      final center =
          _webtoonLoadTarget ??
          _webtoonNavigation.pendingTarget ??
          _webtoonNavigation.viewportPage ??
          _page;
      return (page - center).abs() <= webtoonPageLoadRadius;
    }
    return _viewOfPage(page) == _viewOfPage(_page);
  }

  int _pageLoadFocusPage() => _mode == ReadMode.webtoon
      ? _webtoonLoadTarget ??
            _webtoonNavigation.pendingTarget ??
            _webtoonNavigation.viewportPage ??
            _page
      : _page;

  bool _isFocusedPageLoad(int page) {
    if (_mode == ReadMode.webtoon) return page == _pageLoadFocusPage();
    return _viewOfPage(page) == _viewOfPage(_page);
  }

  int _pageLoadQueuePriority(int page) {
    if (page == _pageLoadFocusPage()) return 0;
    if (_mode != ReadMode.webtoon && _isFocusedPageLoad(page)) return 1;
    return 2 + (page - _pageLoadFocusPage()).abs();
  }

  void _enqueuePageLoad(_QueuedPageLoad request) {
    if (!_isCurrentQueuedPageLoad(request)) return;
    _queuedPageLoads[request.page] = request;
    _pumpPageLoadQueue();
  }

  void _pumpPageLoadQueue() {
    if (!mounted) {
      _queuedPageLoads.clear();
      return;
    }

    final staleRequests = _queuedPageLoads.values
        .where(
          (request) =>
              !_isCurrentQueuedPageLoad(request) ||
              !_pageLoadIsInActiveWindow(request.page),
        )
        .toList();
    for (final request in staleRequests) {
      if (!identical(_queuedPageLoads[request.page], request)) continue;
      _queuedPageLoads.remove(request.page);
      final state = _pageLoads[request.page];
      if (state?.requestGeneration == request.requestGeneration &&
          state?.status == _PageLoadStatus.loading) {
        _pageRequestGenerations[request.page] = request.requestGeneration + 1;
        _pageLoads.remove(request.page);
      }
    }

    // Keep the ordinary budget at three and allow one urgent visible target
    // into Rust's priority queue when stale calls already occupy those slots.
    while (_activePageLoads.length < _maxConcurrentPageLoads + 1) {
      final orderedRequests = _queuedPageLoads.values.toList()
        ..sort((a, b) {
          final priority = _pageLoadQueuePriority(
            a.page,
          ).compareTo(_pageLoadQueuePriority(b.page));
          return priority != 0 ? priority : a.page.compareTo(b.page);
        });
      final focusedActive = _activePageLoads
          .where(
            (active) =>
                _isFocusedPageLoad(active.page) &&
                _pageRequestGenerations[active.page] ==
                    active.requestGeneration,
          )
          .length;
      final backgroundActive = _activePageLoads.length - focusedActive;
      final backgroundLimit = focusedActive >= _maxConcurrentPageLoads
          ? 0
          : _maxConcurrentPageLoads - focusedActive - 1;
      _QueuedPageLoad? next;
      for (final request in orderedRequests) {
        final activeSamePage = _activePageLoads
            .where(
              (active) =>
                  active.page == request.page &&
                  active.openGeneration == request.openGeneration &&
                  identical(active.book, request.book),
            )
            .toList();
        final promotesPrefetch =
            _isFocusedPageLoad(request.page) &&
            activeSamePage.any(_activePrefetchLoads.contains);
        if (activeSamePage.isNotEmpty && !promotesPrefetch) continue;
        if (_isFocusedPageLoad(request.page)) {
          final focusedLimit = _mode == ReadMode.webtoon
              ? 1
              : (pairOf(_page).$2 == null ? 1 : 2);
          if (focusedActive >= focusedLimit && !promotesPrefetch) continue;
        } else if (_activePageLoads.length >= _maxConcurrentPageLoads ||
            backgroundActive >= backgroundLimit) {
          continue;
        }
        next = request;
        break;
      }
      if (next == null) break;
      final request = next;
      _queuedPageLoads.remove(request.page);
      _activePageLoads.add(request);
      final prefetch = !_isFocusedPageLoad(request.page);
      if (prefetch) _activePrefetchLoads.add(request);
      unawaited(
        _loadPage(
          request.page,
          request.book,
          request.openGeneration,
          request.requestGeneration,
          1,
          watch: request.watch,
          prefetch: prefetch,
        ).whenComplete(() {
          _activePageLoads.remove(request);
          _activePrefetchLoads.remove(request);
          _pumpPageLoadQueue();
        }),
      );
    }
  }

  Future<void> _loadPage(
    int page,
    BookInfo book,
    int openGeneration,
    int requestGeneration,
    int attempt, {
    required Stopwatch watch,
    required bool prefetch,
  }) async {
    final handle = book.handle;
    final currentState = _pageLoads[page];
    bool isCurrent() {
      final state = _pageLoads[page];
      return mounted &&
          _openGeneration == openGeneration &&
          identical(_book, book) &&
          book.handle == handle &&
          _pageRequestGenerations[page] == requestGeneration &&
          (state == null ||
              (state.requestGeneration == requestGeneration &&
                  state.bookHandle == handle));
    }

    if (!isCurrent()) return;
    if (currentState?.status != _PageLoadStatus.loading ||
        currentState?.attempts != attempt) {
      setState(() {
        _pageLoads[page] = _PageLoadState(
          status: _PageLoadStatus.loading,
          attempts: attempt,
          requestGeneration: requestGeneration,
          bookHandle: handle,
        );
      });
    }

    try {
      final targetWidth = _renderWidthPixelsForDisplay();
      final original = prefetch
          ? await bookPagePrefetch(
              handle: handle,
              index: page,
              targetWidth: targetWidth,
            )
          : await bookPage(
              handle: handle,
              index: page,
              targetWidth: targetWidth,
            );
      if (!prefetch) _logPageLoadTiming('success', watch.elapsedMilliseconds);
      if (!isCurrent()) return;
      if (page == _page) _completion.observeStablePage(page);
      final previousHeight = page < _webtoonHeights.length
          ? _webtoonHeights[page]
          : 0.0;
      _webtoonAnchor.announceGrowth(
        page,
        previousHeight > 0
            ? previousHeight
            : _webtoonNavigation.heightFor(page),
      );
      setState(() {
        _bytes[page] = original;
        _pageLoads[page] = _PageLoadState(
          status: _PageLoadStatus.success,
          attempts: attempt,
          requestGeneration: requestGeneration,
          bookHandle: handle,
        );
        if (_mode == ReadMode.webtoon && _webtoonLoadTarget == page) {
          _webtoonLoadTarget = null;
        }
      });
      _markFirstPageReady(page);
      if (!widget.skipAiCache && _useAiVersion) {
        unawaited(
          _applyCachedAiPage(
            page: page,
            book: book,
            openGeneration: openGeneration,
            requestGeneration: requestGeneration,
            original: original,
          ),
        );
      }
    } catch (error) {
      if (!prefetch) _logPageLoadTiming('error', watch.elapsedMilliseconds);
      if (!isCurrent()) return;
      if (!_pageLoadIsInActiveWindow(page)) {
        _pageRequestGenerations[page] = requestGeneration + 1;
        setState(() => _pageLoads.remove(page));
        return;
      }
      final isQuarkAuthFailure =
          widget.source?.isQuark == true && isQuarkCookieExpiredError(error);
      if (isQuarkAuthFailure) {
        final recovered = await _tryRecoverQuarkCookie(
          error,
          isCurrent: isCurrent,
        );
        if (recovered || !isCurrent()) return;
      }
      if (attempt < 3 && !isQuarkAuthFailure) {
        setState(() {
          _pageLoads[page] = _PageLoadState(
            status: _PageLoadStatus.loading,
            attempts: attempt,
            requestGeneration: requestGeneration,
            bookHandle: handle,
            error: '$error',
          );
        });
        final queueSaturated = '$error'.contains(
          'blocking request priority queue is full',
        );
        await Future<void>.delayed(
          Duration(
            milliseconds: queueSaturated
                ? (attempt == 1 ? 750 : 1500)
                : (attempt == 1 ? 250 : 750),
          ),
        );
        if (!isCurrent()) return;
        if (!_pageLoadIsInActiveWindow(page)) {
          _pageRequestGenerations[page] = requestGeneration + 1;
          setState(() => _pageLoads.remove(page));
          return;
        }
        await _loadPage(
          page,
          book,
          openGeneration,
          requestGeneration,
          attempt + 1,
          watch: watch,
          prefetch: !_isFocusedPageLoad(page),
        );
        return;
      }
      setState(() {
        _pageLoads[page] = _PageLoadState(
          status: _PageLoadStatus.failed,
          attempts: attempt,
          requestGeneration: requestGeneration,
          bookHandle: handle,
          error: '$error',
        );
      });
      _markFirstPageFailed(page);
    }
  }

  Future<void> _applyCachedAiPage({
    required int page,
    required BookInfo book,
    required int openGeneration,
    required int requestGeneration,
    required Uint8List original,
  }) async {
    try {
      final ai = await lookupCache(pageBytes: original.toList(), scale: 2);
      if (ai == null ||
          !mounted ||
          _openGeneration != openGeneration ||
          !identical(_book, book) ||
          _pageRequestGenerations[page] != requestGeneration ||
          !_useAiVersion ||
          widget.skipAiCache) {
        return;
      }
      final state = _pageLoads[page];
      if (state?.status != _PageLoadStatus.success ||
          state?.requestGeneration != requestGeneration ||
          state?.bookHandle != book.handle) {
        return;
      }
      setState(() => _bytes[page] = ai);
    } catch (_) {
      // The original image is already visible; cache lookup is best-effort.
    }
  }

  void _retryPage(int page) {
    final book = _book;
    final state = _pageLoads[page];
    if (book == null || state?.status != _PageLoadStatus.failed) return;
    _quarkCookieRecoveryAttempted = false;
    final requestGeneration = (_pageRequestGenerations[page] ?? 0) + 1;
    setState(() {
      _bytes.remove(page);
      _pageLoads[page] = _PageLoadState(
        status: _PageLoadStatus.loading,
        attempts: 1,
        requestGeneration: requestGeneration,
        bookHandle: book.handle,
      );
      _pageRequestGenerations[page] = requestGeneration;
      if (page == _initialPageIndex) {
        _openStage = BookOpenStage.loadingFirstPage;
      }
    });
    if (page == _initialPageIndex) {
      _firstPageLogged = false;
      _firstPageWatch = Stopwatch()..start();
    }
    _enqueuePageLoad(
      _QueuedPageLoad(
        page: page,
        book: book,
        openGeneration: _openGeneration,
        requestGeneration: requestGeneration,
        watch: Stopwatch()..start(),
      ),
    );
  }

  Widget _pagePlaceholder(int page) {
    final state = _pageLoads[page];
    if (state?.status != _PageLoadStatus.failed) {
      return const Center(child: CircularProgressIndicator());
    }
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(20),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(Icons.broken_image_outlined, size: 36),
            const SizedBox(height: 8),
            Text('第 ${page + 1} 页加载失败'),
            if ((state?.error ?? '').isNotEmpty) ...[
              const SizedBox(height: 4),
              Text(
                state!.error!,
                maxLines: 3,
                overflow: TextOverflow.ellipsis,
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: Theme.of(context).colorScheme.onSurfaceVariant,
                  fontSize: 12,
                ),
              ),
            ],
            const SizedBox(height: 8),
            TextButton.icon(
              onPressed: () => _retryPage(page),
              icon: const Icon(Icons.refresh),
              label: const Text('重试本页'),
            ),
          ],
        ),
      ),
    );
  }

  // ---- 双页配对 ----
  (int, int?) pairOf(int page) {
    if (_dual == DualPageMode.off) return (page, null);
    final b = _book;
    if (b == null) return (page, null);
    final isManga = _mode == ReadMode.manga;
    if (_skipCover && page == 0) return (0, null);
    int base = _skipCover
        ? 1 + ((page - 1) - (page - 1) % 2)
        : page - (page % 2);
    final first = base, second = base + 1;
    final left = isManga ? second : first, right = isManga ? first : second;
    if (right >= b.pageCount) return (left, null);
    return (left, right);
  }

  bool _isDual() => pairOf(_page).$2 != null;

  // ---- 翻页 ----
  /// 前进一屏。**在已经到达阅读顺序末端时继续前进**才提示"要不要再随机一本"
  /// （而不是一翻到尾页就弹窗——那会在正常阅读到最后一页时打扰）。
  void _forward() {
    final s = _dual != DualPageMode.off ? 2 : 1;
    _go(_mode == ReadMode.manga ? -s : s);
  }

  void _back() {
    final s = _dual != DualPageMode.off ? 2 : 1;
    _go(_mode == ReadMode.manga ? s : -s);
  }

  /// 确保条漫滚动/跳页窗口附近的页面已开始加载。
  void _ensureWebtoonWindow(int centerPage) {
    _ensure(centerPage);
    final requestedTarget = _webtoonLoadTarget;
    if (requestedTarget != null && !_bytes.containsKey(requestedTarget)) {
      return;
    }
    for (var distance = 1; distance <= webtoonPageLoadRadius; distance++) {
      _ensure(centerPage - distance);
      _ensure(centerPage + distance);
    }
  }

  /// Paged mode asks Rust only for the page currently shown and its visible
  /// spread. Reader::spawn_prefetch handles nearby pages at background priority.
  void _ensureVisiblePagedPages(int page) {
    _ensure(page);
    final (leftPage, rightPage) = pairOf(page);
    if (leftPage != page) _ensure(leftPage);
    if (rightPage != null && rightPage != page && rightPage != leftPage) {
      _ensure(rightPage);
    }
  }

  bool _shouldLoadWebtoonPage(int page) {
    final requestedTarget = _webtoonLoadTarget;
    if (requestedTarget != null && !_bytes.containsKey(requestedTarget)) {
      return page == requestedTarget;
    }
    final center =
        _webtoonLoadTarget ??
        _webtoonNavigation.pendingTarget ??
        _webtoonNavigation.viewportPage ??
        _page;
    return (page - center).abs() <= webtoonPageLoadRadius;
  }

  void _schedulePendingWebtoonJumpAfterAttach() {
    if (_webtoonPendingAttachPage == null || _webtoonAttachCallbackScheduled) {
      return;
    }
    _webtoonAttachCallbackScheduled = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _webtoonAttachCallbackScheduled = false;
      if (!mounted || _mode != ReadMode.webtoon) return;
      final page = _webtoonPendingAttachPage;
      if (page == null || !_webtoonCtrl.hasClients) return;
      _webtoonPendingAttachPage = null;
      _scrollWebtoonToPage(page);
    });
  }

  void _scrollWebtoonToPage(int page) {
    final generation = ++_webtoonScrollGeneration;
    _webtoonProgrammaticScroll = false;
    _webtoonNavigation.freezeUnknownHeight();
    if (!_webtoonCtrl.hasClients) {
      _webtoonNavigation.selectExplicit(page);
      _webtoonLoadTarget = _bytes.containsKey(page) ? null : page;
      _webtoonPendingAttachPage = page;
      _ensureWebtoonWindow(page);
      _pumpPageLoadQueue();
      return;
    }
    _webtoonPendingAttachPage = null;
    final intent = _webtoonNavigation.requestTarget(page);
    _webtoonLoadTarget = _bytes.containsKey(intent.targetPage)
        ? null
        : intent.targetPage;
    _webtoonProgrammaticScroll = true;
    _ensureWebtoonWindow(intent.targetPage);
    _pumpPageLoadQueue();
    final targetOffset = _webtoonNavigation.offsetForIntent(intent);
    if ((_webtoonCtrl.offset - targetOffset).abs() < 0.5) {
      _webtoonNavigation.completeProgrammatic(intent);
      _webtoonProgrammaticScroll = false;
      _webtoonLoadTarget = _bytes.containsKey(intent.targetPage)
          ? null
          : intent.targetPage;
      _pumpPageLoadQueue();
      return;
    }
    _webtoonCtrl.jumpTo(targetOffset);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || generation != _webtoonScrollGeneration) return;
      _webtoonNavigation.completeProgrammatic(intent);
      _webtoonProgrammaticScroll = false;
      _webtoonLoadTarget = _bytes.containsKey(intent.targetPage)
          ? null
          : intent.targetPage;
      _pumpPageLoadQueue();
    });
  }

  Future<void> _go(int d) async {
    final b = _book;
    if (b == null) return;
    if (_mode == ReadMode.webtoon) {
      // 条漫: 直接滚动到下一页/上一页(方向由 _forward/_back 已按 manga 翻转传入)。
      final n = (_page + d).clamp(0, b.pageCount - 1);
      if (n == _page) return;
      setState(() => _page = n);
      _disposeDistantPhotoCtrls();
      _completion.observeStablePage(n);
      _webtoonNavigation.freezeUnknownHeight();
      final generation = ++_webtoonScrollGeneration;
      _webtoonProgrammaticScroll = false;
      _webtoonLoadTarget = n;
      _webtoonPendingAttachPage = null;
      if (_webtoonCtrl.hasClients) {
        final intent = _webtoonNavigation.requestTarget(n);
        _webtoonProgrammaticScroll = true;
        _webtoonCtrl
            .animateTo(
              _webtoonNavigation.offsetForIntent(intent),
              duration: const Duration(milliseconds: 220),
              curve: Curves.easeOut,
            )
            .whenComplete(() {
              _webtoonNavigation.completeProgrammatic(intent);
              if (generation == _webtoonScrollGeneration) {
                _webtoonProgrammaticScroll = false;
                _webtoonLoadTarget = _bytes.containsKey(n) ? null : n;
              }
            });
      } else {
        _webtoonNavigation.selectExplicit(n);
        _webtoonPendingAttachPage = n;
      }
      _ensureWebtoonWindow(n);
      final src = widget.source;
      if (src != null) {
        await LibraryStore.instance.recordRead(
          source: src,
          path: widget.path,
          title: widget.title,
          page: n,
        );
      }
      _scheduleEndPrompt();
      return;
    }
    // 用**视口序号**推进，而不是页号加减：
    //   · 双页模式下 1 个视口 = 2 页，按页号 +2 可能落到"配对越界"的页
    //     （实测：200 页的书出现 200-201/200 → 请求不存在的第 201 页 → 永远加载中）；
    //   · 视口推进先做越界判断，越界即"翻过最后一页"，直接提示，不再请求坏页。
    final curView = _viewOfPage(_page);
    final targetView = curView + (d >= 0 ? 1 : -1);
    if (targetView < 0 || targetView >= _viewCount()) return; // 视口边界：原地不动
    final n = _pageOfView(targetView);
    if (n == _page) return;
    setState(() => _page = n);
    _completion.observeStablePage(n);
    _photoCtrlOf(n).reset();
    _scaleStateCtrlOf(n).reset();
    _dualZoomCtrl.value = Matrix4.identity();
    _pageCtrl?.animateToPage(
      _viewOfPage(n),
      duration: const Duration(milliseconds: 220),
      curve: Curves.easeOutCubic,
    );
    _scheduleEndPrompt();
    _disposeDistantPhotoCtrls();
    _ensureVisiblePagedPages(n);
    final s = widget.source;
    if (s != null) {
      await LibraryStore.instance.recordRead(
        source: s,
        path: widget.path,
        title: widget.title,
        page: n,
      );
    }
  }

  // ---- 缩放(仅 +/-/0 键,无滚轮) ----
  void _zoomIn() => _zoomBy(1.25);
  void _zoomOut() => _zoomBy(1 / 1.25);
  void _zoomReset() {
    if (_mode == ReadMode.webtoon) {
      _webtoonZoomCtrl.value = Matrix4.identity();
    } else if (_isDual()) {
      _dualZoomCtrl.value = Matrix4.identity();
    } else {
      _photoCtrlOf(_page).reset();
      _scaleStateCtrlOf(_page).reset();
    }
  }

  void _zoomBy(double f) {
    if (_mode == ReadMode.webtoon) {
      _zoomIV(_webtoonZoomCtrl, f);
    } else if (_isDual()) {
      _zoomIV(_dualZoomCtrl, f);
    } else {
      final c = _photoCtrlOf(_page);
      final cur = c.scale ?? 1.0;
      c.scale = (cur * f).clamp(0.5, 8.0);
    }
  }

  void _zoomIV(TransformationController c, double f) {
    final cur = c.value.getMaxScaleOnAxis();
    final next = (cur * f).clamp(1.0, 4.0);
    c.value = Matrix4.identity()..scaleByDouble(next, next, next, 1.0);
  }

  void _showJumpDialog() {
    final book = _book;
    if (book == null) return;
    final ctrl = TextEditingController();
    showDialog(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('跳转到页码'),
        content: TextField(
          controller: ctrl,
          keyboardType: TextInputType.number,
          autofocus: true,
          decoration: const InputDecoration(
            hintText: '输入页码',
            border: OutlineInputBorder(),
          ),
          onSubmitted: (value) => _doJump(value, ctx),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => _doJump(ctrl.text, ctx),
            child: const Text('跳转'),
          ),
        ],
      ),
    );
  }

  void _doJump(String value, BuildContext ctx) {
    final book = _book;
    if (book == null) return;
    final requestedPage = int.tryParse(value.trim());
    if (requestedPage != null) {
      final page = (requestedPage - 1).clamp(0, book.pageCount - 1);
      final waitingForFirstPage = _openStage == BookOpenStage.loadingFirstPage;
      setState(() {
        _page = page;
        if (waitingForFirstPage) _initialPageIndex = page;
      });
      if (waitingForFirstPage) {
        _firstPageLogged = false;
        _firstPageWatch = Stopwatch()..start();
      }
      _completion.observeStablePage(page);
      if (_mode == ReadMode.webtoon) {
        _scrollWebtoonToPage(page);
        _disposeDistantPhotoCtrls();
      } else {
        _photoCtrlOf(page).reset();
        _scaleStateCtrlOf(page).reset();
        _dualZoomCtrl.value = Matrix4.identity();
        _pageCtrl?.jumpToPage(_viewOfPage(page));
        _disposeDistantPhotoCtrls();
        _ensureVisiblePagedPages(page);
      }
      if (waitingForFirstPage && _bytes.containsKey(page)) {
        _markFirstPageReady(page);
      }
      _pumpPageLoadQueue();
      _scheduleEndPrompt();
    }
    Navigator.of(ctx).pop();
  }

  // ---- 键盘(可自定义的 5 个动作) ----
  KeyEventResult _onKey(FocusNode n, KeyEvent e) {
    if (e is! KeyDownEvent) return KeyEventResult.ignored;
    final k = e.logicalKey;
    if (k == _keys.zoomInKey || k == LogicalKeyboardKey.add) {
      _zoomIn();
      return KeyEventResult.handled;
    }
    if (k == _keys.zoomOutKey || k == LogicalKeyboardKey.numpadSubtract) {
      _zoomOut();
      return KeyEventResult.handled;
    }
    if (k == _keys.zoomResetKey || k == LogicalKeyboardKey.numpad0) {
      _zoomReset();
      return KeyEventResult.handled;
    }
    if (_mode == ReadMode.webtoon) {
      if (k == _keys.forwardKey ||
          k == LogicalKeyboardKey.arrowDown ||
          k == LogicalKeyboardKey.space) {
        _webtoonCtrl.animateTo(
          _webtoonCtrl.offset + 400,
          duration: const Duration(milliseconds: 150),
          curve: Curves.easeOut,
        );
        return KeyEventResult.handled;
      }
      if (k == _keys.backKey || k == LogicalKeyboardKey.arrowUp) {
        _webtoonCtrl.animateTo(
          _webtoonCtrl.offset - 400,
          duration: const Duration(milliseconds: 150),
          curve: Curves.easeOut,
        );
        return KeyEventResult.handled;
      }
      return KeyEventResult.ignored;
    }
    bool isManga = _mode == ReadMode.manga;
    if (k == _keys.forwardKey ||
        k == LogicalKeyboardKey.space ||
        k == LogicalKeyboardKey.pageDown) {
      isManga ? _back() : _forward();
      return KeyEventResult.handled;
    }
    if (k == _keys.backKey || k == LogicalKeyboardKey.pageUp) {
      isManga ? _forward() : _back();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  /// 翻到最后一屏时提示"要不要再随机挑一本"。
  ///
  /// 只在首次到达时弹；选择"再随机一本"会关闭当前阅读器并打开另一本。
  /// 到达最后一屏后**延迟 3 秒**再提示。
  ///
  /// 为什么不判断"翻过末页"：实测各模式（双页/条漫）下落点与方向判断难以覆盖，
  /// 且末页配对容易越界导致不触发；改为"到达末屏即计时"最稳且不打断阅读。
  void _scheduleEndPrompt() {
    final total = _viewCount();
    final b = _book;
    if (b == null || total <= 0) return;
    final atEnd = _viewOfPage(_page) >= total - 1;
    if (!atEnd) {
      // 离开末屏：取消计时并重置，下次再到末屏仍会提示
      _endTimer?.cancel();
      _endTimer = null;
      _endPrompted = false;
      return;
    }
    if (_endPrompted || _endTimer != null) return;
    _endTimer = Timer(const Duration(seconds: 3), () {
      _endTimer = null;
      if (!mounted) return;
      _endPrompted = true;
      _showEndPrompt();
    });
  }

  Future<void> _showEndPrompt() async {
    final again = await showDialog<bool>(
      context: context,
      builder: (c) => AlertDialog(
        title: const Text('已经读到最后一页'),
        content: const Text('要不要再随机挑一本接着看？'),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(c).pop(false),
            child: const Text('退出到漫画详情页'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(c).pop(true),
            child: const Text('再随机一本'),
          ),
        ],
      ),
    );
    if (!mounted) return;
    if (again != true) {
      // 退出到漫画详情页：关闭阅读器
      Navigator.of(context).pop();
      return;
    }
    await _openRandomBook();
  }

  Future<void> _openRandomBook() async {
    if (_randomBookPicking) return;
    setState(() => _randomBookPicking = true);
    final messenger = ScaffoldMessenger.of(context);
    messenger.showSnackBar(const SnackBar(content: Text('正在挑选下一本漫画…')));
    try {
      final store = LibraryStore.instance;
      final next = await RandomReadSelector.instance.pick(
        loadCandidates: LibraryCatalogStore.instance.randomCandidates,
        sourceById: store.sourceById,
        recentBookKeys: store.settings.recentRandomBookKeys,
        recordSelection: store.recordRandomSelection,
        excludeSource: widget.source,
        excludePath: widget.path,
      );
      if (!mounted) {
        messenger.hideCurrentSnackBar();
        return;
      }
      messenger.hideCurrentSnackBar();
      if (next == null) {
        messenger.showSnackBar(
          const SnackBar(content: Text('漫画库里暂时没有别的可随机阅读的漫画')),
        );
        return;
      }
      final nav = Navigator.of(context);
      nav.pop();
      await openBook(nav.context, next.source, next.path, next.title);
    } catch (_) {
      messenger.hideCurrentSnackBar();
      if (!mounted) return;
      messenger.showSnackBar(const SnackBar(content: Text('随机挑选失败，请稍后重试')));
    } finally {
      if (mounted) setState(() => _randomBookPicking = false);
    }
  }

  @override
  void dispose() {
    _openGeneration++;
    _webtoonScrollGeneration++;
    _controllerCleanupGeneration++;
    _endTimer?.cancel();
    AiUpscaleManager.instance.removeListener(_onAiManager);
    final aiReadingOwner = _aiReadingOwner;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      AiUpscaleManager.instance.clearReadingBook(owner: aiReadingOwner);
    });
    if (defaultTargetPlatform == TargetPlatform.android) {
      SystemChrome.setPreferredOrientations(
        _compactAtOpen
            ? [DeviceOrientation.portraitUp]
            : DeviceOrientation.values,
      );
    }
    final b = _book;
    if (b != null) closeBook(handle: b.handle);
    // 第 71 轮：整包下载模式下，若设置在"阅读完成后自动删除整包"，关闭书本即删 raw 包。
    // 只删 raw（封面缓存与页面缓存保留）；流式模式不动任何缓存。
    final delSrc = widget.source;
    final delSettings = LibraryStore.instance.settings;
    if (delSrc != null &&
        delSettings.deletePackageAfterReading &&
        delSettings.bookOpenStrategy != BookOpenStrategy.stream) {
      unawaited(
        deleteRawPackage(
          sourceType: delSrc.type,
          path: _providerPath ?? widget.path,
          url: delSrc.url,
          port: delSrc.port,
          rootPath: delSrc.effectiveRootPath,
          clientId: delSrc.clientId,
          rootId: delSrc.rootId,
          cookieMode: (delSrc.cookie ?? '').isNotEmpty,
        ).catchError((Object e) {
          debugPrint('[reader] delete raw package failed: $e');
          return BigInt.zero;
        }),
      );
    }
    final lease = _cleanupLease;
    if (lease != null) {
      unawaited(
        lease.release(completionCandidate: _completion.completionCandidate),
      );
    }
    for (final c in _photoCtrls.values) {
      c.dispose();
    }
    for (final c in _scaleStateCtrls.values) {
      c.dispose();
    }
    _dualZoomCtrl.removeListener(_onDualZoomChanged);
    _dualZoomCtrl.dispose();
    _webtoonZoomCtrl.dispose();
    _pageCtrl?.dispose();
    _focus.dispose();
    _webtoonCtrl.dispose();
    super.dispose();
  }

  // ========== 布局 ==========
  /// 构建 body: 下载进度 / 加载中 / 错误 / 阅读视图。
  Widget _buildBody() {
    if (_refreshingQuarkIndex) {
      return const Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            CircularProgressIndicator(),
            SizedBox(height: 16),
            Text('正在全量刷新夸克书源索引…'),
          ],
        ),
      );
    }
    if (_error != null) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              const Icon(Icons.cloud_off_outlined, size: 44),
              const SizedBox(height: 12),
              Text(
                _failedAtStage == BookOpenStage.connecting
                    ? '连接书源失败'
                    : '打开漫画失败',
              ),
              const SizedBox(height: 8),
              SelectableText(
                _error!,
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: Theme.of(context).colorScheme.onSurfaceVariant,
                ),
              ),
              const SizedBox(height: 16),
              FilledButton.icon(
                onPressed: _canRefreshQuarkIndex
                    ? _refreshQuarkIndexAndRetry
                    : _retryOpen,
                icon: const Icon(Icons.refresh),
                label: Text(_canRefreshQuarkIndex ? '刷新夸克书源并重试' : '重新打开'),
              ),
            ],
          ),
        ),
      );
    }
    if (_downloadProgress != null) {
      final pct = _downloadProgress!;
      return Center(
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 48),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              const SizedBox(
                width: 48,
                height: 48,
                child: CircularProgressIndicator(strokeWidth: 3),
              ),
              const SizedBox(height: 20),
              const Text('正在下载漫画…', style: TextStyle(fontSize: 16)),
              const SizedBox(height: 12),
              ClipRRect(
                borderRadius: BorderRadius.circular(4),
                child: LinearProgressIndicator(value: pct, minHeight: 8),
              ),
              const SizedBox(height: 8),
              Text(
                '${(pct * 100).toStringAsFixed(0)}%',
                style: const TextStyle(
                  fontSize: 18,
                  fontWeight: FontWeight.w600,
                ),
              ),
              const SizedBox(height: 4),
              Text(
                '下载完成后即可阅读',
                style: TextStyle(
                  fontSize: 11,
                  color: Theme.of(context).colorScheme.onSurfaceVariant,
                ),
              ),
            ],
          ),
        ),
      );
    }
    final b = _book;
    if (b == null) {
      final message = switch (_openStage) {
        BookOpenStage.connecting => '正在连接书源…',
        BookOpenStage.openingBook => '正在打开漫画…',
        BookOpenStage.loadingFirstPage => '正在加载首屏…',
        BookOpenStage.ready => '阅读就绪',
        BookOpenStage.failed => '打开失败',
      };
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const CircularProgressIndicator(),
            const SizedBox(height: 16),
            Text(message),
          ],
        ),
      );
    }
    final reader = _mode == ReadMode.webtoon
        ? _buildWebtoon()
        : _buildMangaOrComic();
    if (_openStage != BookOpenStage.loadingFirstPage) return reader;
    return Stack(
      children: [
        Positioned.fill(child: reader),
        Positioned(
          top: 8,
          left: 0,
          right: 0,
          child: Center(
            child: Material(
              color: Theme.of(context).colorScheme.surfaceContainerHighest,
              borderRadius: BorderRadius.circular(20),
              child: const Padding(
                padding: EdgeInsets.symmetric(horizontal: 14, vertical: 8),
                child: Text('正在加载首屏…'),
              ),
            ),
          ),
        ),
      ],
    );
  }

  Widget _buildImage(Uint8List bytes, int page) {
    final viewer = PhotoView(
      controller: _photoCtrlOf(page),
      scaleStateController: _scaleStateCtrlOf(page),
      imageProvider: ResizeImage(MemoryImage(bytes), width: 2000),
      backgroundDecoration: const BoxDecoration(color: Colors.black),
      initialScale: PhotoViewComputedScale.contained,
      minScale: PhotoViewComputedScale.contained,
      maxScale: PhotoViewComputedScale.covered * 8,
    );
    final q = _rotationOf(page) ~/ 90;
    // 未放大时把水平拖拽让给外层 PageView 翻页(photo_view 官方 PageView 适配)。
    return PhotoViewGestureDetectorScope(
      axis: Axis.horizontal,
      child: q == 0 ? viewer : RotatedBox(quarterTurns: q, child: viewer),
    );
  }

  Widget _rotationButton(int page) => Tooltip(
    message: '旋转该页（当前 ${_rotationOf(page)}°）',
    child: Material(
      color: Theme.of(
        context,
      ).colorScheme.inverseSurface.withValues(alpha: 0.8),
      shape: const CircleBorder(),
      child: InkWell(
        customBorder: const CircleBorder(),
        onTap: () => _rotatePage(page),
        child: Padding(
          padding: EdgeInsets.all(8),
          child: Icon(
            Icons.rotate_right,
            size: 22,
            color: Theme.of(context).colorScheme.onInverseSurface,
          ),
        ),
      ),
    ),
  );

  /// 日漫/美漫模式：PageView 承载页面实现滑动翻页,点按区域/旋转按钮保留。
  Widget _buildMangaOrComic() {
    final b = _book;
    if (b == null) return const Center(child: CircularProgressIndicator());
    final ctrl = _pageCtrl;
    if (ctrl == null) return const Center(child: CircularProgressIndicator());
    return PageView.builder(
      controller: ctrl,
      itemCount: _viewCount(),
      onPageChanged: (v) {
        final p = _pageOfView(v);
        if (p == _page) return;
        setState(() {
          _page = p;
          _photoCtrlOf(p).reset();
          _scaleStateCtrlOf(p).reset();
          _dualZoomCtrl.value = Matrix4.identity();
        });
        _completion.observeStablePage(p);
        _disposeDistantPhotoCtrls();
        _ensureVisiblePagedPages(p);
        final s = widget.source;
        if (s != null) {
          LibraryStore.instance.recordRead(
            source: s,
            path: widget.path,
            title: widget.title,
            page: p,
          );
        }
        _scheduleEndPrompt();
      },
      itemBuilder: (context, v) => _buildMangaOrComicPage(_pageOfView(v)),
    );
  }

  Widget _buildMangaOrComicPage(int page) {
    final bytes = _bytes[page];
    final isManga = _mode == ReadMode.manga;
    final leftAction = isManga
        ? (_invert ? _back : _forward)
        : (_invert ? _forward : _back);
    final rightAction = isManga
        ? (_invert ? _forward : _back)
        : (_invert ? _back : _forward);
    final (leftPage, rightPage) = pairOf(page);
    final isCurrentView = _viewOfPage(page) == _viewOfPage(_page);
    late final Widget pageView;
    if (bytes == null) {
      if (isCurrentView) _ensure(page);
      pageView = _pagePlaceholder(page);
    } else if (rightPage != null) {
      if (isCurrentView) {
        _ensure(leftPage);
        _ensure(rightPage);
      }
      final leftBytes = _bytes[leftPage];
      final rightBytes = _bytes[rightPage];
      pageView = leftBytes != null
          ? _buildPair(
              leftBytes: leftBytes,
              leftIdx: leftPage,
              rightBytes: rightBytes,
              rightIdx: rightPage,
            )
          : _pagePlaceholder(leftPage);
    } else {
      pageView = _buildImage(bytes, page);
    }
    final visiblePages = <int>{page, leftPage, ?rightPage};
    final loading =
        isCurrentView &&
        visiblePages.any(
          (visiblePage) =>
              _pageLoads[visiblePage]?.status == _PageLoadStatus.loading,
        );
    return Stack(
      children: [
        Positioned.fill(child: pageView),
        Positioned(
          left: 0,
          top: 0,
          bottom: 0,
          width: 80,
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTap: leftAction,
          ),
        ),
        Positioned(
          right: 0,
          top: 0,
          bottom: 0,
          width: 80,
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTap: rightAction,
          ),
        ),
        if (loading)
          const Positioned(
            top: 8,
            right: 8,
            child: SizedBox(
              width: 20,
              height: 20,
              child: CircularProgressIndicator(strokeWidth: 2),
            ),
          ),
        if (_rotationMode) ...[
          if (rightPage == null)
            Positioned(
              left: 0,
              right: 0,
              bottom: 10,
              child: Center(child: _rotationButton(page)),
            )
          else ...[
            Positioned(left: 12, bottom: 10, child: _rotationButton(leftPage)),
            Positioned(
              right: 12,
              bottom: 10,
              child: _rotationButton(rightPage),
            ),
          ],
        ],
      ],
    );
  }

  Widget _buildPair({
    required Uint8List leftBytes,
    required int leftIdx,
    Uint8List? rightBytes,
    required int rightIdx,
  }) {
    final isManga = _mode == ReadMode.manga;
    return GestureDetector(
      onDoubleTap: () => _toggleZoomByDoubleTap(_dualZoomCtrl),
      child: Center(
        child: InteractiveViewer(
          transformationController: _dualZoomCtrl,
          minScale: 1.0,
          maxScale: 4.0,
          scaleEnabled: false,
          panEnabled: _dualZoomed,
          child: LayoutBuilder(
            builder: (context, c) {
              final div = _gap.clamp(0, 20);
              // 向下取整保证 2*halfW+div <= maxWidth，避免双页拼接 Row 亚像素溢出
              // （round() 向上取整会偶发 RIGHT OVERFLOWED BY 0.x PIXELS 遮挡画面）。
              final halfW = ((c.maxWidth - div) / 2).floor().clamp(1, 4096);
              Widget tile(Uint8List b, int idx) => ClipRect(
                child: FittedBox(
                  fit: BoxFit.contain,
                  child: RotatedBox(
                    quarterTurns: _rotationOf(idx) ~/ 90,
                    child: SizedBox(
                      width: halfW.toDouble(),
                      child: Image(
                        image: ResizeImage(MemoryImage(b), width: halfW),
                        fit: BoxFit.contain,
                      ),
                    ),
                  ),
                ),
              );
              Widget leftWidget = tile(leftBytes, leftIdx);
              Widget rightWidget = rightBytes != null
                  ? tile(rightBytes, rightIdx)
                  : SizedBox(
                      width: halfW.toDouble(),
                      height: c.maxHeight,
                      child: _pagePlaceholder(rightIdx),
                    );
              return Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  if (isManga) rightWidget,
                  if (isManga && div > 0) SizedBox(width: div.toDouble()),
                  leftWidget,
                  if (!isManga && div > 0) SizedBox(width: div.toDouble()),
                  if (!isManga) rightWidget,
                ],
              );
            },
          ),
        ),
      ),
    );
  }

  // ---- 条漫 ----
  /// 滚动时按各页累计高度定位「视口中心」对应的页，页码(AppBar 标题)实时跟随。
  /// 仅页码变化时 setState，避免滚动期间高频重建。
  void _onWebtoonScroll({bool forceRebuild = false}) {
    if (_mode != ReadMode.webtoon) return;
    final b = _book;
    if (b == null || !_webtoonCtrl.hasClients) return;
    final position = _webtoonCtrl.position;
    _webtoonNavigation.observe(
      offset: position.pixels,
      viewportExtent: position.viewportDimension,
      isScrolling: position.isScrollingNotifier.value,
    );
    final p =
        _webtoonNavigation.pendingTarget ?? _webtoonNavigation.viewportPage;
    if (p == null || b.pageCount == 0) return;
    if (p != _page && mounted) {
      setState(() => _page = p);
      _disposeDistantPhotoCtrls();
      _completion.observeStablePage(p);
      _scheduleEndPrompt(); // 条漫：滚到最后一屏同样计时提示
    } else if (forceRebuild && mounted) {
      setState(() {});
    }
    _pumpPageLoadQueue();
  }

  Widget _buildWebtoon() {
    final book = _book;
    if (book == null) {
      return const Center(child: CircularProgressIndicator());
    }
    _schedulePendingWebtoonJumpAfterAttach();
    return LayoutBuilder(
      builder: (context, constraints) {
        final decodeWidth =
            (constraints.maxWidth * MediaQuery.devicePixelRatioOf(context))
                .ceil()
                .clamp(1, 4096);
        return GestureDetector(
          onDoubleTap: () => _toggleZoomByDoubleTap(_webtoonZoomCtrl),
          child: InteractiveViewer(
            transformationController: _webtoonZoomCtrl,
            minScale: 1.0,
            maxScale: 4.0,
            scaleEnabled: true,
            panEnabled: false,
            child: NotificationListener<ScrollNotification>(
              onNotification: (notification) {
                if (notification is ScrollStartNotification) {
                  if (!_webtoonProgrammaticScroll) {
                    _webtoonNavigation.freezeUnknownHeight();
                  }
                  if (notification.dragDetails != null) {
                    _webtoonNavigation.cancelPendingForUserGesture();
                    _webtoonScrollGeneration++;
                    _webtoonProgrammaticScroll = false;
                    _webtoonLoadTarget = null;
                    _webtoonPendingAttachPage = null;
                    _pumpPageLoadQueue();
                  }
                } else if (notification is ScrollEndNotification &&
                    !_webtoonProgrammaticScroll) {
                  _webtoonNavigation.settle();
                }
                return false;
              },
              child: ListView.builder(
                key: _webtoonListKey,
                controller: _webtoonCtrl,
                itemCount: book.pageCount,
                itemBuilder: (context, page) {
                  final bytes = _bytes[page];
                  late final Widget item;
                  if (bytes == null) {
                    // SliverList may build every item before a distant pixel
                    // offset. Only the active window may start page I/O.
                    if (_shouldLoadWebtoonPage(page)) _ensure(page);
                    item = SizedBox(
                      height: _webtoonNavigation.heightFor(page),
                      child: _pagePlaceholder(page),
                    );
                  } else {
                    item = GestureDetector(
                      onTap: () async {
                        if (_page == page) return;
                        setState(() {
                          _page = page;
                          _webtoonLoadTarget = null;
                          _webtoonPendingAttachPage = null;
                        });
                        _disposeDistantPhotoCtrls();
                        _webtoonNavigation.freezeUnknownHeight();
                        _webtoonNavigation.selectExplicit(page);
                        _pumpPageLoadQueue();
                        _completion.observeStablePage(page);
                        final source = widget.source;
                        if (source != null) {
                          await LibraryStore.instance.recordRead(
                            source: source,
                            path: widget.path,
                            title: widget.title,
                            page: page,
                          );
                        }
                      },
                      child: Image(
                        image: ResizeImage(
                          MemoryImage(bytes),
                          width: decodeWidth,
                        ),
                        fit: BoxFit.fitWidth,
                      ),
                    );
                  }

                  return Builder(
                    builder: (itemContext) {
                      WidgetsBinding.instance.addPostFrameCallback((_) {
                        if (!mounted || !itemContext.mounted) return;
                        final renderObject = itemContext.findRenderObject();
                        if (renderObject is! RenderBox) return;
                        final height = renderObject.size.height;
                        if (height <= 0) return;
                        if (page >= _webtoonHeights.length) {
                          _webtoonHeights.addAll(
                            List<double>.filled(
                              page + 1 - _webtoonHeights.length,
                              0,
                            ),
                          );
                        }
                        _webtoonHeights[page] = height;

                        // A placeholder is only a layout estimate. It must
                        // not be fed back into the measured image heights.
                        final imageHeightChanged =
                            bytes != null &&
                            _webtoonNavigation.measure(page, height);

                        var correctionApplied = false;
                        if (!_webtoonProgrammaticScroll) {
                          final listContext = _webtoonListKey.currentContext;
                          if (listContext != null && listContext.mounted) {
                            final listObject = listContext.findRenderObject();
                            if (listObject is RenderBox) {
                              final top = listObject
                                  .globalToLocal(
                                    renderObject.localToGlobal(Offset.zero),
                                  )
                                  .dy;
                              final correction = _webtoonAnchor.record(
                                index: page,
                                newHeight: height,
                                itemTopInViewport: top,
                              );
                              if (correction != 0 && _webtoonCtrl.hasClients) {
                                final position = _webtoonCtrl.position;
                                final target = (position.pixels + correction)
                                    .clamp(
                                      position.minScrollExtent,
                                      position.maxScrollExtent,
                                    );
                                position.correctBy(target - position.pixels);
                                correctionApplied = true;
                              }
                            }
                          }
                        }
                        if ((imageHeightChanged || correctionApplied) &&
                            mounted) {
                          _onWebtoonScroll(forceRebuild: true);
                        }
                      });
                      return item;
                    },
                  );
                },
              ),
            ),
          ),
        );
      },
    );
  }

  // ---- 右键菜单 ----
  void _onRightClick(TapUpDetails details) {
    showMenu<String>(
      position: RelativeRect.fromLTRB(
        details.globalPosition.dx,
        details.globalPosition.dy,
        details.globalPosition.dx + 1,
        details.globalPosition.dy + 1,
      ),
      context: context,
      items: [
        if (!isAndroidPlatform)
          PopupMenuItem(
            value: 'ai_version',
            child: ListTile(
              leading: Icon(
                _useAiVersion ? Icons.image_not_supported : Icons.auto_fix_high,
              ),
              title: Text(_useAiVersion ? '使用原版' : '使用超分版本'),
              dense: true,
            ),
          ),
        PopupMenuItem(
          value: 'settings',
          child: ListTile(
            leading: Icon(Icons.tune),
            title: Text('阅读设置'),
            dense: true,
          ),
        ),
        if (!isAndroidPlatform)
          PopupMenuItem(
            value: 'ai',
            child: ListTile(
              leading: Icon(Icons.auto_fix_high),
              title: Text('AI 超分 (2x)'),
              dense: true,
            ),
          ),
        PopupMenuItem(
          value: 'rotate',
          child: ListTile(
            leading: Icon(
              _rotationMode ? Icons.rotate_left : Icons.rotate_right,
            ),
            title: Text(_rotationMode ? '退出旋转模式' : '界面旋转'),
            dense: true,
          ),
        ),
      ],
    ).then((value) {
      if (value == 'ai_version') _toggleAiVersion();
      if (value == 'settings') _showSettings();
      if (value == 'ai') _doAiSuperResolve();
      if (value == 'rotate') _toggleRotationMode();
    });
  }

  Future<void> _doAiSuperResolve() async {
    if (_aiProcessing) {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(const SnackBar(content: Text('AI 超分处理中，请稍候')));
      }
      return;
    }
    _aiProcessing = true;
    try {
      final bytes = _bytes[_page];
      if (bytes == null) {
        if (mounted) {
          ScaffoldMessenger.of(
            context,
          ).showSnackBar(const SnackBar(content: Text('当前页尚未加载，请等待加载完成')));
        }
        return;
      }
      final b = _book;
      if (b == null) return;

      setState(() {});
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text('AI 超分处理中...'),
          duration: Duration(seconds: 2),
        ),
      );
      try {
        final result = await superResolve(pageBytes: bytes, scale: 2);
        if (!mounted) return;
        setState(() {
          _bytes[_page] = result;
        });
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text('AI 超分完成 ✓'),
            duration: Duration(seconds: 2),
          ),
        );
      } catch (e) {
        if (!mounted) return;
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('AI 超分失败: $e'),
            duration: const Duration(seconds: 4),
          ),
        );
      }
    } finally {
      _aiProcessing = false;
    }
  }

  // ---- 设置 ----
  void _showSettings() {
    showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      builder: (ctx) => StatefulBuilder(
        builder: (ctx, ss) => SingleChildScrollView(
          padding: EdgeInsets.fromLTRB(20, 12, 20, 24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Center(
                child: Container(
                  width: 36,
                  height: 4,
                  decoration: BoxDecoration(
                    color: Theme.of(context).colorScheme.outlineVariant,
                    borderRadius: BorderRadius.circular(2),
                  ),
                ),
              ),
              const SizedBox(height: 16),
              const Text(
                '阅读设置(仅对本会话)',
                style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600),
              ),
              const SizedBox(height: 12),
              const Text('阅读模式'),
              const SizedBox(height: 6),
              SegmentedButton<ReadMode>(
                segments: ReadMode.values
                    .map((r) => ButtonSegment(value: r, label: Text(r.label)))
                    .toList(),
                selected: {_mode},
                onSelectionChanged: (vs) {
                  ss(() {});
                  setState(() {
                    _mode = vs.first;
                  });
                  _recreatePageCtrl();
                  _disposeDistantPhotoCtrls();
                },
              ),
              const SizedBox(height: 16),
              const Text('双页拼接'),
              const SizedBox(height: 6),
              SegmentedButton<DualPageMode>(
                segments: DualPageMode.values
                    .map((d) => ButtonSegment(value: d, label: Text(d.label)))
                    .toList(),
                selected: {_dual},
                onSelectionChanged: (vs) {
                  ss(() {});
                  setState(() {
                    _dual = vs.first;
                  });
                  _recreatePageCtrl();
                  _disposeDistantPhotoCtrls();
                },
              ),
              const SizedBox(height: 8),
              Row(
                children: [
                  const Text('拼接间隙:'),
                  SizedBox(
                    width: 120,
                    child: Slider(
                      value: _gap.toDouble(),
                      min: 0,
                      max: 20,
                      divisions: 20,
                      label: '${_gap}px',
                      onChanged: (v) {
                        ss(() {});
                        setState(() {
                          _gap = v.toInt();
                        });
                      },
                    ),
                  ),
                  Text('${_gap}px'),
                ],
              ),
              const SizedBox(height: 10),
              Row(
                children: [
                  const Text('首页单独显示(不参与拼接)'),
                  const Spacer(),
                  Switch(
                    value: _skipCover,
                    onChanged: (v) {
                      ss(() {});
                      setState(() {
                        _skipCover = v;
                      });
                      _recreatePageCtrl();
                    },
                  ),
                ],
              ),
              const SizedBox(height: 16),
              SwitchListTile(
                title: const Text('日漫模式点击区反向'),
                subtitle: const Text('打开后右侧区域变为前进'),
                dense: true,
                contentPadding: EdgeInsets.zero,
                value: _invert,
                onChanged: (v) {
                  ss(() {});
                  setState(() {
                    _invert = v;
                  });
                },
              ),
              if (!isAndroidPlatform) ...[
                SizedBox(height: 16),
                Text(
                  '🤖 AI 超分',
                  style: TextStyle(
                    color: Theme.of(context).colorScheme.onSurfaceVariant,
                    fontSize: 12,
                  ),
                ),
                SizedBox(height: 6),
                SizedBox(
                  width: double.infinity,
                  child: Card(
                    child: Padding(
                      padding: EdgeInsets.all(12),
                      child: Text(
                        '右键当前页选择 \'AI 超分 (2x)\' 即可端侧推理放大图片，已启用。',
                        style: TextStyle(
                          fontSize: 12,
                          color: Theme.of(context).colorScheme.onSurfaceVariant,
                        ),
                      ),
                    ),
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final b = _book;
    final isManga = _mode == ReadMode.manga;
    final (leftPg, rightPg) = pairOf(_page);
    // 左键=后退(日漫为前进),右键=前进(日漫为后退);箭头一律指向目标方向。
    final showL = Icons.chevron_left, showR = Icons.chevron_right;
    String pageLabel;
    if (b == null) {
      pageLabel = '';
    } else if (rightPg != null) {
      final l = leftPg + 1, r = rightPg + 1;
      pageLabel = isManga ? '$r-$l / ${b.pageCount}' : '$l-$r / ${b.pageCount}';
    } else {
      pageLabel = '${_page + 1} / ${b.pageCount}';
    }
    return PopScope(
      canPop: true,
      child: Scaffold(
        appBar: AppBar(
          title: GestureDetector(
            onTap: _showJumpDialog,
            child: Text(
              b == null ? widget.title : '${b.title}  ($pageLabel)',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
            ),
          ),
          actions: [
            if (!isAndroidPlatform)
              IconButton(
                icon: Icon(
                  _useAiVersion
                      ? Icons.auto_fix_high
                      : Icons.image_not_supported,
                ),
                tooltip: _useAiVersion ? '当前为超分版本，点击切换原版' : '当前为原版，点击切换超分版本',
                onPressed: _toggleAiVersion,
              ),
            IconButton(
              icon: const Icon(Icons.casino_outlined),
              tooltip: '随机阅读一本',
              onPressed: _randomBookPicking ? null : _openRandomBook,
            ),
            IconButton(
              icon: const Icon(Icons.tune),
              tooltip: '阅读设置',
              onPressed: _showSettings,
            ),
          ],
        ),
        body: Focus(
          focusNode: _focus,
          autofocus: true,
          onKeyEvent: _onKey,
          child: GestureDetector(
            onSecondaryTapUp: _onRightClick,
            onLongPressStart: (d) => _onRightClick(
              TapUpDetails(
                kind: PointerDeviceKind.touch,
                globalPosition: d.globalPosition,
              ),
            ),
            child: _buildBody(),
          ),
        ),
        bottomNavigationBar: b == null
            ? null
            : SafeArea(
                child: Padding(
                  padding: EdgeInsets.symmetric(vertical: 2),
                  child: Row(
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      IconButton(icon: Icon(showL), onPressed: _back),
                      GestureDetector(
                        onTap: _showJumpDialog,
                        child: Text(
                          pageLabel,
                          style: const TextStyle(
                            decoration: TextDecoration.underline,
                            decorationStyle: TextDecorationStyle.dotted,
                          ),
                        ),
                      ),
                      IconButton(icon: Icon(showR), onPressed: _forward),
                    ],
                  ),
                ),
              ),
      ),
    );
  }
}

/// 阅读器「视口 ↔ 真实页」映射。
/// 双页模式下一页视口对应两页;「首页单独显示」时首页再独占一个视口。
class ReaderPaging {
  const ReaderPaging({
    required this.dual,
    required this.skipCover,
    required this.pageCount,
  });

  final bool dual;
  final bool skipCover;
  final int pageCount;

  int get viewCount {
    if (!dual) return pageCount;
    if (pageCount <= 1) return 1;
    return skipCover ? 1 + (pageCount ~/ 2) : (pageCount + 1) ~/ 2;
  }

  /// 真实页(双页模式下为拼接组基准页) → 视口序号。
  int viewOfPage(int page) {
    if (!dual) return page;
    if (skipCover) return page == 0 ? 0 : 1 + ((page - 1) ~/ 2);
    return page ~/ 2;
  }

  /// 视口序号 → 真实基准页。
  int pageOfView(int view) {
    if (!dual) return view;
    if (skipCover) return view == 0 ? 0 : 1 + (view - 1) * 2;
    return view * 2;
  }
}
