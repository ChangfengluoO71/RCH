import 'package:app/store/models.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('AppSettings.fromJson 兼容 keys 的 Map 形态', () {
    final s = AppSettings.fromJson(const {
      'keys': {
        'forward': 123,
        'back': 456,
        'zoomIn': 789,
        'zoomOut': 101112,
        'zoomReset': 0,
      },
    });
    expect(s.keys.forward, 123);
    expect(s.keys.back, 456);
  });

  test('AppSettings.fromJson 兼容 keys 的 JSON 字符串形态', () {
    final s = AppSettings.fromJson({
      'keys':
          '{"forward":123,"back":456,"zoomIn":789,"zoomOut":101112,"zoomReset":0}',
    });
    expect(s.keys.forward, 123);
  });

  test('AppSettings.fromJson 兼容历史坏数据（Dart toString，回落默认）', () {
    final s = AppSettings.fromJson({
      'keys':
          '{forward: 8445061, back: 8445068, zoomIn: 8445063, zoomOut: 8445067, zoomReset: 8445064}',
    });
    expect(s.keys.forward, LogicalKeyboardKey.arrowRight.keyId);
    expect(s.keys.back, LogicalKeyboardKey.arrowLeft.keyId);
  });

  test('AppSettings.fromJson 兼容 keys 缺失', () {
    final s = AppSettings.fromJson(const {});
    expect(s.keys.forward, LogicalKeyboardKey.arrowRight.keyId);
  });
  test(
    'remote scan settings default on and explicit false survives round trip',
    () {
      final defaults = AppSettings.fromJson(const {});
      expect(defaults.remoteBackgroundScanEnabled, isTrue);
      expect(defaults.remoteCoverFetchEnabled, isTrue);

      final disabled = AppSettings.fromJson(const {
        'remoteBackgroundScanEnabled': false,
        'remoteCoverFetchEnabled': false,
      });
      expect(disabled.remoteBackgroundScanEnabled, isFalse);
      expect(disabled.remoteCoverFetchEnabled, isFalse);

      final back = AppSettings.fromJson(disabled.toJson());
      expect(back.remoteBackgroundScanEnabled, isFalse);
      expect(back.remoteCoverFetchEnabled, isFalse);
    },
  );

  /// D7（2026-09-21）：阅读渲染宽度的设置与解析。
  group('阅读渲染宽度（D7）', () {
    test('标准档返回 null（保持历史页缓存路径不变）', () {
      expect(
        renderWidthPixels(
          RenderWidth.standard,
          screenWidth: 2048,
          devicePixelRatio: 1.25,
        ),
        isNull,
      );
    });

    test('省流档固定 1080', () {
      expect(
        renderWidthPixels(
          RenderWidth.dataSaver,
          screenWidth: 2048,
          devicePixelRatio: 1.25,
        ),
        1080,
      );
    });

    test('跟随屏幕 = 逻辑宽 × DPR，并夹取到 [640, 4096]', () {
      expect(
        renderWidthPixels(
          RenderWidth.screen,
          screenWidth: 2048,
          devicePixelRatio: 1.25,
        ),
        2560,
      );
      expect(
        renderWidthPixels(
          RenderWidth.screen,
          screenWidth: 100,
          devicePixelRatio: 1,
        ),
        640,
      );
      expect(
        renderWidthPixels(
          RenderWidth.screen,
          screenWidth: 4000,
          devicePixelRatio: 4,
        ),
        4096,
      );
    });

    test('JSON 往返保留 renderWidth，缺失/未知值回落到标准档', () {
      final s = AppSettings();
      expect(s.renderWidth, RenderWidth.standard, reason: '默认必须与历史行为一致');

      s.renderWidth = RenderWidth.dataSaver;
      expect(AppSettings.fromJson(s.toJson()).renderWidth, RenderWidth.dataSaver);

      final json = s.toJson();
      json.remove('renderWidth');
      expect(AppSettings.fromJson(json).renderWidth, RenderWidth.standard);

      json['renderWidth'] = '不存在的档位';
      expect(AppSettings.fromJson(json).renderWidth, RenderWidth.standard);
    });
  });
}
