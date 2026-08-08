import 'package:app/store/update_manager.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('版本比较', () {
    test('新版本大于旧版本', () {
      expect(UpdateManager.isNewerVersion('0.4.0', '0.3.5'), isTrue);
      expect(UpdateManager.isNewerVersion('0.4.1', '0.4.0'), isTrue);
      expect(UpdateManager.isNewerVersion('0.10.0', '0.9.9'), isTrue);
      expect(UpdateManager.isNewerVersion('1.0.0', '0.99.99'), isTrue);
    });

    test('相同或更低版本不算新版本', () {
      expect(UpdateManager.isNewerVersion('0.4.0', '0.4.0'), isFalse);
      expect(UpdateManager.isNewerVersion('0.4.0', '0.4.0+400'), isFalse);
      expect(UpdateManager.isNewerVersion('0.3.5', '0.4.0'), isFalse);
      expect(UpdateManager.isNewerVersion('0.4.0', '0.4.1'), isFalse);
    });

    test('解析带构建号/缺段版本', () {
      expect(UpdateManager.parseVersion('0.4.0+400'), [0, 4, 0]);
      expect(UpdateManager.parseVersion('0.4'), [0, 4, 0]);
      expect(UpdateManager.parseVersion('1'), [1, 0, 0]);
      expect(UpdateManager.parseVersion('v0.4.0'), [0, 4, 0]);
    });
  });

  group('平台安装包挑选', () {
    final assets = [
      {
        'name': 'app-arm64-v8a-release.apk',
        'size': 40775291,
        'browser_download_url': 'https://example.com/app-arm64-v8a-release.apk',
      },
      {
        'name': 'app-x86_64-release.apk',
        'size': 43505769,
        'browser_download_url': 'https://example.com/app-x86_64-release.apk',
      },
      {
        'name': 'RCH-0.4.0-windows-x64.exe',
        'size': 20444988,
        'browser_download_url': 'https://example.com/RCH-0.4.0-windows-x64.exe',
      },
      {
        'name': 'source.zip',
        'size': 100,
        'browser_download_url': 'https://example.com/source.zip',
      },
    ];

    test('Windows 挑选 RCH-*-windows-x64.exe', () {
      final a = UpdateManager.pickAssetForPlatform(assets, 'windows');
      expect(a?.name, 'RCH-0.4.0-windows-x64.exe');
      expect(a?.size, 20444988);
    });

    test('Android 优先 arm64-v8a', () {
      final a = UpdateManager.pickAssetForPlatform(assets, 'android');
      expect(a?.name, 'app-arm64-v8a-release.apk');
    });

    test('Android 无 arm64 时回退到任意 release APK', () {
      final noArm64 = assets
          .where((a) => a['name'] != 'app-arm64-v8a-release.apk')
          .toList();
      final a = UpdateManager.pickAssetForPlatform(noArm64, 'android');
      expect(a?.name, 'app-x86_64-release.apk');
    });

    test('不支持的平台或无匹配资产返回 null', () {
      expect(UpdateManager.pickAssetForPlatform(assets, 'macos'), isNull);
      expect(
        UpdateManager.pickAssetForPlatform(
            [{'name': 'foo.txt', 'size': 1, 'browser_download_url': 'https://x'}],
            'windows'),
        isNull,
      );
    });
  });
}
