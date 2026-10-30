import 'package:app/repository/book_repository.dart';
import 'package:app/src/rust/api/db.dart';
import 'package:app/store/credential_vault.dart';
import 'package:app/store/models.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test(
    'legacy credentials are verified in the vault before database redaction',
    () async {
      final vault = MemoryCredentialVault();
      final persisted = <BookSourceDto>[];
      final repository = BookRepository.forTesting(
        credentialVault: vault,
        loadSources: () async => [_legacySource()],
        loadMetas: () async => const [],
        upsertSource: (source) async => persisted.add(source),
      );

      await repository.loadFromSqlite();

      final source = repository.sources.single;
      expect(source.password, 'legacy-secret');
      expect(source.credentialRef, 'source:legacy');
      expect(
        decodeCredentialBundleStrict(vault.values['source:legacy']),
        allOf(
          containsPair('password', 'legacy-secret'),
          containsPair('refreshToken', 'legacy-refresh'),
          containsPair('clientSecret', 'legacy-client-secret'),
          containsPair('cookie', 'legacy-cookie'),
        ),
      );
      expect(persisted, hasLength(1));
      expect(persisted.single.password, isNull);
      expect(persisted.single.refreshToken, isNull);
      expect(persisted.single.clientSecret, isNull);
      expect(persisted.single.cookie, isNull);
      expect(persisted.single.credentialRef, 'source:legacy');
    },
  );

  test(
    'vault migration failure keeps the legacy database value untouched',
    () async {
      final persisted = <BookSourceDto>[];
      final repository = BookRepository.forTesting(
        credentialVault: _FailingCredentialVault(),
        loadSources: () async => [_legacySource()],
        loadMetas: () async => const [],
        upsertSource: (source) async => persisted.add(source),
      );

      await repository.loadFromSqlite();

      expect(repository.sources.single.password, 'legacy-secret');
      expect(repository.sources.single.credentialRef, isNull);
      expect(persisted, isEmpty);
    },
  );

  test(
    'unrelated save after a vault read failure preserves the existing bundle',
    () async {
      const credentialKey = 'source:vault-backed';
      final payload = encodeCredentialBundle({'password': 'kept-secret'});
      final vault = _FailFirstReadCredentialVault()
        ..values[credentialKey] = payload;
      final persisted = <BookSourceDto>[];
      final repository = BookRepository.forTesting(
        credentialVault: vault,
        loadSources: () async => [
          const BookSourceDto(
            id: 'vault-backed',
            type: 'sftp',
            name: 'Legacy SFTP',
            path: '/',
            credentialRef: credentialKey,
            note: '',
            capabilityLabel: '',
            remoteOnly: false,
          ),
        ],
        loadMetas: () async => const [],
        upsertSource: (source) async => persisted.add(source),
      );

      await repository.loadFromSqlite();
      final source = repository.sources.single;
      expect(source.password, isNull);
      source.name = 'Renamed while vault unavailable';
      await repository.saveSourceToSqlite(source);

      expect(vault.values[credentialKey], payload);
      expect(vault.putCount, 0);
      expect(persisted.single.credentialRef, credentialKey);

      // The next successful read still hydrates the original secret.
      await repository.loadFromSqlite();
      expect(repository.sources.single.password, 'kept-secret');
    },
  );

  test(
    'saving a source does not fall back to database secrets when vault write fails',
    () async {
      final persisted = <BookSourceDto>[];
      final source = BookSource(
        id: 'legacy',
        type: 'sftp',
        name: 'SFTP',
        password: 'new-secret',
      );
      final repository = BookRepository.forTesting(
        credentialVault: _FailingCredentialVault(),
        loadSources: () async => const [],
        loadMetas: () async => const [],
        upsertSource: (source) async => persisted.add(source),
      );

      await expectLater(
        repository.saveSourceToSqlite(source),
        throwsStateError,
      );
      expect(persisted, isEmpty);
    },
  );

  test(
    'SFTP private key and passphrase are stored only in the credential vault',
    () async {
      final vault = MemoryCredentialVault();
      final persisted = <BookSourceDto>[];
      final source = BookSource(
        id: 'sftp-key',
        type: 'sftp',
        name: 'NAS',
        url: 'nas.local',
        username: 'reader',
        privateKey: 'OPENSSH-PRIVATE-KEY',
        privateKeyPassphrase: 'key-passphrase',
        hostKeyFingerprint: 'SHA256:trusted-pin',
        hostKeyEndpoint: 'nas.local:22',
      );
      final repository = BookRepository.forTesting(
        credentialVault: vault,
        loadSources: () async => const [],
        loadMetas: () async => const [],
        upsertSource: (value) async => persisted.add(value),
      );

      await repository.saveSourceToSqlite(source);

      expect(source.credentialRef, 'source:sftp-key');
      expect(
        decodeCredentialBundleStrict(vault.values['source:sftp-key']),
        containsPair('privateKey', 'OPENSSH-PRIVATE-KEY'),
      );
      expect(
        decodeCredentialBundleStrict(vault.values['source:sftp-key']),
        containsPair('privateKeyPassphrase', 'key-passphrase'),
      );
      expect(persisted.single.hostKeyFingerprint, 'SHA256:trusted-pin');
      expect(persisted.single.hostKeyEndpoint, 'nas.local:22');
      expect(persisted.single.credentialRef, 'source:sftp-key');
      expect(persisted.single.password, isNull);
      expect(
        source.toJson(includeSensitive: false),
        isNot(contains('privateKey')),
      );
      expect(
        source.toJson(includeSensitive: false),
        isNot(contains('privateKeyPassphrase')),
      );
    },
  );

  test('deleting a source removes its vault bundle', () async {
    final vault = MemoryCredentialVault()
      ..values['source:removed'] = '{"password":"secret"}';
    final repository = BookRepository.forTesting(
      credentialVault: vault,
      loadSources: () async => const [],
      loadMetas: () async => const [],
      upsertSource: (_) async {},
    );

    await repository.deleteSourceCredentials(
      BookSource(
        id: 'removed',
        type: 'sftp',
        name: 'NAS',
        credentialRef: 'source:removed',
      ),
    );
    expect(vault.values, isEmpty);
  });

  test('clearing source authentication overwrites old vault secrets', () async {
    final vault = MemoryCredentialVault()
      ..values['source:cleared'] = '{"privateKey":"old-key"}';
    final repository = BookRepository.forTesting(
      credentialVault: vault,
      loadSources: () async => const [],
      loadMetas: () async => const [],
      upsertSource: (_) async {},
    );

    await repository.saveSourceToSqlite(
      BookSource(
        id: 'cleared',
        type: 'sftp',
        name: 'NAS',
        credentialRef: 'source:cleared',
        privateKey: '',
      ),
    );

    expect(
      decodeCredentialBundleStrict(vault.values['source:cleared']),
      isEmpty,
    );
  });

  test(
    'source JSON backup omits secrets when native backup redaction is selected',
    () {
      final source = BookSource(
        id: 'secret',
        type: 'sftp',
        name: 'SFTP',
        username: 'user',
        password: 'password-secret',
        refreshToken: 'refresh-secret',
        clientSecret: 'client-secret',
        cookie: 'cookie-secret',
      );

      expect(includeSensitiveCredentialsInBackup(isWeb: false), isFalse);
      expect(
        source.toJson(includeSensitive: false),
        isNot(contains('password')),
      );
      expect(
        source.toJson(includeSensitive: false),
        isNot(contains('refreshToken')),
      );
      expect(
        source.toJson(includeSensitive: false),
        isNot(contains('clientSecret')),
      );
      expect(source.toJson(includeSensitive: false), isNot(contains('cookie')));
    },
  );
}

BookSourceDto _legacySource() => const BookSourceDto(
  id: 'legacy',
  type: 'sftp',
  name: 'Legacy SFTP',
  path: '/',
  username: 'reader',
  password: 'legacy-secret',
  refreshToken: 'legacy-refresh',
  clientSecret: 'legacy-client-secret',
  cookie: 'legacy-cookie',
  note: '',
  capabilityLabel: '',
  remoteOnly: false,
);

class _FailingCredentialVault implements CredentialVault {
  @override
  Future<void> delete(String key) async {}

  @override
  Future<String?> get(String key) async => null;

  @override
  Future<void> put(String key, String value) async {
    throw StateError('secure storage unavailable');
  }
}

class _FailFirstReadCredentialVault extends MemoryCredentialVault {
  var _failNextRead = true;
  var putCount = 0;

  @override
  Future<String?> get(String key) async {
    if (_failNextRead) {
      _failNextRead = false;
      throw StateError('secure storage temporarily unavailable');
    }
    return super.get(key);
  }

  @override
  Future<void> put(String key, String value) async {
    putCount++;
    await super.put(key, value);
  }
}
