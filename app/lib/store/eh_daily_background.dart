import 'package:flutter/services.dart';

/// Persistent Android scheduling is opt-in and managed independently from
/// the in-process coordinator. WorkManager may delay a check until its
/// network and operating-system constraints are met.
abstract interface class EhDailyBackgroundScheduler {
  Future<void> sync(bool enabled);
}

class AndroidEhDailyBackgroundScheduler implements EhDailyBackgroundScheduler {
  const AndroidEhDailyBackgroundScheduler({
    this.channel = const MethodChannel('rch/eh_daily_background'),
  });

  final MethodChannel channel;

  @override
  Future<void> sync(bool enabled) =>
      channel.invokeMethod<void>('setEnabled', {'enabled': enabled});

  Future<EhDailyBackgroundStatus> status() async {
    final raw = await channel.invokeMapMethod<String, Object?>('status');
    return EhDailyBackgroundStatus.fromMap(raw ?? const {});
  }
}

class EhDailyBackgroundStatus {
  const EhDailyBackgroundStatus({
    required this.scheduled,
    required this.running,
    required this.networkConnected,
    this.lastStatus,
    this.lastFinishedAt,
  });

  final bool scheduled;
  final bool running;
  final bool networkConnected;
  final String? lastStatus;
  final DateTime? lastFinishedAt;

  factory EhDailyBackgroundStatus.fromMap(Map<String, Object?> raw) {
    final millis = (raw['lastFinishedAt'] as num?)?.toInt() ?? 0;
    return EhDailyBackgroundStatus(
      scheduled: raw['scheduled'] == true,
      running: raw['running'] == true,
      networkConnected: raw['networkConnected'] == true,
      lastStatus: raw['lastStatus'] as String?,
      lastFinishedAt: millis > 0
          ? DateTime.fromMillisecondsSinceEpoch(millis)
          : null,
    );
  }
}
