import 'package:app/store/models.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('AppSettings.fromJson 兼容 keys 的 Map 形态', () {
    final s = AppSettings.fromJson(const {
      'keys': {'forward': 123, 'back': 456, 'zoomIn': 789, 'zoomOut': 101112, 'zoomReset': 0},
    });
    expect(s.keys.forward, 123);
    expect(s.keys.back, 456);
  });

  test('AppSettings.fromJson 兼容 keys 的 JSON 字符串形态', () {
    final s = AppSettings.fromJson({
      'keys': '{"forward":123,"back":456,"zoomIn":789,"zoomOut":101112,"zoomReset":0}',
    });
    expect(s.keys.forward, 123);
  });

  test('AppSettings.fromJson 兼容历史坏数据（Dart toString，回落默认）', () {
    final s = AppSettings.fromJson({
      'keys': '{forward: 8445061, back: 8445068, zoomIn: 8445063, zoomOut: 8445067, zoomReset: 8445064}',
    });
    expect(s.keys.forward, LogicalKeyboardKey.arrowRight.keyId);
    expect(s.keys.back, LogicalKeyboardKey.arrowLeft.keyId);
  });

  test('AppSettings.fromJson 兼容 keys 缺失', () {
    final s = AppSettings.fromJson(const {});
    expect(s.keys.forward, LogicalKeyboardKey.arrowRight.keyId);
  });
}
