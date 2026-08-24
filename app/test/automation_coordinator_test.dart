import 'package:app/store/automation_coordinator.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('production safe-auto materialization is enabled', () {
    expect(AutomationCoordinator.automaticMaterializationEnabled, isTrue);
  });
}
