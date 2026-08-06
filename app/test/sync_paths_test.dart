import 'dart:io';

import 'package:app/store/sync_paths.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('isIgnoredSyncFile 识别冲突副本与临时文件', () {
    expect(isIgnoredSyncFile('latest.rchpkg'), false);
    expect(isIgnoredSyncFile('latest (冲突副本).rchpkg'), true);
    expect(isIgnoredSyncFile('latest(1).rchpkg'), true);
    expect(isIgnoredSyncFile('latest (1).rchpkg'), true);
    expect(isIgnoredSyncFile('latest-20260806.rchpkg'), true);
    expect(isIgnoredSyncFile('latest.rchpkg.tmp'), true);
    expect(isIgnoredSyncFile('other.rchpkg'), false);
    expect(isIgnoredSyncFile('comic.cbz'), false);
  });

  test('countIgnoredSyncFiles 只计非正式包', () {
    expect(
      countIgnoredSyncFiles([
        'latest.rchpkg',
        'latest (冲突副本).rchpkg',
        'latest.rchpkg.tmp',
        'a.cbz',
      ]),
      2,
    );
  });

  test('同步盘目录路径构建', () {
    final sep = Platform.pathSeparator;
    expect(syncLatestPath('C:${sep}sync'), 'C:${sep}sync${sep}latest.rchpkg');
    expect(syncArchiveDir('C:${sep}sync'), 'C:${sep}sync${sep}archive');
    expect(
      syncArchivePath('C:${sep}sync', '20260806_090503'),
      'C:${sep}sync${sep}archive${sep}20260806_090503.rchpkg',
    );
  });

  test('WebDAV 远程路径构建（统一斜杠）', () {
    expect(remoteRchDir('/books'), '/books/RCH');
    expect(remoteRchDir('/books/'), '/books/RCH');
    expect(remoteSyncDir('/books'), '/books/RCH/sync');
    expect(remoteLatestPath('/books'), '/books/RCH/sync/latest.rchpkg');
    expect(remoteSyncDir(''), '/RCH/sync');
  });

  test('时间戳归档名', () {
    expect(formatSyncTimestamp(DateTime(2026, 8, 6, 9, 5, 3)), '20260806_090503');
  });
}
