import 'dart:io';

import 'package:app/store/eh_daily_background_bootstrap.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('restores custom root before opening the existing database', () async {
    final temp = await Directory.systemTemp.createTemp('daily-root-');
    addTearDown(() => temp.delete(recursive: true));
    await File(
      '${temp.path}${Platform.pathSeparator}database.db',
    ).writeAsString('existing');
    final calls = <String>[];
    await bootstrapEhDailyStorage(
      readMarker: () async => temp.path,
      readLegacyRoot: () async =>
          throw StateError('marker must take precedence'),
      invalidRoot: (_) => false,
      setRoot: (root) async => calls.add('root:$root'),
      currentRoot: () async {
        calls.add('current');
        return temp.path;
      },
      loadLibrary: () async => calls.add('load'),
    );
    expect(calls, ['root:${temp.path}', 'current', 'load']);
  });

  test(
    'missing selected database cannot silently open an empty default library',
    () async {
      final temp = await Directory.systemTemp.createTemp('daily-missing-');
      addTearDown(() => temp.delete(recursive: true));
      var loaded = false;
      await expectLater(
        bootstrapEhDailyStorage(
          readMarker: () async => temp.path,
          readLegacyRoot: () async => null,
          invalidRoot: (_) => false,
          setRoot: (_) async {},
          currentRoot: () async => temp.path,
          loadLibrary: () async => loaded = true,
        ),
        throwsStateError,
      );
      expect(loaded, isFalse);
    },
  );
}
