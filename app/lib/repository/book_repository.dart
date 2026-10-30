// Repository 层：书源 + 元数据持久化（ADR-016/018）。
//
// BookRepository 是 BookSource 和 BookMeta 的唯一数据持有者。
// 只负责纯数据 CRUD + SQLite 持久化，不负责通知 UI 和跨模块协调。
// UI 通知和跨模块逻辑由 LibraryStore（facade）统一管理。

import 'dart:convert';

import 'package:flutter/foundation.dart';

import '../src/rust/api/db.dart';
import '../store/credential_vault.dart';
import '../store/models.dart';
import '../store/smb_source_config.dart';

class BookRepository {
  BookRepository._({
    this._credentialVault,
    Future<List<BookSourceDto>> Function()? loadSources,
    Future<List<BookMetaDto>> Function()? loadMetas,
    Future<void> Function(BookSourceDto source)? upsertSource,
  }) : _loadSources = loadSources ?? dbLoadAllSources,
       _loadMetas = loadMetas ?? dbLoadAllMetas,
       _upsertSource =
           upsertSource ?? ((source) => dbUpsertSource(source: source));

  @visibleForTesting
  BookRepository.forTesting({
    required CredentialVault credentialVault,
    required Future<List<BookSourceDto>> Function() loadSources,
    required Future<List<BookMetaDto>> Function() loadMetas,
    required Future<void> Function(BookSourceDto source) upsertSource,
  }) : this._(
         credentialVault: credentialVault,
         loadSources: loadSources,
         loadMetas: loadMetas,
         upsertSource: upsertSource,
       );

  static final BookRepository instance = BookRepository._();

  final List<BookSource> sources = [];
  final Map<String, BookMeta> metas = {};

  CredentialVault? _credentialVault;
  final Future<List<BookSourceDto>> Function() _loadSources;
  final Future<List<BookMetaDto>> Function() _loadMetas;
  final Future<void> Function(BookSourceDto source) _upsertSource;

  bool get _usesCredentialVault => usesNativeCredentialVault(isWeb: kIsWeb);

  CredentialVault get _vault => _credentialVault ??= platformCredentialVault();

  String _credentialKey(BookSource source) =>
      source.credentialRef ?? 'source:${source.id}';

  bool _hasSensitive({
    String? password,
    String? refreshToken,
    String? clientSecret,
    String? cookie,
    String? privateKey,
    String? privateKeyPassphrase,
  }) =>
      (password?.isNotEmpty ?? false) ||
      (refreshToken?.isNotEmpty ?? false) ||
      (clientSecret?.isNotEmpty ?? false) ||
      (cookie?.isNotEmpty ?? false) ||
      (privateKey?.isNotEmpty ?? false) ||
      (privateKeyPassphrase?.isNotEmpty ?? false);

  String _encodeSourceCredentials(BookSource source) => encodeCredentialBundle({
    'password': source.password,
    'refreshToken': source.refreshToken,
    'clientSecret': source.clientSecret,
    'cookie': source.cookie,
    'privateKey': source.privateKey,
    'privateKeyPassphrase': source.privateKeyPassphrase,
  });

