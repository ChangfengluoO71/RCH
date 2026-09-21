
import 'package:app/src/rust/api/remote_cover.dart' as rust;
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/store/remote_cover_repository.dart';
import 'package:app/store/remote_scan_coordinator.dart';
import 'package:app/ui/comic_cover.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// P1-E：卡片状态消费契约（state → UI）与"只 request 一次 / 不轮询"。
///
/// 冻结：**只有 `running` 显示 spinner**；`pending` / `retry_wait` / `failed` /
/// `unsupported` / `blocked` 必须渲染各自文案；无 job → 普通 placeholder。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  final source = BookSource(id: 'cloud', type: 'webdav', name: 'Cloud');
  const assetId = 'asset-1';
  const path = '/Series/001.cbz';

  late bool previousRemoteCoverFetchEnabled;

  setUp(() {
    previousRemoteCoverFetchEnabled =
        LibraryStore.instance.settings.remoteCoverFetchEnabled;
    LibraryStore.instance.settings.remoteCoverFetchEnabled = true;
    ComicCover.clear();
    // 注入聚合刷新，避免走真实 FRB（widget 测试无 native 环境）。
    RemoteScanCoordinator.instance.debugCoverAggregateRefresh =
        (String sourceId) async {};
  });

  tearDown(() {
    LibraryStore.instance.settings.remoteCoverFetchEnabled =
        previousRemoteCoverFetchEnabled;
    RemoteScanCoordinator.instance.debugCoverRevisionReader = null;
    RemoteScanCoordinator.instance.debugCoverAggregateRefresh = null;
    ComicCover.clear();
  });

  rust.RemoteCoverStateDto dto(String state, {bool ready = false}) =>
      rust.RemoteCoverStateDto(
        state: state,
        revision: 1,
        ready: ready,
        isPreviousRevision: false,
      );

  ({RemoteCoverRepository repository, List<String> reads, List<String> requests})
  fakeRepository({required String state, bool ready = false}) {
    final reads = <String>[];
    final requests = <String>[];
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
            return null; // 本地未命中 ⇒ 走 requestCover
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
            return dto(state, ready: ready);
          },
      stateLoader:
          ({required sourceId, required assetId, required selection, required profile}) async =>
              dto(state, ready: ready),
      releaseLoader: ({required consumerId}) async {},
    );
    return (repository: repository, reads: reads, requests: requests);
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
              // 统一目录卡片（unified remote job lifecycle）路径
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

  bool hasSpinner() =>
      find.byType(CircularProgressIndicator).evaluate().isNotEmpty;

  // ------------------------------------------------------------ E-STATE-MATRIX
  testWidgets('E-STATE-MATRIX: only running shows a spinner; others render text', (
    tester,
  ) async {
    const expectations = <String, String?>{
      'running': null, // spinner, no text
      'pending': '等待获取',
      'retry_wait': '等待重试',
      'failed': '获取失败',
      'unsupported': '暂不支持',
      'blocked': '暂不可用',
    };

    for (final entry in expectations.entries) {
      // 同类型 widget 会复用 State ⇒ 每轮先卸载，保证是全新卡片。
      await tester.pumpWidget(const SizedBox.shrink());
      ComicCover.clear();
      final fake = fakeRepository(state: entry.key);
      await pumpCard(tester, fake.repository);

      if (entry.key == 'running') {
        expect(hasSpinner(), isTrue, reason: 'running => spinner');
        expect(find.text('未缓存'), findsNothing);
      } else {
        expect(
          hasSpinner(),
          isFalse,
          reason: '${entry.key} must NOT keep spinning',
        );
        expect(
          find.text(entry.value!),
          findsOneWidget,
          reason: '${entry.key} => ${entry.value}',
        );
      }
      expect(fake.requests, [assetId], reason: 'exactly one requestCover');
    }
  });

  // ---------------------------------------------------------- E-REQUEST-ONCE
  testWidgets('E-REQUEST-ONCE: a later revision wake re-reads and never re-requests', (
    tester,
  ) async {
    final fake = fakeRepository(state: 'pending');
    await pumpCard(tester, fake.repository);
    expect(fake.requests.length, 1);
    final readsAfterFirstLoad = fake.reads.length;

    // source-level wake：走 coordinator 的真实 wake 路径（注入 durable reader）
    // ⇒ token 推进 ⇒ 卡片重读（不 request、不轮询）。
    RemoteScanCoordinator.instance.debugCoverRevisionReader =
        (String sourceId) async => 42;
    await RemoteScanCoordinator.instance.catchUpCoverRevision(source.id);
    await tester.runAsync(() async {
      await Future<void>.delayed(const Duration(milliseconds: 50));
    });
    await tester.pump();

    expect(
      fake.requests.length,
      1,
      reason: 'E-REQUEST-ONCE: a revision wake must NEVER call requestCover again',
    );
    expect(
      fake.reads.length,
      greaterThan(readsAfterFirstLoad),
      reason: 'the wake must trigger a durable re-read',
    );
  });

  // --------------------------------------------------------------- E-NO-POLL
  testWidgets('E-NO-POLL: no 350ms/900ms loops — call counts stay flat', (tester) async {
    final fake = fakeRepository(state: 'pending');
    await pumpCard(tester, fake.repository);
    final reads = fake.reads.length;
    final requests = fake.requests.length;

    // 旧实现会在 1.5s 内产生约 4 次 350ms readCover 轮询（以及 900ms 重复 request）。
    await tester.runAsync(() async {
      await Future<void>.delayed(const Duration(milliseconds: 1500));
    });
    await tester.pump();

    expect(fake.reads.length, reads, reason: 'no periodic readCover reintroduction');
    expect(fake.requests.length, requests, reason: 'no repeated requestCover');
    expect(hasSpinner(), isFalse, reason: 'pending must not spin while idle');
  });
}
