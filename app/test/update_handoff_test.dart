import 'package:app/store/update_manager.dart';
import 'package:flutter_test/flutter_test.dart';

/// 更新交接契约（2026-09-21 重写）。
///
/// **为什么重写**：旧版本测试是按当时**计划中的可注入设计**写的
/// （`UpdatePlatformKind`、`UpdateManager.testing(platform:, downloadTransport:)` 等），
/// 而这些 API **从未落地** ⇒ `flutter analyze` 在 CI 上对它有 29 处 error，
/// 是发布门禁的红线。本文件改为**只对着真实存在的 API** 断言，并把"可注入 seam 尚未实现"
/// 作为已知缺口明确记录，避免再次"照着计划设计写测试"。
///
/// 覆盖的真实交接逻辑（纯函数，可直接单测）：
/// 1. 版本解析与比较（决定"是否有新版本"）；
/// 2. 按平台挑选资产（决定下载哪个文件）；
/// 3. 下载候选与镜像前缀拼接（决定从哪里下）。
void main() {
  group('版本比较（是否有新版本）', () {
    test('parseVersion 取 3 段并容忍非数字与 build 号', () {
      expect(UpdateManager.parseVersion('0.6.0+100600'), [0, 6, 0]);
      expect(UpdateManager.parseVersion('1.2'), [1, 2, 0]);
      expect(UpdateManager.parseVersion(' 2.3.4 '), [2, 3, 4]);
      expect(UpdateManager.parseVersion('x.y.z'), [0, 0, 0]);
    });

    test('isNewerVersion 只在远端更高时为真', () {
      expect(UpdateManager.isNewerVersion('0.6.0', '0.5.8'), isTrue);
      expect(UpdateManager.isNewerVersion('0.6.1', '0.6.0'), isTrue);
      expect(UpdateManager.isNewerVersion('1.0.0', '0.9.9'), isTrue);
      expect(UpdateManager.isNewerVersion('0.6.0', '0.6.0'), isFalse);
      expect(UpdateManager.isNewerVersion('0.5.8', '0.6.0'), isFalse);
    });
  });

  group('按平台挑选资产', () {
    final assets = <Map<String, dynamic>>[
      {
        'name': 'app-arm64-v8a-release.apk',
        'size': 10,
        'browser_download_url': 'u-arm64',
      },
      {
        'name': 'app-armeabi-v7a-release.apk',
        'size': 20,
        'browser_download_url': 'u-v7a',
      },
      {
        'name': 'RCH-0.6.0-windows-x64.exe',
        'size': 30,
        'browser_download_url': 'u-exe',
      },
      {'name': 'notes.txt', 'size': 1, 'browser_download_url': 'u-txt'},
      {'name': '', 'size': 0, 'browser_download_url': 'u-empty'},
    ];

    test('Windows 选 RCH-*-windows-x64.exe', () {
      final picked = UpdateManager.pickAssetForPlatform(assets, 'windows');
      expect(picked?.name, 'RCH-0.6.0-windows-x64.exe');
      expect(picked?.url, 'u-exe');
    });

    test('Android 优先 arm64-v8a', () {
      final picked = UpdateManager.pickAssetForPlatform(assets, 'android');
      expect(picked?.name, 'app-arm64-v8a-release.apk');
    });

    test('没有匹配资产时返回 null（不猜、不退回任意文件）', () {
      expect(
        UpdateManager.pickAssetForPlatform(<Map<String, dynamic>>[], 'windows'),
        isNull,
      );
      expect(
        UpdateManager.pickAssetForPlatform(<Map<String, dynamic>>[
          {'name': 'notes.txt', 'size': 1, 'browser_download_url': 'u'},
        ], 'windows'),
        isNull,
      );
    });
  });

  group('下载候选与镜像拼接', () {
    test('downloadCandidates 保留顺序、去重、含选中项', () {
      final urls = UpdateManager.downloadCandidates('official', [
        const MapEntry('m1', 'mirror1'),
        const MapEntry('m2', 'mirror1'),
        const MapEntry('m3', 'mirror2'),
      ]);
      expect(urls, ['official', 'mirror1', 'mirror2']);
    });

    test('buildDownloadUrl 仅在镜像非空时加前缀，并处理尾斜杠', () {
      expect(
        UpdateManager.buildDownloadUrl('https://x/y.exe', ''),
        'https://x/y.exe',
      );
      expect(
        UpdateManager.buildDownloadUrl('https://x/y.exe', '  '),
        'https://x/y.exe',
      );
      expect(
        UpdateManager.buildDownloadUrl('https://x/y.exe', 'https://m'),
        'https://m/https://x/y.exe',
      );
      expect(
        UpdateManager.buildDownloadUrl('https://x/y.exe', 'https://m/'),
        'https://m/https://x/y.exe',
      );
    });
  });

  /// **已知缺口（记录在案）**：旧测试想覆盖的
  /// "并发下载只跑一个传输 / Android 交接失败保留 APK / sha256 校验"，
  /// 需要 `UpdateManager` 提供可注入的平台与传输实现 —— **当前代码没有这个 seam**
  /// （`UpdateManager._()` 私有单例，仅暴露 `instance`）。要覆盖这些契约，
  /// 必须先落地 injectable seam；在那之前不对不存在的能力写测试
  /// （否则 CI 门禁会因为"测了没实现的东西"变红）。
  test('已知缺口：可注入 seam 尚未实现（单例仍为私有构造）', () {
    expect(UpdateManager.instance, isNotNull);
  });
}
