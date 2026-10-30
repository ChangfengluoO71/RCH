import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter/services.dart';

/// Minimal credential boundary used by Android. Callers store one JSON object
/// per source (or sync secret) under a stable key; the native side encrypts the
/// value with an Android Keystore AES-GCM key.
abstract interface class CredentialVault {
  Future<void> put(String key, String value);

  Future<String?> get(String key);

  Future<void> delete(String key);
}

class MethodChannelCredentialVault implements CredentialVault {
  MethodChannelCredentialVault({MethodChannel? channel})
    : _channel = channel ?? const MethodChannel('rch/credentials');

  final MethodChannel _channel;

  @override
  Future<void> put(String key, String value) async {
    await _channel.invokeMethod<void>('put', {'key': key, 'value': value});
  }

  @override
  Future<String?> get(String key) async {
    return _channel.invokeMethod<String>('get', {'key': key});
  }

  @override
  Future<void> delete(String key) async {
    await _channel.invokeMethod<void>('delete', {'key': key});
  }
}

/// Storage operations needed by [FlutterSecureStorageCredentialVault].
/// Keeping this boundary injectable lets tests verify routing and failure
/// behavior without relying on an operating-system credential service.
abstract interface class CredentialKeyValueStore {
  Future<void> write({required String key, required String value});

  Future<String?> read({required String key});

  Future<void> delete({required String key});
}

class FlutterSecureStorageKeyValueStore implements CredentialKeyValueStore {
  FlutterSecureStorageKeyValueStore({FlutterSecureStorage? storage})
    : _storage = storage ?? const FlutterSecureStorage();

  final FlutterSecureStorage _storage;

  @override
  Future<void> write({required String key, required String value}) =>
      _storage.write(key: key, value: value);

  @override
  Future<String?> read({required String key}) => _storage.read(key: key);

  @override
  Future<void> delete({required String key}) => _storage.delete(key: key);
}

class FlutterSecureStorageCredentialVault implements CredentialVault {
  FlutterSecureStorageCredentialVault({CredentialKeyValueStore? storage})
    : _storage = storage ?? FlutterSecureStorageKeyValueStore();

  final CredentialKeyValueStore _storage;

  @override
  Future<void> put(String key, String value) =>
      _storage.write(key: key, value: value);

  @override
  Future<String?> get(String key) => _storage.read(key: key);

  @override
  Future<void> delete(String key) => _storage.delete(key: key);
}

/// In-memory implementation for unit tests and non-Android tooling. It is not
/// selected for Android production paths, so it cannot accidentally become a
/// plaintext persistence fallback there.
class MemoryCredentialVault implements CredentialVault {
  final Map<String, String> values = {};

  @override
  Future<void> put(String key, String value) async => values[key] = value;

  @override
  Future<String?> get(String key) async => values[key];

  @override
  Future<void> delete(String key) async => values.remove(key);
}

CredentialVault platformCredentialVault({TargetPlatform? platform}) =>
    (platform ?? defaultTargetPlatform) == TargetPlatform.android
    ? MethodChannelCredentialVault()
    : FlutterSecureStorageCredentialVault();

/// Native platforms must use a secure store. Web keeps its existing storage
/// behavior because the plugin's browser storage has a different trust model.
bool usesNativeCredentialVault({required bool isWeb}) => !isWeb;

/// Native library backups never contain credentials; web keeps its existing
/// export behavior for compatibility.
bool includeSensitiveCredentialsInBackup({required bool isWeb}) => isWeb;

String encodeCredentialBundle(Map<String, String?> values) => jsonEncode({
  for (final entry in values.entries)
    if (entry.value != null && entry.value!.isNotEmpty) entry.key: entry.value,
});

Map<String, String?> decodeCredentialBundle(String? value) {
  try {
    return decodeCredentialBundleStrict(value);
  } on FormatException {
    // Keep the legacy helper tolerant for callers that only need to inspect a
    // best-effort value. Persistence paths use the strict variant below so a
    // corrupt vault entry can never be mistaken for an intentional clear.
    return <String, String?>{};
  }
}

Map<String, String?> decodeCredentialBundleStrict(String? value) {
  if (value == null || value.isEmpty) return <String, String?>{};
  final decoded = jsonDecode(value);
  if (decoded is! Map) {
    throw const FormatException('credential vault entry must be a JSON object');
  }
  return <String, String?>{
    for (final entry in decoded.entries)
      entry.key.toString(): entry.value?.toString(),
  };
}
