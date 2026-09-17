import 'package:app/src/rust/api/book.dart';
import 'package:app/ui/cloud115_folder_picker.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('115 文件夹选择器只展示目录并返回所选 cid', (tester) async {
    final requested = <String>[];
    Future<List<DirEntry>> listDirectory(String path) async {
      requested.add(path);
      if (path == '0') {
        return [
          DirEntry(
            name: '漫画',
            path: '42',
            isDir: true,
            size: BigInt.zero,
            mtime: 0,
          ),
          DirEntry(
            name: '普通文件.txt',
            path: 'file-pick-code',
            isDir: false,
            size: BigInt.from(10),
            mtime: 0,
          ),
        ];
      }
      return [
        DirEntry(
          name: '单行漫画',
          path: '84',
          isDir: true,
          size: BigInt.zero,
          mtime: 0,
        ),
      ];
    }

    Future<Cloud115FolderChoice?>? picker;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: TextButton(
              onPressed: () {
                picker = showDialog<Cloud115FolderChoice>(
                  context: context,
                  builder: (_) =>
                      Cloud115FolderPickerDialog(listDirectory: listDirectory),
                );
              },
              child: const Text('打开选择器'),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('打开选择器'));
    await tester.pumpAndSettle();
    expect(requested, ['0']);
    expect(find.text('漫画'), findsOneWidget);
    expect(find.text('普通文件.txt'), findsNothing);

    await tester.tap(find.text('漫画'));
    await tester.pumpAndSettle();
    expect(requested, ['0', '42']);
    await tester.tap(find.byTooltip('返回网盘根目录'));
    await tester.pumpAndSettle();
    expect(requested, ['0', '42', '0']);
    expect(find.text('漫画'), findsOneWidget);

    await tester.tap(find.text('漫画'));
    await tester.pumpAndSettle();
    expect(find.text('单行漫画'), findsOneWidget);

    await tester.tap(find.text('选择此文件夹'));
    await tester.pumpAndSettle();
    final choice = await picker!;
    expect(choice, isNotNull);
    expect(choice!.id, '42');
    expect(choice.name, '漫画');
  });

  testWidgets('115 文件夹选择器在窄屏上不溢出', (tester) async {
    tester.view.physicalSize = const Size(360, 480);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);

    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: TextButton(
              onPressed: () => showDialog<void>(
                context: context,
                builder: (_) => Cloud115FolderPickerDialog(
                  listDirectory: (_) async => const [],
                ),
              ),
              child: const Text('打开选择器'),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('打开选择器'));
    await tester.pumpAndSettle();
    expect(find.text('选择 115 漫画文件夹'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
