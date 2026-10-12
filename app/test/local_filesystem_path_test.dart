import 'package:app/store/models.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('repairs legacy remote-normalized Windows paths for local sources', () {
    expect(
      normalizeLocalFilesystemPath(
        '/F:/comic/book',
        isLocalFs: true,
        isWindows: true,
      ),
      'F:/comic/book',
    );
    expect(
      normalizeLocalFilesystemPath(
        r'/F:\comic\book',
        isLocalFs: true,
        isWindows: true,
      ),
      r'F:\comic\book',
    );
  });

  test('preserves normal local paths and non-local paths', () {
    expect(
      normalizeLocalFilesystemPath(
        r'F:\comic\book',
        isLocalFs: true,
        isWindows: true,
      ),
      r'F:\comic\book',
    );
    expect(
      normalizeLocalFilesystemPath(
        '/F:/comic/book',
        isLocalFs: false,
        isWindows: true,
      ),
      '/F:/comic/book',
    );
    expect(
      normalizeLocalFilesystemPath(
        '/F:/comic/book',
        isLocalFs: true,
        isWindows: false,
      ),
      '/F:/comic/book',
    );
    expect(
      normalizeLocalFilesystemPath(
        '/mnt/comic/book',
        isLocalFs: true,
        isWindows: true,
      ),
      '/mnt/comic/book',
    );
  });
}
