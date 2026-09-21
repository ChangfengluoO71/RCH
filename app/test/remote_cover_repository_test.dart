import 'package:app/src/rust/api/remote_cover.dart' as rust;
import 'package:app/store/models.dart';
import 'package:app/store/remote_cover_repository.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'repository forwards one directory/read/request/release boundary',
    () async {
      final source = BookSource(
        id: 'source',
        type: '115',
        name: '115',
        path: '/',
      );
      var directoryCalls = 0;
      var readCalls = 0;
      var requestCalls = 0;
      var releaseCalls = 0;
      final repository = RemoteCoverRepository(
        directoryLoader:
            ({
              required sourceId,
              required logicalPath,
              required offset,
              required limit,
            }) async {
              directoryCalls++;
              expect(sourceId, source.id);
              expect(logicalPath, '/');
              expect(offset, 0);
              expect(limit, 200);
              return rust.RemoteDirectoryViewDto(
                sourceId: source.id,
                logicalPath: '/',
                revision: PlatformInt64Util.from(1),
                listingComplete: false,
                hasMore: false,
                entries: const [],
              );
            },
        readLoader:
            ({
              required sourceId,
              required assetId,
              required selection,
              required profile,
            }) async {
              readCalls++;
              expect(sourceId, source.id);
              expect(assetId, 'asset');
              return null;
            },
        requestLoader:
            ({
              required sourceId,
              required session,
              required assetId,
              required consumerId,
              required selection,
              required profile,
            }) async {
              requestCalls++;
              expect(session, BigInt.from(7));
              expect(consumerId, 'consumer');
              return rust.RemoteCoverStateDto(
                state: 'pending',
                revision: PlatformInt64Util.from(1),
                ready: false,
                isPreviousRevision: false,
              );
            },
        releaseLoader: ({required consumerId}) async {
          releaseCalls++;
          expect(consumerId, 'consumer');
        },
      );
      final selection = const rust.CoverSelectionDto(
        page: 0,
        revision: 'default',
      );
      final profile = const rust.CoverProfileDto(
        width: 340,
        height: 480,
        decoderVersion: 1,
      );

      await repository.directoryView(source: source, logicalPath: '/');
      await repository.readCover(
        sourceId: source.id,
        assetId: 'asset',
        selection: selection,
        profile: profile,
      );
      await repository.requestCover(
        source: source,
        session: BigInt.from(7),
        assetId: 'asset',
        consumerId: 'consumer',
        selection: selection,
        profile: profile,
      );
      await repository.release(consumerId: 'consumer');

      expect(directoryCalls, 1);
      expect(readCalls, 1);
      expect(requestCalls, 1);
      expect(releaseCalls, 1);
    },
  );

  test('cover states have Chinese labels and limit is capped', () async {
    expect(remoteCoverStateLabel('running'), '封面获取中');
    expect(remoteCoverStateLabel('retry_wait'), '稍后重试');
    expect(remoteCoverStateLabel('unsupported'), '暂不支持局部读取');
    expect(remoteCoverStateLabel('unknown'), '等待处理');

    var requestedLimit = -1;
    final repository = RemoteCoverRepository(
      directoryLoader:
          ({
            required sourceId,
            required logicalPath,
            required offset,
            required limit,
          }) async {
            requestedLimit = limit;
            return rust.RemoteDirectoryViewDto(
              sourceId: sourceId,
              logicalPath: logicalPath,
              revision: PlatformInt64Util.from(0),
              listingComplete: false,
              hasMore: false,
              entries: const [],
            );
          },
    );
    await repository.directoryView(
      source: BookSource(id: 's', type: 'webdav', name: 's', path: '/'),
      logicalPath: '/',
      limit: 999,
    );
    expect(requestedLimit, 200);
  });

  /// 2026-09-21（真机："墙上大片获取失败，可图其实抓得到"）：用户主动重试失败封面的入口
  /// 必须**夹取上限**（防止一次误传把整库重排），并把 sourceId/limit 原样交给 loader。
  test('retryFailed clamps the limit and forwards the source id', () async {
    final calls = <String>[];
    var seenLimit = 0;
    final repository = RemoteCoverRepository(
      retryFailedLoader: ({required sourceId, required limit}) async {
        calls.add(sourceId);
        seenLimit = limit;
        return 3;
      },
    );

    expect(await repository.retryFailed(sourceId: 'quark_1', limit: 9999), 3);
    expect(calls, ['quark_1']);
    expect(seenLimit, 500, reason: '上限必须夹到 500');

    await repository.retryFailed(sourceId: 'quark_1', limit: 0);
    expect(seenLimit, 1, reason: '下限必须夹到 1（0 会被后端当成无操作）');
  });
}
