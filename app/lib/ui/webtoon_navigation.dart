/// Pure state model for continuous (webtoon) reading.
///
/// The viewport can move through pages faster than Flutter can measure their
/// extents.  Keeping the observed viewport page separate from the stable
/// logical page prevents transient layout estimates from being persisted as
/// reading progress.
class WebtoonNavigationIntent {
  const WebtoonNavigationIntent({required this.generation, required this.targetPage});

  final int generation;
  final int targetPage;
}

class WebtoonNavigationModel {
  WebtoonNavigationModel({
    required int pageCount,
    int initialPage = 0,
    this.estimatedHeight = 240,
  }) : _pageCount = pageCount.clamp(0, 1 << 30),
       _stablePage = initialPage.clamp(0, pageCount > 0 ? pageCount - 1 : 0),
       _viewportPage = initialPage.clamp(0, pageCount > 0 ? pageCount - 1 : 0),
       _measuredHeights = List<double>.filled(pageCount.clamp(0, 1 << 30), 0);

  final double estimatedHeight;
  int _pageCount;
  int _stablePage;
  int? _viewportPage;
  int? _pendingTarget;
  int _generation = 0;
  List<double> _measuredHeights;

  int get pageCount => _pageCount;
  int get stablePage => _stablePage;
  int? get viewportPage => _viewportPage;
  int? get pendingTarget => _pendingTarget;
  int get generation => _generation;
  List<double> get measuredHeights => List.unmodifiable(_measuredHeights);

  /// Reinitializes the model after the book handle becomes available.
  void reset({required int pageCount, int initialPage = 0}) {
    _pageCount = pageCount.clamp(0, 1 << 30);
    _measuredHeights = List<double>.filled(_pageCount, 0);
    final last = _pageCount > 0 ? _pageCount - 1 : 0;
    _stablePage = initialPage.clamp(0, last);
    _viewportPage = _stablePage;
    _pendingTarget = null;
    _generation++;
  }

  WebtoonNavigationIntent requestTarget(int page) {
    final target = _clampPage(page);
    _generation++;
    _pendingTarget = target;
    return WebtoonNavigationIntent(generation: _generation, targetPage: target);
  }

  bool accepts(int generation) => generation == _generation;

  /// Cancels only the current programmatic intent.  The stable page is left
  /// untouched until a subsequent scroll settles.
  void cancelPendingForUserGesture() {
    if (_pendingTarget != null) {
      _generation++;
      _pendingTarget = null;
    }
  }

  void measure(int page, double height) {
    if (page < 0 || page >= _pageCount || !height.isFinite || height <= 0) return;
    _measuredHeights[page] = height;
  }

  double heightFor(int page) {
    if (page < 0 || page >= _pageCount) return estimatedHeight;
    final measured = _measuredHeights[page];
    return measured > 0 && measured.isFinite ? measured : estimatedHeight;
  }

  /// Returns the offset of the top of [page], using an estimate for pages that
  /// have not been laid out yet.  It is intentionally monotonic even while
  /// images are loading.
  double offsetFor(int page) {
    final end = page.clamp(0, _pageCount);
    var offset = 0.0;
    for (var i = 0; i < end; i++) {
      offset += heightFor(i);
    }
    return offset;
  }

  double offsetForIntent(WebtoonNavigationIntent intent) => offsetFor(intent.targetPage);

  /// Observes the page around the viewport center.  During motion this only
  /// updates [viewportPage]; a stable logical page is committed after motion
  /// has ended and [settle] is called.
  void observe({required double offset, required double viewportExtent, required bool isScrolling}) {
    if (_pageCount == 0) {
      _viewportPage = 0;
      return;
    }
    final center = (offset.isFinite ? offset : 0) +
        (viewportExtent.isFinite && viewportExtent > 0 ? viewportExtent / 2 : 0);
    var cumulative = 0.0;
    var page = _pageCount - 1;
    for (var i = 0; i < _pageCount; i++) {
      cumulative += heightFor(i);
      if (center < cumulative) {
        page = i;
        break;
      }
    }
    _viewportPage = page;
    if (!isScrolling && _pendingTarget == null) {
      _stablePage = page;
    }
  }

