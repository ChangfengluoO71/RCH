import 'package:app/repository/record_repository.dart';
import 'package:app/store/models.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'remote archive alias keeps the record and updates its physical path',
    () {
      final repository = RecordRepository.instance;
      repository.clearAll();
      addTearDown(repository.clearAll);

      final source = BookSource(
        id: 'alias-source',
        type: 'webdav',
        name: 'test',
      );
      final record = repository.upsert(
        source: source,
        path: '/book.zip',
        title: 'book.zip',
      );

      final stale = repository.purgeStale(
        [source],
        remoteTombstones: {
          source.id: {'/book.zip'},
        },
        remoteLivePaths: {
          source.id: {'/book.cbz'},
        },
      );

      expect(stale, isEmpty);
      expect(repository.records[record.key]?.path, '/book.cbz');
    },
  );

  test('remote tombstone without a live alias still removes the record', () {
    final repository = RecordRepository.instance;
    repository.clearAll();
    addTearDown(repository.clearAll);

    final source = BookSource(
      id: 'deleted-source',
      type: 'webdav',
      name: 'test',
    );
    final record = repository.upsert(
      source: source,
      path: '/gone.cbz',
      title: 'gone.cbz',
    );

    final stale = repository.purgeStale(
      [source],
      remoteTombstones: {
        source.id: {'/gone.cbz'},
      },
      remoteLivePaths: {source.id: <String>{}},
    );

    expect(stale.map((r) => r.key), contains(record.key));
    expect(repository.records, isEmpty);
  });
}