  Future<String?> _migrateOrHydrateCredentials(BookSource source) async {
    if (!_usesCredentialVault) return source.credentialRef;

    final key =
        source.credentialRef ??
        (_hasSensitive(
              password: source.password,
              refreshToken: source.refreshToken,
              clientSecret: source.clientSecret,
              cookie: source.cookie,
              privateKey: source.privateKey,
              privateKeyPassphrase: source.privateKeyPassphrase,
            )
            ? _credentialKey(source)
            : null);
    if (key == null) return null;

    Map<String, String?> bundle = <String, String?>{};
    var migrated = false;
    try {
      final stored = await _vault.get(key);
      if (stored != null && stored.isNotEmpty) {
        bundle = decodeCredentialBundleStrict(stored);
      } else if (_hasSensitive(
        password: source.password,
        refreshToken: source.refreshToken,
        clientSecret: source.clientSecret,
        cookie: source.cookie,
        privateKey: source.privateKey,
        privateKeyPassphrase: source.privateKeyPassphrase,
      )) {
        // One-time migration for legacy SQLite/library.json rows. The
        // read-back is mandatory before clearing SQLite in the next step.
        final payload = _encodeSourceCredentials(source);
        await _vault.put(key, payload);
        final verified = await _vault.get(key);
        if (verified != payload) {
          throw StateError('凭据保险库回读校验失败');
        }
        bundle = decodeCredentialBundleStrict(verified);
        migrated = true;
      }
    } catch (error, stack) {
      // Never turn a vault failure into an intentional credential clear. Keep
      // legacy values in memory, and make unrelated saves preserve this ref;
      // the next load retries hydration.
      source.credentialHydrationFailed = true;
      debugPrint(
        '[BookRepository] credential vault migration failed (${error.runtimeType})',
      );
      debugPrintStack(stackTrace: stack);
      return source.credentialRef;
    }

    source.credentialHydrationFailed = false;
    source.credentialRef = key;
    source.password = bundle['password'];
    source.refreshToken = bundle['refreshToken'];
    source.clientSecret = bundle['clientSecret'];
    source.cookie = bundle['cookie'];
    source.privateKey = bundle['privateKey'];
    source.privateKeyPassphrase = bundle['privateKeyPassphrase'];

    if (migrated) {
      // Clear only after the vault payload was verified. The in-memory source
      // remains hydrated for the current session.
      try {
        await _upsertSource(
          BookSourceDto(
            id: source.id,
            type: source.type,
            name: source.name,
            path: source.path,
            url: source.url,
            username: source.username,
            password: null,
            port: source.port,
            hostKeyFingerprint: source.hostKeyFingerprint,
            hostKeyEndpoint: source.hostKeyEndpoint,
            sftpAuthMethod: source.sftpAuthMethod,
            refreshToken: null,
            clientId: source.clientId,
            clientSecret: null,
            rootId: source.rootId,
            cookie: null,
            credentialRef: source.credentialRef,
            note: source.note,
            capabilityLabel: source.capabilityLabel,
            remoteOnly: source.remoteOnly,
            originDeviceId: source.originDeviceId,
          ),
        );
      } catch (error, stack) {
        debugPrint('[BookRepository] credential row redaction failed: $error');
        debugPrintStack(stackTrace: stack);
        // Keep hydrated values and let the next save retry. No value was
        // discarded from the in-memory model.
      }
    }
    return source.credentialRef;
  }

  // ---- Source CRUD ----

  void addSource(BookSource s) => sources.add(s);

  void removeSource(String id) => sources.removeWhere((s) => s.id == id);

  /// Remove the system-keychain bundle after the database row is deleted.
  Future<void> deleteSourceCredentials(BookSource source) async {
    if (!_usesCredentialVault) return;
    final key = source.credentialRef;
    if (key != null && key.isNotEmpty) await _vault.delete(key);
  }

  void updateSource(
    String id, {
    String? name,
    String? url,
    String? username,
    String? password,
    int? port,
    String? path,
    String? refreshToken,
    String? clientId,
    String? clientSecret,
    String? rootId,
    String? cookie,
    String? hostKeyFingerprint,
    String? hostKeyEndpoint,
    String? sftpAuthMethod,
    String? privateKey,
    String? privateKeyPassphrase,
    String? credentialRef,
    String? note,
  }) {
    for (final s in sources) {
      if (s.id == id) {
        if (name != null) s.name = name;
        if (url != null) s.url = url;
        if (username != null) s.username = username;
        if (password != null) s.password = password;
        if (port != null) s.port = port;
        if (path != null) s.path = path;
        if (refreshToken != null) s.refreshToken = refreshToken;
        if (clientId != null) s.clientId = clientId;
        if (clientSecret != null) s.clientSecret = clientSecret;
        if (rootId != null) s.rootId = rootId;
        if (cookie != null) s.cookie = cookie;
        if (hostKeyFingerprint != null) {
          s.hostKeyFingerprint = hostKeyFingerprint;
        }
        if (hostKeyEndpoint != null) s.hostKeyEndpoint = hostKeyEndpoint;
        if (sftpAuthMethod != null) s.sftpAuthMethod = sftpAuthMethod;
        if (privateKey != null) s.privateKey = privateKey;
        if (privateKeyPassphrase != null) {
          s.privateKeyPassphrase = privateKeyPassphrase;
        }
        if (credentialRef != null) s.credentialRef = credentialRef;
        if (note != null) s.note = note;
      }
    }
  }

