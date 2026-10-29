import 'dart:io';

/// Restore the same root as the activity before any database call. A missing
/// custom library is a recoverable configuration error, never permission to
/// open or create a different empty library from a headless engine.
Future<void> bootstrapEhDailyStorage({
  required Future<String?> Function() readMarker,
  required Future<String?> Function() readLegacyRoot,
  required bool Function(String) invalidRoot,
  required Future<void> Function(String) setRoot,
  required Future<String> Function() currentRoot,
  required Future<void> Function() loadLibrary,
}) async {
  final selectedRoot = await readMarker() ?? await readLegacyRoot();
  if (selectedRoot != null && selectedRoot.isNotEmpty) {
    if (invalidRoot(selectedRoot) || !await Directory(selectedRoot).exists()) {
      throw StateError('后台数据目录不可用，请打开应用检查缓存目录授权。');
    }
    await setRoot(selectedRoot);
  }
  final root = await currentRoot();
  final database = File('$root${Platform.pathSeparator}database.db');
  if (!await database.exists()) {
    throw StateError('后台数据目录中的资料库不可用，请打开应用检查缓存目录。');
  }
  // Existence alone is insufficient when a persisted external-storage grant
  // has been revoked. Do not allow a database open to create a fresh file.
  final handle = await database.open(mode: FileMode.read);
  await handle.close();
  await loadLibrary();
}
