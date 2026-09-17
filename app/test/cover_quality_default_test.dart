import 'package:app/store/models.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('new and legacy settings choose small covers without changing explicit quality', () {
    expect(AppSettings().coverQuality.size, (170, 240));
    expect(AppSettings.fromJson({}).coverQuality.size, (170, 240));
    for (final quality in CoverQuality.values) {
      final settings = AppSettings(coverQuality: quality);
      expect(AppSettings.fromJson(settings.toJson()).coverQuality, quality);
    }
  });
}
