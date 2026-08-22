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

  /// 清空全部阅读记录（内存态）。
  void clearAll() => records.clear();

  /// 清理失效记录，返回被移除的记录列表（调用方需据此同步删除
  /// SQLite 行与磁盘缓存；saveToSqlite 只 upsert 不删行）。
  ///
  /// 失效判定（按优先级）：
  /// 1. 书源已删除（sources 中不存在该 sourceId）；
  /// 2. 远程书源：离线索引中该路径已被标记为已删除（[remoteTombstones] 的 deleted=1 墓碑，
  ///    即整源重建时远程文件已消失的软删条目）；
  /// 3. 本地文件系统书源：本地文件不存在。
  List<ReadRecord> purgeStale(
    List<BookSource> sources, {
    Map<String, Set<String>> remoteTombstones = const {},
  }) {
    final stale = <ReadRecord>[];
    for (final r in records.values) {
      final src = sources.cast<BookSource?>().firstWhere(
        (s) => s?.id == r.sourceId,
        orElse: () => null,
      );
      if (src == null) {
        stale.add(r);
      } else if (!src.isLocalFs && (remoteTombstones[src.id]?.contains(r.path) ?? false)) {
        stale.add(r);
      } else if (src.isLocalFs && !File(r.path).existsSync()) {
        stale.add(r);
      }
    }
    for (final r in stale) {
      records.remove(r.key);
    }
    return stale;
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