  void updateSourceCapability(String id, String label) {
    for (final s in sources) {
      if (s.id == id) s.capabilityLabel = label;
    }
  }

  BookSource? sourceById(String id) {
    for (final s in sources) {
      if (s.id == id) return s;
    }
    return null;
  }

  // ---- Meta CRUD ----

  /// 获取（或创建）一本书的元数据。
  BookMeta metaOf(BookSource source, String path) {
    final key = bookKeyOf(source.type, source.id, path);
    return metas.putIfAbsent(key, () => BookMeta(key: key));
  }

  void updateMeta(BookMeta m) => metas[m.key] = m;

  // ---- Query helpers ----

  /// 收集所有被用作元数据（author/genre/series）的标签名。
  /// hasAnyRead 由 LibraryStore 传入（跨模块依赖）。
  Set<String> metaTagNames({required bool hasAnyRead}) {
    final set = <String>{if (hasAnyRead) '已读'};
    for (final m in metas.values) {
      if (m.author.isNotEmpty) set.add(m.author);
      if (m.genre.isNotEmpty) set.add(m.genre);
      if (m.series.isNotEmpty) set.add(m.series);
    }
    return set;
  }

  // ---- SQLite ----

  Future<void> loadFromSqlite() async {
    sources.clear();
    final srcDtos = await _loadSources();
    for (final dto in srcDtos) {
      final source = BookSource(
        id: dto.id,
        type: dto.type,
        name: dto.name,
        path: dto.path,
        url: dto.url,
        username: dto.username,
        password: dto.password,
        port: dto.port?.toInt(),
        refreshToken: dto.refreshToken,
        clientId: dto.clientId,
        clientSecret: dto.clientSecret,
        rootId: dto.rootId,
        cookie: dto.cookie,
        hostKeyFingerprint: dto.hostKeyFingerprint,
        hostKeyEndpoint: dto.hostKeyEndpoint,
        sftpAuthMethod: dto.sftpAuthMethod,
        credentialRef: dto.credentialRef,
        note: dto.note,
        capabilityLabel: dto.capabilityLabel,
        remoteOnly: dto.remoteOnly,
        originDeviceId: dto.originDeviceId,
      );
      await _migrateOrHydrateCredentials(source);
      if (source.isSmb) {
        try {
          final config = SmbSourceConfig.fromSource(source);
          source.url = config.server;
          source.port = config.port;
          source.path = config.canonicalPath;
        } on FormatException catch (error) {
          // Keep malformed legacy rows loadable; the session boundary will
          // return this actionable configuration error when the source opens.
          debugPrint(
            '[BookRepository] SMB source configuration is invalid: ${error.message}',
          );
        }
      }
      sources.add(source);
    }

    metas.clear();
    final metaDtos = await _loadMetas();
    for (final dto in metaDtos) {
      metas[dto.key] = bookMetaFromDto(dto);
    }
  }

  /// SQLite DTO → 内存模型（纯映射，抽出来便于单测钉住字段）。
  ///
  /// 这里**必须**带上 volume/chapter：第 130 轮补 `BookMeta` 字段时只改了写方向
  /// （`saveToSqlite` 与本文件里的 `BookMetaDto` 构造），读方向漏在这一处 ⇒
  /// SQLite 里即便有值，Dart 内存里也永远是空串，号码到不了匹配层与显示层。
  /// 回归见 `test/book_meta_sequence_test.dart`。
  static BookMeta bookMetaFromDto(BookMetaDto dto) => BookMeta(
    key: dto.key,
    coverPage: dto.coverPage,
    cropX: dto.cropX,
    cropY: dto.cropY,
    cropW: dto.cropW,
    cropH: dto.cropH,
    author: dto.author,
    genre: dto.genre,
    series: dto.series,
    volume: dto.volume,
    chapter: dto.chapter,
    title: dto.title,
    chineseTitle: dto.chineseTitle,
    summary: dto.summary,
    comment: dto.comment,
    rotations: parseBookRotations(dto.rotations),
  );

