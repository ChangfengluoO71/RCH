// Repository 层：书源 + 元数据持久化（ADR-016/018）。
//
// BookRepository 是 BookSource 和 BookMeta 的唯一数据持有者。
// 只负责纯数据 CRUD + SQLite 持久化，不负责通知 UI 和跨模块协调。
// UI 通知和跨模块逻辑由 LibraryStore（facade）统一管理。

import 'dart:convert';

import '../src/rust/api/db.dart';
import '../store/models.dart';

class BookRepository {
  BookRepository._();
  static final BookRepository instance = BookRepository._();

  final List<BookSource> sources = [];
  final Map<String, BookMeta> metas = {};

  // ---- Source CRUD ----

  void addSource(BookSource s) => sources.add(s);

  void removeSource(String id) => sources.removeWhere((s) => s.id == id);

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
    final srcDtos = await dbLoadAllSources();
    for (final dto in srcDtos) {
      sources.add(
        BookSource(
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
          note: dto.note,
          capabilityLabel: dto.capabilityLabel,
          remoteOnly: dto.remoteOnly,
          originDeviceId: dto.originDeviceId,
        ),
      );
    }

    metas.clear();
    final metaDtos = await dbLoadAllMetas();
    for (final dto in metaDtos) {
      metas[dto.key] = BookMeta(
        key: dto.key,
        coverPage: dto.coverPage,
        cropX: dto.cropX,
        cropY: dto.cropY,
        cropW: dto.cropW,
        cropH: dto.cropH,
        author: dto.author,
        genre: dto.genre,
        series: dto.series,
        title: dto.title,
        chineseTitle: dto.chineseTitle,
        summary: dto.summary,
        comment: dto.comment,
        rotations: parseBookRotations(dto.rotations),
      );
    }
  }

  Future<void> saveToSqlite() async {
    // Each DB call yields to the event loop. Work from immutable snapshots so
    // a concurrent catalog reload/source edit cannot mutate the iterables.
    final sourceSnapshot = List<BookSource>.of(sources, growable: false);
    final metaSnapshot = metas.values.toList(growable: false);
    for (final s in sourceSnapshot) {
      await dbUpsertSource(
        source: BookSourceDto(
          id: s.id,
          type: s.type,
          name: s.name,
          path: s.path,
          url: s.url,
          username: s.username,
          password: s.password,
          port: s.port,
          refreshToken: s.refreshToken,
          clientId: s.clientId,
          clientSecret: s.clientSecret,
          rootId: s.rootId,
          cookie: s.cookie,
          note: s.note,
          capabilityLabel: s.capabilityLabel,
          remoteOnly: s.remoteOnly,
          originDeviceId: s.originDeviceId,
        ),
      );
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

  Map<String, dynamic> toJson() => {
    'sources': sources.map((e) => e.toJson()).toList(),
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
