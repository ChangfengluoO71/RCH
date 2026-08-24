// LibraryIndexService 纯逻辑测试（ADR-020/021）：扩展名识别、root_hash、本地扫描。

import 'dart:io';

import 'package:app/src/rust/api/db.dart';
import 'package:app/store/library_index_service.dart';
import 'package:app/store/folder_snapshot_store.dart';
import 'package:app/store/models.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('isComicPath 识别漫画扩展名（大小写不敏感）', () {
    expect(LibraryIndexService.isComicPath('/a/b.cbz'), isTrue);
    expect(LibraryIndexService.isComicPath('/a/b.CBZ'), isTrue);
    expect(LibraryIndexService.isComicPath('/a/b.epub'), isTrue);
    expect(LibraryIndexService.isComicPath('/a/b.pdf'), isTrue);
    expect(LibraryIndexService.isComicPath('/a/b.txt'), isFalse);
    expect(LibraryIndexService.isComicPath('/a/b'), isFalse);
  });

  test('libraryIndexId 与 rootHashOf 稳定且随内容变化', () {
    final fp = 'fp-x';
    final idA = LibraryIndexService.libraryIndexId(fp, '/books/a.cbz');
    final idB = LibraryIndexService.libraryIndexId(fp, '/books/a.cbz');
    expect(idA, idB);
    expect(idA, isNot(LibraryIndexService.libraryIndexId(fp, '/books/b.cbz')));

    final e1 = LibraryIndexDto(
      id: idA,
      sourceId: 's1',
      parentId: null,
      name: 'a.cbz',
      path: '/books/a.cbz',
      entryType: 'file',
      size: 100,
      modifiedAt: 1000,
      coverPath: null,
      updatedAt: 1,
      deleted: false,
    );
    final h1 = LibraryIndexService.rootHashOf([e1]);
    final h2 = LibraryIndexService.rootHashOf([e1]);
    expect(h1, h2);
    final e2 = LibraryIndexDto(
      id: idA,
      sourceId: 's1',
      parentId: null,
      name: 'a.cbz',
      path: '/books/a.cbz',
      entryType: 'file',
      size: 999,
      modifiedAt: 1000,
      coverPath: null,
      updatedAt: 1,
      deleted: false,
    );
    expect(LibraryIndexService.rootHashOf([e2]), isNot(h1));
  });

  test('entryHashOf 稳定且随元数据变化（非内容哈希）', () {
    final a = LibraryIndexService.entryHashOf(
      path: '/books/a.cbz',
      name: 'a.cbz',
      entryType: 'file',
      size: 100,
      modifiedAt: 1000,
    );
    final b = LibraryIndexService.entryHashOf(
      path: '/books/a.cbz',
      name: 'a.cbz',
      entryType: 'file',
      size: 100,
      modifiedAt: 1000,
    );
    expect(a, b);
    final c = LibraryIndexService.entryHashOf(
      path: '/books/a.cbz',
      name: 'a.cbz',
      entryType: 'file',
      size: 999,
      modifiedAt: 1000,
    );
    expect(c, isNot(a));
  });

  test('scanLocalSource 只收录目录与漫画文件，id/parent 稳定', () async {
    final tmp = await Directory.systemTemp.createTemp('rch_li_test');
    addTearDown(() => tmp.delete(recursive: true));
    await File('${tmp.path}${Platform.pathSeparator}a.cbz').writeAsString('x');
    await File(
      '${tmp.path}${Platform.pathSeparator}readme.txt',
    ).writeAsString('x');
    final sub = Directory('${tmp.path}${Platform.pathSeparator}sub');
    await sub.create();
    await File('${sub.path}${Platform.pathSeparator}b.zip').writeAsString('x');
    await File(
      '${sub.path}${Platform.pathSeparator}notes.md',
    ).writeAsString('x');

    const fp = 'fp-local';
    final entries = await LibraryIndexService.scanLocalSource(
      sourceId: 's1',
      fingerprint: fp,
      rootPath: tmp.path,
    );

    final files = entries.where((e) => e.entryType == 'file').toList();
    final dirs = entries.where((e) => e.entryType == 'dir').toList();
    expect(files.map((e) => e.name), containsAll(['a.cbz', 'b.zip']));
    expect(files.map((e) => e.name), isNot(contains('readme.txt')));
    expect(files.map((e) => e.name), isNot(contains('notes.md')));
    expect(dirs.map((e) => e.name), ['sub']);
    // id = sha256(fp|path)，稳定
    expect(files[0].id, LibraryIndexService.libraryIndexId(fp, files[0].path));
    // 顶层条目 parent = 根目录 id；子目录条目 parent = 子目录 id
    final rootId = LibraryIndexService.libraryIndexId(fp, tmp.path);
    final subEntry = dirs.first;
    final b = files.firstWhere((e) => e.name == 'b.zip');
    expect(b.parentId, subEntry.id);
    final a = files.firstWhere((e) => e.name == 'a.cbz');
    expect(a.parentId, rootId);
    expect(a.size, isNotNull);
    expect(a.modifiedAt, isNotNull);
    // Phase 5.0：扫描条目带元数据哈希
    expect(a.hash, isNotNull);
    expect(
      a.hash,
      LibraryIndexService.entryHashOf(
        path: a.path,
        name: a.name,
        entryType: a.entryType,
        size: a.size,
        modifiedAt: a.modifiedAt,
      ),
    );
  });

  test('scanLocalSource 增量：目录 mtime 未变子树保留，新增文件被捕获', () async {
    final tmp = await Directory.systemTemp.createTemp('rch_li_inc');
    addTearDown(() => tmp.delete(recursive: true));
    final sep = Platform.pathSeparator;
    await File('${tmp.path}${sep}a.cbz').writeAsString('x');
    final sub = Directory('${tmp.path}${sep}sub');
    await sub.create();
    await File('${sub.path}${sep}b.zip').writeAsString('x');

    const fp = 'fp-inc';
    final full = await LibraryIndexService.scanLocalSource(
      sourceId: 's1',
      fingerprint: fp,
      rootPath: tmp.path,
    );
    expect(full.map((e) => e.name), containsAll(['a.cbz', 'sub', 'b.zip']));

    // 新增一个漫画文件（根目录 mtime 变化）→ 增量扫描应捕获
    await File('${tmp.path}${sep}c.cbz').writeAsString('x');
    final inc = await LibraryIndexService.scanLocalSource(
      sourceId: 's1',
      fingerprint: fp,
      rootPath: tmp.path,
      previous: full,
    );
    final names = inc.map((e) => e.name).toSet();
    expect(names, containsAll(['a.cbz', 'c.cbz', 'sub', 'b.zip']));

    // 无变化的子树（sub）条目原样保留
    final subEntry = inc.firstWhere((e) => e.name == 'b.zip');
    final prevSub = full.firstWhere((e) => e.name == 'b.zip');
    expect(subEntry.id, prevSub.id);
  });

  test('scanLocalSource force 全量扫描忽略 previous', () async {
    final tmp = await Directory.systemTemp.createTemp('rch_li_force');
    addTearDown(() => tmp.delete(recursive: true));
    final sep = Platform.pathSeparator;
    await File('${tmp.path}${sep}a.cbz').writeAsString('x');

    const fp = 'fp-force';
    final full = await LibraryIndexService.scanLocalSource(
      sourceId: 's1',
      fingerprint: fp,
      rootPath: tmp.path,
    );
    // force=true 且 previous 含旧目录 mtime：仍全量遍历（结果一致即证明未跳过）
    final again = await LibraryIndexService.scanLocalSource(
      sourceId: 's1',
      fingerprint: fp,
      rootPath: tmp.path,
      previous: full,
      force: true,
    );
    expect(again.length, full.length);
    expect(again.map((e) => e.name), contains('a.cbz'));
  });

  test('115 crawl uses rootId as the effective catalog root', () async {
    final source = BookSource(
      id: '115-root-test',
      type: '115',
      name: '115',
      path: 'old-root',
      rootId: 'new-root',
    );
    final requested = <String>[];
    final entries = await LibraryIndexService.crawlRemoteSource(
      source: source,
      fingerprint: 'fp-115-root',
      listRemote: (path) async {
        requested.add(path);
        return const <FolderSnapshotEntry>[];
      },
      force: true,
    );

    expect(entries, isEmpty);
    expect(requested, ['new-root']);
  });

  test('115 empty rootId resolves to the service root', () {
    final source = BookSource(
      id: '115-empty-root-test',
      type: '115',
      name: '115',
      path: 'legacy-root',
      rootId: '',
    );
    expect(source.effectiveRootPath, '0');
  });

  test('quark uses rootId as the effective catalog root', () {
    final source = BookSource(
      id: 'quark-root-test',
      type: 'quark',
      name: 'Quark',
      path: 'legacy-root',
      rootId: 'quark-root',
    );
    expect(source.effectiveRootPath, 'quark-root');
  });
}
