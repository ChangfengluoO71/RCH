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
