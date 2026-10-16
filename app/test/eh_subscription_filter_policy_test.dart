import 'package:app/store/eh_subscription_store.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test("legacy policy migration preserves a user's saved download ladder", () {
    final existingTiers = [
      {'min_age_years': 5.0, 'min_downloads': 1875},
      {'min_age_years': 0.0, 'min_downloads': 175},
    ];

    final migrated = EhSubscriptionStore.migrateFilterPolicyRules(
      oldRules: {
        'filter_policy_version': 0,
        'min_rating': 4.0,
        'age_tiers': existingTiers,
      },
      defaultRules: {
        'age_tiers': [
          {'min_age_years': 5.0, 'min_downloads': 2000},
          {'min_age_years': 0.0, 'min_downloads': 100},
        ],
      },
    );

    expect(migrated['age_tiers'], existingTiers);
    expect(migrated['min_rating'], 4.4);
    expect(migrated['filter_policy_version'], 1);
  });
}
