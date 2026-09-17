import 'package:flutter_test/flutter_test.dart';
import 'package:app/store/sync_manager.dart';

void main() {
  test('empty sync credential references normalize to null', () {
    expect(normalizeCredentialRef(null), isNull);
    expect(normalizeCredentialRef(''), isNull);
    expect(normalizeCredentialRef('  \u0000  '), isNull);
  });

  test('non-empty sync credential references are sanitized and trimmed', () {
    expect(normalizeCredentialRef('  sync:webdav-password\u0001 '),
        'sync:webdav-password');
  });
}
