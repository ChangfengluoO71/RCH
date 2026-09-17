import 'package:app/ui/webtoon_navigation.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('WebtoonNavigationModel', () {
    test('unknown extents use a non-zero estimate and keep offsets monotonic', () {
      final model = WebtoonNavigationModel(pageCount: 100, estimatedHeight: 240);

      expect(model.offsetFor(0), 0);
      expect(model.offsetFor(1), 240);
      expect(model.offsetFor(50), 50 * 240);
      expect(model.offsetFor(100), 100 * 240);
    });

    test('latest programmatic target wins and stale generations are ignored', () {
      final model = WebtoonNavigationModel(pageCount: 20);
      final first = model.requestTarget(10);
      final second = model.requestTarget(3);

      expect(first.generation, lessThan(second.generation));
      expect(model.pendingTarget, 3);
      expect(model.accepts(first.generation), isFalse);
      expect(model.accepts(second.generation), isTrue);
      expect(model.offsetForIntent(first), model.offsetFor(10));
    });

    test('programmatic completion clears only the matching pending target', () {
      final model = WebtoonNavigationModel(pageCount: 20);
      final first = model.requestTarget(10);
      final second = model.requestTarget(3);

      expect(model.completeProgrammatic(first), isFalse);
      expect(model.pendingTarget, 3);
      expect(model.completeProgrammatic(second), isTrue);
      expect(model.pendingTarget, isNull);
      expect(model.stablePage, 3);
    });

    test('fast scroll observes viewport without overwriting stable page until settle', () {
      final model = WebtoonNavigationModel(pageCount: 20, estimatedHeight: 200);
      expect(model.stablePage, 0);

      model.observe(offset: 1700, viewportExtent: 200, isScrolling: true);
      expect(model.viewportPage, 9);
      expect(model.stablePage, 0);

      model.observe(offset: 1700, viewportExtent: 200, isScrolling: false);
      model.settle();
      expect(model.stablePage, 9);
    });

    test('user gesture cancels a pending target without jumping to stale target', () {
      final model = WebtoonNavigationModel(pageCount: 20, estimatedHeight: 200);
      final intent = model.requestTarget(12);
      model.cancelPendingForUserGesture();
      model.observe(offset: 400, viewportExtent: 200, isScrolling: false);
      model.settle();

      expect(model.accepts(intent.generation), isFalse);
      expect(model.pendingTarget, isNull);
      expect(model.stablePage, 2);
    });

    test('measured heights replace estimates while preserving monotonic offsets', () {
      final model = WebtoonNavigationModel(pageCount: 3, estimatedHeight: 200);
      model.measure(0, 500);
      model.measure(1, 100);

      expect(model.offsetFor(1), 500);
      expect(model.offsetFor(2), 600);
      expect(model.offsetFor(3), 800);
    });
  });
}
