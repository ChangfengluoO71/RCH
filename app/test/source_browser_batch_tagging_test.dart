import 'package:app/src/rust/api/book.dart';
import 'package:app/store/models.dart';
import 'package:app/ui/batch_tagging.dart';
import 'package:flutter_test/flutter_test.dart';

DirEntry _entry({
  required String name,
  required String path,
  required bool isDir,
}) =>
    DirEntry(name: name, path: path, isDir: isDir, size: BigInt.zero, mtime: 0);

void main() {
  test('文件批量标签目标保持为文件', () async {
    const root = r'C:\library';
    const file = r'C:\library\book.cbz';

    final targets = await collectBatchTagTargets(
      selectedPaths: [file],
      currentEntries: [_entry(name: 'book.cbz', path: file, isDir: false)],
      effectiveRootPath: root,
      isLocalFs: true,
      listDirectory: (_) async => const [],
      isComicFolder: (_) async => false,
      isComicEntry: (entry) => entry.name.endsWith('.cbz'),
    );

    expect(targets, [const BatchTagTarget.file(file)]);
  });

  test('图片型漫画文件夹自身作为目录标签目标', () async {
    const root = r'C:\library';
    const folder = r'C:\library\book';

    final targets = await collectBatchTagTargets(
      selectedPaths: [folder],
      currentEntries: [_entry(name: 'book', path: folder, isDir: true)],
      effectiveRootPath: root,
      isLocalFs: true,
      listDirectory: (path) async => [
        _entry(name: '001.png', path: '$path\\001.png', isDir: false),
      ],
      isComicFolder: (path) async => path == folder,
      isComicEntry: (entry) => entry.name.endsWith('.cbz'),
    );

    expect(targets, [const BatchTagTarget.directory(folder)]);
  });

  test('文件与文件夹混合选择时保留两类标签目标', () async {
    const root = r'C:\library';
    const file = r'C:\library\book.cbz';
    const folder = r'C:\library\book-images';

    final targets = await collectBatchTagTargets(
      selectedPaths: [file, folder],
      currentEntries: [
        _entry(name: 'book.cbz', path: file, isDir: false),
        _entry(name: 'book-images', path: folder, isDir: true),
      ],
      effectiveRootPath: root,
      isLocalFs: true,
      listDirectory: (_) async => const [],
      isComicFolder: (path) async => path == folder,
      isComicEntry: (entry) => entry.name.endsWith('.cbz'),
    );

    expect(targets, [
      const BatchTagTarget.file(file),
      const BatchTagTarget.directory(folder),
    ]);
  });

  test('重复检测同一文件夹时复用进行中的结果', () async {
    const folder = r'C:\library\book-images';
    var calls = 0;
    final checker = MemoizedBatchTagComicFolderChecker((path) async {
      calls++;
      await Future<void>.delayed(const Duration(milliseconds: 1));
      return path == folder;
    });

    final first = checker(folder);
    final second = checker(folder);

    expect(identical(first, second), isTrue);
    expect(await first, isTrue);
    expect(await second, isTrue);
    expect(calls, 1);

    checker.clear();
    expect(await checker(folder), isTrue);
    expect(calls, 2);
  });
}
