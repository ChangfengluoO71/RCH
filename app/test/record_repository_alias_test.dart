import 'package:app/repository/record_repository.dart';
import 'package:app/store/models.dart';
import 'package:flutter_test/flutter_test.dart';

/// 阅读记录的“远程墓碑”契约（2026-09-21 重写）。
///
/// **背景**：旧测试调用 `purgeStale(..., remoteLivePaths: ...)`，而该命名参数**已不存在**
/// （现在只接受 `remoteTombstones`）⇒ CI `flutter analyze` 2 处 error（发布门禁红线）。
/// 旧测试要断言的“远程别名（archive → 新物理路径）”改写逻辑**当前 API 里没有**
/// ⇒ 本文件只锁**现存契约**，并把缺口明确记录在案，不再对不存在的能力写测试。
void main() {
  final source = BookSource(id: 'alias-source', type: 'webdav', name: 'test');

  late RecordRepository repository;
  setUp(() {
    repository = RecordRepository.instance;
    repository.clearAll();
  });
  tearDown(() => repository.clearAll());

  test('远程墓碑会清掉对应记录', () {
    repository.upsert(source: source, path: '/book.zip', title: 'book.zip');

    final stale = repository.purgeStale(
      [source],
      remoteTombstones: {
        source.id: {'/book.zip'},
      },
    );

    expect(stale.map((r) => r.path).toList(), ['/book.zip']);
  });

  test('没有墓碑时记录保留（不误删）', () {
    final record = repository.upsert(
      source: source,
      path: '/book.zip',
      title: 'book.zip',
    );

    final stale = repository.purgeStale([source]);

    expect(stale, isEmpty);
    expect(repository.records[record.key]?.path, '/book.zip');
  });

  test('来源已不在清单中的记录会被清理', () {
    repository.upsert(source: source, path: '/book.zip', title: 'book.zip');

    final stale = repository.purgeStale(const []);

    expect(stale, hasLength(1));
  });

  test('已知缺口：远程别名改写（archive → 新物理路径）不在当前 API 中', () {
    // 旧测试断言：墓碑命中 + 该路径仍"活跃"（live paths）⇒ 记录保留，且 path 更新为
    // 新物理路径。`purgeStale` 现在既没有 live paths 参数，也没有别名改写逻辑；
    // 若将来落地该能力，请在此处补回契约测试。
    expect(repository.records, isEmpty);
  });
}
