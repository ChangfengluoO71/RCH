import 'package:app/src/rust/api/db.dart';
import 'package:app/store/credential_vault.dart';
import 'package:app/store/eh_daily_background_sources.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'headless source loading reads the vault without rewriting the source',
    () async {
      final vault = MemoryCredentialVault();
      await vault.put(
        'source:one',
        encodeCredentialBundle({'cookie': 'UID=42_saved'}),
      );
      final rows = [
        const BookSourceDto(
          id: 'one',
          type: '115',
          name: 'one',
          path: '',
          rootId: '5',
          credentialRef: 'source:one',
          note: '',
          capabilityLabel: '',
          remoteOnly: false,
        ),
      ];
      final sources = await loadEhDailyBackgroundSources(
        readSources: () async => rows,
        vault: vault,
      );
      expect(sources.single.cookie, 'UID=42_saved');
      expect(sources.single.rootId, '5');
      expect(rows.single.cookie, isNull);
      expect(await vault.get('source:one'), contains('UID=42_saved'));
    },
  );

  test(
    'missing vault entry is not replaced by a stale plaintext cookie',
    () async {
      final rows = [
        const BookSourceDto(
          id: 'one',
          type: '115',
          name: 'one',
          path: '',
          cookie: 'stale',
          credentialRef: 'source:one',
          note: '',
          capabilityLabel: '',
          remoteOnly: false,
        ),
      ];
      final sources = await loadEhDailyBackgroundSources(
        readSources: () async => rows,
        vault: MemoryCredentialVault(),
      );
      expect(sources.single.cookie, isNull);
    },
  );
  test('headless source loading preserves remote-only status', () async {
    final sources = await loadEhDailyBackgroundSources(
      readSources: () async => [const BookSourceDto(
        id: 'other', type: '115', name: 'other', path: '', note: '',
        capabilityLabel: '', remoteOnly: true,
      )],
      vault: MemoryCredentialVault(),
    );
    expect(sources.single.remoteOnly, isTrue);
  });
}
