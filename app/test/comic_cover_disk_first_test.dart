import 'dart:typed_data';

import 'package:app/src/rust/api/book.dart';
import 'package:app/store/library_store.dart';
import 'package:app/store/models.dart';
import 'package:app/store/remote_cover_repository.dart';
import 'package:app/ui/comic_cover.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// P1-D disk-first contract: a cover that already exists locally is displayed
/// even while remote cover fetching (联网开关) is disabled, and displaying it
/// never starts a remote fetch. Only a local miss may consult the network
/// switch, and a miss still degrades to the placeholder instead of throwing.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  final source = BookSource(id: 'cloud', type: 'webdav', name: 'Cloud');
  const assetId = 'asset-1';
  const path = '/Series/001.cbz';

  /// Bytes for one opaque pixel: the already-materialized local cover the
  /// disk reader hands back. Decoded by the widget, never by the fakes.
  PageImage localCoverBytes() => PageImage(
    rgba: Uint8List.fromList(const [0, 0, 0, 255]),
    width: 1,
    height: 1,
  );

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

  /// Fake repository with no provider session and no global state. Every
  /// loader that would reach the network records the call and throws, so a
  /// remote fetch shows up as a recorded call instead of staying invisible.
  ///
  /// [remoteReads] stays empty unless the network path is actually entered:
  /// it is the local index read that the remote branch performs after a local
  /// miss, and it is only reached once the network gate has been passed.
  ({
    RemoteCoverRepository repository,
    List<String> localReads,
    List<String> sessionRequests,
  })
  fakeRepository({required bool localCoverExists}) {
    final localReads = <String>[];
    final sessionRequests = <String>[];
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
          }) async {
            localReads.add(assetId);
            return localCoverExists ? localCoverBytes() : null;
          },
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
          }) async {
            sessionRequests.add(assetId);
            throw StateError('no session request expected');
          },
      releaseLoader: ({required consumerId}) async {},
    );
    return (
      repository: repository,
      localReads: localReads,
      sessionRequests: sessionRequests,
    );
  }

  /// Mounts the card and lets its disk-first decision finish.
  ///
  /// The pumps run in [WidgetTester.runAsync] because the card decodes the
  /// cover through decodeImageFromPixels, whose callback needs real async.
  /// Bounded pumps rather than pumpAndSettle: a card that never resolves keeps
  /// a spinner turning, which would make pumpAndSettle time out instead of
  /// letting us assert the placeholder.
  Future<void> pumpCover(
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
              repository: repository,
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

  testWidgets(
    'network disabled + local disk cover exists => cover visible, no remote fetch',
    (tester) async {
      LibraryStore.instance.settings.remoteCoverFetchEnabled = false;
      final fake = fakeRepository(localCoverExists: true);

      await pumpCover(tester, fake.repository);

      expect(find.byType(RawImage), findsOneWidget);
      expect(find.byType(CircularProgressIndicator), findsNothing);
      expect(find.text('等待扫描'), findsNothing);
      expect(fake.localReads, [assetId]);
      expect(fake.sessionRequests, isEmpty);
    },
  );

  testWidgets(
    'network disabled + no local cover => placeholder, no session, no remote fetch',
    (tester) async {
      LibraryStore.instance.settings.remoteCoverFetchEnabled = false;
      final fake = fakeRepository(localCoverExists: false);

      await pumpCover(tester, fake.repository);

      // The local question is asked before the switch is consulted...
      expect(fake.localReads, [assetId]);
      // ...the card degrades to the placeholder instead of throwing...
      expect(find.byType(RawImage), findsNothing);
      expect(find.text('等待扫描'), findsOneWidget);
      // ...and the disabled switch means no session and no remote work.
      expect(fake.sessionRequests, isEmpty);
    },
  );
}
