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

/// P1-E：**scan terminal 之后** cover 更新必须完全由
/// `commit → revision wake → durable reread` 驱动，不得依赖 500ms scan monitor。
///
/// 本文件刻意**不推进任何 scan timer**，也不产生任何 scan status。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  final source = BookSource(id: 'cloud-terminal', type: 'webdav', name: 'Cloud');
  const assetId = 'asset-terminal';
  const path = '/Series/002.cbz';

  setUp(() {
    LibraryStore.instance.settings.remoteCoverFetchEnabled = true;
    ComicCover.clear();
    RemoteScanCoordinator.instance.debugCoverAggregateRefresh =
        (String sourceId) async {};
  });

  tearDown(() {
    RemoteScanCoordinator.instance.debugCoverRevisionReader = null;
    RemoteScanCoordinator.instance.debugCoverAggregateRefresh = null;
    ComicCover.clear();
  });

  rust.RemoteCoverStateDto stateDto(String state, {required bool ready}) =>
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
    bool Function() coverAvailable,
    void Function({required bool available, required String state}) apply,
  })
  fake() {
    // 可变 holder：闭包读写同一份状态（之前的空 set 闭包是本文件的 bug）。
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
              rgba: Uint8List.fromList(List<int>.filled(4 * 4 * 4, 180)),
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
            return stateDto(holder.state, ready: false);
          },
      stateLoader:
          ({required sourceId, required assetId, required selection, required profile}) async =>
              stateDto(holder.state, ready: holder.state == 'ready'),
      releaseLoader: ({required consumerId}) async {},
    );
    return (
      repository: repository,
      requests: requests,
      reads: reads,
      coverAvailable: () => holder.available,
      apply: ({required bool available, required String state}) {
        holder.available = available;
        holder.state = state;
      },
    );
  }

  Future<void> pumpCard(
    WidgetTester tester,
    RemoteCoverRepository repository,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Center(
          child: SizedBox(
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
    // RGBA -> ui.Image 的解码需要真实异步时间；给足窗口并多次 pump 以完成重建。
    await tester.runAsync(() async {
      for (var i = 0; i < 8; i++) {
        await Future<void>.delayed(const Duration(milliseconds: 50));
      }
    });
    await tester.pump();
    await tester.pump();
    await tester.pump();
  }

  // ------------------------------------------------- E-SCAN-TERMINAL-READY
  testWidgets(
    'E-SCAN-TERMINAL-READY: running -> ready via commit wake, with the scan monitor stopped',
    (tester) async {
      final f = fake();
      await pumpCard(tester, f.repository);

      // 阶段 1：running ⇒ spinner（且已 request 一次）。
      expect(find.byType(CircularProgressIndicator), findsOneWidget);
      expect(find.byType(RawImage), findsNothing);
      expect(f.requests, [assetId]);
      expect(
        RemoteScanCoordinator.instance.statusFor(source.id).value,
        isNull,
        reason: 'cover correctness must not depend on scan status',
      );

      // 阶段 2：worker commit（running → ready）。只发 wake，不推进任何 timer。
      f.apply(available: true, state: 'ready');
      await wake(tester, 5);

      expect(find.byType(RawImage), findsOneWidget, reason: 'ready => 封面显示');
      expect(find.byType(CircularProgressIndicator), findsNothing);
      expect(
        f.requests,
        [assetId],
        reason: 'the wake must not issue a second requestCover',
      );
    },
  );

  // ------------------------------------------ E-SCAN-TERMINAL-FAILED
  testWidgets(
    'E-SCAN-TERMINAL-FAILED: running -> failed via commit wake removes the spinner',
    (tester) async {
      final f = fake();
      await pumpCard(tester, f.repository);
      expect(find.byType(CircularProgressIndicator), findsOneWidget);

      // worker commit（running → failed）：只发 wake。
      f.apply(available: false, state: 'failed');
      await wake(tester, 9);

      expect(find.byType(CircularProgressIndicator), findsNothing);
      expect(find.text('获取失败'), findsOneWidget);
      expect(find.byType(RawImage), findsNothing);
      expect(f.requests, [assetId], reason: 'still exactly one requestCover');
    },
  );

  // ------------------------------------------------------- E-LIFECYCLE
  testWidgets('E-LIFECYCLE: unmounting the card detaches it from revision wakes', (
    tester,
  ) async {
    final f = fake();
    await pumpCard(tester, f.repository);
    final readsAtUnmount = f.reads.length;

    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump();

    await wake(tester, 13);
    expect(
      f.reads.length,
      readsAtUnmount,
      reason: 'a disposed card must not re-read covers on a wake',
    );
  });
}

class _Holder {
  bool available = false;
  String state = 'running';
}
