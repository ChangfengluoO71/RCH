import 'dart:typed_data';

import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/remote_cover.dart' as rust;
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/store/remote_cover_repository.dart';
import 'package:app/store/remote_scan_coordinator.dart';
import 'package:app/ui/comic_cover.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// P1-E/F 最终架构验收：**scan terminal 之后**，封面与进度聚合必须由
/// **同一个** cover revision wake 驱动，且全程不推进任何 scan timer。
///
/// 链路（不 mock 掉 coordinator 的 wake 处理）：
///   production durable mutation → post-commit notify
///     → coordinator 收到 event → 读 durable revision →（去重后）
///         ├─ 卡片重读 durable state（E 侧）
///         └─ 本地聚合刷新（F 侧）
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  final source = BookSource(id: 'cloud-fst', type: 'webdav', name: 'FSTerminal');
  const assetId = 'asset-fst';
  const path = '/Series/003.cbz';

  setUp(() {
    LibraryStore.instance.settings.remoteCoverFetchEnabled = true;
    ComicCover.clear();
  });

  tearDown(() {
    RemoteScanCoordinator.instance.debugCoverRevisionReader = null;
    RemoteScanCoordinator.instance.debugCoverAggregateRefresh = null;
    ComicCover.clear();
  });

  rust.RemoteCoverStateDto dto(String state, {required bool ready}) =>
      rust.RemoteCoverStateDto(
        state: state,
        revision: 1,
        ready: ready,
        isPreviousRevision: false,
      );

  ({
    RemoteCoverRepository repository,
    List<String> requests,
    List<String> reads,
    void Function({required bool available, required String state}) apply,
  })
  fake() {
    final holder = _Holder();
    final requests = <String>[];
    final reads = <String>[];
    final repository = RemoteCoverRepository(
      directoryLoader:
          ({
            required sourceId,
            required logicalPath,
            required offset,
            required limit,
          }) async => throw StateError('no directory read expected'),
      localReadLoader:
          ({
            required sourceId,
            required assetId,
            required selection,
            required profile,
          }) async => null,
      readLoader:
          ({
            required sourceId,
            required assetId,
            required selection,
            required profile,
          }) async {
            reads.add(assetId);
            if (!holder.available) return null;
            return PageImage(
              rgba: Uint8List.fromList(List<int>.filled(4 * 4 * 4, 120)),
              width: 4,
              height: 4,
            );
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
            requests.add(assetId);
            return dto(holder.state, ready: false);
          },
      stateLoader:
          ({required sourceId, required assetId, required selection, required profile}) async =>
              dto(holder.state, ready: holder.state == 'ready'),
      releaseLoader: ({required consumerId}) async {},
    );
    return (
      repository: repository,
      requests: requests,
      reads: reads,
      apply: ({required bool available, required String state}) {
        holder.available = available;
        holder.state = state;
      },
    );
  }

  Future<void> pumpCards(
    WidgetTester tester,
    List<RemoteCoverRepository> repositories,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Center(
          child: Row(
            children: [
              for (final repository in repositories)
                SizedBox(
                  width: 64,
                  height: 64,
                  child: ComicCover(
                    source: source,
                    path: path,
                    remoteAssetId: assetId,
                    remoteSession: BigInt.one,
                    preferUnifiedRemote: true,
                    repository: repository,
                  ),
                ),
            ],
          ),
        ),
      ),
    );
    await tester.runAsync(() async {
      for (var i = 0; i < 8; i++) {
        await Future<void>.delayed(const Duration(milliseconds: 25));
      }
    });
    await tester.pump();
  }

  Future<void> wake(WidgetTester tester, int revision) async {
    RemoteScanCoordinator.instance.debugCoverRevisionReader =
        (String sourceId) async => revision;
    await RemoteScanCoordinator.instance.catchUpCoverRevision(source.id);
    // RGBA → ui.Image 的解码需要真实异步时间。
    await tester.runAsync(() async {
      for (var i = 0; i < 8; i++) {
        await Future<void>.delayed(const Duration(milliseconds: 50));
      }
    });
    await tester.pump();
    await tester.pump();
    await tester.pump();
  }

  // --------------------------------------------------- F-SCAN-TERMINAL
  testWidgets(
    'F-SCAN-TERMINAL: one cover wake drives the card AND the local aggregate, with no scan timer',
    (tester) async {
      var aggregateRefreshes = 0;
      final refreshSources = <String>[];
      RemoteScanCoordinator.instance.debugCoverAggregateRefresh = (
        String sourceId,
      ) async {
        aggregateRefreshes++;
        refreshSources.add(sourceId);
      };

      final f = fake();
      await pumpCards(tester, [f.repository]);

      // 阶段 1：running ⇒ spinner；此时还没有任何聚合刷新。
      expect(find.byType(CircularProgressIndicator), findsOneWidget);
      expect(f.requests, [assetId], reason: 'exactly one requestCover on first miss');
      expect(aggregateRefreshes, 0);
      // scan monitor 从未启动（没有 scan status）⇒ cover 正确性不依赖它。
      expect(RemoteScanCoordinator.instance.statusFor(source.id).value, isNull);

      // 阶段 2：worker commit（running → ready）后只发一次 wake。
      f.apply(available: true, state: 'ready');
      await wake(tester, 7);

      // E 侧：封面显示，spinner 消失，且没有第二次 requestCover。
      expect(find.byType(RawImage), findsOneWidget, reason: 'ready => 封面显示');
      expect(find.byType(CircularProgressIndicator), findsNothing);
      expect(f.requests, [assetId]);

      // F 侧：**同一次** wake 也刷新了本地聚合（source scope）。
      expect(aggregateRefreshes, 1, reason: 'the same wake drives the F aggregate');
      expect(refreshSources, [source.id]);

      // 不推进任何 timer：静置一段时间后不得出现新的重读或聚合刷新。
      final readsAfter = f.reads.length;
      await tester.runAsync(() async {
        await Future<void>.delayed(const Duration(milliseconds: 1200));
      });
      await tester.pump();
      expect(f.reads.length, readsAfter, reason: 'no periodic reread');
      expect(aggregateRefreshes, 1, reason: 'no periodic aggregate refresh');
      expect(find.byType(RawImage), findsOneWidget);
    },
  );

  // ------------------------------------------------------- E-SAME-ASSET
  testWidgets(
    'E-SAME-ASSET: two cards on the same source+asset stay consistent and do not double-enqueue',
    (tester) async {
      RemoteScanCoordinator.instance.debugCoverAggregateRefresh =
          (String sourceId) async {};

      final a = fake();
      final b = fake();
      await pumpCards(tester, [a.repository, b.repository]);
      expect(find.byType(CircularProgressIndicator), findsNWidgets(2));
      // 同一 source+asset 的两张卡片**共享同一个 in-flight 加载**，
      // 因此两者合计只发出**一次** requestCover（enqueue/provider 不翻倍）。
      expect(
        a.requests.length + b.requests.length,
        1,
        reason: 'same asset must NOT be enqueued twice for two mounted cards',
      );

      // 一个 transition ⇒ 一次 source-level wake ⇒ 两张卡片同时收敛到同一状态。
      a.apply(available: true, state: 'ready');
      b.apply(available: true, state: 'ready');
      await wake(tester, 11);

      expect(find.byType(RawImage), findsNWidgets(2), reason: 'both cards converge');
      expect(find.byType(CircularProgressIndicator), findsNothing);
      expect(
        a.requests.length + b.requests.length,
        1,
        reason: 'a revision wake must not issue any further requestCover',
      );
    },
  );
}

class _Holder {
  bool available = false;
  String state = 'running';
}
