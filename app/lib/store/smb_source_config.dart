import 'models.dart';

/// Validated, share-relative SMB coordinates shared by source persistence and
/// network-session setup.
class SmbSourceConfig {
  const SmbSourceConfig({
    required this.server,
    required this.port,
    required this.share,
    required this.rootPath,
  });

  final String server;
  final int port;
  final String share;

  /// Directory below [share], normalized to `/` separators.
  final String rootPath;

  String get canonicalPath => rootPath.isEmpty ? share : '$share/$rootPath';

  factory SmbSourceConfig.fromSource(BookSource source) {
    final host = source.url?.trim() ?? '';
    if (host.isEmpty) {
      final legacy = parseLegacyUnc(source.path);
      return SmbSourceConfig(
        server: legacy.server,
        port: _validatedPort(source.port ?? legacy.port),
        share: legacy.share,
        rootPath: legacy.rootPath,
      );
    }

    _validateServer(host);
    final segments = _pathSegments(source.path);
    if (segments.isEmpty) {
      throw const FormatException('SMB path must include a share name');
    }
    return SmbSourceConfig(
      server: host,
      port: _validatedPort(source.port ?? 445),
      share: segments.first,
      rootPath: segments.skip(1).join('/'),
    );
  }

  static SmbSourceConfig parseLegacyUnc(String path) {
    const backslash = '\\';
    final normalized = path.replaceAll('/', backslash);
    if (!normalized.startsWith('$backslash$backslash') ||
        normalized.startsWith('$backslash$backslash$backslash')) {
      throw const FormatException('SMB path must be a UNC path');
    }

    var segments = normalized
        .split(backslash)
        .where((segment) => segment.isNotEmpty)
        .toList(growable: false);
    if (segments.length >= 2 &&
        segments[0] == '?' &&
        segments[1].toLowerCase() == 'unc') {
      segments = segments.skip(2).toList(growable: false);
    }
    if (segments.length < 2) {
      throw const FormatException('SMB UNC path must include a share name');
    }

    final server = segments[0].trim();
    _validateServer(server);
    return SmbSourceConfig(
      server: server,
      port: 445,
      share: segments[1],
      rootPath: segments.skip(2).join('/'),
    );
  }

  static List<String> _pathSegments(String path) => path
      .replaceAll('\\', '/')
      .split('/')
      .where((segment) => segment.isNotEmpty)
      .toList(growable: false);

  static int _validatedPort(int port) {
    if (port < 1 || port > 65535) {
      throw const FormatException('SMB port must be between 1 and 65535');
    }
    return port;
  }

  static void _validateServer(String server) {
    if (server.isEmpty || server.contains('/') || server.contains('\\')) {
      throw const FormatException('SMB server must be a host name or address');
    }
  }
}
