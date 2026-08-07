import 'package:app/store/library_store.dart';
import 'package:app/ui/common.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  tearDown(() {
    LibraryStore.instance.settings.tabletLayout = 'auto';
  });

  Widget harness(Size size, void Function(BuildContext) capture) => MediaQuery(
        data: MediaQueryData(size: size),
        child: Builder(builder: (context) {
          capture(context);
          return const SizedBox();
        }),
      );

  testWidgets('isCompact：手机宽度（<600dp）恒为紧凑布局', (tester) async {
    bool compact = true;
    await tester.pumpWidget(
      harness(const Size(400, 800), (c) => compact = isCompact(c)),
    );
    expect(compact, isTrue);
  });

  testWidgets('isCompact：平板宽度（>=600dp）auto/desktop 用桌面布局', (tester) async {
    LibraryStore.instance.settings.tabletLayout = 'auto';
    bool compact = true;
    await tester.pumpWidget(
      harness(const Size(800, 1280), (c) => compact = isCompact(c)),
    );
    expect(compact, isFalse);

    LibraryStore.instance.settings.tabletLayout = 'desktop';
    await tester.pumpWidget(
      harness(const Size(800, 1280), (c) => compact = isCompact(c)),
    );
    expect(compact, isFalse);
  });

  testWidgets('isCompact：平板宽度下 tabletLayout=mobile 用手机布局', (tester) async {
    LibraryStore.instance.settings.tabletLayout = 'mobile';
    bool compact = false;
    await tester.pumpWidget(
      harness(const Size(800, 1280), (c) => compact = isCompact(c)),
    );
    expect(compact, isTrue);
  });
}
