import 'package:app/store/models.dart';
import 'package:app/store/remote_cache_cleanup.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('verified deletion forwards only its exact dependency paths', () async {
    final calls = <(String, String, List<String>)>[];
    final source = BookSource(
      id: 'source',
      name: 'Remote',
      type: 'webdav',
      path: '/',
      url: 'https://host/dav',
    );

    final freed = await purgeVerifiedRemoteTombstones(
      source: source,
      tombstones: const [
        VerifiedRemoteTombstone(
          logicalPath: '/Series',
          dependencyPaths: ['/Series/001.jpg', '/Series/poster.jpg'],
        ),
      ],
      cleanup: (sourceId, logicalPath, dependencyPaths) async {
        calls.add((sourceId, logicalPath, dependencyPaths));
        return BigInt.from(12);
      },
    );

    expect(freed, BigInt.from(12));
    expect(calls, hasLength(1));
    expect(calls.single.$1, 'source');
    expect(calls.single.$2, '/Series');
    expect(calls.single.$3, ['/Series/001.jpg', '/Series/poster.jpg']);
  });

  test(
    'failed refresh supplies no tombstones and performs no cleanup',
    () async {
      var calls = 0;
      final source = BookSource(
        id: 'source',
        name: 'Remote',
        type: 'webdav',
        path: '/',
        url: 'https://host/dav',
      );

      final freed = await purgeVerifiedRemoteTombstones(
        source: source,
        tombstones: const [],
        cleanup: (_, _, _) async {
          calls++;
          return BigInt.one;
        },
      );

      expect(freed, BigInt.zero);
      expect(calls, 0);
    },
  );

  test(
    'proof rejection excludes that tombstone from destructive cleanup',
    () async {
      final errors = <String>[];
      final result = await purgeVerifiedRemoteTombstonesSafely(
        sourceId: 'source',
        tombstones: const [
          VerifiedRemoteTombstone(
            logicalPath: '/stale-proof.cbz',
            dependencyPaths: [],
          ),
          VerifiedRemoteTombstone(
            logicalPath: '/verified.cbz',
            dependencyPaths: [],
          ),
        ],
        cleanup: (_, logicalPath, _) async {
          if (logicalPath == '/stale-proof.cbz') {
            throw StateError('generation changed');
          }
          return BigInt.from(7);
        },
        onError: (tombstone, _) => errors.add(tombstone.logicalPath),
      );

      expect(result.freedBytes, BigInt.from(7));
      expect(result.verifiedTombstones.map((row) => row.logicalPath), [
        '/verified.cbz',
      ]);
      expect(errors, ['/stale-proof.cbz']);
    },
  );
}
