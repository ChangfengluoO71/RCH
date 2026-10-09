import 'dart:math' as math;
import 'dart:typed_data';

import 'models.dart';

enum DisplayPageRegion { whole, left, right }

bool widePageSplittingAllowed({required DualPageMode dualPageMode}) =>
    dualPageMode == DualPageMode.off;

/// A virtual page maps to one source image without changing physical progress.
class DisplayPage {
  const DisplayPage({
    required this.sourcePageIndex,
    required this.region,
    required this.partNumber,
    required this.partCount,
    this.seamX = 0.5,
    this.quarterTurns = 0,
  });

  const DisplayPage.whole(int sourcePageIndex)
    : this(
        sourcePageIndex: sourcePageIndex,
        region: DisplayPageRegion.whole,
        partNumber: 1,
        partCount: 1,
      );

  final int sourcePageIndex;
  final DisplayPageRegion region;

  /// Position in reading order, one-based.
  final int partNumber;
  final int partCount;

  /// The split boundary in source-image coordinates.
  final double seamX;
  final int quarterTurns;

  double get cropLeft => switch (region) {
    DisplayPageRegion.whole => 0,
    DisplayPageRegion.left => 0,
    DisplayPageRegion.right => seamX,
  };

  double get cropRight => switch (region) {
    DisplayPageRegion.whole => 1,
    DisplayPageRegion.left => seamX,
    DisplayPageRegion.right => 1,
  };

  @override
  bool operator ==(Object other) =>
      other is DisplayPage &&
      other.sourcePageIndex == sourcePageIndex &&
      other.region == region &&
      other.partNumber == partNumber &&
      other.partCount == partCount &&
      other.seamX == seamX &&
      other.quarterTurns == quarterTurns;

  @override
  int get hashCode => Object.hash(
    sourcePageIndex,
    region,
    partNumber,
    partCount,
    seamX,
    quarterTurns,
  );
}

/// Maps virtual PageView items to physical source pages.
class ReaderPaging {
  const ReaderPaging({
    required this.dual,
    required this.skipCover,
    required this.pageCount,
    this.splitSeams = const <int, double>{},
    this.splitQuarterTurns = const <int, int>{},
    this.rightToLeft = true,
  });

  final bool dual;
  final bool skipCover;
  final int pageCount;
  final Map<int, double> splitSeams;
  final Map<int, int> splitQuarterTurns;
  final bool rightToLeft;

  List<int> get _splitPages =>
      splitSeams.keys.where((page) => page >= 0 && page < pageCount).toList()
        ..sort();

  List<DisplayPage> get displayPages {
    return List<DisplayPage>.generate(viewCount, displayPageOfView);
  }

  int get viewCount {
    if (!dual) return pageCount + _splitPages.length;
    if (pageCount <= 1) return 1;
    return skipCover ? 1 + (pageCount ~/ 2) : (pageCount + 1) ~/ 2;
  }

  /// Physical page to its first virtual view (or paired spread).
  int viewOfPage(int page) {
    if (dual) {
      if (skipCover) return page == 0 ? 0 : 1 + ((page - 1) ~/ 2);
      return page ~/ 2;
    }
    if (pageCount <= 0) return 0;
    final sourcePage = page.clamp(0, pageCount - 1);
    return sourcePage +
        _splitPages.where((splitPage) => splitPage < sourcePage).length;
  }

  /// View index to the physical page represented by the view's first item.
  int pageOfView(int view) {
    if (dual) {
      if (skipCover) return view == 0 ? 0 : 1 + (view - 1) * 2;
      return view * 2;
    }
    return displayPageOfView(view).sourcePageIndex;
  }

  DisplayPage displayPageOfView(int view) {
    if (dual) return DisplayPage.whole(pageOfView(view));
    if (pageCount <= 0) return const DisplayPage.whole(0);
    final boundedView = view.clamp(0, viewCount - 1);
    var insertedPages = 0;
    for (final splitPage in _splitPages) {
      final splitStart = splitPage + insertedPages;
      if (boundedView < splitStart) break;
      if (boundedView == splitStart) return _splitDisplayPage(splitPage, 1);
      if (boundedView == splitStart + 1) return _splitDisplayPage(splitPage, 2);
      insertedPages++;
    }
    return DisplayPage.whole(boundedView - insertedPages);
  }

