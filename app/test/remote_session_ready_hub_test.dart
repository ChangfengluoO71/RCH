import 'package:app/store/models.dart';
import 'package:app/store/remote_scan_coordinator.dart';
import 'package:flutter_test/flutter_test.dart';

/// P1-C：`RemoteSessionSuccessHub` 必须在**会话真正建立/更新之后**，把
/// "该 source 已获得有效 session"这一事实交给 Rust。
///
/// 这里用注入口（`notifyReady`）替代真实 FRB，因此不触碰任何 provider / FFI。
void main() {
  BookSource source(String id) => BookSource(
    id: id,
    type: 'webdav',
    name: 'test-source',
    path: '',
  );

  test('emit reports the session-ready fact to Rust exactly once per event', () async {
    final calls = <(String, BigInt)>[];
    final hub = RemoteSessionSuccessHub(
      notifyReady: (sourceId, session) async {
        calls.add((sourceId, session));
      },
    );

    hub.emit(source('src-a'), BigInt.from(42));
    hub.emit(source('src-b'), BigInt.from(43));
    // 通知是异步 fire-and-forget，让微任务跑完。
    await Future<void>.delayed(Duration.zero);

    expect(calls, [('src-a', BigInt.from(42)), ('src-b', BigInt.from(43))]);
    await hub.dispose();
  });

  test('a failing lifecycle notification never breaks session delivery', () async {
    var delivered = 0;
    final hub = RemoteSessionSuccessHub(
      notifyReady: (sourceId, session) async {
        throw StateError('lifecycle notification must be best-effort');
      },
    );
    final subscription = hub.events.listen((_) => delivered++);

    hub.emit(source('src-a'), BigInt.from(42));
    await Future<void>.delayed(Duration.zero);

    // 事件照常投递；通知异常被吞掉，不影响会话获取本身。
    expect(delivered, 1);
    await subscription.cancel();
    await hub.dispose();
  });
}