  /// Marks a programmatic animation as complete.  A generation mismatch is a
  /// stale callback and must not clear a newer target.
  bool completeProgrammatic(WebtoonNavigationIntent intent) {
    if (!accepts(intent.generation) || _pendingTarget != intent.targetPage) return false;
    _pendingTarget = null;
    _viewportPage ??= intent.targetPage;
    _stablePage = intent.targetPage;
    return true;
  }

  /// Commits the latest observed page once no programmatic target is pending.
  void settle() {
    if (_pendingTarget == null && _viewportPage != null) {
      _stablePage = _clampPage(_viewportPage!);
    }
  }

  /// Explicit taps are user intent and can be committed immediately.
  void selectExplicit(int page) {
    _generation++;
    _pendingTarget = null;
    _stablePage = _clampPage(page);
    _viewportPage = _stablePage;
  }

  int _clampPage(int page) => page.clamp(0, _pageCount > 0 ? _pageCount - 1 : 0);
}

/// 条漫滚动锚点守护：占位高度 → 真实高度的收敛**不得**推动可见内容。
///
/// 为什么需要：条漫用 `ListView`（无 `itemExtent`），未加载页先是占位高度、图片就绪后
/// 换成真实高度（条漫页常 1000–4000px）。`SliverList` 只保持**像素偏移**，因此
/// **视口上方**的条目一变高，同一偏移对应的可见内容就整体后退；快速下拉时前几页占位
/// 同时收敛，用户看到的就是"划着划着突然跳回好几页前"。
///
/// 修复方式：对"旧底边仍在视口顶边之上"的条目，把测高变化量等量补偿回滚动偏移
/// （调用方用 `ScrollPosition.correctBy` 静默施加，避免打断快速下拉的惯性）。
/// 跨在视口顶边上的条目不补偿——它的增长发生在顶边之下，且用户正看着它变化。
class WebtoonAnchorKeeper {
  final Map<int, double> _rendered = <int, double>{};
  final Map<int, double> _announced = <int, double>{};

  double? renderedHeightOf(int index) => _rendered[index];

  int get trackedCount => _rendered.length;

  /// 某页"未加载占位 → 已有字节"时调用：告知它在**下次测量前**的高度是 [fromHeight]。
  ///
  /// 这是必要的：视口上方的页可能**从未被构建过**（sliver 会回收视口外的子项），
  /// 一旦字节到达，它下次进入布局时就会直接以真实高度出现，把下方内容整体推走；
  /// 此时 keeper 没有它的前值记录，只看"测过的前值"就会漏掉这次位移。
  void announceGrowth(int index, double fromHeight) {
    if (!fromHeight.isFinite || fromHeight <= 0) return;
    _announced[index] = fromHeight;
  }

  /// 记录本帧测得的实际高度，返回需要施加的滚动偏移补偿（`0` = 不补偿）。
  ///
  /// [itemTopInViewport]：条目顶边相对视口顶边的位置（视口内为正、已滚过为负）。
  /// 条目自身增高的方向是向下的，所以它的**顶边**在前后两帧相同，可用同一值判定。
  double record({
    required int index,
    required double newHeight,
    required double itemTopInViewport,
  }) {
    if (!newHeight.isFinite || newHeight <= 0) return 0;
    final announced = _announced.remove(index);
    final previous = _rendered[index] ?? announced;
    _rendered[index] = newHeight;
    if (previous == null || previous == newHeight) return 0;
    // 旧底边仍在视口顶边之上 ⇒ 这次变化整段发生在视口上方，必须补偿。
    if (itemTopInViewport + previous > 0) return 0;
    return newHeight - previous;
  }

  /// 切换 AI 版本 / 重新进入条漫时清空（此前测量的高度与预警都已失效）。
  void reset() {
    _rendered.clear();
    _announced.clear();
  }
}
