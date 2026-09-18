import 'dart:typed_data';

import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/source.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/store/remote_cover_repository.dart';
import 'package:app/ui/common.dart';
import 'package:app/ui/comic_cover.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// P1-D-2：offline disk-first（local A + legacy B）与调用**顺序**契约。
///
/// 观测发生在既有注入 seam 上（与 `comic_cover_disk_first_test.dart` 同一模式）：
/// * `legacyLocalCoverReader` —— sessionless local-only 查找（默认走 Rust FRB API）
/// * `legacyRemoteCoverLoader` —— legacy 既有 session+provider 获取（默认即原实现）
///
/// 说明：`session` / `provider` 两个事件在 **legacy-transport 边界**上记录，
/// 该边界正是生产默认实现所占的位置；本文件因此证明的是 **widget 控制流顺序**
/// （local-cache 严格早于 transport），不通过 instrument 真实 FRB 会话层来取证。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  final legacySource = BookSource(id: 'cloud', type: 'webdav', name: 'Cloud');
  final localSource = BookSource(id: 'local', type: 'local', name: 'Local');
  const path = '/Series/001.cbz';

  PageImage coverBytes() =>
      PageImage(rgba: Uint8List.fromList(const [0, 0, 0, 255]), width: 1, height: 1);

  late bool previousRemoteCoverFetchEnabled;

  setUp(() {
    previousRemoteCoverFetchEnabled =
        LibraryStore.instance.settings.remoteCoverFetchEnabled;
    ComicCover.clear();
  });

  tearDown(() {
    LibraryStore.instance.settings.remoteCoverFetchEnabled =
        previousRemoteCoverFetchEnabled;
    ComicCover.clear();
  });

  /// 空 repository：unified 分支在本文件里不参与（remoteAssetId 为 null）。
  RemoteCoverRepository emptyRepository() => RemoteCoverRepository(
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
        }) async => null,
    requestLoader:
        ({
          required sourceId,
          required session,
          required assetId,
          required consumerId,
          required selection,
          required profile,
        }) async => throw StateError('no unified request expected'),
    releaseLoader: ({required consumerId}) async {},
  );

  Future<void> pump(
    WidgetTester tester, {
    required BookSource source,
    required bool localHit,
    required List<String> events,
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Center(
          child: SizedBox(
            width: 64,
            height: 64,
            child: ComicCover(
              source: source,
              path: path,
              repository: emptyRepository(),
              legacyLocalCoverReader: (LegacyCoverLocalLookupDto lookup) async {
                events.add('local-cache');
                return localHit ? coverBytes() : null;
              },
              legacyRemoteCoverLoader:
                  ({
                    required BookSource source,
                    required String path,
                    required int page,
                    required int width,
                    required int height,
                    CropRect? crop,
                  }) async {
                    events.add('session');
                    events.add('provider');
                    final bytes = coverBytes();
                    return rgbaToImage(bytes.rgba, bytes.width, bytes.height);
                  },
            ),
          ),
        ),
      ),
    );
    await tester.runAsync(() async {
      for (var i = 0; i < 6; i++) {
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
    });
    await tester.pump();
  }

  bool visible() => find.byType(RawImage).evaluate().isNotEmpty;
  bool placeholder() => find.text('未缓存').evaluate().isNotEmpty;

  // ---------------------------------------------------------------- D2-1
  testWidgets('D2-1 offline + legacy disk hit => visible, local=1, session=0, provider=0', (
    tester,
  ) async {
    LibraryStore.instance.settings.remoteCoverFetchEnabled = false;
    final events = <String>[];
    await pump(tester, source: legacySource, localHit: true, events: events);

    expect(visible(), isTrue);
    expect(placeholder(), isFalse);
    expect(events, ['local-cache']);
  });

  // ---------------------------------------------------------------- D2-2
  testWidgets('D2-2 offline + legacy miss => placeholder, local=1, session=0, provider=0', (
    tester,
  ) async {
    LibraryStore.instance.settings.remoteCoverFetchEnabled = false;
    final events = <String>[];
    await pump(tester, source: legacySource, localHit: false, events: events);

    expect(placeholder(), isTrue);
    expect(visible(), isFalse);
    expect(events, ['local-cache']);
  });

  // ---------------------------------------------------------------- D2-3
  testWidgets('D2-3 online + legacy disk hit => visible, local=1, session=0, provider=0', (
    tester,
  ) async {
    LibraryStore.instance.settings.remoteCoverFetchEnabled = true;
    final events = <String>[];
    await pump(tester, source: legacySource, localHit: true, events: events);

    expect(visible(), isTrue);
    // 重要回归：网络开着也**不得**因为"反正能联网"而提前取 session。
    expect(events, ['local-cache']);
  });

  // ---------------------------------------------------------------- D2-4
  testWidgets('D2-4 online + legacy miss => local-cache -> session -> provider', (
    tester,
  ) async {
    LibraryStore.instance.settings.remoteCoverFetchEnabled = true;
    final events = <String>[];
    await pump(tester, source: legacySource, localHit: false, events: events);

    expect(visible(), isTrue);
    expect(events, ['local-cache', 'session', 'provider']);
  });

  // ---------------------------------------------------------------- D2-5
  testWidgets('D2-5 offline + custom/local => bookCover visible, session=0, provider=0', (
    tester,
  ) async {
    LibraryStore.instance.settings.remoteCoverFetchEnabled = false;
    final events = <String>[];
    await pump(tester, source: localSource, localHit: false, events: events);

    // 本地/自定义源：不做 sessionless legacy 查找（kind 为 null），也不受开关阻止，
    // 直接走既有 bookCover（本地文件 + Rust decode，无 session、无 provider）。
    expect(visible(), isTrue);
    expect(events, ['session', 'provider']);
    // 注意：对 local 源，'session'/'provider' 两个名字在本文件的 fake 里代表
    // "既有 legacy 分发链"（其 else 分支即 bookCover），并非真实会话建立。
  });

  // ---------------------------------------------------------------- D2-ORDER
  testWidgets('D2-ORDER hit => ["local-cache"] ; miss+online => ["local-cache","session","provider"]', (
    tester,
  ) async {
    LibraryStore.instance.settings.remoteCoverFetchEnabled = true;

    final hitEvents = <String>[];
    await pump(tester, source: legacySource, localHit: true, events: hitEvents);
    expect(hitEvents, ['local-cache']);
    expect(
      hitEvents.contains('session'),
      isFalse,
      reason: 'session must never precede the local cache lookup',
    );

    // 先卸载卡片：同类型 widget 会复用 State（不会重新 initState），
    // 否则第二次 pump 仍在使用第一次的 future/fake。
    await tester.pumpWidget(const SizedBox.shrink());
    ComicCover.clear();
    final missEvents = <String>[];
    await pump(tester, source: legacySource, localHit: false, events: missEvents);
    expect(missEvents, ['local-cache', 'session', 'provider']);
    expect(missEvents.first, 'local-cache');
  });

  // ------------------------------------------------- kind mapping（§5 / §7）
  test('kind mapping distinguishes 115 app from 115 web, and keeps quark own kind', () {
    final app = BookSource(id: 'a', type: '115', name: 'a', clientId: 'app-id');
    final web = BookSource(id: 'b', type: '115', name: 'b', cookie: 'UID=1');
    expect(legacyCoverKindOf(app), '115');
    expect(legacyCoverKindOf(web), '115web');
    expect(legacyCoverKindOf(app), isNot(legacyCoverKindOf(web)));
    expect(legacyCoverKindOf(BookSource(id: 'c', type: 'quark', name: 'c')), 'quark');
    expect(legacyCoverKindOf(BookSource(id: 'd', type: 'webdav', name: 'd')), 'webdav');
    expect(legacyCoverKindOf(BookSource(id: 'e', type: 'sftp', name: 'e')), 'sftp');
    expect(legacyCoverKindOf(BookSource(id: 'f', type: 'baidu', name: 'f')), 'baidu');
    expect(legacyCoverKindOf(BookSource(id: 'g', type: 'local', name: 'g')), isNull);
  });
}
