import 'package:app/store/library_store.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'alias tombstones retain metadata but still target old physical cache',
    () {
      final paths = staleCachePathsForTombstones(
        const ['/book.zip', '/book.cbz', '/gone.cbz'],
        {'/book.cbz'},
      );

      expect(paths, ['/book.zip', '/gone.cbz']);
    },
  );

  test('duplicate live path tombstone does not target active cache', () {
    final paths = staleCachePathsForTombstones(
      const ['/book.cbz'],
      {'/book.cbz'},
    );

    expect(paths, isEmpty);
  });
}
