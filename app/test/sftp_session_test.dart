import 'package:app/store/sftp_session.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('an exact nonempty host-key pin is accepted', () {
    expect(
      requireTrustedSftpHostKey(
        host: 'nas.local',
        port: 22,
        expectedFingerprint: 'SHA256:abc123',
        actualFingerprint: 'SHA256:abc123',
        expectedEndpoint: 'nas.local:22',
        actualEndpoint: 'nas.local:22',
      ),
      'SHA256:abc123',
    );
  });

  test('an unknown or changed host key requires explicit trust', () {
    for (final (expected, actual, expectedEndpoint, actualEndpoint)
        in <(String, String, String, String)>[
          ('', 'SHA256:abc123', '', 'nas.local:2222'),
          (
            'SHA256:abc123',
            'SHA256:changed',
            'nas.local:2222',
            'nas.local:2222',
          ),
          ('SHA256:abc123', '', 'nas.local:2222', 'nas.local:2222'),
          ('SHA256:abc123', 'SHA256:abc123', 'nas.local:22', 'nas.local:2222'),
        ]) {
      expect(
        () => requireTrustedSftpHostKey(
          host: 'nas.local',
          port: 2222,
          expectedFingerprint: expected,
          actualFingerprint: actual,
          expectedEndpoint: expectedEndpoint,
          actualEndpoint: actualEndpoint,
        ),
        throwsA(
          isA<SftpHostKeyTrustRequired>()
              .having((e) => e.host, 'host', 'nas.local')
              .having((e) => e.port, 'port', 2222),
        ),
      );
    }
  });
}
