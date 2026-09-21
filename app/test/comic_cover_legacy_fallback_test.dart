import 'dart:typed_data';

import 'package:app/src/rust/api/book.dart';
import 'package:app/src/rust/api/remote_cover.dart' as rust;
import 'package:app/src/rust/api/source.dart' show LegacyCoverLocalLookupDto;
import 'package:app/store/models.dart';
import 'package:app/store/remote_cover_repository.dart';
import 'package:app/ui/comic_cover.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// 2026-09-21（真机反馈："详情页已经有封面了，海报墙却显示获取失败"）。
///
/// 根因：**unified 缓存与 legacy 缓存是两套互不相通的东西**。
/// - 详情页的 `ComicCover` **不传** `remoteAssetId` ⇒ 走 legacy 路径
///   （Rust `read_legacy_cover_local`，键 = authority + 逻辑路径，纯本地读）⇒ 能出图；
/// - 墙上卡片传了 `remoteAssetId` ⇒ 走 unified 路径，读不到 unified 字节就渲染"获取失败"。
/// ⇒ 契约：unified 拿不到字节时，抛"获取失败"之前必须补一次 **legacy 纯本地回退**
/// （零网络：不建 session、不联网、不建 job、不发事件）。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  final source = BookSource(
    id: 'quark_1',
    type: 'quark',
    name: 'Quark',
    path: '0',
    rootId: '0',
  );
  const path = '/日漫/金牌得主/2.mobi';
  const assetId = 'asset-mobi-2';

  setUp(() => ComicCover.clear());
  tearDown(() => ComicCover.clear());

  PageImage onePixel() => PageImage(
    rgba: Uint8List.fromList(const [0, 0, 0, 255]),
    width: 1,
    height: 1,
  );

  rust.RemoteCoverStateDto failedState() => const rust.RemoteCoverStateDto(
    state: 'failed',
    revision: 1,
    ready: false,
    isPreviousRevision: false,
  );

  testWidgets('unified 状态失败时，legacy 本地缓存仍应出图（而不是"获取失败"）', (tester) async {
    var legacyLookups = 0;
    final repository = RemoteCoverRepository(
      directoryLoader:
          ({
            required sourceId,
            required logicalPath,
            required offset,
            required limit,
          }) async => throw StateError('no directory read expected'),
      // unified 的三个入口都读不到字节（模拟"unified 从来没抓好"）
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
          }) async => failedState(),
      stateLoader:
          ({
            required sourceId,
            required assetId,
            required selection,
            required profile,
          }) async => failedState(),
      releaseLoader: ({required consumerId}) async {},
    );

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
              // 注入一个"已有 session"，避免测试里去碰原生桥
              remoteSession: BigInt.from(1),
              repository: repository,
              legacyLocalCoverReader: (LegacyCoverLocalLookupDto lookup) async {
                legacyLookups++;
                return onePixel();
              },
            ),
          ),
        ),
      ),
    );
    await tester.runAsync(() async {
      for (var i = 0; i < 8; i++) {
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
    });
    await tester.pump();

    expect(
      legacyLookups,
      greaterThan(0),
      reason: 'unified 失败后必须尝试 legacy 纯本地回退',
    );
    expect(
      find.byType(RawImage),
      findsOneWidget,
      reason: 'legacy 本地有图 ⇒ 墙上必须显示封面，而不是"获取失败"',
    );
    expect(find.text('获取失败'), findsNothing);
  });

  testWidgets('legacy 也没有字节时保持原有失败语义（不回退成占位文案之外的怪状态）', (tester) async {
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
          }) async => null,
      requestLoader:
          ({
            required sourceId,
            required session,
            required assetId,
            required consumerId,
            required selection,
            required profile,
          }) async => failedState(),
      stateLoader:
          ({
            required sourceId,
            required assetId,
            required selection,
            required profile,
          }) async => failedState(),
      releaseLoader: ({required consumerId}) async {},
    );

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
              remoteSession: BigInt.from(1),
              repository: repository,
              legacyLocalCoverReader: (LegacyCoverLocalLookupDto lookup) async =>
                  null,
            ),
          ),
        ),
      ),
    );
    await tester.runAsync(() async {
      for (var i = 0; i < 8; i++) {
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
    });
    await tester.pump();

    expect(find.byType(RawImage), findsNothing);
    expect(find.text('获取失败'), findsOneWidget);
  });
}
