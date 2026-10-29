import 'package:app/src/rust/api/db.dart';
import 'package:app/store/credential_vault.dart';
import 'package:app/store/models.dart';

/// Read only the 115 records needed by the headless workflow. Loading the UI
/// library would run migrations and full-store reconciliation from another
/// engine, which could overwrite newer foreground edits.
Future<List<BookSource>> loadEhDailyBackgroundSources({
  required Future<List<BookSourceDto>> Function() readSources,
  required CredentialVault vault,
}) async {
  final rows = await readSources();
  final sources = <BookSource>[];
  for (final row in rows.where((row) => row.type == '115')) {
    String? cookie = row.cookie;
    final reference = row.credentialRef;
    if (reference != null && reference.isNotEmpty) {
      try {
        cookie = decodeCredentialBundleStrict(
          await vault.get(reference),
        )['cookie'];
      } catch (_) {
        // A failed vault read cannot be treated as an intentional update or
        // fall back to a stale plaintext value.
        cookie = null;
      }
    }
    sources.add(
      BookSource(
        id: row.id,
        type: row.type,
        name: row.name,
        path: row.path,
        rootId: row.rootId,
        cookie: cookie,
        credentialRef: reference,
        remoteOnly: row.remoteOnly,
      ),
    );
  }
  return sources;
}
