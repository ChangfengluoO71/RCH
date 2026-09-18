import 'dart:async';

import 'package:app/src/rust/api/remote_cover.dart' as rust_cover;
import 'package:app/store/remote_scan_coordinator.dart';
import 'package:flutter_test/flutter_test.dart';

/// P1-E：Dart coordinator 的 cover revision bridge 契约。
///
/// 冻结语义：
/// * 事件只是«该 source 的 cover durable truth 可能变了»，**不携带**状态；
///   consumer 收到后必须自己重读 durable revision（`remoteViewRevision`）。
/// * 去重：`durable <= lastSeen` ⇒ duplicate，忽略且**不重复刷新聚合**。
/// * 无 timer：不得周期性读取 revision。
/// * dispose ⇒ 取消订阅、释放 notifier、此后不再有任何更新。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  ({
    RemoteScanCoordinator coordinator,
    StreamController<rust_cover.CoverRevisionEvent> controller,
    List<String> reads,
    List<String> refreshes,
  })
  harness() {
    final controller = StreamController<rust_cover.CoverRevisionEvent>.broadcast();
    final coordinator = RemoteScanCoordinator();
    final reads = <String>[];
    final refreshes = <String>[];
    coordinator.debugCoverRevisionStream = controller.stream;
    coordinator.debugCoverRevisionReader = (String sourceId) async {
      reads.add(sourceId);
      return durableRevision;
    };
    coordinator.debugCoverAggregateRefresh = (String sourceId) async {
      refreshes.add(sourceId);
    };
    coordinator.startCoverRevisionWatch();
    return (
      coordinator: coordinator,
      controller: controller,
      reads: reads,
      refreshes: refreshes,
    );
  }

  test('wake applies the durable revision once and dedups duplicates (STREAM-6)', () async {
    final h = harness();
    addTearDown(() async {
      await h.controller.close();
      await h.coordinator.dispose();
    });

    durableRevision = 5;
    h.controller.add(const rust_cover.CoverRevisionEvent(sourceId: 's'));
    await pumpEventQueue();

    expect(h.coordinator.coverRevisionFor('s').value, 5);
    expect(h.coordinator.lastSeenCoverRevision('s'), 5);
    expect(h.refreshes, ['s'], reason: 'one durable change => exactly one aggregate refresh');

    // duplicate wake：durable token 未变 ⇒ 被 dedup，绝不重复刷新
    h.controller.add(const rust_cover.CoverRevisionEvent(sourceId: 's'));
    await pumpEventQueue();
    expect(h.refreshes, ['s'], reason: 'duplicate wake must be ignored by the revision token');
    expect(h.coordinator.coverRevisionFor('s').value, 5);

    // 更新的 durable revision ⇒ 应用一次
    durableRevision = 9;
    h.controller.add(const rust_cover.CoverRevisionEvent(sourceId: 's'));
    await pumpEventQueue();
    expect(h.coordinator.coverRevisionFor('s').value, 9);
    expect(h.refreshes, ['s', 's']);
  });

  test('duplicate delivery never causes a second durable read side effect beyond reads', () async {
    final h = harness();
    addTearDown(() async {
      await h.controller.close();
      await h.coordinator.dispose();
    });
    durableRevision = 3;
    h.controller.add(const rust_cover.CoverRevisionEvent(sourceId: 's'));
    await pumpEventQueue();
    final readsAfterFirst = h.reads.length;
    // 连续重复 5 次
    for (var i = 0; i < 5; i++) {
      h.controller.add(const rust_cover.CoverRevisionEvent(sourceId: 's'));
    }
    await pumpEventQueue();
    expect(h.refreshes.length, 1, reason: 'no extra refresh for duplicates');
    expect(h.reads.length, greaterThanOrEqualTo(readsAfterFirst));
  });

  test('missed-event catch-up applies durable revision with no wake', () async {
    final h = harness();
    addTearDown(() async {
      await h.controller.close();
      await h.coordinator.dispose();
    });
    durableRevision = 7;
    await h.coordinator.catchUpCoverRevision('s');
    expect(h.coordinator.coverRevisionFor('s').value, 7);
    expect(h.refreshes, ['s']);
    // catch-up 本身不产生 wake，也不重复刷新
    await h.coordinator.catchUpCoverRevision('s');
    expect(h.refreshes, ['s']);
  });

  test('no periodic revision polling (E-NO-POLL for the bridge)', () async {
    final h = harness();
    addTearDown(() async {
      await h.controller.close();
      await h.coordinator.dispose();
    });
    durableRevision = 42;
    // 不投递任何事件：等待明显长于任何"轮询周期"的时间
    await Future<void>.delayed(const Duration(milliseconds: 1200));
    expect(
      h.reads,
      isEmpty,
      reason: 'the bridge must never read the durable revision on a timer',
    );
    expect(h.refreshes, isEmpty);
    expect(h.coordinator.coverRevisionFor('s').value, 0);
  });

  test('dispose cancels the subscription and stops applying updates', () async {
    final h = harness();
    durableRevision = 11;
    await h.coordinator.dispose();
    h.controller.add(const rust_cover.CoverRevisionEvent(sourceId: 's'));
    await pumpEventQueue();
    expect(h.refreshes, isEmpty, reason: 'no updates after dispose');
    // 再次 dispose 是幂等的
    await h.coordinator.dispose();
    await h.controller.close();
  });
}

/// 测试可控的 durable revision（模拟 `remoteViewRevision` 的返回值）。
int durableRevision = 0;