  int viewOfDisplayPage(DisplayPage page) {
    if (dual) return viewOfPage(page.sourcePageIndex);
    final firstView = viewOfPage(page.sourcePageIndex);
    if (!splitSeams.containsKey(page.sourcePageIndex)) return firstView;
    return firstView + (page.partNumber > 1 ? 1 : 0);
  }

  DisplayPage _splitDisplayPage(int sourcePage, int partNumber) {
    final first = rightToLeft
        ? DisplayPageRegion.right
        : DisplayPageRegion.left;
    final second = rightToLeft
        ? DisplayPageRegion.left
        : DisplayPageRegion.right;
    return DisplayPage(
      sourcePageIndex: sourcePage,
      region: partNumber == 1 ? first : second,
      partNumber: partNumber,
      partCount: 2,
      seamX: splitSeams[sourcePage] ?? 0.5,
      quarterTurns: splitQuarterTurns[sourcePage] ?? 0,
    );
  }
}

/// Center-seam analysis for an already downscaled RGBA8888 preview.
class WidePageDetector {
  const WidePageDetector._();

  static double? detect({
    required Uint8List rgba,
    required int width,
    required int height,
  }) {
    if (width < 40 || height < 40 || width > 512 || height > 4096) return null;
    if (rgba.length < width * height * 4) return null;

    final minimumBand = math.max(1, (width * 0.01).ceil());
    final maximumBand = math.max(minimumBand, (width * 0.04).floor());
    final minCenter = (width * 0.45).round();
    final maxCenter = (width * 0.55).round();
    final rowThreshold = (height * 0.85).ceil();
    double? best;
    var bestDistance = double.infinity;

    int gray(int x, int y) {
      final offset = (y * width + x) * 4;
      return ((rgba[offset] * 299 +
                  rgba[offset + 1] * 587 +
                  rgba[offset + 2] * 114) /
              1000)
          .round();
    }

    for (var bandWidth = minimumBand; bandWidth <= maximumBand; bandWidth++) {
      for (
        var left = minCenter - bandWidth ~/ 2;
        left <= maxCenter - bandWidth ~/ 2;
        left++
      ) {
        final right = left + bandWidth;
        if (left < bandWidth || right + bandWidth >= width) continue;
        var lowTextureRows = 0;
        for (var y = 0; y < height; y++) {
          var minGray = 255;
          var maxGray = 0;
          var horizontalDifference = 0;
          for (var x = left; x < right; x++) {
            final value = gray(x, y);
            minGray = math.min(minGray, value);
            maxGray = math.max(maxGray, value);
            if (x + 1 < right) {
              horizontalDifference += (value - gray(x + 1, y)).abs();
            }
          }
          var verticalDifference = 0;
          if (y + 1 < height) {
            for (var x = left; x < right; x++) {
              verticalDifference += (gray(x, y) - gray(x, y + 1)).abs();
            }
          }
          final horizontalSamples = math.max(1, bandWidth - 1);
          final meanHorizontal = horizontalDifference / horizontalSamples;
          final meanVertical = verticalDifference / bandWidth;
          if (maxGray - minGray <= 42 &&
              meanHorizontal <= 12 &&
              meanVertical <= 12) {
            lowTextureRows++;
          }
        }
        if (lowTextureRows < rowThreshold) continue;

        final centerEdges = _edgeDensity(gray, left, right, height, width);
        final neighborEdges =
            (_edgeDensity(gray, left - bandWidth, left, height, width) +
                _edgeDensity(gray, right, right + bandWidth, height, width)) /
            2;
        if (neighborEdges < 0.02 || centerEdges > neighborEdges * 0.5) {
          continue;
        }
        final seamCenter = (left + right) / (2 * width);
        final distance = (seamCenter - 0.5).abs();
        if (distance < bestDistance) {
          best = seamCenter;
          bestDistance = distance;
        }
      }
    }
    return best;
  }

