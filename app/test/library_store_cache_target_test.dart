import 'package:app/repository/book_repository.dart';
import 'package:app/repository/record_repository.dart';
import 'package:app/repository/tag_repository.dart';
import 'package:app/src/rust/api/db.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
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

  test(
    'purgeStaleData preserves logical state when a live archive alias remains',
    () async {
      final books = BookRepository.instance;
      final records = RecordRepository.instance;
      final tags = TagRepository.instance;
      books.sources.clear();
      books.metas.clear();
      records.records.clear();

      final source = BookSource(
        id: 'remote-alias-source',
        type: 'webdav',
        name: 'Remote',
        path: '/',
        url: 'https://example.invalid/dav',
      );
      books.sources.add(source);
      final record = records.upsert(
        source: source,
        path: '/book.cbz',
        title: 'Book',
        page: 17,
      );
      record.readCount = 4;
      final meta = books.metaOf(source, '/book.cbz')
        ..coverPage = 3
        ..cropX = 0.1
        ..cropY = 0.2
        ..cropW = 0.7
        ..cropH = 0.6
        ..comment = 'keep history and custom cover';
      tags.setBookTags(record.key, const ['favorite']);
      var physicalPurges = 0;

      final result = await LibraryStore.instance.purgeStaleData(
        alignRemote: false,
        bindings: PurgeStaleDataBindings(
          loadVerifiedTombstones: (_) async => const [
            VerifiedRemoteTombstoneDto(
              logicalPath: '/book.zip',
              dependencyPaths: [],
            ),
          ],
          loadIndex: (_) async => const [
            LibraryIndexDto(
              id: 'live-cbz',
              sourceId: 'remote-alias-source',
              name: 'book.cbz',
              path: '/book.cbz',
              entryType: 'file',
              updatedAt: 1,
              deleted: false,
            ),
            LibraryIndexDto(
              id: 'gone-zip',
              sourceId: 'remote-alias-source',
              name: 'book.zip',
              path: '/book.zip',
              entryType: 'file',
              updatedAt: 1,
              deleted: true,
            ),
          ],
          purgeVerifiedAsset: (sourceId, path, dependencies) async {
            expect(sourceId, source.id);
            expect(path, '/book.zip');
            physicalPurges++;
            return BigInt.from(23);
          },
          reloadCatalog: () async {},
        ),
      );

      expect(physicalPurges, 1);
      expect(result, (0, 0, 23, 0));
      expect(records.records[record.key]?.lastPage, 17);
      expect(records.records[record.key]?.readCount, 4);
      expect(books.metas[record.key], same(meta));
      expect(meta.coverPage, 3);
      expect(meta.hasCrop, isTrue);
      expect(meta.comment, 'keep history and custom cover');
      expect(tags.tagsForBook(record.key), contains('favorite'));

      tags.setBookTags(record.key, const []);
      books.sources.clear();
      books.metas.clear();
      records.records.clear();
    },
  );

  test('remote cleanup log id does not expose the logical path', () {
    final id = remoteAssetLogId('source', '/private/account/book.cbz');
    expect(id, isNot(contains('/private/account/book.cbz')));
    expect(id, matches(RegExp(r'^[0-9a-f]+$')));
  });
}
