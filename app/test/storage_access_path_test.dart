import 'package:app/store/storage_access.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('isAndroidSharedStoragePath', () {
    test('recognizes common shared-storage roots', () {
      expect(isAndroidSharedStoragePath('/sdcard/comics'), isTrue);
      expect(
        isAndroidSharedStoragePath('/storage/emulated/0/Download'),
        isTrue,
      );
      expect(isAndroidSharedStoragePath('/storage/ABCD-1234/comics'), isTrue);
      expect(
        isAndroidSharedStoragePath('/mnt/media_rw/ABCD-1234/comics'),
        isTrue,
      );
    });

    test(
      'does not classify app-private or non-Android paths as shared storage',
      () {
        expect(
          isAndroidSharedStoragePath('/data/user/0/app/files/books'),
          isFalse,
        );
        expect(isAndroidSharedStoragePath(r'F:\comic'), isFalse);
        expect(
          isAndroidSharedStoragePath('content://provider/tree/123'),
          isFalse,
        );
        expect(isAndroidSharedStoragePath(''), isFalse);
      },
    );
  });
}
