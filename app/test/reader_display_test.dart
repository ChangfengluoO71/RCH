import 'dart:typed_data';

import 'package:app/store/models.dart';
import 'package:app/store/reader_display.dart';
import 'package:app/store/remote_cache_cleanup.dart';
import 'package:flutter_test/flutter_test.dart';

Uint8List _spreadPages({
  required bool seam,
  int width = 200,
  int height = 120,
}) {
  final pixels = Uint8List(width * height * 4);
  for (var y = 0; y < height; y++) {
    for (var x = 0; x < width; x++) {
      // High-frequency black/white content on both pages.
      final inGutter = seam && x >= 97 && x < 103;
      final value = inGutter ? 255 : ((x + y).isEven ? 0 : 255);
      final offset = (y * width + x) * 4;
      pixels[offset] = value;
      pixels[offset + 1] = value;
      pixels[offset + 2] = value;
      pixels[offset + 3] = 255;
    }
  }
  return pixels;
}

void main() {
  group('ReaderPaging 虚拟分页', () {
    test('RTL 与 LTR 分别按阅读顺序排列拆分页', () {
      final rtl = ReaderPaging(
        dual: false,
        skipCover: false,
        pageCount: 1,
        splitSeams: const {0: 0.52},
        rightToLeft: true,
      );
      final ltr = ReaderPaging(
        dual: false,
        skipCover: false,
        pageCount: 1,
        splitSeams: const {0: 0.52},
        rightToLeft: false,
      );

      expect(rtl.viewCount, 2);
      expect(rtl.displayPageOfView(0).region, DisplayPageRegion.right);
      expect(rtl.displayPageOfView(1).region, DisplayPageRegion.left);
      expect(ltr.displayPageOfView(0).region, DisplayPageRegion.left);
      expect(ltr.displayPageOfView(1).region, DisplayPageRegion.right);
      expect(rtl.displayPageOfView(0).partNumber, 1);
      expect(rtl.displayPageOfView(1).partNumber, 2);
    });

    test('普通页、拆分页混排，映射回物理页稳定', () {
      const paging = ReaderPaging(
        dual: false,
        skipCover: false,
        pageCount: 4,
        splitSeams: {1: 0.5, 3: 0.49},
      );
      expect(paging.viewCount, 6);
      expect(
        [for (var i = 0; i < paging.viewCount; i++) paging.pageOfView(i)],
        [0, 1, 1, 2, 3, 3],
      );
      expect(paging.viewOfPage(3), 4);
      expect(
        paging.viewOfDisplayPage(
          const DisplayPage(
            sourcePageIndex: 1,
            region: DisplayPageRegion.left,
            partNumber: 2,
            partCount: 2,
            seamX: 0.5,
          ),
        ),
        2,
      );
    });

    test('双页拼接抑制虚拟拆分并保留封面跳过', () {
      const paging = ReaderPaging(
        dual: true,
        skipCover: true,
        pageCount: 5,
        splitSeams: {1: 0.5, 2: 0.5},
      );
      expect(paging.viewCount, 3);
      expect(paging.displayPageOfView(0).sourcePageIndex, 0);
      expect(paging.pageOfView(1), 1);
      expect(paging.displayPageOfView(1).region, DisplayPageRegion.whole);
    });

    test('跳转物理页落在阅读方向的第一半，首尾夹取正确', () {
      const paging = ReaderPaging(
        dual: false,
        skipCover: true,
        pageCount: 3,
        splitSeams: {0: 0.5, 2: 0.5},
      );
      expect(paging.viewOfPage(0), 0);
      expect(paging.viewOfPage(2), 3);
      expect(paging.displayPageOfView(-1).sourcePageIndex, 0);
      expect(paging.displayPageOfView(99).sourcePageIndex, 2);
    });

    test('移除当前页拆分时，第二半位置可回映到同一物理页整页视口', () {
      const split = ReaderPaging(
        dual: false,
        skipCover: false,
        pageCount: 3,
        splitSeams: {1: 0.5},
      );
      final secondHalf = split.displayPageOfView(2);
      const whole = ReaderPaging(dual: false, skipCover: false, pageCount: 3);

      final remappedView = whole.viewOfDisplayPage(secondHalf);
      expect(remappedView, 1);
      expect(whole.pageOfView(remappedView), 1);
    });
  });

  group('宽页中缝检测', () {
    test('中心存在足够宽且低纹理的连续中缝时返回切分位置', () {
      final seam = WidePageDetector.detect(
        rgba: _spreadPages(seam: true),
        width: 200,
        height: 120,
      );
      expect(seam, isNotNull);
      expect(seam!, inInclusiveRange(0.45, 0.55));
    });

    test('没有中缝时保守地不切分', () {
      expect(
        WidePageDetector.detect(
          rgba: _spreadPages(seam: false),
          width: 200,
          height: 120,
        ),
        isNull,
      );
    });

    test('中心连续构图会因中线边缘密度而拒绝自动切分', () {
      final pixels = _spreadPages(seam: true);
      for (var y = 0; y < 120; y++) {
        for (var x = 97; x < 103; x++) {
          final value = ((x + y).isEven ? 0 : 255);
          final offset = (y * 200 + x) * 4;
          pixels[offset] = value;
          pixels[offset + 1] = value;
          pixels[offset + 2] = value;
        }
      }
      expect(
        WidePageDetector.detect(rgba: pixels, width: 200, height: 120),
        isNull,
      );
    });

    test('旋转后按显示方向评估宽高比', () {
      expect(effectiveAspectRatio(200, 100, quarterTurns: 0), 2);
      expect(effectiveAspectRatio(200, 100, quarterTurns: 1), 0.5);
      expect(
        shouldAutoSplitWidePage(
          width: 200,
          height: 100,
          quarterTurns: 1,
          portraitViewport: true,
          hasCenterSeam: true,
        ),
        isFalse,
      );
    });

    test('智能只拆竖屏高置信度宽页，强制和逐页纠错采用明确覆盖策略', () {
      const smart = WidePageMode.smart;
      const split = WidePageMode.split;
      const keepWhole = WidePageMode.keepWhole;

      expect(
        shouldSplitWidePage(
          mode: smart,
          pageOverride: null,
          width: 200,
          height: 100,
          quarterTurns: 0,
          portraitViewport: true,
          hasCenterSeam: true,
        ),
        isTrue,
      );
      expect(
        shouldSplitWidePage(
          mode: smart,
          pageOverride: null,
          width: 200,
          height: 100,
          quarterTurns: 0,
          portraitViewport: false,
          hasCenterSeam: true,
        ),
        isFalse,
      );
      expect(
        shouldSplitWidePage(
          mode: smart,
          pageOverride: null,
          width: 200,
          height: 100,
          quarterTurns: 0,
          portraitViewport: true,
          hasCenterSeam: false,
        ),
        isFalse,
      );
      expect(
        shouldSplitWidePage(
          mode: split,
          pageOverride: null,
          width: 200,
          height: 100,
          quarterTurns: 0,
          portraitViewport: false,
          hasCenterSeam: false,
        ),
        isTrue,
      );
      expect(
        shouldSplitWidePage(
          mode: keepWhole,
          pageOverride: true,
          width: 100,
          height: 200,
          quarterTurns: 0,
          portraitViewport: true,
          hasCenterSeam: false,
        ),
        isTrue,
      );
      expect(
        shouldSplitWidePage(
          mode: split,
          pageOverride: false,
          width: 200,
          height: 100,
          quarterTurns: 0,
          portraitViewport: true,
          hasCenterSeam: true,
        ),
        isFalse,
      );
    });

    test('宽页拆分仅在强制双页拼接时关闭', () {
      expect(widePageSplittingAllowed(dualPageMode: DualPageMode.off), isTrue);
      expect(
        widePageSplittingAllowed(dualPageMode: DualPageMode.force),
        isFalse,
      );
    });
  });

  test('拆分页渲染宽度翻倍并限制上界，标准普通页保留旧缓存档位', () {
    expect(
      readerSourceTargetWidth(
        RenderWidth.standard,
        screenWidth: 400,
        devicePixelRatio: 3,
        split: false,
      ),
      isNull,
    );
    expect(
      readerSourceTargetWidth(
        RenderWidth.standard,
        screenWidth: 400,
        devicePixelRatio: 3,
        split: true,
      ),
      3200,
    );
    expect(
      readerSourceTargetWidth(
        RenderWidth.screen,
        screenWidth: 2000,
        devicePixelRatio: 3,
        split: true,
      ),
      8192,
    );
  });

  test('旧设置缺少宽页选项时使用智能模式，显式值可以往返', () {
    expect(AppSettings.fromJson(const {}).widePageMode, WidePageMode.smart);
    final settings = AppSettings.fromJson(const {'widePageMode': 'keepWhole'});
    expect(settings.widePageMode, WidePageMode.keepWhole);
    expect(
      AppSettings.fromJson(settings.toJson()).widePageMode,
      WidePageMode.keepWhole,
    );
  });

  test('最后一页只有当前虚拟页为最终半页时才产生完成候选', () {
    final completion = ReadingCompletionState(pageCount: 2);
    completion.observeStableDisplayPage(
      const DisplayPage(
        sourcePageIndex: 1,
        region: DisplayPageRegion.right,
        partNumber: 1,
        partCount: 2,
        seamX: 0.5,
      ),
    );
    expect(completion.completionCandidate, isFalse);
    completion.observeStableDisplayPage(
      const DisplayPage(
        sourcePageIndex: 1,
        region: DisplayPageRegion.left,
        partNumber: 2,
        partCount: 2,
        seamX: 0.5,
      ),
    );
    expect(completion.completionCandidate, isTrue);
  });
}