  static double _edgeDensity(
    int Function(int x, int y) gray,
    int left,
    int right,
    int height,
    int imageWidth,
  ) {
    var edges = 0;
    var samples = 0;
    for (var y = 0; y < height; y++) {
      for (var x = left; x < right; x++) {
        final value = gray(x, y);
        if (x + 1 < imageWidth) {
          samples++;
          if ((value - gray(x + 1, y)).abs() >= 28) edges++;
        }
        if (y + 1 < height) {
          samples++;
          if ((value - gray(x, y + 1)).abs() >= 28) edges++;
        }
      }
    }
    return samples == 0 ? 0 : edges / samples;
  }
}

({Uint8List rgba, int width, int height}) rotateRgbaQuarterTurns({
  required Uint8List rgba,
  required int width,
  required int height,
  required int quarterTurns,
}) {
  final turns = quarterTurns % 4;
  if (turns == 0) return (rgba: rgba, width: width, height: height);
  final outputWidth = turns.isOdd ? height : width;
  final outputHeight = turns.isOdd ? width : height;
  final output = Uint8List(outputWidth * outputHeight * 4);
  for (var y = 0; y < height; y++) {
    for (var x = 0; x < width; x++) {
      final (outputX, outputY) = switch (turns) {
        1 => (height - 1 - y, x),
        2 => (width - 1 - x, height - 1 - y),
        _ => (y, width - 1 - x),
      };
      final sourceOffset = (y * width + x) * 4;
      final targetOffset = (outputY * outputWidth + outputX) * 4;
      output.setRange(targetOffset, targetOffset + 4, rgba, sourceOffset);
    }
  }
  return (rgba: output, width: outputWidth, height: outputHeight);
}

double effectiveAspectRatio(
  int width,
  int height, {
  required int quarterTurns,
}) {
  if (width <= 0 || height <= 0) return 0;
  return quarterTurns.abs().isOdd ? height / width : width / height;
}

bool shouldAutoSplitWidePage({
  required int width,
  required int height,
  required int quarterTurns,
  required bool portraitViewport,
  required bool hasCenterSeam,
}) =>
    portraitViewport &&
    effectiveAspectRatio(width, height, quarterTurns: quarterTurns) >= 1.3 &&
    hasCenterSeam;

bool maySplitWidePage({
  required WidePageMode mode,
  required bool? pageOverride,
  required int width,
  required int height,
  required int quarterTurns,
  required bool portraitViewport,
}) {
  if (pageOverride == false ||
      (pageOverride == null && mode == WidePageMode.keepWhole)) {
    return false;
  }
  if (pageOverride == true) return true;
  final ratio = effectiveAspectRatio(width, height, quarterTurns: quarterTurns);
  return switch (mode) {
    WidePageMode.smart => portraitViewport && ratio >= 1.3,
    WidePageMode.split => ratio >= 1.0,
    WidePageMode.keepWhole => false,
  };
}

bool shouldSplitWidePage({
  required WidePageMode mode,
  required bool? pageOverride,
  required int width,
  required int height,
  required int quarterTurns,
  required bool portraitViewport,
  required bool hasCenterSeam,
}) {
  if (!maySplitWidePage(
    mode: mode,
    pageOverride: pageOverride,
    width: width,
    height: height,
    quarterTurns: quarterTurns,
    portraitViewport: portraitViewport,
  )) {
    return false;
  }
  if (pageOverride == true || mode == WidePageMode.split) return true;
  return shouldAutoSplitWidePage(
    width: width,
    height: height,
    quarterTurns: quarterTurns,
    portraitViewport: portraitViewport,
    hasCenterSeam: hasCenterSeam,
  );
}

/// Request width used by the Rust renderer. A normal standard page retains the
/// legacy `None` cache profile; a split source gets twice the target width.
int? readerSourceTargetWidth(
  RenderWidth mode, {
  required double screenWidth,
  required double devicePixelRatio,
  required bool split,
}) {
  final configured = renderWidthPixels(
    mode,
    screenWidth: screenWidth,
    devicePixelRatio: devicePixelRatio,
  );
  if (!split) return configured;
  final singlePageTarget = configured ?? 1600;
  return (singlePageTarget * 2).clamp(1, 8192);
}
