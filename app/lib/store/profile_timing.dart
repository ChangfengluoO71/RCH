import 'package:flutter/foundation.dart';

/// Records elapsed time for a named operation in Profile mode without adding
/// paths, source names, cookies, or other user data to the log.
Future<T> withProfileTiming<T>(
  String phase,
  Future<T> Function() operation,
) async {
  if (!kProfileMode) return operation();
  final stopwatch = Stopwatch()..start();
  try {
    return await operation();
  } finally {
    debugPrint(
      '[performance] phase=$phase elapsed_ms=${stopwatch.elapsedMilliseconds}',
    );
  }
}
