import 'package:app/store/automation_coordinator.dart';
import 'package:app/src/rust/api/db.dart';
import 'package:app/store/library_store.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('production safe-auto materialization is enabled', () {
    expect(AutomationCoordinator.automaticMaterializationEnabled, isTrue);
  });

  test('remote cleanup only trusts a completed index replacement', () {
    expect(
      LibraryStore.isUsableRemoteRefreshRevision('missing-fingerprint'),
      isFalse,
    );
    expect(
      LibraryStore.isUsableRemoteRefreshRevision('remote-refresh-not-allowed'),
      isFalse,
    );
    expect(LibraryStore.isUsableRemoteRefreshRevision('root-hash-123'), isTrue);
  });

  test(
    'generated tag projection migration runs until its marker is persisted',
    () {
      expect(
        AutomationCoordinator.needsGeneratedTagProjectionMigration(const []),
        isTrue,
      );
      expect(
        AutomationCoordinator.needsGeneratedTagProjectionMigration([
          const SettingEntryDto(
            key: 'generated_tag_projection_version',
            value: 'v3',
          ),
        ]),
        isFalse,
      );
      expect(
        AutomationCoordinator.needsGeneratedTagProjectionMigration([
          const SettingEntryDto(
            key: 'generated_tag_projection_version',
            value: 'v2',
          ),
        ]),
        isTrue,
      );
      expect(
        AutomationCoordinator.needsGeneratedTagProjectionMigration([
          const SettingEntryDto(
            key: 'generated_tag_projection_version',
            value: 'v1',
          ),
        ]),
        isTrue,
      );
    },
  );

  test(
    'applied ready proposals remain eligible for projection reconciliation',
    () {
      // materialization_status is deliberately not part of this gate: Rust
      // must be allowed to repair missing canonical tags on a re-scrape.
      expect(
        AutomationCoordinator.shouldAttemptMaterialization(
          inputRevision: 'catalog-revision',
          conflictsJson: '[]',
        ),
        isTrue,
      );
      expect(
        AutomationCoordinator.shouldAttemptMaterialization(
          inputRevision: '',
          conflictsJson: '[]',
        ),
        isFalse,
      );
      expect(
        AutomationCoordinator.shouldAttemptMaterialization(
          inputRevision: 'catalog-revision',
          conflictsJson: '["title"]',
        ),
        isFalse,
      );
    },
  );
}
