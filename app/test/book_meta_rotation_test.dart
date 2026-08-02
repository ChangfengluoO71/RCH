import 'package:app/store/models.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('BookMeta.rotations JSON round-trip', () {
    final m = BookMeta(key: 'local|1|/a/b.zip')
      ..rotations[0] = 90
      ..rotations[3] = 270;

    final restored = BookMeta.fromJson(m.toJson());
    expect(restored.rotations, {0: 90, 3: 270});
  });

  test('BookMeta.rotations 缺省为空，向后兼容旧数据', () {
    final m = BookMeta.fromJson({'key': 'x'});
    expect(m.rotations, isEmpty);
  });

  test('parseBookRotations 兼容 JSON 字符串与非法输入', () {
    expect(parseBookRotations('{"2":180}'), {2: 180});
    expect(parseBookRotations('bad json'), isEmpty);
    expect(parseBookRotations(null), isEmpty);
  });
}