  /// Persist one source without rewriting the full metadata snapshot.
  ///
  /// Interactive source creation uses this before refreshing the catalog tree;
  /// the library-wide JSON backup remains on LibraryStore's debounced path.
  Future<void> saveSourceToSqlite(BookSource source) =>
      _saveSourceToSqlite(source);

  Future<void> _saveSourceToSqlite(BookSource s) async {
    if (_usesCredentialVault) {
      final hasSensitiveCredentials = _hasSensitive(
        password: s.password,
        refreshToken: s.refreshToken,
        clientSecret: s.clientSecret,
        cookie: s.cookie,
        privateKey: s.privateKey,
        privateKeyPassphrase: s.privateKeyPassphrase,
      );
      final key =
          s.credentialRef ??
          (hasSensitiveCredentials ? _credentialKey(s) : null);
      if (key != null &&
          (!s.credentialHydrationFailed || hasSensitiveCredentials)) {
        final payload = _encodeSourceCredentials(s);
        await _vault.put(key, payload);
        if (await _vault.get(key) != payload) {
          throw StateError('凭据保险库回读校验失败');
        }
        s.credentialRef = key;
        s.credentialHydrationFailed = false;
      }
    }
    await _upsertSource(
      BookSourceDto(
        id: s.id,
        type: s.type,
        name: s.name,
        path: s.path,
        url: s.url,
        username: s.username,
        password: _usesCredentialVault ? null : s.password,
        port: s.port,
        refreshToken: _usesCredentialVault ? null : s.refreshToken,
        clientId: s.clientId,
        clientSecret: _usesCredentialVault ? null : s.clientSecret,
        rootId: s.rootId,
        cookie: _usesCredentialVault ? null : s.cookie,
        credentialRef: s.credentialRef,
        hostKeyFingerprint: s.hostKeyFingerprint,
        hostKeyEndpoint: s.hostKeyEndpoint,
        sftpAuthMethod: s.sftpAuthMethod,
        note: s.note,
        capabilityLabel: s.capabilityLabel,
        remoteOnly: s.remoteOnly,
        originDeviceId: s.originDeviceId,
      ),
    );
  }

  Future<void> saveToSqlite() async {
    // Each DB call yields to the event loop. Work from immutable snapshots so
    // a concurrent catalog reload/source edit cannot mutate the iterables.
    final sourceSnapshot = List<BookSource>.of(sources, growable: false);
    final metaSnapshot = metas.values.toList(growable: false);
    for (final s in sourceSnapshot) {
      await _saveSourceToSqlite(s);
    }
    for (final m in metaSnapshot) {
      await dbUpsertMeta(
        meta: BookMetaDto(
          key: m.key,
          coverPage: m.coverPage,
          cropX: m.cropX,
          cropY: m.cropY,
          cropW: m.cropW,
          cropH: m.cropH,
          author: m.author,
          genre: m.genre,
          series: m.series,
          volume: m.volume,
          chapter: m.chapter,
          title: m.title,
          chineseTitle: m.chineseTitle,
          summary: m.summary,
          comment: m.comment,
          rotations: jsonEncode(m.rotations.map((k, v) => MapEntry('$k', v))),
        ),
      );
    }
  }

  // ---- JSON ----

  Map<String, dynamic> toJson({bool includeSensitive = true}) => {
    'sources': sources
        .map((e) => e.toJson(includeSensitive: includeSensitive))
        .toList(),
    'metas': metas.map((k, v) => MapEntry(k, v.toJson())),
  };

  void loadFromJson(Map<String, dynamic> j) {
    sources.clear();
    sources.addAll(
      (j['sources'] as List? ?? []).map(
        (e) => BookSource.fromJson(Map<String, dynamic>.from(e)),
      ),
    );
    metas.clear();
    metas.addEntries(
      (j['metas'] as Map? ?? {}).entries.map(
        (e) => MapEntry(
          e.key,
          BookMeta.fromJson(Map<String, dynamic>.from(e.value)),
        ),
      ),
    );
  }
}
