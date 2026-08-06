// Repository 层：阅读记录持久化（ADR-016/018）。
//
// RecordRepository 是 ReadRecord 的唯一数据持有者。
// 只负责纯数据 CRUD + SQLite 持久化，不负责通知 UI 和跨模块协调。

import 'dart:io';

import '../src/rust/api/db.dart';
import '../store/models.dart';

class RecordRepository {
  RecordRepository._();
  static final RecordRepository instance = RecordRepository._();

  final Map<String, ReadRecord> records = {};

  // ---- Key ----

  static String keyOf(String sourceType, String sourceId, String path) =>
      bookKeyOf(sourceType, sourceId, path);

  // ---- CRUD ----

  ReadRecord? of(BookSource source, String path) =>
      records[keyOf(source.type, source.id, path)];

  /// 获取或创建一条阅读记录。
  /// page == null → 只加 readCount（"打开"）；page != null → 更新 lastPage。
  ReadRecord upsert({
    required BookSource source,
    required String path,
    required String title,
    int? page,
  }) {
    final key = keyOf(source.type, source.id, path);
    final r = records[key] ?? ReadRecord(
      key: key,
      sourceId: source.id,
      sourceType: source.type,
      path: path,
      title: title,
    );
    r.lastReadAt = DateTime.now().millisecondsSinceEpoch;
    if (page == null) {
      r.readCount++;
    } else {
      r.lastPage = page;
    }
    records[key] = r;
    return r;
  }

  void removeByPrefix(String prefix) {
    records.removeWhere((k, _) => k.startsWith(prefix));
  }

  /// 清理失效记录（源已删除 / 本地文件丢失），返回被移除的 key 列表，
  /// 调用方需据此同步删除 SQLite 中的行（saveToSqlite 只 upsert 不删行）。
  List<String> purgeStale(List<BookSource> sources) {
    final staleKeys = <String>[];
    for (final r in records.values) {
      final src = sources.cast<BookSource?>().firstWhere(
        (s) => s?.id == r.sourceId,
        orElse: () => null,
      );
      if (src == null) {
        staleKeys.add(r.key);
      } else if (src.isLocalFs && !File(r.path).existsSync()) {
        staleKeys.add(r.key);
      }
    }
    for (final k in staleKeys) {
      records.remove(k);
    }
    return staleKeys;
  }

  // ---- Queries ----

  List<ReadRecord> recent() {
    final list = records.values.toList();
    list.sort((a, b) => b.lastReadAt.compareTo(a.lastReadAt));
    return list;
  }

  List<ReadRecord> mostRead() {
    final list = records.values.toList();
    list.sort((a, b) => b.readCount.compareTo(a.readCount));
    return list;
  }

  bool hasAnyRead() => records.values.any((r) => r.readCount > 0);

  // ---- SQLite ----

  Future<void> loadFromSqlite() async {
    records.clear();
    final recDtos = await dbLoadAllRecords();
    for (final dto in recDtos) {
      records[dto.key] = ReadRecord(
        key: dto.key,
        sourceId: dto.sourceId,
        sourceType: dto.sourceType,
        path: dto.path,
        title: dto.title,
        lastPage: dto.lastPage,
        readCount: dto.readCount,
        lastReadAt: dto.lastReadAt.toInt(),
      );
    }
  }

  Future<void> saveToSqlite() async {
    for (final r in records.values) {
      await dbUpsertRecord(record: ReadRecordDto(
        key: r.key,
        sourceId: r.sourceId,
        sourceType: r.sourceType,
        path: r.path,
        title: r.title,
        lastPage: r.lastPage,
        readCount: r.readCount,
        lastReadAt: r.lastReadAt,
      ));
    }
  }

  /// 单条记录写入 SQLite（高频更新路径）。
  Future<void> saveOneToSqlite(ReadRecord r) async {
    await dbUpsertRecord(record: ReadRecordDto(
      key: r.key,
      sourceId: r.sourceId,
      sourceType: r.sourceType,
      path: r.path,
      title: r.title,
      lastPage: r.lastPage,
      readCount: r.readCount,
      lastReadAt: r.lastReadAt,
    ));
  }

  // ---- JSON ----

  Map<String, dynamic> toJson() => {
    'records': records.map((k, v) => MapEntry(k, v.toJson())),
  };

  void loadFromJson(Map<String, dynamic> j) {
    records.clear();
    records.addEntries((j['records'] as Map? ?? {}).entries.map((e) =>
        MapEntry(e.key, ReadRecord.fromJson(Map<String, dynamic>.from(e.value)))));
  }
}
