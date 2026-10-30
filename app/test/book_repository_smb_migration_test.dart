import 'package:app/repository/book_repository.dart';
import 'package:app/src/rust/api/db.dart';
import 'package:app/store/credential_vault.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test(
    'loading a legacy SMB UNC row normalizes it to host and share-relative path',
    () async {
      final repository = BookRepository.forTesting(
        credentialVault: MemoryCredentialVault(),
        loadSources: () async => [
          const BookSourceDto(
            id: 'legacy-smb',
            type: 'smb',
            name: 'Old NAS',
            path: r'\\nas\comics\manga\第一卷',
            note: '',
            capabilityLabel: '',
            remoteOnly: false,
          ),
        ],
        loadMetas: () async => const [],
        upsertSource: (_) async {},
      );

      await repository.loadFromSqlite();

      final source = repository.sources.single;
      expect(source.url, 'nas');
      expect(source.port, 445);
      expect(source.path, 'comics/manga/第一卷');
    },
  );
}
