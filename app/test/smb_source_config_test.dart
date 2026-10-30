import 'package:app/store/models.dart';
import 'package:app/store/smb_source_config.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'legacy UNC parsing preserves server, share and nested Unicode paths',
    () {
      final config = SmbSourceConfig.parseLegacyUnc(
        r'\\NAS Box\漫画共享\One Piece\第一卷',
      );

      expect(config.server, 'NAS Box');
      expect(config.port, 445);
      expect(config.share, '漫画共享');
      expect(config.rootPath, 'One Piece/第一卷');
      expect(config.canonicalPath, '漫画共享/One Piece/第一卷');
    },
  );

  test('canonical SMB paths normalize slash styles without trimming names', () {
    final config = SmbSourceConfig.fromSource(
      BookSource(
        id: 'smb',
        type: 'smb',
        name: 'NAS',
        url: 'nas.local',
        path: r'//comics/// One Piece\第一卷/',
      ),
    );

    expect(config.server, 'nas.local');
    expect(config.port, 445);
    expect(config.share, 'comics');
    expect(config.rootPath, ' One Piece/第一卷');
  });

  test(
    'SMB is a remote session source and its browser root omits the share',
    () {
      final source = BookSource(
        id: 'smb',
        type: 'smb',
        name: 'NAS',
        url: 'nas.local',
        port: 445,
        path: 'comic/漫画',
      );

      expect(source.isLocalFs, isFalse);
      expect(source.needsSession, isTrue);
      expect(source.effectiveRootPath, 'comic/漫画');
    },
  );

  test('legacy UNC requires both a server and a share', () {
    expect(
      () => SmbSourceConfig.parseLegacyUnc(r'\\nas'),
      throwsFormatException,
    );
    expect(
      () => SmbSourceConfig.parseLegacyUnc(r'\\\share\folder'),
      throwsFormatException,
    );
  });

  test('old BookSource JSON round-trips through legacy UNC parsing', () {
    final restored = BookSource.fromJson({
      'id': 'old-smb',
      'type': 'smb',
      'name': 'Old NAS',
      'path': r'\\nas\comics\manga',
    });
    final config = SmbSourceConfig.fromSource(restored);

    expect(config.server, 'nas');
    expect(config.share, 'comics');
    expect(config.rootPath, 'manga');
  });
}
